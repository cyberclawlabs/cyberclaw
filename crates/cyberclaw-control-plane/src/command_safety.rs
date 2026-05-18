//! Command injection prevention and safe command execution.
//!
//! This module provides security validation for commands executed through the
//! CyberClaw execution pipeline, including:
//! - Block-list based command filtering
//! - Shell metacharacter detection
//! - Dangerous sequence detection
//! - Safe subprocess execution (bypassing shell interpretation)

// ─── Command Injection Prevention ─────────────────────────────────────────────

/// Commands that are blocked for security reasons.
/// These include destructive, privilege-escalation, and network tools
/// that should never be invoked directly through the execution pipeline.
const BLOCKED_COMMANDS: &[&str] = &[
    // Destructive / privilege escalation
    "rm",
    "dd",
    "mkfs",
    "kill",
    "pkill",
    "sudo",
    "su",
    "shutdown",
    "reboot",
    "passwd",
    "chown",
    "chmod",
    "iptables",
    "systemctl",
    "service",
    // Network tools
    "curl",
    "wget",
    "nc",
    "netcat",
    "telnet",
    "ssh",
    "scp",
    "rsync",
    "socat",
    "nmap",
    // Shell interpreters (can run arbitrary code)
    "bash",
    "sh",
    "zsh",
    "fish",
    "dash",
    "csh",
    // Script interpreters
    "python",
    "python3",
    "python2",
    "node",
    "perl",
    "ruby",
    "php",
    // Bypass tools
    "env",
    "nohup",
    "xargs",
    "strace",
    "ltrace",
    "gdb",
    // Container / orchestration
    "docker",
    "kubectl",
    "podman",
    // File manipulation (write/move/copy sensitive data)
    "cp",
    "mv",
    "tee",
    "truncate",
    "shred",
];

/// Shell metacharacters that enable command injection via shell interpretation.
const DANGEROUS_CHARS: &[char] = &[
    ';', '|', '&', '>', '<', '`', '$', '(', ')', '{', '}', '[', ']', '\n', '\r',
];

/// Multi-character sequences used for command chaining or substitution.
const DANGEROUS_SEQUENCES: &[&str] = &["&&", "||", ">>", "<<", "$(", "${", "`"];

/// Errors produced by command validation and safe execution.
#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    /// The supplied command string is empty or contains only whitespace.
    #[error("Invalid command: {0}")]
    InvalidCommand(String),

    /// The base command name appears on the security block-list.
    #[error("Forbidden command: {0}")]
    ForbiddenCommand(String),

    /// The command string contains a shell metacharacter.
    #[error("Command contains dangerous character: {0}")]
    DangerousCharacters(String),

    /// The command string contains a dangerous multi-character sequence.
    #[error("Command contains dangerous sequence: {0}")]
    DangerousSequence(String),

    /// The command did not complete within the allowed time window.
    #[error("Command execution timed out")]
    Timeout,

    /// The OS could not spawn the requested process.
    #[error("Failed to spawn process: {0}")]
    SpawnFailed(String),

    /// The spawned process returned a non-zero exit code or an I/O error.
    #[error("Command execution failed: {0}")]
    ExecutionFailed(String),
}

/// Output captured from a safely-executed subprocess.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Data written to standard output by the process.
    pub stdout: String,
    /// Data written to standard error by the process.
    pub stderr: String,
    /// Exit status code; `-1` when the process was terminated by a signal.
    pub exit_code: i32,
}

/// Validates a command string to prevent injection attacks.
///
/// # Security Checks
/// - Rejects empty commands.
/// - Rejects commands whose base name appears in [`BLOCKED_COMMANDS`].
/// - Rejects commands containing shell metacharacters from [`DANGEROUS_CHARS`].
/// - Rejects commands containing chaining sequences from [`DANGEROUS_SEQUENCES`].
/// - Strips leading path components so `/usr/bin/rm` is treated the same as `rm`.
///
/// # Errors
/// Returns an [`ExecutionError`] variant describing the specific violation.
pub fn validate_command(cmd: &str) -> Result<(), ExecutionError> {
    if cmd.trim().is_empty() {
        return Err(ExecutionError::InvalidCommand("Empty command".to_string()));
    }

    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return Err(ExecutionError::InvalidCommand(
            "No command specified".to_string(),
        ));
    }

    // Strip any leading path so `/usr/bin/rm` → `rm`.
    let base_cmd = parts[0].rsplit('/').next().unwrap_or(parts[0]);

    if BLOCKED_COMMANDS.contains(&base_cmd) {
        return Err(ExecutionError::ForbiddenCommand(format!(
            "Command '{}' is blocked for security reasons",
            base_cmd
        )));
    }

    for ch in DANGEROUS_CHARS {
        if cmd.contains(*ch) {
            return Err(ExecutionError::DangerousCharacters(format!(
                "Command contains forbidden character: '{}'",
                ch
            )));
        }
    }

    for seq in DANGEROUS_SEQUENCES {
        if cmd.contains(seq) {
            return Err(ExecutionError::DangerousSequence(format!(
                "Command contains forbidden sequence: '{}'",
                seq
            )));
        }
    }

    Ok(())
}

/// Executes a command safely using an explicit argv array instead of a shell.
///
/// Bypassing the shell eliminates an entire class of injection vectors: no
/// glob expansion, no variable substitution, no command chaining.  Each
/// element of `args` is passed verbatim to the kernel's `execve` syscall.
///
/// # Arguments
/// * `cmd`         – The executable name or absolute path (validated before use).
/// * `args`        – Positional arguments forwarded verbatim to the process.
/// * `working_dir` – Optional working directory; inherits the caller's cwd when `None`.
///
/// # Errors
/// Returns an [`ExecutionError`] if:
/// - `cmd` fails [`validate_command`].
/// - The process cannot be spawned.
/// - The execution exceeds a 300-second timeout.
pub async fn execute_command_safe(
    cmd: &str,
    args: &[&str],
    working_dir: Option<&std::path::Path>,
) -> Result<CommandOutput, ExecutionError> {
    use std::process::Command;
    use std::time::Duration;

    validate_command(cmd)?;

    // Build a owned copy so it can be moved into spawn_blocking.
    let mut command = Command::new(cmd);
    command.args(args);

    if let Some(dir) = working_dir {
        command.current_dir(dir);
    }

    // Execute on a blocking thread to avoid starving the async runtime.
    let output = tokio::time::timeout(
        Duration::from_secs(300),
        tokio::task::spawn_blocking(move || command.output()),
    )
    .await
    .map_err(|_| ExecutionError::Timeout)?
    .map_err(|e| ExecutionError::SpawnFailed(e.to_string()))?
    .map_err(|e| ExecutionError::ExecutionFailed(e.to_string()))?;

    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_command_rejected() {
        assert!(matches!(
            validate_command(""),
            Err(ExecutionError::InvalidCommand(_))
        ));
        assert!(matches!(
            validate_command("   "),
            Err(ExecutionError::InvalidCommand(_))
        ));
    }

    #[test]
    fn blocked_commands_rejected() {
        for cmd in BLOCKED_COMMANDS {
            assert!(
                matches!(
                    validate_command(cmd),
                    Err(ExecutionError::ForbiddenCommand(_))
                ),
                "Expected '{}' to be blocked",
                cmd
            );
        }
    }

    #[test]
    fn blocked_command_with_path_rejected() {
        assert!(matches!(
            validate_command("/usr/bin/rm"),
            Err(ExecutionError::ForbiddenCommand(_))
        ));
        assert!(matches!(
            validate_command("/usr/local/bin/python3"),
            Err(ExecutionError::ForbiddenCommand(_))
        ));
    }

    #[test]
    fn shell_interpreters_blocked() {
        for cmd in &[
            "bash", "sh", "zsh", "python", "python3", "node", "perl", "ruby",
        ] {
            assert!(
                matches!(
                    validate_command(cmd),
                    Err(ExecutionError::ForbiddenCommand(_))
                ),
                "Expected interpreter '{}' to be blocked",
                cmd
            );
        }
    }

    #[test]
    fn bypass_tools_blocked() {
        for cmd in &["env", "nohup", "xargs", "docker", "kubectl"] {
            assert!(
                matches!(
                    validate_command(cmd),
                    Err(ExecutionError::ForbiddenCommand(_))
                ),
                "Expected bypass tool '{}' to be blocked",
                cmd
            );
        }
    }

    #[test]
    fn dangerous_chars_rejected() {
        for ch in DANGEROUS_CHARS {
            let cmd = format!("echo{}world", ch);
            assert!(
                matches!(
                    validate_command(&cmd),
                    Err(ExecutionError::DangerousCharacters(_))
                ),
                "Expected char '{}' to be rejected",
                ch
            );
        }
    }

    #[test]
    fn dangerous_sequences_rejected() {
        // Sequences contain chars also in DANGEROUS_CHARS, so the rejection may come
        // from either check. We only verify the command is rejected.
        for seq in DANGEROUS_SEQUENCES {
            let cmd = format!("ls {}true", seq);
            assert!(
                validate_command(&cmd).is_err(),
                "Expected command containing '{}' to be rejected",
                seq
            );
        }
    }

    #[test]
    fn clean_commands_accepted() {
        assert!(validate_command("ls").is_ok());
        assert!(validate_command("echo hello world").is_ok());
        assert!(validate_command("cat file.txt").is_ok());
        assert!(validate_command("/usr/bin/grep pattern").is_ok());
    }

    #[test]
    fn blocked_command_via_absolute_path() {
        // Absolute path to a blocked command is still rejected after path stripping
        assert!(validate_command("/bin/rm").is_err());
        assert!(validate_command("/usr/sbin/reboot").is_err());
        assert!(validate_command("/usr/bin/python3").is_err());
    }
}
