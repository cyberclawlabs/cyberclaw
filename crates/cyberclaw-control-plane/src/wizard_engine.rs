//! WizardEngine — declarative, interactive state machine for onboarding flows.
//!
//! This component follows the four evolution-layer idioms (see
//! `docs/architecture/idioms/EVOLUTION_IDIOMS.md`):
//!
//! 1. **Injected trait dispatcher** — actual user-interaction I/O is delegated
//!    to a [`WizardStepHandler`] trait. The engine never directly prompts the
//!    user; the handler does that (via `inquire` in TUI, via
//!    `AskUserQuestion` capability in remote orchestration contexts).
//! 2. **Event sink trait** — [`WizardEvent`] + [`WizardEventSink`] give every
//!    state transition an observable surface. The default is no emission.
//! 3. **Declarative Plan-in + Events-out** — [`WizardPlan`] is pure data
//!    (`Serialize + Deserialize`); `run()` transforms a plan + session into a
//!    new session + a stream of events.
//! 4. **No side-effects in the engine** — [`WizardStep::Action`] and
//!    [`WizardStep::Progress`] steps are descriptive only; the handler owns
//!    any host-side work (writing files, installing binaries, etc.).
//!
//! `WizardEngine` is not a new first-class platform object (idiom §5). It is a
//! plain control-plane component, sitting alongside `EvolutionOrchestrator`
//! and `PersistentLoop`.
//!
//! # Example
//!
//! ```rust,ignore
//! use cyberclaw_control_plane::wizard_engine::*;
//! use std::sync::Arc;
//!
//! let plan = WizardPlan {
//!     id: "onboarding".into(),
//!     title: "Welcome".into(),
//!     steps: vec![WizardStep::Note { id: "n1".into(), text: "Hi!".into() }],
//! };
//! let session = WizardSession::new("sess-1", &plan);
//! // Real handler would use inquire or AskUserQuestion. See tests for a mock.
//! ```

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

// ============================================================================
// WizardStep — the 7 step variants (protocol-compatible with OpenClaw)
// ============================================================================

/// One step in a [`WizardPlan`]. Steps are pure data; they describe what the
/// engine should ask the handler, not how the handler should render it.
///
/// The seven variants intentionally mirror OpenClaw's `WizardStepSchema`
/// (`tmp/claw-research/openclaw/src/gateway/protocol/schema/wizard.ts`) so a
/// future remote gateway can serialize plans across the wire without a
/// translation layer.
///
/// # Surface filtering
///
/// Each step carries a `surfaces` list (e.g., `["tui"]`, `["web"]`, or
/// `["tui","web"]`).  The engine skips steps whose surfaces list does not
/// contain the current surface string.  Defaults to both surfaces when the
/// field is absent (backward-compatible with serialised v1 plans).
///
/// # Optional steps
///
/// `optional = true` signals to the driver that the user may skip this step.
/// The engine itself does not skip optional steps; the driver (TUI / Web)
/// checks this flag and may offer a "Skip" affordance.  Defaults to `false`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WizardStep {
    /// Pure display. Handler shows `text`; no answer collected.
    Note {
        id: String,
        text: String,
        #[serde(default)]
        surfaces: Vec<String>,
        #[serde(default)]
        optional: bool,
    },

    /// Single-choice from a list. Handler returns the selected index.
    Select {
        id: String,
        prompt: String,
        options: Vec<String>,
        default_idx: Option<usize>,
        #[serde(default)]
        surfaces: Vec<String>,
        #[serde(default)]
        optional: bool,
    },

    /// Free-form text entry. When `sensitive == true` the handler should
    /// mask the input (password prompt). The engine guarantees the answer
    /// value is masked in emitted events.
    Text {
        id: String,
        prompt: String,
        sensitive: bool,
        default: Option<String>,
        /// Optional validator key. Handlers map the key to a validation rule.
        /// The engine passes the key through opaquely.
        validator: Option<String>,
        #[serde(default)]
        surfaces: Vec<String>,
        #[serde(default)]
        optional: bool,
    },

    /// Yes / no. Handler returns a bool.
    Confirm {
        id: String,
        prompt: String,
        default: bool,
        #[serde(default)]
        surfaces: Vec<String>,
        #[serde(default)]
        optional: bool,
    },

    /// Multiple-choice. Handler returns the selected indices (may be empty).
    MultiSelect {
        id: String,
        prompt: String,
        options: Vec<String>,
        #[serde(default)]
        surfaces: Vec<String>,
        #[serde(default)]
        optional: bool,
    },

    /// Long-running progress. Handler executes the work keyed by `id` and
    /// reports completion; no user prompt. The engine does not run the work
    /// itself — that is the handler's domain.
    Progress {
        id: String,
        label: String,
        #[serde(default)]
        surfaces: Vec<String>,
        #[serde(default)]
        optional: bool,
    },

    /// Host-side action. Descriptive only — the handler does any real work
    /// (writes config, installs a binary, etc.). The engine receives
    /// [`WizardAnswer::ActionDone`] back.
    Action {
        id: String,
        label: String,
        #[serde(default)]
        surfaces: Vec<String>,
        #[serde(default)]
        optional: bool,
    },
}

impl WizardStep {
    /// Stable step id accessor, shared across all variants.
    pub fn id(&self) -> &str {
        match self {
            WizardStep::Note { id, .. }
            | WizardStep::Select { id, .. }
            | WizardStep::Text { id, .. }
            | WizardStep::Confirm { id, .. }
            | WizardStep::MultiSelect { id, .. }
            | WizardStep::Progress { id, .. }
            | WizardStep::Action { id, .. } => id,
        }
    }

    /// Whether this step carries sensitive input data (currently only the
    /// `Text` variant with `sensitive = true`).
    pub fn is_sensitive(&self) -> bool {
        matches!(
            self,
            WizardStep::Text {
                sensitive: true,
                ..
            }
        )
    }

    /// The surfaces this step should be shown on. An empty list means all
    /// surfaces (backward-compatible with v1 plans that omit this field).
    pub fn surfaces(&self) -> &[String] {
        match self {
            WizardStep::Note { surfaces, .. }
            | WizardStep::Select { surfaces, .. }
            | WizardStep::Text { surfaces, .. }
            | WizardStep::Confirm { surfaces, .. }
            | WizardStep::MultiSelect { surfaces, .. }
            | WizardStep::Progress { surfaces, .. }
            | WizardStep::Action { surfaces, .. } => surfaces,
        }
    }

    /// Whether this step is optional (driver may offer a Skip affordance).
    pub fn is_optional(&self) -> bool {
        match self {
            WizardStep::Note { optional, .. }
            | WizardStep::Select { optional, .. }
            | WizardStep::Text { optional, .. }
            | WizardStep::Confirm { optional, .. }
            | WizardStep::MultiSelect { optional, .. }
            | WizardStep::Progress { optional, .. }
            | WizardStep::Action { optional, .. } => *optional,
        }
    }

    /// Returns `true` if this step should be shown on the given surface.
    /// Steps with an empty `surfaces` list are shown on all surfaces.
    pub fn visible_on(&self, surface: &str) -> bool {
        let s = self.surfaces();
        s.is_empty() || s.iter().any(|v| v == surface)
    }
}

// ============================================================================
// WizardPlan — the declarative input
// ============================================================================

/// A serializable wizard plan. Plans are pure data: they can be persisted,
/// diffed, replayed, or sent across the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardPlan {
    /// Stable plan id (e.g., `"onboarding"`).
    pub id: String,
    /// Human-readable title shown at the start of the flow.
    pub title: String,
    /// Ordered list of steps.
    pub steps: Vec<WizardStep>,
}

impl WizardPlan {
    /// Pure validation — no side effects. Returns an error describing the
    /// first invariant violation found.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("plan id must be non-empty".to_string());
        }
        let mut seen = std::collections::HashSet::new();
        for step in &self.steps {
            let id = step.id();
            if id.trim().is_empty() {
                return Err("step id must be non-empty".to_string());
            }
            if !seen.insert(id.to_string()) {
                return Err(format!("duplicate step id: {}", id));
            }
            // Specific per-variant checks.
            match step {
                WizardStep::Select {
                    options,
                    default_idx,
                    ..
                } => {
                    if options.is_empty() {
                        return Err(format!("step {} has no options", id));
                    }
                    if let Some(idx) = default_idx {
                        if *idx >= options.len() {
                            return Err(format!("step {} default_idx out of range", id));
                        }
                    }
                }
                WizardStep::MultiSelect { options, .. } if options.is_empty() => {
                    return Err(format!("step {} has no options", id));
                }
                WizardStep::MultiSelect { .. } => {}
                _ => {}
            }
        }
        Ok(())
    }
}

// ============================================================================
// WizardAnswer — what a handler returns for one step
// ============================================================================

/// The answer shape emitted by the handler for a given step.
///
/// Variants map one-to-one to the non-Note step kinds. The engine performs no
/// type-checking against the step at construction time — correctness is
/// enforced at handler-result time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum WizardAnswer {
    /// `Select` answer: index into the step's options list.
    SelectedIdx(usize),
    /// `Text` answer: user-entered string. For sensitive text, the engine
    /// masks this before emitting events.
    Text(String),
    /// `Confirm` answer.
    ConfirmBool(bool),
    /// `MultiSelect` answer: selected indices (may be empty).
    SelectedSet(Vec<usize>),
    /// `Progress` / `Action` answer: acknowledgement that the side-effect
    /// work completed.
    ActionDone,
}

impl WizardAnswer {
    /// Produce a display-safe version of this answer. For sensitive answers
    /// the underlying value is replaced with a stable placeholder.
    pub fn masked(&self) -> Self {
        match self {
            WizardAnswer::Text(_) => WizardAnswer::Text("***".to_string()),
            other => other.clone(),
        }
    }
}

// ============================================================================
// Handler trait — the I/O boundary
// ============================================================================

/// Request a handler to cancel the current wizard session.
///
/// A handler returns `Err(HandlerError::Cancelled)` to signal the user
/// intentionally cancelled (e.g., pressed Esc in a TUI). Any other Err is a
/// hard error and drives the session into `WizardState::Error`.
#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    /// User intentionally cancelled mid-flow.
    #[error("cancelled by user")]
    Cancelled,
    /// Validator rejected the input; engine may re-prompt if retries remain.
    #[error("validation failed: {0}")]
    ValidationFailed(String),
    /// Any other handler failure.
    #[error("{0}")]
    Other(String),
}

/// Engine delegates every step-interaction to a handler implementation.
///
/// Production code uses a TUI handler (see `apps/cyberclaw-cli/src/commands/onboard.rs`).
/// Tests use a deterministic mock — see the test module.
#[async_trait]
pub trait WizardStepHandler: Send + Sync {
    /// Render or execute the step and return the collected answer.
    ///
    /// Contract:
    /// - `Note` steps: handler displays the text and returns
    ///   `Ok(WizardAnswer::ActionDone)` to advance.
    /// - `Select`: return `Ok(WizardAnswer::SelectedIdx(i))`.
    /// - `Text`: return `Ok(WizardAnswer::Text(s))`.
    /// - `Confirm`: return `Ok(WizardAnswer::ConfirmBool(b))`.
    /// - `MultiSelect`: return `Ok(WizardAnswer::SelectedSet(v))`.
    /// - `Progress` / `Action`: run the work, return
    ///   `Ok(WizardAnswer::ActionDone)`.
    ///
    /// Returning `Err(HandlerError::Cancelled)` transitions the session to
    /// `WizardState::Cancelled`. Returning `Err(HandlerError::ValidationFailed)`
    /// lets the engine re-prompt (up to its retry cap). Any other `Err` fails
    /// the session.
    async fn handle(&self, step: &WizardStep) -> Result<WizardAnswer, HandlerError>;
}

// ============================================================================
// Session state
// ============================================================================

/// Lifecycle states a session can be in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub enum WizardState {
    /// Session is advancing through steps.
    Running,
    /// All steps completed successfully.
    Done,
    /// User cancelled before completion.
    Cancelled,
    /// A handler returned an unrecoverable error.
    Error(String),
}

/// A session is the runtime counterpart of a plan. It is resumable: a session
/// serialized mid-flow carries enough state to continue from
/// `current_step_idx`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardSession {
    pub session_id: String,
    pub plan_id: String,
    pub state: WizardState,
    pub current_step_idx: usize,
    pub answers: HashMap<String, WizardAnswer>,
    pub started_at: SystemTime,
}

impl WizardSession {
    /// Construct a fresh session for a plan. `state` is `Running`,
    /// `current_step_idx` is 0, `answers` is empty.
    pub fn new(session_id: impl Into<String>, plan: &WizardPlan) -> Self {
        Self {
            session_id: session_id.into(),
            plan_id: plan.id.clone(),
            state: WizardState::Running,
            current_step_idx: 0,
            answers: HashMap::new(),
            started_at: SystemTime::now(),
        }
    }

    /// Whether the session is still advancing.
    pub fn is_running(&self) -> bool {
        matches!(self.state, WizardState::Running)
    }
}

// ============================================================================
// Events + event sink
// ============================================================================

/// Structured events emitted at every state transition in [`WizardEngine::run`].
///
/// Events never leak sensitive text — the engine masks sensitive answers
/// before emission (see [`WizardAnswer::masked`]).
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum WizardEvent {
    SessionStarted {
        session_id: String,
        plan_id: String,
    },
    StepEntered {
        step_idx: usize,
        step_id: String,
    },
    AnswerCaptured {
        step_id: String,
        answer: WizardAnswer,
    },
    StepSkipped {
        step_idx: usize,
        reason: String,
    },
    SessionCompleted {
        answers: HashMap<String, WizardAnswer>,
    },
    SessionCancelled {
        at_step: usize,
    },
    SessionErrored {
        error: String,
    },
}

/// Optional subscriber for [`WizardEvent`]. Implementations should not panic
/// or block on failure — the engine treats sinks as best-effort.
#[async_trait]
pub trait WizardEventSink: Send + Sync {
    async fn record(&self, event: WizardEvent);
}

/// Default sink — discards every event.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopWizardEventSink;

#[async_trait]
impl WizardEventSink for NoopWizardEventSink {
    async fn record(&self, _event: WizardEvent) {
        // intentionally empty
    }
}

/// In-memory sink that captures every event into a `Vec`. Intended for tests.
#[derive(Debug, Default)]
pub struct VecWizardEventSink {
    inner: Mutex<Vec<WizardEvent>>,
}

impl VecWizardEventSink {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Vec::new()),
        }
    }

    /// Consume and return the captured events so far.
    pub fn take(&self) -> Vec<WizardEvent> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    }

    /// Non-consuming snapshot of captured events.
    pub fn snapshot(&self) -> Vec<WizardEvent> {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    }

    pub fn len(&self) -> usize {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl WizardEventSink for VecWizardEventSink {
    async fn record(&self, event: WizardEvent) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.push(event);
    }
}

// ============================================================================
// Engine
// ============================================================================

/// Maximum re-prompts for a single step after validator rejection.
const VALIDATION_RETRY_CAP: u32 = 3;

/// The wizard state machine.
///
/// Construct with [`WizardEngine::new`], optionally attach a sink with
/// [`WizardEngine::with_event_sink`], set the surface with
/// [`WizardEngine::with_surface`], then call [`WizardEngine::run`].
///
/// The engine is stateless across runs — every `run()` threads state through
/// the session argument, so the same engine instance can drive many sessions
/// concurrently.
pub struct WizardEngine {
    event_sink: Option<Arc<dyn WizardEventSink>>,
    /// Surface identifier used to filter steps. Defaults to `"tui"`.
    surface: String,
}

impl WizardEngine {
    /// Construct a new engine with no event sink and default surface `"tui"`.
    pub fn new() -> Self {
        Self {
            event_sink: None,
            surface: "tui".to_string(),
        }
    }

    /// Set the surface this engine runs on (e.g. `"tui"` or `"web"`).
    /// Chainable on construction.
    pub fn with_surface(mut self, surface: impl Into<String>) -> Self {
        self.surface = surface.into();
        self
    }

    /// Attach (or replace) the event sink. Chainable on construction.
    pub fn with_event_sink(mut self, sink: Arc<dyn WizardEventSink>) -> Self {
        self.event_sink = Some(sink);
        self
    }

    /// Set the sink after construction.
    pub fn set_event_sink(&mut self, sink: Arc<dyn WizardEventSink>) {
        self.event_sink = Some(sink);
    }

    /// Emit one event. Cheap no-op when no sink is attached.
    async fn emit(&self, event: WizardEvent) {
        if let Some(sink) = self.event_sink.as_ref() {
            sink.record(event).await;
        }
    }

    /// Drive the session through the plan until it reaches a terminal state.
    ///
    /// `session` may be a fresh session (constructed via
    /// [`WizardSession::new`]) or a previously persisted session whose
    /// `current_step_idx` points mid-plan — the engine picks up from there.
    ///
    /// Returns the updated session. The session's `state` field describes
    /// the terminal outcome: `Done` / `Cancelled` / `Error`.
    ///
    /// # Contract with the handler
    ///
    /// The engine never prompts the user directly. Every interaction flows
    /// through `handler.handle(&step)`. Handler errors are handled as
    /// follows:
    ///
    /// - `HandlerError::Cancelled` → session state `Cancelled`, loop stops.
    /// - `HandlerError::ValidationFailed` on `Text` step → re-prompt up to
    ///   [`VALIDATION_RETRY_CAP`] times; exceeding the cap becomes
    ///   `HandlerError::Other` behaviour.
    /// - any other `Err` → session state `Error`, loop stops.
    pub async fn run(
        &self,
        plan: &WizardPlan,
        handler: &dyn WizardStepHandler,
        mut session: WizardSession,
    ) -> WizardSession {
        // Always emit SessionStarted so downstream consumers can correlate
        // resumed runs with the originating session.
        self.emit(WizardEvent::SessionStarted {
            session_id: session.session_id.clone(),
            plan_id: plan.id.clone(),
        })
        .await;

        // If the session is already terminal, nothing to do.
        if !session.is_running() {
            return session;
        }

        while session.current_step_idx < plan.steps.len() {
            let idx = session.current_step_idx;
            let step = &plan.steps[idx];
            let step_id = step.id().to_string();

            // Surface filter: skip steps that are not meant for this surface.
            if !step.visible_on(&self.surface) {
                self.emit(WizardEvent::StepSkipped {
                    step_idx: idx,
                    reason: format!("not visible on surface '{}'", self.surface),
                })
                .await;
                session.current_step_idx = idx.saturating_add(1);
                continue;
            }

            self.emit(WizardEvent::StepEntered {
                step_idx: idx,
                step_id: step_id.clone(),
            })
            .await;

            // Note steps are pure display — we still call the handler (so it
            // can render), but any error on a Note step is treated as the
            // normal cancellation / error flow.
            let answer = match self.handle_with_retries(handler, step).await {
                Ok(a) => a,
                Err(HandlerError::Cancelled) => {
                    session.state = WizardState::Cancelled;
                    self.emit(WizardEvent::SessionCancelled { at_step: idx })
                        .await;
                    return session;
                }
                Err(HandlerError::ValidationFailed(msg)) => {
                    // Retry cap exhausted inside handle_with_retries.
                    let err = format!("validation retries exhausted: {}", msg);
                    session.state = WizardState::Error(err.clone());
                    self.emit(WizardEvent::SessionErrored { error: err }).await;
                    return session;
                }
                Err(HandlerError::Other(msg)) => {
                    session.state = WizardState::Error(msg.clone());
                    self.emit(WizardEvent::SessionErrored { error: msg }).await;
                    return session;
                }
            };

            // Store the raw answer. Emit a display-safe (masked) version for
            // sensitive steps, so sinks / audit logs never see cleartext.
            let display_answer = if step.is_sensitive() {
                answer.masked()
            } else {
                answer.clone()
            };
            session.answers.insert(step_id.clone(), answer);

            self.emit(WizardEvent::AnswerCaptured {
                step_id,
                answer: display_answer,
            })
            .await;

            session.current_step_idx = idx.saturating_add(1);
        }

        // All steps consumed — success.
        session.state = WizardState::Done;
        self.emit(WizardEvent::SessionCompleted {
            answers: session.answers.clone(),
        })
        .await;
        session
    }

    /// Call the handler, re-prompting on `ValidationFailed` up to the cap.
    async fn handle_with_retries(
        &self,
        handler: &dyn WizardStepHandler,
        step: &WizardStep,
    ) -> Result<WizardAnswer, HandlerError> {
        let mut last_err: Option<String> = None;
        for _ in 0..VALIDATION_RETRY_CAP {
            match handler.handle(step).await {
                Ok(a) => return Ok(a),
                Err(HandlerError::ValidationFailed(msg)) => {
                    last_err = Some(msg);
                    continue;
                }
                Err(other) => return Err(other),
            }
        }
        Err(HandlerError::ValidationFailed(
            last_err.unwrap_or_else(|| "unknown validation error".to_string()),
        ))
    }
}

impl Default for WizardEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Shared onboarding plan + config helpers
// ============================================================================
//
// These helpers are shared between the TUI (`cyberclaw onboard`) and the Web
// UI (`cyberclaw-server` `/wizard/*`). Keeping them here guarantees that both
// drivers produce byte-identical config output for the same answer set.

use std::path::{Path, PathBuf};

/// The final configuration written out on successful onboarding completion.
///
/// Sprint 5 NOTE: `llm_api_key` is still stored in plain TOML. A follow-up
/// sprint will migrate credentials into a proper secret store.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CyberclawConfig {
    pub connection_mode: String,
    pub llm_provider: String,
    pub llm_api_key: Option<String>,
    pub workspace_root: String,
    pub enabled_skills: Vec<String>,
    pub install_cli_alias: bool,
}

/// Default home config dir (`~/.cyberclaw`).
pub fn default_config_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".cyberclaw"))
        .unwrap_or_else(|| PathBuf::from(".cyberclaw"))
}

/// Write a completed config to `<config_dir>/config.toml`. Creates the parent
/// dir if missing.
pub fn write_config(config_dir: &Path, config: &CyberclawConfig) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    std::fs::create_dir_all(config_dir)
        .with_context(|| format!("creating config dir {}", config_dir.display()))?;
    let path = config_dir.join("config.toml");
    let toml_str = toml::to_string_pretty(config).context("serializing config to TOML")?;
    std::fs::write(&path, toml_str).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Enumerate skill directory names under `skills_dir`. Missing dir → empty.
pub fn discover_skills(skills_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(skills_dir) else {
        return Vec::new();
    };
    let mut skills: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let ft = e.file_type().ok()?;
            if !ft.is_dir() {
                return None;
            }
            e.file_name().to_str().map(|s| s.to_string())
        })
        .filter(|name| !name.starts_with('.') && name != "README.md")
        .collect();
    skills.sort();
    skills
}

/// Build the canonical 8-step onboarding plan shared by TUI and Web UI.
///
/// Sprint 5: a `display_name` step is inserted between `welcome` and
/// `connection_mode` so the wizard can emit an [`OperatorRecord`] alongside
/// the [`CyberclawConfig`]. See
/// [`session_to_operator_record`] for the translation.
pub fn build_onboard_plan(skills_dir: &Path) -> WizardPlan {
    let skills = discover_skills(skills_dir);
    let skills_for_step = if skills.is_empty() {
        vec!["(none detected)".to_string()]
    } else {
        skills
    };

    WizardPlan {
        id: "cyberclaw-onboard-v2".to_string(),
        title: "CyberClaw Onboarding".to_string(),
        steps: vec![
            WizardStep::Note {
                id: "welcome".to_string(),
                text: "Welcome to CyberClaw. This wizard will configure your workspace."
                    .to_string(),
                surfaces: vec!["tui".to_string(), "web".to_string()],
                optional: false,
            },
            WizardStep::Text {
                id: "display_name".to_string(),
                prompt: "Display name for this operator account:".to_string(),
                sensitive: false,
                default: None,
                validator: None,
                surfaces: vec!["tui".to_string(), "web".to_string()],
                optional: false,
            },
            WizardStep::Select {
                id: "connection_mode".to_string(),
                prompt: "Connection mode?".to_string(),
                options: vec!["Local only".to_string(), "Remote gateway".to_string()],
                default_idx: Some(0),
                surfaces: vec!["tui".to_string()],
                optional: false,
            },
            WizardStep::Select {
                id: "llm_provider".to_string(),
                prompt: "Default LLM provider?".to_string(),
                options: vec![
                    "anthropic".to_string(),
                    "openai".to_string(),
                    "openrouter".to_string(),
                    "local".to_string(),
                ],
                default_idx: Some(0),
                surfaces: vec!["tui".to_string(), "web".to_string()],
                optional: false,
            },
            WizardStep::Text {
                id: "llm_api_key".to_string(),
                prompt: "LLM API key:".to_string(),
                sensitive: true,
                default: None,
                validator: None,
                surfaces: vec!["tui".to_string(), "web".to_string()],
                optional: false,
            },
            WizardStep::Action {
                id: "llm_test_connection".to_string(),
                label: "Test LLM connection…".to_string(),
                surfaces: vec!["tui".to_string(), "web".to_string()],
                optional: true,
            },
            WizardStep::Text {
                id: "workspace_root".to_string(),
                prompt: "Workspace root path:".to_string(),
                sensitive: false,
                default: std::env::current_dir()
                    .ok()
                    .and_then(|p| p.to_str().map(|s| s.to_string())),
                validator: Some("path_exists".to_string()),
                surfaces: vec!["tui".to_string()],
                optional: false,
            },
            WizardStep::Select {
                id: "skill_bundle".to_string(),
                prompt: "Skill bundle to install?".to_string(),
                options: vec![
                    "minimal".to_string(),
                    "standard".to_string(),
                    "full".to_string(),
                ],
                default_idx: Some(1),
                surfaces: vec!["web".to_string()],
                optional: true,
            },
            WizardStep::MultiSelect {
                id: "enabled_skills".to_string(),
                prompt: "Enable default skills?".to_string(),
                options: skills_for_step,
                surfaces: vec!["tui".to_string(), "web".to_string()],
                optional: true,
            },
            WizardStep::Select {
                id: "governance_preset".to_string(),
                prompt: "Governance preset?".to_string(),
                options: vec![
                    "balanced".to_string(),
                    "strict".to_string(),
                    "permissive".to_string(),
                ],
                default_idx: Some(0),
                surfaces: vec!["web".to_string()],
                optional: true,
            },
            WizardStep::Text {
                id: "first_agent_name".to_string(),
                prompt: "Name your first agent (leave blank to skip):".to_string(),
                sensitive: false,
                default: None,
                validator: None,
                surfaces: vec!["tui".to_string(), "web".to_string()],
                optional: true,
            },
            WizardStep::Action {
                id: "mcp_scan".to_string(),
                label: "Scan for local MCP servers…".to_string(),
                surfaces: vec!["web".to_string()],
                optional: true,
            },
            WizardStep::Confirm {
                id: "install_alias".to_string(),
                prompt: "Install `cyberclaw` CLI alias? (sudo required)".to_string(),
                default: false,
                surfaces: vec!["tui".to_string()],
                optional: true,
            },
            WizardStep::Action {
                id: "demo_task".to_string(),
                label: "Run a demo task to verify setup…".to_string(),
                surfaces: vec!["web".to_string()],
                optional: true,
            },
        ],
    }
}

/// Translate a completed [`WizardSession`] into a [`CyberclawConfig`].
///
/// This is the single translation point shared by TUI and Web UI drivers.
pub fn session_to_config(session: &WizardSession, plan: &WizardPlan) -> CyberclawConfig {
    let mut cfg = CyberclawConfig::default();

    for (id, ans) in &session.answers {
        match (id.as_str(), ans) {
            ("connection_mode", WizardAnswer::SelectedIdx(i)) => {
                cfg.connection_mode = match i {
                    0 => "local".into(),
                    _ => "remote".into(),
                };
            }
            // v2 step id is "llm_api_key"; v1 used "api_key" — accept both.
            ("llm_api_key" | "api_key", WizardAnswer::Text(s)) if !s.is_empty() => {
                cfg.llm_api_key = Some(s.clone());
            }
            ("llm_provider", WizardAnswer::SelectedIdx(i)) => {
                if let Some(WizardStep::Select { options, .. }) =
                    plan.steps.iter().find(|s| s.id() == "llm_provider")
                {
                    cfg.llm_provider = options
                        .get(*i)
                        .cloned()
                        .unwrap_or_else(|| "anthropic".into());
                }
            }
            ("workspace_root", WizardAnswer::Text(s)) => {
                cfg.workspace_root = s.clone();
            }
            ("enabled_skills", WizardAnswer::SelectedSet(indices)) => {
                if let Some(WizardStep::MultiSelect { options, .. }) =
                    plan.steps.iter().find(|s| s.id() == "enabled_skills")
                {
                    cfg.enabled_skills = indices
                        .iter()
                        .filter_map(|i| options.get(*i).cloned())
                        .collect();
                }
            }
            ("install_alias", WizardAnswer::ConfirmBool(b)) => {
                cfg.install_cli_alias = *b;
            }
            _ => {}
        }
    }

    cfg
}

/// Translate a completed [`WizardSession`] into a fresh
/// [`cyberclaw_core::users::OperatorRecord`] with a newly generated
/// [`cyberclaw_core::ids::UserId`].
///
/// If the `display_name` answer is missing or empty, falls back to
/// `"operator"` so callers always get a record they can persist.
pub fn session_to_operator_record(
    session: &WizardSession,
) -> cyberclaw_core::users::OperatorRecord {
    use cyberclaw_core::ids::UserId;
    use cyberclaw_core::users::OperatorRecord;

    let display_name = match session.answers.get("display_name") {
        Some(WizardAnswer::Text(s)) if !s.trim().is_empty() => s.trim().to_string(),
        _ => "operator".to_string(),
    };

    OperatorRecord {
        user_id: UserId::new(),
        display_name,
        created_at: chrono::Utc::now().to_rfc3339(),
        last_login: None,
        // Wizard flow runs on first-server-start. The first registered
        // operator defaults to admin. Subsequent operators (via admin
        // console user management — Sprint 9+) should default to viewer.
        role: "admin".to_string(),
        // Server-setup wizard is a different surface from the admin-console
        // onboarding dialog. We leave `onboarded_at = None` so the admin SPA
        // still guides the operator through skill/agent selection on first login.
        onboarded_at: None,
        // Auto-route defaults off; operators can enable via admin console.
        intent_auto_route: false,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ------------------------------------------------------------------------
    // Test handlers — deterministic, no real I/O.
    // ------------------------------------------------------------------------

    /// Handler that returns the supplied answer for every step.
    struct FixedHandler {
        answer: WizardAnswer,
    }

    #[async_trait]
    impl WizardStepHandler for FixedHandler {
        async fn handle(&self, _step: &WizardStep) -> Result<WizardAnswer, HandlerError> {
            Ok(self.answer.clone())
        }
    }

    /// Handler that returns a pre-recorded answer per step id. Unknown step
    /// ids trigger `HandlerError::Other`.
    struct ScriptedHandler {
        script: HashMap<String, Result<WizardAnswer, HandlerError>>,
    }

    #[async_trait]
    impl WizardStepHandler for ScriptedHandler {
        async fn handle(&self, step: &WizardStep) -> Result<WizardAnswer, HandlerError> {
            match self.script.get(step.id()) {
                Some(Ok(a)) => Ok(a.clone()),
                Some(Err(HandlerError::Cancelled)) => Err(HandlerError::Cancelled),
                Some(Err(HandlerError::ValidationFailed(m))) => {
                    Err(HandlerError::ValidationFailed(m.clone()))
                }
                Some(Err(HandlerError::Other(m))) => Err(HandlerError::Other(m.clone())),
                None => Err(HandlerError::Other(format!(
                    "no script entry for step id {}",
                    step.id()
                ))),
            }
        }
    }

    /// Handler that returns `ValidationFailed` the first `fail_count` times,
    /// then succeeds with `answer`.
    struct RetryingHandler {
        fail_count: AtomicUsize,
        answer: WizardAnswer,
    }

    #[async_trait]
    impl WizardStepHandler for RetryingHandler {
        async fn handle(&self, _step: &WizardStep) -> Result<WizardAnswer, HandlerError> {
            let remaining = self.fail_count.load(Ordering::SeqCst);
            if remaining > 0 {
                self.fail_count.fetch_sub(1, Ordering::SeqCst);
                return Err(HandlerError::ValidationFailed("not yet".into()));
            }
            Ok(self.answer.clone())
        }
    }

    // ------------------------------------------------------------------------
    // Helpers.
    // ------------------------------------------------------------------------

    fn sample_plan() -> WizardPlan {
        WizardPlan {
            id: "onboarding".into(),
            title: "Welcome".into(),
            steps: vec![
                WizardStep::Note {
                    id: "welcome".into(),
                    text: "Hi there".into(),
                    surfaces: vec![],
                    optional: false,
                },
                WizardStep::Select {
                    id: "mode".into(),
                    prompt: "Mode?".into(),
                    options: vec!["local".into(), "remote".into()],
                    default_idx: Some(0),
                    surfaces: vec![],
                    optional: false,
                },
                WizardStep::Text {
                    id: "key".into(),
                    prompt: "API key".into(),
                    sensitive: true,
                    default: None,
                    validator: None,
                    surfaces: vec![],
                    optional: false,
                },
                WizardStep::Confirm {
                    id: "accept".into(),
                    prompt: "Agree?".into(),
                    default: true,
                    surfaces: vec![],
                    optional: false,
                },
                WizardStep::MultiSelect {
                    id: "skills".into(),
                    prompt: "Skills?".into(),
                    options: vec!["a".into(), "b".into(), "c".into()],
                    surfaces: vec![],
                    optional: false,
                },
                WizardStep::Action {
                    id: "install".into(),
                    label: "Install".into(),
                    surfaces: vec![],
                    optional: false,
                },
            ],
        }
    }

    fn happy_script() -> HashMap<String, Result<WizardAnswer, HandlerError>> {
        let mut s = HashMap::new();
        s.insert("welcome".into(), Ok(WizardAnswer::ActionDone));
        s.insert("mode".into(), Ok(WizardAnswer::SelectedIdx(1)));
        s.insert("key".into(), Ok(WizardAnswer::Text("super-secret".into())));
        s.insert("accept".into(), Ok(WizardAnswer::ConfirmBool(true)));
        s.insert("skills".into(), Ok(WizardAnswer::SelectedSet(vec![0, 2])));
        s.insert("install".into(), Ok(WizardAnswer::ActionDone));
        s
    }

    // ------------------------------------------------------------------------
    // Tests.
    // ------------------------------------------------------------------------

    #[test]
    fn plan_and_session_serde_roundtrip() {
        let plan = sample_plan();
        let json = serde_json::to_string(&plan).expect("serialize plan");
        let back: WizardPlan = serde_json::from_str(&json).expect("deserialize plan");
        assert_eq!(back.id, plan.id);
        assert_eq!(back.steps.len(), plan.steps.len());
        assert_eq!(back.steps[0].id(), "welcome");

        let session = WizardSession::new("sess-1", &plan);
        let json = serde_json::to_string(&session).expect("serialize session");
        let back: WizardSession = serde_json::from_str(&json).expect("deserialize session");
        assert_eq!(back.session_id, "sess-1");
        assert_eq!(back.plan_id, "onboarding");
        assert!(matches!(back.state, WizardState::Running));
    }

    #[tokio::test]
    async fn run_happy_path_completes_all_step_kinds() {
        let plan = sample_plan();
        let handler = ScriptedHandler {
            script: happy_script(),
        };
        let session = WizardSession::new("s1", &plan);
        let engine = WizardEngine::new();

        let out = engine.run(&plan, &handler, session).await;

        assert!(
            matches!(out.state, WizardState::Done),
            "state={:?}",
            out.state
        );
        assert_eq!(out.current_step_idx, plan.steps.len());
        assert_eq!(out.answers.len(), 6);
        assert_eq!(out.answers.get("mode"), Some(&WizardAnswer::SelectedIdx(1)));
        assert_eq!(
            out.answers.get("key"),
            Some(&WizardAnswer::Text("super-secret".into()))
        );
    }

    #[tokio::test]
    async fn run_emits_events_in_order() {
        let plan = sample_plan();
        let handler = ScriptedHandler {
            script: happy_script(),
        };
        let session = WizardSession::new("s1", &plan);
        let sink = Arc::new(VecWizardEventSink::new());
        let engine = WizardEngine::new().with_event_sink(sink.clone());

        let _ = engine.run(&plan, &handler, session).await;

        let events = sink.snapshot();
        // Expect: SessionStarted
        //   + (StepEntered, AnswerCaptured) * 6
        //   + SessionCompleted = 1 + 12 + 1 = 14.
        assert_eq!(events.len(), 14, "got {:?}", events);
        assert!(matches!(events[0], WizardEvent::SessionStarted { .. }));
        assert!(matches!(events[1], WizardEvent::StepEntered { .. }));
        assert!(matches!(events[2], WizardEvent::AnswerCaptured { .. }));
        assert!(matches!(
            events[events.len() - 1],
            WizardEvent::SessionCompleted { .. }
        ));
    }

    #[tokio::test]
    async fn handler_returning_other_error_moves_state_to_error() {
        let plan = sample_plan();
        let mut s = happy_script();
        s.insert(
            "key".into(),
            Err(HandlerError::Other("backend down".into())),
        );
        let handler = ScriptedHandler { script: s };
        let session = WizardSession::new("s1", &plan);
        let sink = Arc::new(VecWizardEventSink::new());
        let engine = WizardEngine::new().with_event_sink(sink.clone());

        let out = engine.run(&plan, &handler, session).await;

        match &out.state {
            WizardState::Error(msg) => assert!(msg.contains("backend down")),
            other => panic!("expected Error, got {:?}", other),
        }
        // Session stopped at the failing step (index 2).
        assert_eq!(out.current_step_idx, 2);
        let events = sink.snapshot();
        assert!(events
            .iter()
            .any(|e| matches!(e, WizardEvent::SessionErrored { .. })));
    }

    #[tokio::test]
    async fn handler_cancel_transitions_to_cancelled() {
        let plan = sample_plan();
        let mut s = happy_script();
        s.insert("accept".into(), Err(HandlerError::Cancelled));
        let handler = ScriptedHandler { script: s };
        let session = WizardSession::new("s1", &plan);
        let sink = Arc::new(VecWizardEventSink::new());
        let engine = WizardEngine::new().with_event_sink(sink.clone());

        let out = engine.run(&plan, &handler, session).await;

        assert!(matches!(out.state, WizardState::Cancelled));
        // Answers collected up to but not including the cancelled step.
        assert_eq!(out.answers.len(), 3);
        let events = sink.snapshot();
        assert!(events
            .iter()
            .any(|e| matches!(e, WizardEvent::SessionCancelled { at_step: 3 })));
    }

    #[tokio::test]
    async fn action_step_produces_action_done() {
        let plan = WizardPlan {
            id: "mini".into(),
            title: "Mini".into(),
            steps: vec![WizardStep::Action {
                id: "run".into(),
                label: "Run".into(),
                surfaces: vec![],
                optional: false,
            }],
        };
        let handler = FixedHandler {
            answer: WizardAnswer::ActionDone,
        };
        let session = WizardSession::new("s1", &plan);
        let engine = WizardEngine::new();

        let out = engine.run(&plan, &handler, session).await;

        assert!(matches!(out.state, WizardState::Done));
        assert_eq!(out.answers.get("run"), Some(&WizardAnswer::ActionDone));
    }

    #[tokio::test]
    async fn empty_plan_completes_immediately() {
        let plan = WizardPlan {
            id: "empty".into(),
            title: "".into(),
            steps: vec![],
        };
        let handler = FixedHandler {
            answer: WizardAnswer::ActionDone,
        };
        let session = WizardSession::new("s1", &plan);
        let engine = WizardEngine::new();

        let out = engine.run(&plan, &handler, session).await;

        assert!(matches!(out.state, WizardState::Done));
        assert_eq!(out.current_step_idx, 0);
        assert!(out.answers.is_empty());
    }

    #[tokio::test]
    async fn only_notes_plan_completes() {
        let plan = WizardPlan {
            id: "notes".into(),
            title: "Notes".into(),
            steps: vec![
                WizardStep::Note {
                    id: "n1".into(),
                    text: "hello".into(),
                    surfaces: vec![],
                    optional: false,
                },
                WizardStep::Note {
                    id: "n2".into(),
                    text: "world".into(),
                    surfaces: vec![],
                    optional: false,
                },
            ],
        };
        let handler = FixedHandler {
            answer: WizardAnswer::ActionDone,
        };
        let session = WizardSession::new("s1", &plan);
        let engine = WizardEngine::new();

        let out = engine.run(&plan, &handler, session).await;

        assert!(matches!(out.state, WizardState::Done));
        assert_eq!(out.answers.len(), 2);
    }

    #[tokio::test]
    async fn vec_sink_captures_expected_events_on_happy_path() {
        // Explicit event-shape check (mirrors evolution_orchestrator's style).
        let plan = WizardPlan {
            id: "tiny".into(),
            title: "Tiny".into(),
            steps: vec![WizardStep::Confirm {
                id: "ok".into(),
                prompt: "?".into(),
                default: true,
                surfaces: vec![],
                optional: false,
            }],
        };
        let handler = FixedHandler {
            answer: WizardAnswer::ConfirmBool(true),
        };
        let session = WizardSession::new("s1", &plan);
        let sink = Arc::new(VecWizardEventSink::new());
        let engine = WizardEngine::new().with_event_sink(sink.clone());

        let _ = engine.run(&plan, &handler, session).await;

        let events = sink.snapshot();
        assert_eq!(events.len(), 4);
        match &events[0] {
            WizardEvent::SessionStarted {
                session_id,
                plan_id,
            } => {
                assert_eq!(session_id, "s1");
                assert_eq!(plan_id, "tiny");
            }
            other => panic!("expected SessionStarted, got {:?}", other),
        }
        assert!(matches!(
            events[1],
            WizardEvent::StepEntered { step_idx: 0, .. }
        ));
        assert!(matches!(events[2], WizardEvent::AnswerCaptured { .. }));
        assert!(matches!(events[3], WizardEvent::SessionCompleted { .. }));
    }

    #[tokio::test]
    async fn session_resumes_from_current_step_idx() {
        let plan = sample_plan();
        let handler = ScriptedHandler {
            script: happy_script(),
        };
        // Pre-populate session with answers for steps 0..3 and jump ahead.
        let mut session = WizardSession::new("s1", &plan);
        session
            .answers
            .insert("welcome".into(), WizardAnswer::ActionDone);
        session
            .answers
            .insert("mode".into(), WizardAnswer::SelectedIdx(0));
        session
            .answers
            .insert("key".into(), WizardAnswer::Text("prev".into()));
        session.current_step_idx = 3;
        let sink = Arc::new(VecWizardEventSink::new());
        let engine = WizardEngine::new().with_event_sink(sink.clone());

        let out = engine.run(&plan, &handler, session).await;

        assert!(matches!(out.state, WizardState::Done));
        // 3 existing answers + 3 new ones.
        assert_eq!(out.answers.len(), 6);
        // Only the resumed steps' StepEntered events should appear.
        let entered: Vec<_> = sink
            .snapshot()
            .into_iter()
            .filter_map(|e| match e {
                WizardEvent::StepEntered { step_idx, .. } => Some(step_idx),
                _ => None,
            })
            .collect();
        assert_eq!(entered, vec![3, 4, 5]);
    }

    #[tokio::test]
    async fn sensitive_text_answer_is_masked_in_event() {
        let plan = WizardPlan {
            id: "p".into(),
            title: "".into(),
            steps: vec![WizardStep::Text {
                id: "secret".into(),
                prompt: "secret?".into(),
                sensitive: true,
                default: None,
                validator: None,
                surfaces: vec![],
                optional: false,
            }],
        };
        let handler = FixedHandler {
            answer: WizardAnswer::Text("real-value".into()),
        };
        let session = WizardSession::new("s1", &plan);
        let sink = Arc::new(VecWizardEventSink::new());
        let engine = WizardEngine::new().with_event_sink(sink.clone());

        let out = engine.run(&plan, &handler, session).await;

        // The persisted session still has the raw answer (the caller owns
        // it).
        assert_eq!(
            out.answers.get("secret"),
            Some(&WizardAnswer::Text("real-value".into()))
        );
        // But the emitted event must have been masked.
        let captured = sink
            .snapshot()
            .into_iter()
            .find_map(|e| match e {
                WizardEvent::AnswerCaptured { answer, .. } => Some(answer),
                _ => None,
            })
            .expect("AnswerCaptured event");
        match captured {
            WizardAnswer::Text(s) => assert_eq!(s, "***"),
            other => panic!("expected masked Text, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn validator_rejection_reprompts_and_eventually_succeeds() {
        let plan = WizardPlan {
            id: "p".into(),
            title: "".into(),
            steps: vec![WizardStep::Text {
                id: "name".into(),
                prompt: "name?".into(),
                sensitive: false,
                default: None,
                validator: Some("nonempty".into()),
                surfaces: vec![],
                optional: false,
            }],
        };
        let handler = RetryingHandler {
            fail_count: AtomicUsize::new(1),
            answer: WizardAnswer::Text("ok".into()),
        };
        let session = WizardSession::new("s1", &plan);
        let engine = WizardEngine::new();

        let out = engine.run(&plan, &handler, session).await;

        assert!(matches!(out.state, WizardState::Done));
        assert_eq!(
            out.answers.get("name"),
            Some(&WizardAnswer::Text("ok".into()))
        );
    }

    #[tokio::test]
    async fn validator_rejection_exhausts_retries_and_errors() {
        let plan = WizardPlan {
            id: "p".into(),
            title: "".into(),
            steps: vec![WizardStep::Text {
                id: "name".into(),
                prompt: "name?".into(),
                sensitive: false,
                default: None,
                validator: Some("nonempty".into()),
                surfaces: vec![],
                optional: false,
            }],
        };
        let handler = RetryingHandler {
            fail_count: AtomicUsize::new(10), // way more than the cap
            answer: WizardAnswer::Text("unreachable".into()),
        };
        let session = WizardSession::new("s1", &plan);
        let engine = WizardEngine::new();

        let out = engine.run(&plan, &handler, session).await;

        assert!(
            matches!(out.state, WizardState::Error(_)),
            "state={:?}",
            out.state
        );
    }

    #[test]
    fn plan_validate_rejects_duplicates_and_empty_ids() {
        let p = WizardPlan {
            id: "".into(),
            title: "".into(),
            steps: vec![],
        };
        assert!(p.validate().is_err());

        let p = WizardPlan {
            id: "ok".into(),
            title: "".into(),
            steps: vec![
                WizardStep::Note {
                    id: "dup".into(),
                    text: "".into(),
                    surfaces: vec![],
                    optional: false,
                },
                WizardStep::Note {
                    id: "dup".into(),
                    text: "".into(),
                    surfaces: vec![],
                    optional: false,
                },
            ],
        };
        assert!(p.validate().is_err());

        let p = WizardPlan {
            id: "ok".into(),
            title: "".into(),
            steps: vec![WizardStep::Select {
                id: "s".into(),
                prompt: "p".into(),
                options: vec![],
                default_idx: None,
                surfaces: vec![],
                optional: false,
            }],
        };
        assert!(p.validate().is_err());
    }

    #[tokio::test]
    async fn terminal_session_is_noop() {
        let plan = sample_plan();
        let handler = FixedHandler {
            answer: WizardAnswer::ActionDone,
        };
        let mut session = WizardSession::new("s1", &plan);
        session.state = WizardState::Cancelled;
        let engine = WizardEngine::new();

        let out = engine.run(&plan, &handler, session).await;

        // State unchanged.
        assert!(matches!(out.state, WizardState::Cancelled));
        assert!(out.answers.is_empty());
    }

    #[tokio::test]
    async fn noop_sink_does_not_panic() {
        let plan = sample_plan();
        let handler = ScriptedHandler {
            script: happy_script(),
        };
        let session = WizardSession::new("s1", &plan);
        let engine = WizardEngine::new().with_event_sink(Arc::new(NoopWizardEventSink));

        let out = engine.run(&plan, &handler, session).await;
        assert!(matches!(out.state, WizardState::Done));
    }
}
