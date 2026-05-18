//! Integration tests for AutopilotWorkspace boundary enforcement

use cyberclaw_control_plane::autopilot_workspace::AutopilotWorkspace;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_path_traversal_prevention() {
    let temp = TempDir::new().unwrap();
    let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

    // Test various path traversal attempts
    let traversal_attempts = vec![
        "../etc/passwd",
        "../../etc/passwd",
        "../../../etc/shadow",
        "foo/../../../etc/passwd",
        "./foo/../../bar/../../../etc/passwd",
        "foo/%2e%2e/bar",
        "foo/%2E%2E/bar",
    ];

    for path in traversal_attempts {
        assert!(
            workspace.check_path(path).is_err(),
            "Path traversal should be blocked: {}",
            path
        );
    }
}

#[test]
fn test_system_path_blocking() {
    let temp = TempDir::new().unwrap();
    let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

    // Test that system paths are blocked
    let system_paths = vec![
        "/etc/passwd",
        "/etc/shadow",
        "/sys/kernel/config",
        "/proc/1/status",
        "/dev/null",
        "/root/.ssh/id_rsa",
        "/var/log/syslog",
    ];

    for path in system_paths {
        assert!(
            workspace.check_path(path).is_err(),
            "System path should be blocked: {}",
            path
        );
    }
}

#[test]
fn test_workspace_boundary_enforcement() {
    let temp = TempDir::new().unwrap();
    let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

    // Create a safe file within workspace
    let safe_file = temp.path().join("safe.txt");
    fs::write(&safe_file, "test content").unwrap();

    // Should allow files within workspace
    assert!(workspace.check_path("safe.txt").is_ok());
    assert!(workspace.check_path(&safe_file).is_ok());

    // Should block files outside workspace
    let outside = "/tmp/outside_workspace.txt";
    assert!(workspace.check_path(outside).is_err());

    // Should block absolute paths to other directories
    assert!(workspace.check_path("/home/user/file.txt").is_err());
}

#[test]
fn test_null_byte_prevention() {
    let temp = TempDir::new().unwrap();
    let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

    // Null byte injection attempts
    let null_byte_paths = vec![
        "file.txt\0.exe",
        "normal\0/etc/passwd",
        "test\0../../etc/shadow",
    ];

    for path in null_byte_paths {
        assert!(
            workspace.check_path(path).is_err(),
            "Null byte should be blocked: {:?}",
            path
        );
    }
}

#[test]
fn test_length_limits() {
    let temp = TempDir::new().unwrap();
    let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

    // Test path length limit
    let long_path = "a".repeat(5000);
    assert!(
        workspace.check_path(&long_path).is_err(),
        "Very long path should be blocked"
    );

    // Test filename length limit
    let long_filename = format!("file_{}.txt", "x".repeat(300));
    assert!(
        workspace.check_path(&long_filename).is_err(),
        "Very long filename should be blocked"
    );

    // Normal length should be fine
    let normal_path = "normal/path/to/file.txt";
    assert!(workspace.check_path(normal_path).is_ok());
}

#[test]
fn test_hidden_file_policy() {
    let temp = TempDir::new().unwrap();

    // Test default policy (hidden files blocked)
    let workspace = AutopilotWorkspace::new(temp.path()).unwrap();
    assert!(workspace.check_path(".hidden").is_err());
    assert!(workspace.check_path(".ssh/config").is_err());
    assert!(workspace.check_path("dir/.hidden/file").is_err());

    // Test with hidden files allowed
    let workspace = AutopilotWorkspace::new(temp.path())
        .unwrap()
        .with_hidden_files(true);
    assert!(workspace.check_path(".gitignore").is_ok());
    assert!(workspace.check_path(".github/workflows/test.yml").is_ok());
}

#[test]
fn test_extension_whitelist() {
    let temp = TempDir::new().unwrap();
    let workspace = AutopilotWorkspace::new(temp.path())
        .unwrap()
        .with_extensions(vec!["rs", "toml", "md", "txt"]);

    // Allowed extensions
    assert!(workspace.check_path("main.rs").is_ok());
    assert!(workspace.check_path("Cargo.toml").is_ok());
    assert!(workspace.check_path("README.md").is_ok());
    assert!(workspace.check_path("notes.txt").is_ok());

    // Blocked extensions
    assert!(workspace.check_path("script.sh").is_err());
    assert!(workspace.check_path("binary.exe").is_err());
    assert!(workspace.check_path("config.yaml").is_err());

    // No extension
    assert!(workspace.check_path("Makefile").is_err());
}

#[test]
fn test_control_characters_blocked() {
    let temp = TempDir::new().unwrap();
    let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

    // Various control characters
    assert!(workspace.check_path("file\x00.txt").is_err()); // Null
    assert!(workspace.check_path("file\x1b.txt").is_err()); // Escape
    assert!(workspace.check_path("file\x7f.txt").is_err()); // Delete
    assert!(workspace.check_path("file\x08.txt").is_err()); // Backspace

    // Tab, newline, carriage return should be allowed (common in some systems)
    // But most implementations would still block these for safety
}

#[test]
fn test_windows_specific_patterns() {
    let temp = TempDir::new().unwrap();
    let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

    // Windows-specific dangerous patterns
    assert!(workspace.check_path("\\\\?\\C:\\file").is_err());
    assert!(workspace.check_path("\\\\.\\COM1").is_err());
    assert!(workspace.check_path("file.txt::$DATA").is_err());
    assert!(workspace.check_path("file:Zone.Identifier").is_err());
}

#[test]
fn test_nested_safe_paths() {
    let temp = TempDir::new().unwrap();
    let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

    // Create nested directory structure
    let nested_dir = temp.path().join("a").join("b").join("c");
    fs::create_dir_all(&nested_dir).unwrap();

    let nested_file = nested_dir.join("file.txt");
    fs::write(&nested_file, "test").unwrap();

    // Should allow deeply nested but safe paths
    assert!(workspace.check_path("a/b/c/file.txt").is_ok());
    assert!(workspace.check_path(&nested_file).is_ok());
}

#[test]
fn test_would_allow_non_destructive() {
    let temp = TempDir::new().unwrap();
    let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

    // Test non-destructive checking
    assert!(workspace.would_allow("safe.txt"));
    assert!(!workspace.would_allow("../etc/passwd"));
    assert!(!workspace.would_allow("/etc/passwd"));
    assert!(!workspace.would_allow(".hidden"));

    // With hidden files allowed
    let workspace = AutopilotWorkspace::new(temp.path())
        .unwrap()
        .with_hidden_files(true);
    assert!(workspace.would_allow(".gitignore"));
}

#[test]
fn test_canonicalization_of_nonexistent_files() {
    let temp = TempDir::new().unwrap();
    let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

    // Should handle files that don't exist yet
    let new_file = "new_file_that_doesnt_exist.txt";
    let result = workspace.check_path(new_file);
    assert!(result.is_ok());

    let canonical = result.unwrap();
    // Use canonicalized workspace root for comparison
    assert!(canonical.starts_with(workspace.workspace_root()));
    assert!(canonical.ends_with("new_file_that_doesnt_exist.txt"));
}

#[test]
fn test_unicode_traversal_attempts() {
    let temp = TempDir::new().unwrap();
    let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

    // Unicode characters that might be interpreted as dots
    assert!(workspace.check_path("foo/\u{2024}/bar").is_err());
    assert!(workspace.check_path("foo/\u{FF0E}\u{FF0E}/bar").is_err());
}

#[test]
fn test_complex_attack_scenario() {
    let temp = TempDir::new().unwrap();
    let workspace = AutopilotWorkspace::new(temp.path())
        .unwrap()
        .with_extensions(vec!["txt", "md"]);

    // Complex attack combining multiple techniques
    let attacks = vec![
        "../../../etc/passwd",                  // Direct traversal
        "valid/../../../etc/passwd",            // Traversal after valid component
        "file.txt/../../../etc/passwd",         // Traversal after file
        "%2e%2e%2f%2e%2e%2fetc%2fpasswd",       // URL encoded
        "....//....//etc/passwd",               // Double dots
        "foo/bar/../../../../../../etc/passwd", // Deep traversal
    ];

    for attack in attacks {
        assert!(
            workspace.check_path(attack).is_err(),
            "Attack should be blocked: {}",
            attack
        );
    }

    // Valid paths should still work
    assert!(workspace.check_path("valid/path/file.txt").is_ok());
    assert!(workspace.check_path("README.md").is_ok());
}
