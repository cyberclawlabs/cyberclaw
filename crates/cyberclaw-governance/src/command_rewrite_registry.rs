//! Command-level permission registry for CyberClaw governance.
//!
//! Provides a rule-based gate for shell commands delivered by external Agents.
//! When a command arrives, the checker:
//!
//! 1. Splits compound commands (`&&`, `||`, `;`, `|`) into individual segments,
//!    respecting quoted strings.
//! 2. Strips common transparent prefixes (`sudo`, `env KEY=VAL`, `nohup`, etc.)
//!    so the underlying command is evaluated correctly.
//! 3. Evaluates each segment against deny / ask / allow rule lists in that order.
//! 4. Returns the most restrictive [`CommandPermissionVerdict`] across all
//!    segments: **Deny > Ask > Allow > Default**.
//!
//! Inspired by RTK's `discover/registry.rs` and `hooks/permissions.rs`.
//!
//! # Examples
//!
//! ```
//! use cyberclaw_governance::command_rewrite_registry::{
//!     CommandPermissionChecker, CommandPermissionVerdict,
//! };
//!
//! let checker = CommandPermissionChecker::with_defaults();
//! assert_eq!(checker.check("ls -la"), CommandPermissionVerdict::Allow);
//! assert_eq!(checker.check("rm -rf /"), CommandPermissionVerdict::Deny);
//! assert_eq!(checker.check("git push --force"), CommandPermissionVerdict::Ask);
//! ```

use regex::Regex;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Permission verdict for a command.
///
/// Precedence when combining verdicts: **Deny > Ask > Allow > Default**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandPermissionVerdict {
    /// An explicit allow rule matched — safe to auto-execute.
    Allow,
    /// An ask rule matched — execute but require user confirmation.
    Ask,
    /// A deny rule matched — block execution entirely.
    Deny,
    /// No rule matched — defaults to ask (least-privilege).
    Default,
}

/// The verdict assigned by a single [`CommandPermissionRule`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuleVerdict {
    Allow,
    Ask,
    Deny,
}

/// Whether [`CommandPermissionRule::pattern`] is a glob or a regex.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PatternType {
    /// Shell-style glob (`*` and `?`).
    Glob,
    /// Full POSIX-compatible regular expression.
    Regex,
}

/// A single permission rule applied to a raw command string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandPermissionRule {
    /// Glob or regex pattern matched against the command.
    pub pattern: String,
    /// Interpretation of [`pattern`](CommandPermissionRule::pattern).
    pub pattern_type: PatternType,
    /// Verdict when this rule fires.
    pub verdict: RuleVerdict,
    /// Optional human-readable description used in audit trails.
    pub description: Option<String>,
}

/// Errors that can occur when building a [`CommandPermissionChecker`].
#[derive(Debug, thiserror::Error)]
pub enum CommandPermissionError {
    /// A regex pattern failed to compile.
    #[error("Invalid regex pattern '{pattern}': {message}")]
    InvalidPattern { pattern: String, message: String },

    /// Two rules with the same pattern but different verdicts were supplied.
    #[error("Conflicting rules for pattern: {0}")]
    ConflictingRules(String),
}

// ---------------------------------------------------------------------------
// Internal compiled form
// ---------------------------------------------------------------------------

#[derive(Debug)]
#[allow(dead_code)]
struct CompiledRule {
    pattern: Regex,
    description: Option<String>,
}

// ---------------------------------------------------------------------------
// CommandPermissionChecker
// ---------------------------------------------------------------------------

/// Checks commands against ordered deny / ask / allow rule lists.
///
/// Rules within each list are evaluated in insertion order; the first
/// matching rule in the list wins.  Precedence across lists is always:
/// **deny > ask > allow > default**.
#[derive(Debug)]
pub struct CommandPermissionChecker {
    deny_rules: Vec<CompiledRule>,
    ask_rules: Vec<CompiledRule>,
    allow_rules: Vec<CompiledRule>,

    // Keep the raw rules so callers can add more at runtime.
    raw_rules: Vec<CommandPermissionRule>,
}

impl CommandPermissionChecker {
    /// Build a checker from a list of rules.
    ///
    /// All regex patterns are compiled eagerly; an error is returned on the
    /// first invalid pattern.
    pub fn new(rules: Vec<CommandPermissionRule>) -> Result<Self, CommandPermissionError> {
        let mut checker = Self {
            deny_rules: Vec::new(),
            ask_rules: Vec::new(),
            allow_rules: Vec::new(),
            raw_rules: Vec::new(),
        };
        for rule in rules {
            checker.add_rule(rule)?;
        }
        Ok(checker)
    }

    /// Create a checker pre-loaded with a curated set of default rules.
    ///
    /// The defaults cover common dangerous, risky, and safe-read-only
    /// shell commands.  They are intentionally conservative.
    pub fn with_defaults() -> Self {
        Self::new(default_rules()).expect("built-in default rules must be valid")
    }

    /// Evaluate a single (non-compound) command string.
    ///
    /// The command is stripped of transparent prefixes before matching.
    /// Precedence: deny > ask > allow > default.
    pub fn check(&self, command: &str) -> CommandPermissionVerdict {
        let stripped = strip_command_prefix(command);
        self.check_stripped(stripped)
    }

    /// Evaluate a potentially compound shell command.
    ///
    /// The command is first split on `&&`, `||`, `;`, and `|` (respecting
    /// quoted strings).  Then each segment is checked independently:
    ///
    /// - If **any** segment is **Deny** → the whole command is `Deny`.
    /// - If **any** segment is **Ask** → the whole command is `Ask`.
    /// - If **all** segments are **Allow** → the whole command is `Allow`.
    /// - Otherwise (mix of Allow + Default, or all Default) → `Default`.
    pub fn check_compound(&self, command: &str) -> CommandPermissionVerdict {
        let segments = split_compound_command(command);
        if segments.is_empty() {
            return self.check(command);
        }

        let mut worst = CommandPermissionVerdict::Allow;
        for seg in &segments {
            let verdict = self.check(seg.trim());
            worst = merge_verdicts(worst, verdict);
            if worst == CommandPermissionVerdict::Deny {
                // Short-circuit: nothing can be worse than Deny.
                break;
            }
        }
        worst
    }

    /// Dynamically append a rule to the checker.
    ///
    /// The rule is compiled immediately; an error is returned if the pattern
    /// is invalid.  New rules are appended to the **end** of the relevant
    /// list, so earlier rules still take precedence.
    pub fn add_rule(&mut self, rule: CommandPermissionRule) -> Result<(), CommandPermissionError> {
        let compiled = compile_rule(&rule)?;
        match rule.verdict {
            RuleVerdict::Deny => self.deny_rules.push(compiled),
            RuleVerdict::Ask => self.ask_rules.push(compiled),
            RuleVerdict::Allow => self.allow_rules.push(compiled),
        }
        self.raw_rules.push(rule);
        Ok(())
    }

    // --- private helpers ---------------------------------------------------

    /// Check a command that has already had its prefix stripped.
    fn check_stripped(&self, cmd: &str) -> CommandPermissionVerdict {
        if rule_matches(&self.deny_rules, cmd) {
            return CommandPermissionVerdict::Deny;
        }
        if rule_matches(&self.ask_rules, cmd) {
            return CommandPermissionVerdict::Ask;
        }
        if rule_matches(&self.allow_rules, cmd) {
            return CommandPermissionVerdict::Allow;
        }
        CommandPermissionVerdict::Default
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Return `true` if any rule in `list` matches `cmd`.
fn rule_matches(list: &[CompiledRule], cmd: &str) -> bool {
    list.iter().any(|r| r.pattern.is_match(cmd))
}

/// Compile a [`CommandPermissionRule`] into a [`CompiledRule`].
///
/// Glob patterns are converted to an equivalent anchored regex before
/// compilation.
fn compile_rule(rule: &CommandPermissionRule) -> Result<CompiledRule, CommandPermissionError> {
    let regex_src = match rule.pattern_type {
        PatternType::Regex => rule.pattern.clone(),
        PatternType::Glob => glob_to_regex(&rule.pattern),
    };

    let pattern = Regex::new(&regex_src).map_err(|e| CommandPermissionError::InvalidPattern {
        pattern: rule.pattern.clone(),
        message: e.to_string(),
    })?;

    Ok(CompiledRule {
        pattern,
        description: rule.description.clone(),
    })
}

/// Convert a simple glob pattern (`*`, `?`) to an anchored regex string.
fn glob_to_regex(glob: &str) -> String {
    let mut out = String::from("(?i)^");
    for ch in glob.chars() {
        match ch {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            c => {
                // Escape regex meta-characters.
                if "^$.|+()[]{}\\".contains(c) {
                    out.push('\\');
                }
                out.push(c);
            }
        }
    }
    out.push('$');
    out
}

/// Merge two verdicts, keeping the more restrictive one.
///
/// Precedence order: Deny > Ask > Allow > Default.
fn merge_verdicts(
    a: CommandPermissionVerdict,
    b: CommandPermissionVerdict,
) -> CommandPermissionVerdict {
    use CommandPermissionVerdict::*;
    match (&a, &b) {
        (Deny, _) | (_, Deny) => Deny,
        (Ask, _) | (_, Ask) => Ask,
        (Allow, Allow) => Allow,
        (Allow, Default) | (Default, Allow) => Default,
        (Default, Default) => Default,
    }
}

/// Split a compound shell command into individual segments.
///
/// Handles operators `&&`, `||`, `;`, and `|` (pipe).
/// Single-quoted and double-quoted strings are treated as opaque; operators
/// inside quotes are **not** treated as delimiters.
pub fn split_compound_command(cmd: &str) -> Vec<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = cmd.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                current.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                current.push(ch);
            }
            '&' if !in_single && !in_double => {
                if chars.peek() == Some(&'&') {
                    chars.next(); // consume second '&'
                    let seg = current.trim().to_string();
                    if !seg.is_empty() {
                        segments.push(seg);
                    }
                    current.clear();
                } else {
                    current.push(ch);
                }
            }
            '|' if !in_single && !in_double => {
                if chars.peek() == Some(&'|') {
                    chars.next(); // consume second '|'
                    let seg = current.trim().to_string();
                    if !seg.is_empty() {
                        segments.push(seg);
                    }
                    current.clear();
                } else {
                    // Single pipe.
                    let seg = current.trim().to_string();
                    if !seg.is_empty() {
                        segments.push(seg);
                    }
                    current.clear();
                }
            }
            ';' if !in_single && !in_double => {
                let seg = current.trim().to_string();
                if !seg.is_empty() {
                    segments.push(seg);
                }
                current.clear();
            }
            other => {
                current.push(other);
            }
        }
    }

    let remaining = current.trim().to_string();
    if !remaining.is_empty() {
        segments.push(remaining);
    }

    segments
}

/// Strip well-known transparent command prefixes that do not change the
/// effective command for permission-matching purposes.
///
/// Stripped prefixes: `sudo`, `nohup`, `time`, `nice`, `env KEY=VAL`.
pub fn strip_command_prefix(cmd: &str) -> &str {
    let cmd = cmd.trim();
    // Iterate: each pass may strip one prefix token.
    let mut rest = cmd;
    loop {
        // Strip leading whitespace.
        rest = rest.trim_start();

        if rest.starts_with("sudo ") || rest.starts_with("sudo\t") {
            rest = rest["sudo ".len()..].trim_start();
            continue;
        }
        if rest.starts_with("nohup ") || rest.starts_with("nohup\t") {
            rest = rest["nohup ".len()..].trim_start();
            continue;
        }
        if rest.starts_with("time ") || rest.starts_with("time\t") {
            rest = rest["time ".len()..].trim_start();
            continue;
        }
        if rest.starts_with("nice ") || rest.starts_with("nice\t") {
            rest = rest["nice ".len()..].trim_start();
            continue;
        }
        // Strip `env KEY=VAL` assignments: token containing `=` with no
        // spaces is an env override.
        if let Some(first_space) = rest.find(|c: char| c.is_whitespace()) {
            let first_token = &rest[..first_space];
            if first_token.starts_with("env") && first_token == "env" {
                // `env VAR=val cmd` form.
                let after_env = rest[first_space..].trim_start();
                // Consume all `KEY=VALUE` tokens.
                let remaining = skip_env_assignments(after_env);
                if remaining != after_env {
                    rest = remaining;
                    continue;
                }
            } else if first_token.contains('=') && !first_token.starts_with('-') {
                // Bare `KEY=VAL cmd` prefix.
                rest = rest[first_space..].trim_start();
                continue;
            }
        }
        // Nothing more to strip.
        break;
    }
    rest
}

/// Skip over `KEY=VALUE` tokens (used after `env`), returning the remainder.
fn skip_env_assignments(s: &str) -> &str {
    let mut rest = s;
    loop {
        let token_end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        let token = &rest[..token_end];
        if token.contains('=') && !token.starts_with('-') {
            rest = rest[token_end..].trim_start();
        } else {
            break;
        }
    }
    rest
}

// ---------------------------------------------------------------------------
// Default rules
// ---------------------------------------------------------------------------

/// Returns the built-in set of command permission rules.
///
/// Rules are grouped by verdict in priority order:
/// 1. **Deny** — irreversible / destructive operations.
/// 2. **Ask** — risky operations that require human confirmation.
/// 3. **Allow** — safe read-only operations.
fn default_rules() -> Vec<CommandPermissionRule> {
    use PatternType::Regex as Re;
    use RuleVerdict::*;

    fn r(pattern: &str, verdict: RuleVerdict, desc: &str) -> CommandPermissionRule {
        CommandPermissionRule {
            pattern: pattern.to_string(),
            pattern_type: Re,
            verdict,
            description: Some(desc.to_string()),
        }
    }

    vec![
        // -- Deny: irreversible / destructive ---------------------------------
        r(r"rm\s+-rf\s+/", Deny, "Recursive delete from root"),
        r(r"mkfs\.", Deny, "Filesystem format"),
        r(r"dd\s+.*of=/dev/", Deny, "Direct device write"),
        r(
            r"(?i)DROP\s+(TABLE|DATABASE|SCHEMA)",
            Deny,
            "Database destruction",
        ),
        r(r":\(\)\{\ :\|:&\ \};:", Deny, "Fork bomb"),
        r(r">\s*/dev/sd[a-z]", Deny, "Direct block device overwrite"),
        r(r"shred\s+", Deny, "Secure erase"),
        r(r"wipefs\s+", Deny, "Wipe filesystem signatures"),
        // -- Ask: risky, requires confirmation --------------------------------
        r(r"git\s+push\s+--force", Ask, "Force push"),
        r(r"git\s+reset\s+--hard", Ask, "Hard reset"),
        r(r"rm\s+-rf", Ask, "Recursive force delete"),
        r(r"chmod\s+777", Ask, "World-writable permission"),
        r(r"chmod\s+\+s", Ask, "Setuid/setgid bit"),
        r(r"chown\s+root", Ask, "Transfer ownership to root"),
        r(r"sudo\s+", Ask, "Privilege escalation via sudo"),
        r(r"iptables\s+", Ask, "Firewall rule modification"),
        r(
            r"ufw\s+(enable|disable|reset)",
            Ask,
            "Firewall state change",
        ),
        r(
            r"systemctl\s+(stop|disable|mask)\s+",
            Ask,
            "Stop/disable service",
        ),
        r(r"crontab\s+-[ri]", Ask, "Crontab removal or replace"),
        r(r"curl\s+.*\|\s*(ba)?sh", Ask, "Download-and-execute"),
        r(r"wget\s+.*\|\s*(ba)?sh", Ask, "Download-and-execute"),
        r(r"pip\s+install\s+--user", Ask, "User-scope package install"),
        r(r"npm\s+install\s+-g", Ask, "Global npm install"),
        r(r"cargo\s+install\s+", Ask, "Cargo binary install"),
        // -- Allow: safe read-only --------------------------------------------
        r(
            r"^(ls|cat|head|tail|less|more|wc|file|stat|du|df)(\s|$)",
            Allow,
            "Safe read-only file operations",
        ),
        r(
            r"^git\s+(status|log|diff|show|branch|remote|tag|fetch|stash list)(\s|$)",
            Allow,
            "Safe read-only git operations",
        ),
        r(
            r"^(echo|printf|date|whoami|uname|hostname|pwd|env|printenv|id)(\s|$)",
            Allow,
            "Safe informational commands",
        ),
        r(
            r"^cargo\s+(check|clippy|test|fmt|doc|build)(\s|$)",
            Allow,
            "Safe cargo commands",
        ),
        r(
            r"^(grep|rg|find|locate|which|type|whereis)(\s|$)",
            Allow,
            "Safe search commands",
        ),
        r(
            r"^(ps|top|htop|pgrep|lsof|netstat|ss|ifconfig|ip)(\s|$)",
            Allow,
            "Safe process/network inspection",
        ),
        r(
            r"^(man|help|--help|-h|info)(\s|$)",
            Allow,
            "Documentation lookup",
        ),
    ]
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // 1. Allow: safe read-only commands
    // -----------------------------------------------------------------------
    #[test]
    fn test_allow_safe_commands() {
        let c = CommandPermissionChecker::with_defaults();
        assert_eq!(c.check("ls -la"), CommandPermissionVerdict::Allow);
        assert_eq!(c.check("cat README.md"), CommandPermissionVerdict::Allow);
        assert_eq!(c.check("git status"), CommandPermissionVerdict::Allow);
        assert_eq!(
            c.check("git log --oneline"),
            CommandPermissionVerdict::Allow
        );
        assert_eq!(c.check("echo hello"), CommandPermissionVerdict::Allow);
        assert_eq!(c.check("cargo check"), CommandPermissionVerdict::Allow);
        assert_eq!(c.check("cargo test"), CommandPermissionVerdict::Allow);
    }

    // -----------------------------------------------------------------------
    // 2. Deny: dangerous commands
    // -----------------------------------------------------------------------
    #[test]
    fn test_deny_dangerous_commands() {
        let c = CommandPermissionChecker::with_defaults();
        assert_eq!(c.check("rm -rf /"), CommandPermissionVerdict::Deny);
        assert_eq!(c.check("DROP TABLE users"), CommandPermissionVerdict::Deny);
        assert_eq!(
            c.check("mkfs.ext4 /dev/sda1"),
            CommandPermissionVerdict::Deny
        );
        assert_eq!(
            c.check("dd if=/dev/zero of=/dev/sda"),
            CommandPermissionVerdict::Deny
        );
    }

    // -----------------------------------------------------------------------
    // 3. Ask: risky commands
    // -----------------------------------------------------------------------
    #[test]
    fn test_ask_risky_commands() {
        let c = CommandPermissionChecker::with_defaults();
        assert_eq!(c.check("git push --force"), CommandPermissionVerdict::Ask);
        assert_eq!(
            c.check("git reset --hard HEAD~1"),
            CommandPermissionVerdict::Ask
        );
        assert_eq!(
            c.check("rm -rf ./node_modules"),
            CommandPermissionVerdict::Ask
        );
        assert_eq!(
            c.check("chmod 777 /tmp/file"),
            CommandPermissionVerdict::Ask
        );
    }

    // -----------------------------------------------------------------------
    // 4. Default: unknown commands
    // -----------------------------------------------------------------------
    #[test]
    fn test_default_is_ask_for_unknown_command() {
        let c = CommandPermissionChecker::with_defaults();
        // An unknown command with no matching rule returns Default.
        assert_eq!(
            c.check("my_custom_binary --flag"),
            CommandPermissionVerdict::Default
        );
    }

    // -----------------------------------------------------------------------
    // 5. Compound: deny wins if any segment is deny
    // -----------------------------------------------------------------------
    #[test]
    fn test_compound_deny_any_segment() {
        let c = CommandPermissionChecker::with_defaults();
        // "git status" is Allow, but "rm -rf /" is Deny → whole chain is Deny.
        assert_eq!(
            c.check_compound("git status && rm -rf /"),
            CommandPermissionVerdict::Deny
        );
    }

    // -----------------------------------------------------------------------
    // 6. Compound: all allow segments → Allow
    // -----------------------------------------------------------------------
    #[test]
    fn test_compound_allow_all_segments() {
        let c = CommandPermissionChecker::with_defaults();
        assert_eq!(
            c.check_compound("ls && cat README.md"),
            CommandPermissionVerdict::Allow
        );
    }

    // -----------------------------------------------------------------------
    // 7. Compound: mix of ask and allow → Ask
    // -----------------------------------------------------------------------
    #[test]
    fn test_compound_mixed_ask() {
        let c = CommandPermissionChecker::with_defaults();
        assert_eq!(
            c.check_compound("git status && git push --force"),
            CommandPermissionVerdict::Ask
        );
    }

    // -----------------------------------------------------------------------
    // 8. Prefix stripping: sudo
    // -----------------------------------------------------------------------
    #[test]
    fn test_strip_sudo_prefix() {
        // "sudo rm -rf ..." — after stripping sudo, "rm -rf" matches Ask.
        let c = CommandPermissionChecker::with_defaults();
        let verdict = c.check("sudo rm -rf ./old");
        // sudo itself matches Ask rule; rm -rf also matches Ask.
        assert!(
            verdict == CommandPermissionVerdict::Ask || verdict == CommandPermissionVerdict::Deny
        );
    }

    // -----------------------------------------------------------------------
    // 9. Prefix stripping: env VAR=val
    // -----------------------------------------------------------------------
    #[test]
    fn test_strip_env_prefix() {
        let stripped = strip_command_prefix("env MY_VAR=hello ls -la");
        assert_eq!(stripped, "ls -la");

        let stripped2 = strip_command_prefix("KEY=value ls");
        assert_eq!(stripped2, "ls");
    }

    // -----------------------------------------------------------------------
    // 10. Compound split: quoted strings preserved
    // -----------------------------------------------------------------------
    #[test]
    fn test_quoted_strings_preserved() {
        // The `&&` inside quotes must NOT split the command.
        let segments = split_compound_command(r#"echo "hello && world""#);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0], r#"echo "hello && world""#);
    }

    // -----------------------------------------------------------------------
    // 11. Dynamic rule addition
    // -----------------------------------------------------------------------
    #[test]
    fn test_add_dynamic_rule() {
        let mut c = CommandPermissionChecker::with_defaults();
        // Add a custom deny rule.
        c.add_rule(CommandPermissionRule {
            pattern: r"my_dangerous_tool".to_string(),
            pattern_type: PatternType::Regex,
            verdict: RuleVerdict::Deny,
            description: Some("Custom deny".to_string()),
        })
        .expect("rule should compile");

        assert_eq!(
            c.check("my_dangerous_tool --run"),
            CommandPermissionVerdict::Deny
        );
    }

    // -----------------------------------------------------------------------
    // 12. Deny takes precedence over Ask and Allow
    // -----------------------------------------------------------------------
    #[test]
    fn test_deny_takes_precedence() {
        // Build a checker where a deny and an allow rule both match.
        let rules = vec![
            CommandPermissionRule {
                pattern: r"^ls".to_string(),
                pattern_type: PatternType::Regex,
                verdict: RuleVerdict::Allow,
                description: None,
            },
            CommandPermissionRule {
                pattern: r"^ls".to_string(),
                pattern_type: PatternType::Regex,
                verdict: RuleVerdict::Deny,
                description: None,
            },
        ];
        let c = CommandPermissionChecker::new(rules).unwrap();
        // Deny list is checked before Allow list → Deny wins.
        assert_eq!(c.check("ls -la"), CommandPermissionVerdict::Deny);
    }

    // -----------------------------------------------------------------------
    // 13. Pipe command split
    // -----------------------------------------------------------------------
    #[test]
    fn test_pipe_command_split() {
        let segments = split_compound_command("cat file.txt | grep foo | wc -l");
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0], "cat file.txt");
        assert_eq!(segments[1], "grep foo");
        assert_eq!(segments[2], "wc -l");
    }

    // -----------------------------------------------------------------------
    // 14. Semicolon split
    // -----------------------------------------------------------------------
    #[test]
    fn test_semicolon_split() {
        let segments = split_compound_command("echo start; ls; echo done");
        assert_eq!(segments.len(), 3);
    }

    // -----------------------------------------------------------------------
    // 15. OR operator split (||)
    // -----------------------------------------------------------------------
    #[test]
    fn test_or_operator_split() {
        let segments = split_compound_command("ls /nonexistent || echo fallback");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0], "ls /nonexistent");
        assert_eq!(segments[1], "echo fallback");
    }

    // -----------------------------------------------------------------------
    // 16. Invalid regex returns error
    // -----------------------------------------------------------------------
    #[test]
    fn test_invalid_regex_returns_error() {
        let result = CommandPermissionChecker::new(vec![CommandPermissionRule {
            pattern: r"[invalid".to_string(),
            pattern_type: PatternType::Regex,
            verdict: RuleVerdict::Deny,
            description: None,
        }]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CommandPermissionError::InvalidPattern { .. }));
    }

    // -----------------------------------------------------------------------
    // 17. Glob pattern type compiles and matches
    // -----------------------------------------------------------------------
    #[test]
    fn test_glob_pattern_type() {
        let c = CommandPermissionChecker::new(vec![CommandPermissionRule {
            pattern: "rm*".to_string(),
            pattern_type: PatternType::Glob,
            verdict: RuleVerdict::Ask,
            description: Some("rm variants".to_string()),
        }])
        .unwrap();
        assert_eq!(c.check("rm file.txt"), CommandPermissionVerdict::Ask);
        assert_eq!(c.check("rmdir empty/"), CommandPermissionVerdict::Ask);
    }

    // -----------------------------------------------------------------------
    // 18. Merge verdicts: deny beats everything
    // -----------------------------------------------------------------------
    #[test]
    fn test_merge_verdicts_deny_beats_all() {
        use CommandPermissionVerdict::*;
        assert_eq!(merge_verdicts(Allow, Deny), Deny);
        assert_eq!(merge_verdicts(Ask, Deny), Deny);
        assert_eq!(merge_verdicts(Default, Deny), Deny);
        assert_eq!(merge_verdicts(Deny, Allow), Deny);
    }

    // -----------------------------------------------------------------------
    // 19. strip_command_prefix: multiple layers
    // -----------------------------------------------------------------------
    #[test]
    fn test_strip_multiple_prefixes() {
        let result = strip_command_prefix("sudo nohup ls -la");
        assert_eq!(result, "ls -la");
    }

    // -----------------------------------------------------------------------
    // 20. Empty command does not panic
    // -----------------------------------------------------------------------
    #[test]
    fn test_empty_command() {
        let c = CommandPermissionChecker::with_defaults();
        // Should not panic; default verdict is Default.
        let verdict = c.check("");
        assert_eq!(verdict, CommandPermissionVerdict::Default);
    }
}
