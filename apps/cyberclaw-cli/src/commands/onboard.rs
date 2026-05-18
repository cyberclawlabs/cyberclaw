//! `cyberclaw onboard` — interactive TUI wizard for first-run configuration.
//!
//! This command is the human-facing front door for [`WizardEngine`]. It wires
//! a [`TuiWizardStepHandler`] (backed by the `inquire` crate) to a hand-curated
//! [`WizardPlan`] that walks a new user through:
//!
//! 1. Welcome note
//! 2. Connection mode (Local / Remote)
//! 3. LLM API key (sensitive text)
//! 4. Default LLM provider (select)
//! 5. Workspace root path (text with existence validation)
//! 6. Default skills (multi-select from `ecosystem/skills/`)
//! 7. CLI alias install confirmation
//!
//! On completion, answers are rendered into `~/.cyberclaw/config.toml`.
//! Partial sessions (cancelled before completion) are persisted to
//! `~/.cyberclaw/wizard_session.json` so the next run offers to resume.
//!
//! # Architectural note on the `inquire` choice
//!
//! Architect review P0-3 requires that interactive prompts NOT invent a second
//! protocol alongside the existing `AskUserQuestion` capability. The rule
//! applies when an **orchestrator** wants to prompt a **remote / programmatic**
//! user — then it must flow through `Connector → Capability`. The `onboard`
//! command is a different case: it is a direct human-to-TUI interaction on
//! the local terminal where `AskUserQuestion`'s JSON-over-the-wire shape is
//! not the right surface. `inquire` is the legitimate UI library for that
//! case; `TuiWizardStepHandler` is still a `WizardStepHandler` implementation,
//! so the [`WizardEngine`] remains the single state-machine authority.

use anyhow::{Context, Result};
use async_trait::async_trait;
use clap::Args;
use cyberclaw_control_plane::wizard_engine::{
    build_onboard_plan, default_config_dir, session_to_config, session_to_operator_record,
    write_config, HandlerError, WizardAnswer, WizardEngine, WizardPlan, WizardSession, WizardState,
    WizardStep, WizardStepHandler,
};
#[cfg(test)]
use cyberclaw_control_plane::wizard_engine::{discover_skills, CyberclawConfig};
use cyberclaw_core::users::UsersConfig;
use inquire::{Confirm, MultiSelect, Password, Select, Text};
use std::fs;
use std::path::{Path, PathBuf};

// ============================================================================
// CLI args
// ============================================================================

/// Arguments for the `onboard` subcommand.
#[derive(Debug, Args, Clone, Default)]
pub struct OnboardArgs {
    /// Override the config directory (defaults to `~/.cyberclaw`).
    #[arg(long)]
    pub config_dir: Option<PathBuf>,

    /// Override the source directory for skill discovery (defaults to
    /// `./ecosystem/skills`).
    #[arg(long)]
    pub skills_dir: Option<PathBuf>,

    /// Skip the session-resume check (always start fresh).
    #[arg(long)]
    pub no_resume: bool,

    /// Accept all optional steps without prompting (non-interactive mode).
    /// Also read from env var CYBERCLAW_ONBOARD_YES=1.
    #[arg(long)]
    pub yes: bool,

    /// Skip onboarding entirely — write a marker so subsequent runs also
    /// short-circuit. Use when the user does not want to be prompted again.
    /// To re-enable, delete `~/.cyberclaw/.onboarding_skipped`.
    #[arg(long)]
    pub skip: bool,

    /// Pre-fill display name. Also read from CYBERCLAW_ONBOARD_DISPLAY_NAME.
    #[arg(long)]
    pub display_name: Option<String>,

    /// Pre-fill LLM provider. Also read from CYBERCLAW_ONBOARD_LLM_PROVIDER.
    #[arg(long)]
    pub llm_provider: Option<String>,

    /// Pre-fill LLM API key. Also read from CYBERCLAW_ONBOARD_API_KEY.
    #[arg(long)]
    pub api_key: Option<String>,
}

// ============================================================================
// Session persistence (CLI-specific; `CyberclawConfig` + `write_config` live
// in `cyberclaw_control_plane::wizard_engine` so the Web UI uses the same
// translation logic.)
// ============================================================================

/// Persist a partial session so a future run can resume.
pub fn save_session(config_dir: &Path, session: &WizardSession) -> Result<PathBuf> {
    fs::create_dir_all(config_dir)
        .with_context(|| format!("creating config dir {}", config_dir.display()))?;
    let path = config_dir.join("wizard_session.json");
    let json = serde_json::to_string_pretty(session).context("serializing session")?;
    fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Load a previously persisted session, if any.
pub fn load_session(config_dir: &Path) -> Result<Option<WizardSession>> {
    let path = config_dir.join("wizard_session.json");
    if !path.exists() {
        return Ok(None);
    }
    let data = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let session: WizardSession =
        serde_json::from_str(&data).context("deserializing wizard session")?;
    Ok(Some(session))
}

/// Remove the persisted session file on successful completion.
pub fn clear_session(config_dir: &Path) -> Result<()> {
    let path = config_dir.join("wizard_session.json");
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

// ============================================================================
// TUI handler
// ============================================================================

/// [`WizardStepHandler`] backed by the `inquire` crate. One handler instance
/// per `onboard` invocation.
pub struct TuiWizardStepHandler {
    /// When `true`, optional steps are auto-skipped without prompting.
    pub yes: bool,
    /// Pre-filled answers from CLI flags / env vars.
    pub prefill_display_name: Option<String>,
    pub prefill_llm_provider: Option<String>,
    pub prefill_api_key: Option<String>,
    /// Plan reference for computing step progress.
    pub plan: WizardPlan,
}

impl TuiWizardStepHandler {
    pub fn new() -> Self {
        Self {
            yes: false,
            prefill_display_name: None,
            prefill_llm_provider: None,
            prefill_api_key: None,
            plan: WizardPlan {
                id: String::new(),
                title: String::new(),
                steps: vec![],
            },
        }
    }

    /// Returns `(current_1based, total)` for TUI-visible steps only.
    fn step_progress(&self, step_id: &str) -> (usize, usize) {
        let tui_steps: Vec<_> = self
            .plan
            .steps
            .iter()
            .filter(|s| s.visible_on("tui"))
            .collect();
        let total = tui_steps.len();
        let current = tui_steps
            .iter()
            .position(|s| s.id() == step_id)
            .map(|i| i + 1)
            .unwrap_or(0);
        (current, total)
    }
}

impl Default for TuiWizardStepHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Translate an `inquire::InquireError` into our `HandlerError` taxonomy.
fn inquire_err(err: inquire::InquireError) -> HandlerError {
    use inquire::InquireError;
    match err {
        InquireError::OperationCanceled
        | InquireError::OperationInterrupted
        | InquireError::NotTTY => HandlerError::Cancelled,
        other => HandlerError::Other(other.to_string()),
    }
}

#[async_trait]
impl WizardStepHandler for TuiWizardStepHandler {
    async fn handle(&self, step: &WizardStep) -> Result<WizardAnswer, HandlerError> {
        // Print step progress header for interactive steps.
        let (current, total) = self.step_progress(step.id());
        if current > 0 && total > 0 {
            let kind = if step.is_optional() {
                "optional"
            } else {
                "required"
            };
            println!("\n📋 Step {} of {} ({})", current, total, kind);
        }

        // If --yes and optional, skip without prompting.
        if self.yes && step.is_optional() {
            return Ok(WizardAnswer::ActionDone);
        }

        match step {
            WizardStep::Note { text, .. } => {
                println!("\n{}\n", text);
                Ok(WizardAnswer::ActionDone)
            }

            WizardStep::Select {
                id,
                prompt,
                options,
                default_idx,
                ..
            } => {
                // Handle prefill for llm_provider.
                if id == "llm_provider" {
                    if let Some(ref prov) = self.prefill_llm_provider {
                        if let Some(idx) = options.iter().position(|o| o == prov) {
                            return Ok(WizardAnswer::SelectedIdx(idx));
                        }
                    }
                }
                // --yes mode: pick default_idx (or first option) without
                // prompting. Without this fallback, required Select steps
                // (e.g. connection_mode) block on tty read in
                // non-interactive mode and the wizard cancels.
                if self.yes {
                    let idx = default_idx.unwrap_or(0);
                    return Ok(WizardAnswer::SelectedIdx(idx));
                }
                let mut s = Select::new(prompt, options.clone());
                if let Some(idx) = default_idx {
                    s = s.with_starting_cursor(*idx);
                }
                let picked = s.prompt().map_err(inquire_err)?;
                let idx = options
                    .iter()
                    .position(|o| o == &picked)
                    .ok_or_else(|| HandlerError::Other("selection not in options".into()))?;
                Ok(WizardAnswer::SelectedIdx(idx))
            }

            WizardStep::Text {
                id,
                prompt,
                sensitive,
                default,
                validator,
                ..
            } => {
                // Handle prefilled answers.
                let prefilled = match id.as_str() {
                    "display_name" => self.prefill_display_name.clone(),
                    "llm_api_key" => self.prefill_api_key.clone(),
                    _ => None,
                };
                let val = if let Some(pf) = prefilled {
                    pf
                } else if self.yes {
                    // --yes mode for required Text steps: fall back to the
                    // step's default. If no default, return an empty string
                    // so required-validation surfaces a clear error rather
                    // than blocking on tty.
                    default.clone().unwrap_or_default()
                } else if *sensitive {
                    let p = Password::new(prompt)
                        .without_confirmation()
                        .with_display_mode(inquire::PasswordDisplayMode::Masked);
                    p.prompt().map_err(inquire_err)?
                } else {
                    let mut t = Text::new(prompt);
                    if let Some(d) = default {
                        t = t.with_default(d);
                    }
                    t.prompt().map_err(inquire_err)?
                };

                // Apply the engine's opaque validator key.
                if let Some(key) = validator {
                    if key == "path_exists" && !Path::new(&val).exists() {
                        return Err(HandlerError::ValidationFailed(format!(
                            "path does not exist: {}",
                            val
                        )));
                    }
                }
                Ok(WizardAnswer::Text(val))
            }

            WizardStep::Confirm {
                prompt, default, ..
            } => {
                if self.yes {
                    return Ok(WizardAnswer::ConfirmBool(*default));
                }
                let b = Confirm::new(prompt)
                    .with_default(*default)
                    .prompt()
                    .map_err(inquire_err)?;
                Ok(WizardAnswer::ConfirmBool(b))
            }

            WizardStep::MultiSelect {
                prompt, options, ..
            } => {
                if self.yes {
                    return Ok(WizardAnswer::SelectedSet(Vec::new()));
                }
                let picked = MultiSelect::new(prompt, options.clone())
                    .prompt()
                    .map_err(inquire_err)?;
                let indices: Vec<usize> = picked
                    .iter()
                    .filter_map(|p| options.iter().position(|o| o == p))
                    .collect();
                Ok(WizardAnswer::SelectedSet(indices))
            }

            WizardStep::Progress { label, .. } | WizardStep::Action { label, .. } => {
                println!("... {}", label);
                Ok(WizardAnswer::ActionDone)
            }
        }
    }
}

// ============================================================================
// Entry point
// ============================================================================

/// Run the onboard command. Returns the path of the written config on success.
///
/// Behavior:
/// - If a persisted session exists and `--no-resume` was not passed, prompts
///   the user to resume. Declining discards the old session.
/// - Runs the wizard to completion.
/// - On `Done`, writes `config.toml` and clears any saved session.
/// - On `Cancelled`, persists the session so a future run can resume.
/// - On `Error`, persists the session and returns an error.
pub async fn handle_onboard(args: OnboardArgs) -> Result<()> {
    let config_dir = args.config_dir.unwrap_or_else(default_config_dir);
    let skills_dir = args
        .skills_dir
        .unwrap_or_else(|| PathBuf::from("ecosystem/skills"));

    // --skip: write a marker and exit immediately. Subsequent runs also
    // short-circuit so the user is not prompted again until they delete the
    // marker file.
    let skip_marker = config_dir.join(".onboarding_skipped");
    if args.skip {
        fs::create_dir_all(&config_dir)
            .with_context(|| format!("creating config dir {}", config_dir.display()))?;
        fs::write(&skip_marker, "skipped\n")
            .with_context(|| format!("writing {}", skip_marker.display()))?;
        println!(
            "Onboarding skipped. To re-enable, delete {}",
            skip_marker.display()
        );
        return Ok(());
    }
    if skip_marker.exists() {
        println!(
            "Onboarding previously skipped. Delete {} to re-prompt next time.",
            skip_marker.display()
        );
        return Ok(());
    }

    // Resolve env-var overrides.
    let yes = args.yes
        || std::env::var("CYBERCLAW_ONBOARD_YES")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    let prefill_display_name = args
        .display_name
        .or_else(|| std::env::var("CYBERCLAW_ONBOARD_DISPLAY_NAME").ok());
    let prefill_llm_provider = args
        .llm_provider
        .or_else(|| std::env::var("CYBERCLAW_ONBOARD_LLM_PROVIDER").ok());
    let prefill_api_key = args
        .api_key
        .or_else(|| std::env::var("CYBERCLAW_ONBOARD_API_KEY").ok());

    let plan = build_onboard_plan(&skills_dir);

    // Figure out whether to resume.
    let session = if args.no_resume {
        WizardSession::new(new_session_id(), &plan)
    } else {
        match load_session(&config_dir)? {
            Some(prior) if prior.plan_id == plan.id => {
                let resume = Confirm::new(&format!(
                    "Found a paused onboarding session at step {}. Resume?",
                    prior.current_step_idx + 1
                ))
                .with_default(true)
                .prompt()
                .unwrap_or(false);
                if resume {
                    // Reset state to Running in case it was saved as
                    // Cancelled; the engine's early-return would otherwise
                    // no-op.
                    let mut s = prior;
                    s.state = WizardState::Running;
                    s
                } else {
                    let _ = clear_session(&config_dir);
                    WizardSession::new(new_session_id(), &plan)
                }
            }
            _ => WizardSession::new(new_session_id(), &plan),
        }
    };

    let handler = TuiWizardStepHandler {
        yes,
        prefill_display_name,
        prefill_llm_provider,
        prefill_api_key,
        plan: plan.clone(),
    };
    let engine = WizardEngine::new().with_surface("tui");
    let finished = engine.run(&plan, &handler, session).await;

    match &finished.state {
        WizardState::Done => {
            let cfg = session_to_config(&finished, &plan);
            let path = write_config(&config_dir, &cfg)?;

            // Sprint 5: persist the operator identity alongside the config.
            let users_path = config_dir.join("users.toml");
            let mut users_cfg =
                UsersConfig::load_from_path(&users_path).unwrap_or_else(|_| UsersConfig::default());
            let record = session_to_operator_record(&finished);
            users_cfg.upsert(record);
            users_cfg
                .save_to_path(&users_path)
                .with_context(|| format!("writing {}", users_path.display()))?;

            let _ = clear_session(&config_dir);
            println!("\nConfiguration written to {}", path.display());
            println!("Operator identity written to {}", users_path.display());
            Ok(())
        }
        WizardState::Cancelled => {
            let saved = save_session(&config_dir, &finished)?;
            println!(
                "\nOnboarding cancelled. Partial progress saved to {} — rerun `cyberclaw onboard` to resume.",
                saved.display()
            );
            Ok(())
        }
        WizardState::Error(msg) => {
            let _ = save_session(&config_dir, &finished);
            anyhow::bail!("onboarding error: {}", msg)
        }
        WizardState::Running => {
            // The engine does not return from run() while Running.
            anyhow::bail!("engine returned a running session; this is a bug")
        }
    }
}

fn new_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("onboard-{}", nanos)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn build_onboard_plan_has_fourteen_steps() {
        let dir = tempdir().unwrap();
        let plan = build_onboard_plan(dir.path());
        // v2 plan: 14 steps covering TUI + WebUI surfaces.
        assert_eq!(plan.steps.len(), 14);
        assert_eq!(plan.steps[0].id(), "welcome");
        assert_eq!(plan.steps[1].id(), "display_name");
        assert_eq!(plan.steps[12].id(), "install_alias");
        assert_eq!(plan.id, "cyberclaw-onboard-v2");
    }

    #[test]
    fn build_onboard_plan_validates() {
        let dir = tempdir().unwrap();
        let plan = build_onboard_plan(dir.path());
        assert!(plan.validate().is_ok());
    }

    #[test]
    fn discover_skills_reads_ecosystem_dir() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("plan")).unwrap();
        fs::create_dir_all(dir.path().join("verify")).unwrap();
        fs::create_dir_all(dir.path().join("debug")).unwrap();
        fs::write(dir.path().join("README.md"), "hi").unwrap();

        let skills = discover_skills(dir.path());
        assert_eq!(skills.len(), 3);
        assert!(skills.contains(&"plan".to_string()));
        assert!(skills.contains(&"verify".to_string()));
    }

    #[test]
    fn write_config_creates_parent_dir_and_toml() {
        let tmp = tempdir().unwrap();
        let cfg_dir = tmp.path().join("nested").join(".cyberclaw");
        let cfg = CyberclawConfig {
            connection_mode: "local".into(),
            llm_provider: "anthropic".into(),
            llm_api_key: Some("k".into()),
            workspace_root: "/tmp".into(),
            enabled_skills: vec!["plan".into()],
            install_cli_alias: false,
        };

        let path = write_config(&cfg_dir, &cfg).unwrap();
        assert!(path.exists());
        let round: CyberclawConfig = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(round.llm_provider, "anthropic");
        assert_eq!(round.enabled_skills, vec!["plan".to_string()]);
    }

    #[test]
    fn session_save_load_roundtrip() {
        let tmp = tempdir().unwrap();
        let skills_dir = tempdir().unwrap();
        let plan = build_onboard_plan(skills_dir.path());
        let mut session = WizardSession::new("s-42", &plan);
        session.current_step_idx = 3;
        session
            .answers
            .insert("connection_mode".into(), WizardAnswer::SelectedIdx(0));

        let saved = save_session(tmp.path(), &session).unwrap();
        assert!(saved.exists());

        let loaded = load_session(tmp.path()).unwrap().expect("session present");
        assert_eq!(loaded.session_id, "s-42");
        assert_eq!(loaded.current_step_idx, 3);
        assert_eq!(
            loaded.answers.get("connection_mode"),
            Some(&WizardAnswer::SelectedIdx(0))
        );

        clear_session(tmp.path()).unwrap();
        assert!(load_session(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn session_to_config_translates_answers() {
        let skills_dir = tempdir().unwrap();
        fs::create_dir_all(skills_dir.path().join("plan")).unwrap();
        let plan = build_onboard_plan(skills_dir.path());
        let mut session = WizardSession::new("s-1", &plan);
        session
            .answers
            .insert("connection_mode".into(), WizardAnswer::SelectedIdx(1));
        session
            .answers
            .insert("llm_provider".into(), WizardAnswer::SelectedIdx(2));
        session
            .answers
            .insert("llm_api_key".into(), WizardAnswer::Text("sk-xxx".into()));
        session
            .answers
            .insert("workspace_root".into(), WizardAnswer::Text("/tmp".into()));
        session
            .answers
            .insert("enabled_skills".into(), WizardAnswer::SelectedSet(vec![0]));
        session
            .answers
            .insert("install_alias".into(), WizardAnswer::ConfirmBool(true));

        let cfg = session_to_config(&session, &plan);
        assert_eq!(cfg.connection_mode, "remote");
        assert_eq!(cfg.llm_provider, "openrouter");
        assert_eq!(cfg.llm_api_key.as_deref(), Some("sk-xxx"));
        assert_eq!(cfg.workspace_root, "/tmp");
        assert_eq!(cfg.enabled_skills, vec!["plan".to_string()]);
        assert!(cfg.install_cli_alias);
    }

    #[test]
    fn onboard_args_default() {
        // The Clap subcommand must be constructible with defaults (used by
        // the no-arg invocation path).
        let a = OnboardArgs::default();
        assert!(a.config_dir.is_none());
        assert!(a.skills_dir.is_none());
        assert!(!a.no_resume);
    }
}
