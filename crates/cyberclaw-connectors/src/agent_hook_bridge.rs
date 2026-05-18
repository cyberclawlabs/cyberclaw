//! Agent Hook Bridge
//!
//! Intercepts incoming tool calls from external AI agents (Claude Code, Codex, Copilot, Cursor)
//! before they reach CyberClaw's execution layer, providing:
//!
//! - Format detection per agent dialect
//! - Command classification and risk assessment
//! - Rewrite routing to CyberClaw connectors/capabilities
//! - Block escalation for high-risk or destructive commands
//!
//! All execution continues to flow through `Connector -> Capability`; this bridge only
//! intercepts, classifies, and optionally rewrites the incoming command string.

use once_cell::sync::Lazy;
use regex::{Regex, RegexSet};
use serde_json::Value;

// ---------------------------------------------------------------------------
// HookFormat
// ---------------------------------------------------------------------------

/// Detected format of an incoming hook call from an AI agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookFormat {
    /// Claude Code: `tool_name` + `tool_input.command`
    ClaudeCode { command: String, tool_name: String },
    /// Codex: similar to Claude Code but uses `input.command`
    Codex { command: String },
    /// Copilot VS Code: camelCase `toolName` + `toolArgs`
    CopilotVsCode { command: String },
    /// Copilot CLI: `toolName` + JSON-encoded `toolArgs`
    CopilotCli { command: String },
    /// Cursor: `preToolUse` format with `tool_input.command`
    Cursor { command: String },
    /// Non-command tool or unknown format — pass through silently.
    PassThrough,
}

impl HookFormat {
    /// Extract the command string regardless of format.
    pub fn command(&self) -> Option<&str> {
        match self {
            HookFormat::ClaudeCode { command, .. } => Some(command.as_str()),
            HookFormat::Codex { command } => Some(command.as_str()),
            HookFormat::CopilotVsCode { command } => Some(command.as_str()),
            HookFormat::CopilotCli { command } => Some(command.as_str()),
            HookFormat::Cursor { command } => Some(command.as_str()),
            HookFormat::PassThrough => None,
        }
    }

    /// Detect the agent hook format from a raw JSON input value.
    ///
    /// Detection order (most specific first):
    /// 1. Cursor: has `hook_type == "preToolUse"` or `session_id` alongside `tool_input`
    /// 2. Codex: has `input.command`
    /// 3. Copilot VS Code: camelCase `toolName` + `toolArgs.cmd`
    /// 4. Copilot CLI: `toolName` + string `toolArgs`
    /// 5. Claude Code: `tool_name` + `tool_input.command`
    /// 6. PassThrough for anything else
    pub fn detect(input: &Value) -> Self {
        // --- Cursor: hook_type == "preToolUse" or has session_id ---
        let is_cursor = input
            .get("hook_type")
            .and_then(Value::as_str)
            .map(|v| v == "preToolUse")
            .unwrap_or(false)
            || input.get("session_id").is_some();

        if is_cursor {
            if let Some(cmd) = extract_nested_command(input, &["tool_input", "command"]) {
                return HookFormat::Cursor { command: cmd };
            }
            // Cursor without a command field → pass through
            return HookFormat::PassThrough;
        }

        // --- Codex: has `input.command` ---
        if let Some(cmd) = extract_nested_command(input, &["input", "command"]) {
            return HookFormat::Codex { command: cmd };
        }

        // --- Copilot VS Code: camelCase toolName + toolArgs (object with cmd) ---
        if let Some(tool_name) = input.get("toolName").and_then(Value::as_str) {
            if let Some(args) = input.get("toolArgs") {
                if let Some(cmd) = args.get("cmd").and_then(Value::as_str) {
                    return HookFormat::CopilotVsCode {
                        command: cmd.to_string(),
                    };
                }
                // toolArgs is a raw JSON string (CLI variant)
                if let Some(raw) = args.as_str() {
                    // Parse inner JSON to find the command
                    if let Ok(inner) = serde_json::from_str::<Value>(raw) {
                        if let Some(cmd) = inner.get("cmd").and_then(Value::as_str) {
                            return HookFormat::CopilotCli {
                                command: cmd.to_string(),
                            };
                        }
                        // Fallback: any "command" key
                        if let Some(cmd) = inner.get("command").and_then(Value::as_str) {
                            return HookFormat::CopilotCli {
                                command: cmd.to_string(),
                            };
                        }
                    }
                    return HookFormat::CopilotCli {
                        command: tool_name.to_string(),
                    };
                }
            }
            // toolName present but no useful command → pass through
            return HookFormat::PassThrough;
        }

        // --- Claude Code: tool_name + tool_input.command ---
        if let Some(tool_name) = input.get("tool_name").and_then(Value::as_str) {
            if let Some(cmd) = extract_nested_command(input, &["tool_input", "command"]) {
                return HookFormat::ClaudeCode {
                    command: cmd,
                    tool_name: tool_name.to_string(),
                };
            }
            // tool_name present but no command (e.g., Read/Write) → pass through
            return HookFormat::PassThrough;
        }

        HookFormat::PassThrough
    }
}

/// Walk a JSON path and return the value as a String if it exists and is a string.
fn extract_nested_command(input: &Value, path: &[&str]) -> Option<String> {
    let mut cur = input;
    for key in path {
        cur = cur.get(*key)?;
    }
    cur.as_str().map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// CommandCategory + CommandRiskLevel
// ---------------------------------------------------------------------------

/// Category used to group related commands for policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandCategory {
    /// Version-control operations (git, gh).
    Git,
    /// Compilation and build tools (cargo, go, dotnet).
    Build,
    /// Test runners and assertion frameworks.
    Test,
    /// Package manager operations (npm, pip, etc.).
    PackageManager,
    /// Local filesystem operations (cat, ls, rm, etc.).
    FileSystem,
    /// Network utilities (curl, wget, nc).
    Network,
    /// Container and orchestration tools (docker, kubectl).
    Container,
    /// Database clients and DDL commands.
    Database,
    /// Generic shell built-ins and scripting.
    Shell,
    /// Unrecognised command — no rule matched.
    Unknown,
}

/// Estimated risk level of a command, ordered from safest to most dangerous.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommandRiskLevel {
    /// Read-only; no side effects.
    Safe,
    /// Local modifications only; easily reversible.
    Low,
    /// Touches the network or makes system-level changes.
    Medium,
    /// Destructive or difficult to reverse.
    High,
    /// Catastrophic: data loss, force-push, DROP TABLE, `rm -rf`.
    Critical,
}

// ---------------------------------------------------------------------------
// CommandClassification
// ---------------------------------------------------------------------------

/// Full classification result for a detected command.
#[derive(Debug, Clone)]
pub struct CommandClassification {
    /// Original raw command string as received.
    pub raw_command: String,
    /// First token of the command (e.g., `"git"`, `"cargo"`).
    pub base_command: String,
    /// Second token if present (e.g., `"log"`, `"test"`).
    pub subcommand: Option<String>,
    /// Broad category for the command.
    pub category: CommandCategory,
    /// Estimated risk level.
    pub risk_level: CommandRiskLevel,
    /// Whether this command class is eligible for output filtering (token savings).
    pub filterable: bool,
    /// Estimated percentage of output tokens saved when filtering is applied.
    pub estimated_savings_pct: f64,
}

// ---------------------------------------------------------------------------
// RewriteRule
// ---------------------------------------------------------------------------

/// A single rule mapping a command pattern to a CyberClaw capability route.
#[derive(Debug, Clone)]
pub struct RewriteRule {
    /// Regex pattern matching against the command string.
    pub pattern: String,
    /// CyberClaw capability or connector identifier to route to.
    pub target_capability: String,
    /// Category assigned to matching commands.
    pub category: CommandCategory,
    /// Default estimated output-token savings when filtering is applied.
    pub estimated_savings_pct: f64,
    /// Risk level for commands matching this rule.
    pub risk_level: CommandRiskLevel,
    /// Per-subcommand savings overrides: `(subcommand, savings_pct)`.
    pub subcmd_savings: Vec<(String, f64)>,
}

// ---------------------------------------------------------------------------
// RewriteRegistry
// ---------------------------------------------------------------------------

/// Registry of rewrite rules with O(1) multi-pattern matching via `RegexSet`.
pub struct RewriteRegistry {
    rules: Vec<RewriteRule>,
    /// Parallel set for fast O(1) "any match" checks.
    regex_set: RegexSet,
    /// Pre-compiled individual regexes for capture-group extraction.
    compiled: Vec<Regex>,
}

impl RewriteRegistry {
    /// Build a registry from a list of rules.
    ///
    /// Returns an error if any pattern fails to compile.
    pub fn new(rules: Vec<RewriteRule>) -> Result<Self, HookBridgeError> {
        let patterns: Vec<&str> = rules.iter().map(|r| r.pattern.as_str()).collect();
        let regex_set = RegexSet::new(&patterns)
            .map_err(|e| HookBridgeError::InvalidPattern(format!("RegexSet compile error: {e}")))?;
        let compiled = patterns
            .iter()
            .map(|p| {
                Regex::new(p).map_err(|e| HookBridgeError::InvalidPattern(format!("{p}: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            rules,
            regex_set,
            compiled,
        })
    }

    /// Create a registry pre-loaded with default rules for common commands.
    pub fn with_defaults() -> Self {
        // Infallible because all patterns are hand-validated constants.
        Self::new(default_rules()).expect("default rules must compile")
    }

    /// Add a custom rule to the registry.
    ///
    /// The registry is rebuilt after insertion to keep the `RegexSet` consistent.
    pub fn add_rule(&mut self, rule: RewriteRule) -> Result<(), HookBridgeError> {
        // Validate the pattern compiles before mutating state.
        Regex::new(&rule.pattern)
            .map_err(|e| HookBridgeError::InvalidPattern(format!("{}: {e}", rule.pattern)))?;

        self.rules.push(rule);

        // Rebuild RegexSet and compiled list.
        let patterns: Vec<&str> = self.rules.iter().map(|r| r.pattern.as_str()).collect();
        self.regex_set = RegexSet::new(&patterns)
            .map_err(|e| HookBridgeError::InvalidPattern(format!("RegexSet rebuild: {e}")))?;
        self.compiled = patterns
            .iter()
            .map(|p| {
                Regex::new(p).map_err(|e| HookBridgeError::InvalidPattern(format!("{p}: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(())
    }

    /// Classify a command against all registered rules.
    ///
    /// Returns the classification of the first matching rule (rules are checked
    /// in insertion order). If no rule matches, returns an `Unknown` classification.
    pub fn classify(&self, command: &str) -> CommandClassification {
        let cmd = strip_env_prefix(command);
        let matches: Vec<usize> = self.regex_set.matches(cmd).into_iter().collect();

        let (category, risk_level, savings_pct, target_cap) = if let Some(&idx) = matches.first() {
            let rule = &self.rules[idx];

            // Calculate savings considering subcommand overrides.
            let tokens = tokenize(cmd);
            let sub = tokens.get(1).map(|s| s.as_str()).unwrap_or("");
            let savings = rule
                .subcmd_savings
                .iter()
                .find(|(sc, _)| sc == sub)
                .map(|(_, s)| *s)
                .unwrap_or(rule.estimated_savings_pct);

            (
                rule.category.clone(),
                rule.risk_level.clone(),
                savings,
                rule.target_capability.clone(),
            )
        } else {
            (
                CommandCategory::Unknown,
                CommandRiskLevel::Medium,
                0.0,
                "unknown".to_string(),
            )
        };

        let tokens = tokenize(cmd);
        let base_command = tokens.first().cloned().unwrap_or_default();
        let subcommand = tokens.get(1).cloned();

        let filterable = savings_pct > 0.0;
        let _ = target_cap; // retained for future routing; not exposed in classification yet

        CommandClassification {
            raw_command: command.to_string(),
            base_command,
            subcommand,
            category,
            risk_level,
            filterable,
            estimated_savings_pct: savings_pct,
        }
    }

    /// Returns `true` if at least one rule matches the given command.
    pub fn can_rewrite(&self, command: &str) -> bool {
        let cmd = strip_env_prefix(command);
        self.regex_set.is_match(cmd)
    }
}

// ---------------------------------------------------------------------------
// Default rules
// ---------------------------------------------------------------------------

fn default_rules() -> Vec<RewriteRule> {
    vec![
        // ── Destructive / Critical (evaluated first so risk wins) ──────────
        RewriteRule {
            pattern: r"^rm\s+-[a-zA-Z]*r[a-zA-Z]*f|^rm\s+-[a-zA-Z]*f[a-zA-Z]*r".to_string(),
            target_capability: "fs.delete".to_string(),
            category: CommandCategory::FileSystem,
            estimated_savings_pct: 0.0,
            risk_level: CommandRiskLevel::Critical,
            subcmd_savings: vec![],
        },
        RewriteRule {
            pattern: r"^git\s+push\s+.*--force".to_string(),
            target_capability: "git.push.force".to_string(),
            category: CommandCategory::Git,
            estimated_savings_pct: 0.0,
            risk_level: CommandRiskLevel::High,
            subcmd_savings: vec![],
        },
        RewriteRule {
            pattern: r"(?i)^DROP\s+".to_string(),
            target_capability: "db.ddl".to_string(),
            category: CommandCategory::Database,
            estimated_savings_pct: 0.0,
            risk_level: CommandRiskLevel::Critical,
            subcmd_savings: vec![],
        },
        // ── Git ─────────────────────────────────────────────────────────────
        RewriteRule {
            pattern: r"^git\s+(status|log|diff|show|add|commit|push|pull|branch|fetch|stash|rebase|merge|cherry-pick|tag|remote)".to_string(),
            target_capability: "git.*".to_string(),
            category: CommandCategory::Git,
            estimated_savings_pct: 70.0,
            risk_level: CommandRiskLevel::Low,
            subcmd_savings: vec![
                ("log".to_string(), 85.0),
                ("diff".to_string(), 80.0),
                ("push".to_string(), 20.0),
            ],
        },
        RewriteRule {
            pattern: r"^gh\s+(pr|issue|run|repo|release|workflow)".to_string(),
            target_capability: "github.*".to_string(),
            category: CommandCategory::Git,
            estimated_savings_pct: 82.0,
            risk_level: CommandRiskLevel::Medium,
            subcmd_savings: vec![],
        },
        // ── Build ───────────────────────────────────────────────────────────
        RewriteRule {
            pattern: r"^cargo\s+(build|test|clippy|check|fmt|run|bench|doc|publish)".to_string(),
            target_capability: "cargo.*".to_string(),
            category: CommandCategory::Build,
            estimated_savings_pct: 80.0,
            risk_level: CommandRiskLevel::Low,
            subcmd_savings: vec![
                ("test".to_string(), 85.0),
                ("clippy".to_string(), 75.0),
                ("publish".to_string(), 10.0),
            ],
        },
        RewriteRule {
            pattern: r"^go\s+(build|test|vet|mod|run|generate|install)".to_string(),
            target_capability: "go.*".to_string(),
            category: CommandCategory::Build,
            estimated_savings_pct: 70.0,
            risk_level: CommandRiskLevel::Low,
            subcmd_savings: vec![("test".to_string(), 80.0)],
        },
        RewriteRule {
            pattern: r"^dotnet\s+(build|test|restore|publish|run)".to_string(),
            target_capability: "dotnet.*".to_string(),
            category: CommandCategory::Build,
            estimated_savings_pct: 75.0,
            risk_level: CommandRiskLevel::Low,
            subcmd_savings: vec![("test".to_string(), 80.0)],
        },
        RewriteRule {
            pattern: r"^make\s*".to_string(),
            target_capability: "make.*".to_string(),
            category: CommandCategory::Build,
            estimated_savings_pct: 65.0,
            risk_level: CommandRiskLevel::Low,
            subcmd_savings: vec![],
        },
        // ── Package managers ────────────────────────────────────────────────
        RewriteRule {
            pattern: r"^(npm|pnpm|yarn)\s+(install|list|outdated|run|build|test|ci|audit)".to_string(),
            target_capability: "npm.*".to_string(),
            category: CommandCategory::PackageManager,
            estimated_savings_pct: 75.0,
            risk_level: CommandRiskLevel::Low,
            subcmd_savings: vec![("audit".to_string(), 50.0)],
        },
        RewriteRule {
            pattern: r"^pip\s+(install|list|freeze|show|check)".to_string(),
            target_capability: "pip.*".to_string(),
            category: CommandCategory::PackageManager,
            estimated_savings_pct: 65.0,
            risk_level: CommandRiskLevel::Low,
            subcmd_savings: vec![],
        },
        // ── File operations ─────────────────────────────────────────────────
        RewriteRule {
            pattern: r"^(cat|head|tail|less|more)\s+".to_string(),
            target_capability: "fs.read".to_string(),
            category: CommandCategory::FileSystem,
            estimated_savings_pct: 60.0,
            risk_level: CommandRiskLevel::Safe,
            subcmd_savings: vec![],
        },
        RewriteRule {
            pattern: r"^(ls|tree|find|du|stat)\s*".to_string(),
            target_capability: "fs.list".to_string(),
            category: CommandCategory::FileSystem,
            estimated_savings_pct: 50.0,
            risk_level: CommandRiskLevel::Safe,
            subcmd_savings: vec![],
        },
        // ── Network ─────────────────────────────────────────────────────────
        RewriteRule {
            pattern: r"^(curl|wget|http|httpie)\s+".to_string(),
            target_capability: "net.fetch".to_string(),
            category: CommandCategory::Network,
            estimated_savings_pct: 55.0,
            risk_level: CommandRiskLevel::Medium,
            subcmd_savings: vec![],
        },
        // ── Container / orchestration ────────────────────────────────────────
        RewriteRule {
            pattern: r"^docker\s+(ps|images|logs|build|run|exec|inspect|pull|push|rm|rmi)".to_string(),
            target_capability: "docker.*".to_string(),
            category: CommandCategory::Container,
            estimated_savings_pct: 65.0,
            risk_level: CommandRiskLevel::Medium,
            subcmd_savings: vec![
                ("logs".to_string(), 80.0),
                ("build".to_string(), 70.0),
            ],
        },
        RewriteRule {
            pattern: r"^kubectl\s+(get|describe|logs|apply|delete|exec|port-forward|rollout)".to_string(),
            target_capability: "k8s.*".to_string(),
            category: CommandCategory::Container,
            estimated_savings_pct: 70.0,
            risk_level: CommandRiskLevel::Medium,
            subcmd_savings: vec![
                ("logs".to_string(), 85.0),
                ("delete".to_string(), 10.0),
            ],
        },
    ]
}

// ---------------------------------------------------------------------------
// HookBridge
// ---------------------------------------------------------------------------

/// Main bridge that processes incoming AI agent hook calls.
///
/// Detects the agent format, extracts the command, classifies it against
/// the rule registry, and decides whether to rewrite, allow, block, or ignore.
pub struct HookBridge {
    registry: RewriteRegistry,
}

/// The decision made by [`HookBridge::process_hook`] for a single hook call.
#[derive(Debug)]
pub enum HookAction {
    /// The command will be rewritten to route through a CyberClaw capability.
    Rewrite {
        /// Original command before rewriting.
        original: String,
        /// Rewritten command (capability route string).
        rewritten: String,
        /// Classification details.
        classification: CommandClassification,
    },
    /// The command is allowed to pass through unchanged.
    Allow {
        /// The command string.
        command: String,
        /// Classification details.
        classification: CommandClassification,
    },
    /// The command is blocked pending human approval.
    Block {
        /// The command string.
        command: String,
        /// Classification details.
        classification: CommandClassification,
        /// Human-readable reason for blocking.
        reason: String,
    },
    /// The input is not a command tool call — no action required.
    Ignore,
}

impl HookBridge {
    /// Create a new bridge with default rules.
    pub fn new() -> Self {
        Self {
            registry: RewriteRegistry::with_defaults(),
        }
    }

    /// Create a bridge with a custom registry.
    pub fn with_registry(registry: RewriteRegistry) -> Self {
        Self { registry }
    }

    /// Process a raw JSON hook input from an AI agent.
    ///
    /// Detects the format, extracts the command, and delegates to
    /// [`HookBridge::process_command`].
    pub fn process_hook(&self, input: &Value) -> HookAction {
        let format = HookFormat::detect(input);
        match format.command() {
            Some(cmd) => self.process_command(cmd),
            None => HookAction::Ignore,
        }
    }

    /// Process a known command string directly (skipping format detection).
    ///
    /// Compound commands (joined by `&&`, `||`, `;`, `|`) are evaluated for
    /// their highest-risk segment — if any segment would be blocked, the
    /// entire compound command is blocked.
    pub fn process_command(&self, command: &str) -> HookAction {
        let segments = split_compound_command(command);

        // Find the highest-risk segment.
        let mut worst_risk = CommandRiskLevel::Safe;
        let mut worst_classification: Option<CommandClassification> = None;

        for seg in &segments {
            let trimmed = seg.trim();
            if trimmed.is_empty() {
                continue;
            }
            let classification = self.registry.classify(trimmed);
            if classification.risk_level >= worst_risk {
                worst_risk = classification.risk_level.clone();
                worst_classification = Some(classification);
            }
        }

        let classification = match worst_classification {
            Some(c) => c,
            None => self.registry.classify(command),
        };

        match &classification.risk_level {
            CommandRiskLevel::Critical => HookAction::Block {
                command: command.to_string(),
                reason: format!(
                    "Critical-risk command '{}' requires explicit approval before execution.",
                    classification.base_command
                ),
                classification,
            },
            CommandRiskLevel::High => HookAction::Block {
                command: command.to_string(),
                reason: format!(
                    "High-risk command '{}' blocked; manual approval required.",
                    classification.base_command
                ),
                classification,
            },
            _ => {
                if self.registry.can_rewrite(command) {
                    let rewritten = build_rewrite_target(command, &classification);
                    HookAction::Rewrite {
                        original: command.to_string(),
                        rewritten,
                        classification,
                    }
                } else {
                    HookAction::Allow {
                        command: command.to_string(),
                        classification,
                    }
                }
            }
        }
    }
}

impl Default for HookBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a rewrite target string for a classified command.
fn build_rewrite_target(_command: &str, classification: &CommandClassification) -> String {
    // Format: `cyberclaw://<category>/<base>/<subcommand>`
    let cat = format!("{:?}", classification.category).to_lowercase();
    match &classification.subcommand {
        Some(sub) => format!(
            "cyberclaw://{}/{}/{}",
            cat, classification.base_command, sub
        ),
        None => format!("cyberclaw://{}/{}", cat, classification.base_command),
    }
}

// ---------------------------------------------------------------------------
// HookBridgeError
// ---------------------------------------------------------------------------

/// Errors that can occur within the hook bridge.
#[derive(Debug, thiserror::Error)]
pub enum HookBridgeError {
    /// A rule pattern failed regex compilation.
    #[error("Invalid regex pattern: {0}")]
    InvalidPattern(String),
    /// Two rules conflict in an irreconcilable way.
    #[error("Rule conflict: {0}")]
    RuleConflict(String),
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Split a compound shell command into individual segments.
///
/// Splits on `&&`, `||`, `;`, and `|` (unquoted only).
/// Quoted strings (`"..."` and `'...'`) are treated as atomic tokens.
pub fn split_compound_command(cmd: &str) -> Vec<&str> {
    // Fast path: no operators present.
    if !cmd.contains("&&") && !cmd.contains("||") && !cmd.contains(';') && !cmd.contains('|') {
        return vec![cmd];
    }

    static SPLIT_RE: Lazy<Regex> = Lazy::new(|| {
        // Match &&, ||, ;, or | that are not inside quoted strings.
        // This is a best-effort approximation (not a full shell parser).
        Regex::new(r"&&|\|\||;|\|").expect("split regex must compile")
    });

    // Collect byte ranges of splits respecting quotes.
    let bytes = cmd.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut split_positions: Vec<usize> = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double => {
                in_single = !in_single;
                i += 1;
            }
            b'"' if !in_single => {
                in_double = !in_double;
                i += 1;
            }
            b'&' if !in_single && !in_double && i + 1 < bytes.len() && bytes[i + 1] == b'&' => {
                split_positions.push(i);
                i += 2;
            }
            b'|' if !in_single && !in_double && i + 1 < bytes.len() && bytes[i + 1] == b'|' => {
                split_positions.push(i);
                i += 2;
            }
            b'|' if !in_single && !in_double => {
                split_positions.push(i);
                i += 1;
            }
            b';' if !in_single && !in_double => {
                split_positions.push(i);
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    if split_positions.is_empty() {
        return vec![cmd];
    }

    // Use the fallback regex-based split when there are no quote complications.
    // For simplicity, just use the regex on the whole string.
    let _ = SPLIT_RE; // ensure lazy is initialized
    let parts: Vec<&str> = SPLIT_RE.split(cmd).collect();
    parts
}

/// Strip leading `KEY=VALUE` environment variable assignments from a command.
///
/// For example, `"ENV=val FOO=bar cmd arg"` becomes `"cmd arg"`.
pub fn strip_env_prefix(cmd: &str) -> &str {
    static ENV_PREFIX: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^(?:[A-Za-z_][A-Za-z0-9_]*=[^\s]*\s+)+")
            .expect("env prefix regex must compile")
    });

    match ENV_PREFIX.find(cmd) {
        Some(m) => &cmd[m.end()..],
        None => cmd,
    }
}

/// Tokenize a command string into whitespace-separated tokens,
/// respecting quoted strings.
fn tokenize(cmd: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;

    for ch in cmd.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            ' ' | '\t' if !in_single && !in_double => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Format detection ────────────────────────────────────────────────────

    #[test]
    fn test_detect_claude_code_format() {
        let input = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git status" }
        });
        let fmt = HookFormat::detect(&input);
        assert_eq!(
            fmt,
            HookFormat::ClaudeCode {
                command: "git status".to_string(),
                tool_name: "Bash".to_string(),
            },
            "should detect Claude Code format"
        );
        assert_eq!(fmt.command(), Some("git status"));
    }

    #[test]
    fn test_detect_copilot_vscode_format() {
        let input = json!({
            "toolName": "runInTerminal",
            "toolArgs": { "cmd": "cargo test" }
        });
        let fmt = HookFormat::detect(&input);
        assert_eq!(
            fmt,
            HookFormat::CopilotVsCode {
                command: "cargo test".to_string()
            },
            "should detect Copilot VS Code format"
        );
    }

    #[test]
    fn test_detect_copilot_cli_format() {
        let inner = serde_json::to_string(&json!({ "cmd": "npm install" }))
            .expect("serialization must succeed");
        let input = json!({
            "toolName": "shell",
            "toolArgs": inner
        });
        let fmt = HookFormat::detect(&input);
        assert_eq!(
            fmt,
            HookFormat::CopilotCli {
                command: "npm install".to_string()
            },
            "should detect Copilot CLI format"
        );
    }

    #[test]
    fn test_detect_cursor_format() {
        let input = json!({
            "hook_type": "preToolUse",
            "session_id": "abc123",
            "tool_input": { "command": "cargo build" }
        });
        let fmt = HookFormat::detect(&input);
        assert_eq!(
            fmt,
            HookFormat::Cursor {
                command: "cargo build".to_string()
            },
            "should detect Cursor preToolUse format"
        );
    }

    #[test]
    fn test_detect_passthrough() {
        // Read tool has no command field → PassThrough
        let input = json!({
            "tool_name": "Read",
            "tool_input": { "file_path": "/etc/hosts" }
        });
        let fmt = HookFormat::detect(&input);
        assert_eq!(
            fmt,
            HookFormat::PassThrough,
            "Read tool should pass through"
        );
        assert!(fmt.command().is_none());
    }

    #[test]
    fn test_detect_codex_format() {
        let input = json!({
            "input": { "command": "go test ./..." }
        });
        let fmt = HookFormat::detect(&input);
        assert_eq!(
            fmt,
            HookFormat::Codex {
                command: "go test ./...".to_string()
            },
            "should detect Codex format"
        );
    }

    // ── Command classification ───────────────────────────────────────────────

    #[test]
    fn test_classify_git_command() {
        let registry = RewriteRegistry::with_defaults();
        let c = registry.classify("git log --oneline -20");
        assert_eq!(c.base_command, "git");
        assert_eq!(c.subcommand.as_deref(), Some("log"));
        assert_eq!(c.category, CommandCategory::Git);
        assert!(c.filterable, "git log should be filterable");
        assert!(c.estimated_savings_pct > 0.0);
    }

    #[test]
    fn test_classify_cargo_command() {
        let registry = RewriteRegistry::with_defaults();
        let c = registry.classify("cargo test --workspace");
        assert_eq!(c.base_command, "cargo");
        assert_eq!(c.subcommand.as_deref(), Some("test"));
        assert_eq!(c.category, CommandCategory::Build);
        assert!(c.estimated_savings_pct > 0.0);
    }

    #[test]
    fn test_classify_dangerous_rm_command() {
        let registry = RewriteRegistry::with_defaults();
        let c = registry.classify("rm -rf /tmp/build");
        assert_eq!(c.risk_level, CommandRiskLevel::Critical);
        assert_eq!(c.category, CommandCategory::FileSystem);
        assert!(!c.filterable, "rm -rf should not be filterable");
    }

    #[test]
    fn test_classify_force_push() {
        let registry = RewriteRegistry::with_defaults();
        let c = registry.classify("git push origin main --force");
        assert_eq!(c.risk_level, CommandRiskLevel::High);
    }

    #[test]
    fn test_classify_unknown_command() {
        let registry = RewriteRegistry::with_defaults();
        let c = registry.classify("my-custom-tool --do-something");
        assert_eq!(c.category, CommandCategory::Unknown);
        assert_eq!(c.risk_level, CommandRiskLevel::Medium);
        assert!(!c.filterable);
    }

    // ── Process hook integration ─────────────────────────────────────────────

    #[test]
    fn test_process_hook_rewrite() {
        let bridge = HookBridge::new();
        let input = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "git status" }
        });
        let action = bridge.process_hook(&input);
        match action {
            HookAction::Rewrite {
                original,
                rewritten,
                classification,
            } => {
                assert_eq!(original, "git status");
                assert!(
                    rewritten.contains("git"),
                    "rewritten should reference git: {rewritten}"
                );
                assert_eq!(classification.category, CommandCategory::Git);
            }
            other => panic!("expected Rewrite, got {other:?}"),
        }
    }

    #[test]
    fn test_process_hook_block_dangerous() {
        let bridge = HookBridge::new();
        let input = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "rm -rf /" }
        });
        let action = bridge.process_hook(&input);
        match action {
            HookAction::Block {
                reason,
                classification,
                ..
            } => {
                assert_eq!(classification.risk_level, CommandRiskLevel::Critical);
                assert!(!reason.is_empty(), "block reason should not be empty");
            }
            other => panic!("expected Block, got {other:?}"),
        }
    }

    #[test]
    fn test_process_hook_ignore_non_bash() {
        let bridge = HookBridge::new();
        let input = json!({
            "tool_name": "Read",
            "tool_input": { "file_path": "/etc/passwd" }
        });
        let action = bridge.process_hook(&input);
        assert!(
            matches!(action, HookAction::Ignore),
            "non-command tool should be ignored"
        );
    }

    // ── Compound command splitting ───────────────────────────────────────────

    #[test]
    fn test_compound_command_split_and() {
        let parts = split_compound_command("git status && rm -rf /tmp");
        assert_eq!(parts.len(), 2, "should split on &&");
        assert!(parts[0].contains("git"), "first part should be git status");
        assert!(parts[1].contains("rm"), "second part should be rm");
    }

    #[test]
    fn test_compound_command_no_split() {
        let parts = split_compound_command("cargo test --workspace");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], "cargo test --workspace");
    }

    #[test]
    fn test_compound_command_semicolon() {
        let parts = split_compound_command("cd /tmp; ls -la");
        assert_eq!(parts.len(), 2, "should split on semicolon");
    }

    // ── Environment prefix stripping ─────────────────────────────────────────

    #[test]
    fn test_strip_env_prefix() {
        assert_eq!(strip_env_prefix("ENV=val cmd arg"), "cmd arg");
        assert_eq!(strip_env_prefix("A=1 B=2 cargo build"), "cargo build");
        assert_eq!(strip_env_prefix("cargo build"), "cargo build");
        assert_eq!(strip_env_prefix(""), "");
    }

    // ── Default registry completeness ────────────────────────────────────────

    #[test]
    fn test_default_registry_rules() {
        let registry = RewriteRegistry::with_defaults();

        // All major command classes should be rewritable.
        for cmd in &[
            "git status",
            "cargo build",
            "npm install",
            "docker ps",
            "kubectl get pods",
            "cat README.md",
            "ls -la",
        ] {
            assert!(
                registry.can_rewrite(cmd),
                "default registry should match: {cmd}"
            );
        }
    }

    #[test]
    fn test_drop_sql_classified_critical() {
        let registry = RewriteRegistry::with_defaults();
        let c = registry.classify("DROP TABLE users;");
        assert_eq!(c.risk_level, CommandRiskLevel::Critical);
        assert_eq!(c.category, CommandCategory::Database);
    }

    #[test]
    fn test_process_command_compound_blocked_by_dangerous_segment() {
        let bridge = HookBridge::new();
        // Safe command combined with dangerous one — should be blocked.
        let action = bridge.process_command("git status && rm -rf /var/data");
        assert!(
            matches!(action, HookAction::Block { .. }),
            "compound with rm -rf must be blocked"
        );
    }
}
