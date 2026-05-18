// HIGH #8 FIX: Error message sanitization to prevent leaking sensitive system information

use regex::Regex;

/// Sanitize error messages to prevent leaking sensitive system information.
///
/// This function removes or redacts:
/// - Absolute file paths → `<path>`
/// - Usernames → "***"
/// - IP addresses → "x.x.x.x" or "x:x:x:x:x:x:x:x"
/// - System-specific paths (e.g., /Users/..., C:\Users\..., /home/...)
/// - Port numbers → ":****"
/// - Environment variable values → "VAR=***"
///
/// The original error is preserved in internal logs (via tracing), while sanitized
/// messages are returned to external callers.
///
/// # Examples
///
/// ```
/// use cyberclaw_connectors::error_sanitizer::sanitize_error;
///
/// let error = "Failed to read /Users/alice/secret/config.json";
/// let sanitized = sanitize_error(error);
/// assert!(!sanitized.contains("/Users/alice"));
/// assert!(sanitized.contains("<path>"));
/// ```
pub fn sanitize_error(error_msg: &str) -> String {
    let mut sanitized = error_msg.to_string();

    // Pattern 1: Absolute Unix paths (/Users/..., /home/..., /root/..., /tmp/..., /var/...)
    let unix_path_regex = Regex::new(r"/(?:Users|home|root|tmp|var|opt|usr|etc)/[^\s:,;)}\]]+")
        .expect("Unix path regex should compile");
    sanitized = unix_path_regex
        .replace_all(&sanitized, "<path>")
        .to_string();

    // Pattern 2: Absolute Windows paths (C:\Users\..., D:\...)
    let windows_path_regex =
        Regex::new(r"[A-Z]:\\(?:Users|Documents and Settings|Windows|Program Files)[^\s:,;)}\]]*")
            .expect("Windows path regex should compile");
    sanitized = windows_path_regex
        .replace_all(&sanitized, "<path>")
        .to_string();

    // Pattern 3: IPv4 addresses
    let ipv4_regex =
        Regex::new(r"\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}\b").expect("IPv4 regex should compile");
    sanitized = ipv4_regex.replace_all(&sanitized, "x.x.x.x").to_string();

    // Pattern 4: IPv6 addresses (simplified pattern)
    let ipv6_regex = Regex::new(r"\b(?:[0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}\b")
        .expect("IPv6 regex should compile");
    sanitized = ipv6_regex
        .replace_all(&sanitized, "x:x:x:x:x:x:x:x")
        .to_string();

    // Pattern 5: Port numbers in the format :PORT
    let port_regex = Regex::new(r":(\d{2,5})\b").expect("Port regex should compile");
    sanitized = port_regex.replace_all(&sanitized, ":****").to_string();

    // Pattern 6: Usernames in common patterns (user: ..., username: ..., USER=...)
    let username_regex = Regex::new(r"(?i)\b(?:user|username|USER)(?:=|:\s*)([^\s,;)}\]]+)")
        .expect("Username regex should compile");
    sanitized = username_regex
        .replace_all(&sanitized, "user: ***")
        .to_string();

    // Pattern 7: Environment variable values (VAR=value patterns, but keep VAR name)
    let env_var_regex =
        Regex::new(r"([A-Z_]+)=([^\s,;)}\]]+)").expect("Env var regex should compile");
    sanitized = env_var_regex.replace_all(&sanitized, "$1=***").to_string();

    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HIGH #8 FIX: Test that sanitize_error removes Unix absolute paths
    #[test]
    fn test_sanitize_unix_paths() {
        let error = "Failed to read /Users/alice/sensitive/config.json";
        let sanitized = sanitize_error(error);
        assert!(!sanitized.contains("/Users/alice"));
        assert!(sanitized.contains("<path>"));
    }

    /// HIGH #8 FIX: Test that sanitize_error removes Windows absolute paths
    #[test]
    fn test_sanitize_windows_paths() {
        let error = "Access denied: C:\\Users\\Bob\\Documents\\secret.txt";
        let sanitized = sanitize_error(error);
        assert!(!sanitized.contains("C:\\Users\\Bob"));
        assert!(sanitized.contains("<path>"));
    }

    /// HIGH #8 FIX: Test that sanitize_error removes IPv4 addresses
    #[test]
    fn test_sanitize_ipv4() {
        let error = "Connection failed to 192.168.1.100";
        let sanitized = sanitize_error(error);
        assert!(!sanitized.contains("192.168.1.100"));
        assert!(sanitized.contains("x.x.x.x"));
    }

    /// HIGH #8 FIX: Test that sanitize_error removes IPv6 addresses
    #[test]
    fn test_sanitize_ipv6() {
        let error = "Connection to 2001:0db8:85a3:0000:0000:8a2e:0370:7334 failed";
        let sanitized = sanitize_error(error);
        assert!(!sanitized.contains("2001:0db8:85a3:0000:0000:8a2e:0370:7334"));
        assert!(sanitized.contains("x:x:x:x:x:x:x:x"));
    }

    /// HIGH #8 FIX: Test that sanitize_error removes port numbers
    #[test]
    fn test_sanitize_ports() {
        let error = "Failed to connect to server:8080";
        let sanitized = sanitize_error(error);
        assert!(!sanitized.contains(":8080"));
        assert!(sanitized.contains(":****"));
    }

    /// HIGH #8 FIX: Test that sanitize_error removes usernames
    #[test]
    fn test_sanitize_usernames() {
        let error1 = "user: alice failed authentication";
        let sanitized1 = sanitize_error(error1);
        assert!(!sanitized1.contains("alice"));
        assert!(sanitized1.contains("user: ***"));

        let error2 = "USERNAME=bob in environment";
        let sanitized2 = sanitize_error(error2);
        assert!(!sanitized2.contains("bob"));
    }

    /// HIGH #8 FIX: Test that sanitize_error removes environment variable values
    #[test]
    fn test_sanitize_env_vars() {
        let error = "API_KEY=sk-1234567890abcdef DATABASE_URL=postgres://localhost/db";
        let sanitized = sanitize_error(error);
        assert!(!sanitized.contains("sk-1234567890abcdef"));
        assert!(!sanitized.contains("postgres://localhost/db"));
        assert!(sanitized.contains("API_KEY=***"));
        assert!(sanitized.contains("DATABASE_URL=***"));
    }

    /// HIGH #8 FIX: Test sanitization preserves useful error context
    #[test]
    fn test_sanitize_preserves_context() {
        let error = "Permission denied while accessing /home/user/file.txt at line 42";
        let sanitized = sanitize_error(error);

        // Should preserve useful context
        assert!(sanitized.contains("Permission denied"));
        assert!(sanitized.contains("at line 42"));

        // Should sanitize sensitive parts
        assert!(!sanitized.contains("/home/user/file.txt"));
        assert!(sanitized.contains("<path>"));
    }

    /// HIGH #8 FIX: Test multiple patterns in one error message
    #[test]
    fn test_sanitize_multiple_patterns() {
        let error =
            "user: alice at 10.0.0.5:3306 failed to access /var/lib/mysql/data with API_KEY=secret123";
        let sanitized = sanitize_error(error);

        assert!(!sanitized.contains("alice"));
        assert!(!sanitized.contains("10.0.0.5"));
        assert!(!sanitized.contains(":3306"));
        assert!(!sanitized.contains("/var/lib/mysql/data"));
        assert!(!sanitized.contains("secret123"));

        assert!(sanitized.contains("user: ***"));
        assert!(sanitized.contains("x.x.x.x"));
        assert!(sanitized.contains(":****"));
        assert!(sanitized.contains("<path>"));
        assert!(sanitized.contains("API_KEY=***"));
    }

    /// HIGH #8 FIX: Test that empty or simple errors are handled gracefully
    #[test]
    fn test_sanitize_simple_errors() {
        assert_eq!(sanitize_error(""), "");
        assert_eq!(sanitize_error("Simple error"), "Simple error");
        assert_eq!(sanitize_error("File not found"), "File not found");
    }
}
