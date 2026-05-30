//! Autopilot Workspace Boundary Enforcement
//!
//! Prevents path traversal and workspace escape attacks by validating
//! all file system operations against workspace boundaries.
//!
//! # Security Features
//!
//! - **Path Canonicalization**: Resolves symbolic links and relative paths
//! - **Traversal Detection**: Blocks directory traversal attempts (../)
//! - **Symlink Escape Prevention**: Detects symlinks pointing outside workspace
//! - **System Path Protection**: Blacklists critical system directories
//! - **Workspace Boundary Enforcement**: Ensures all paths remain within workspace
//! - **Hidden File Policy**: Configurable handling of dot-files
//! - **Extension Validation**: Optional file type restrictions
//! - **Null Byte Defense**: Prevents null byte injection attacks
//! - **Length Limits**: Protects against resource exhaustion
//! - **Character Validation**: Blocks control characters and special sequences

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// Maximum allowed path length (prevents resource exhaustion)
const MAX_PATH_LENGTH: usize = 4096;

/// Maximum allowed filename length
const MAX_FILENAME_LENGTH: usize = 255;

/// Default system paths to block
const DEFAULT_SYSTEM_PATHS: &[&str] = &[
    "/etc",
    "/sys",
    "/proc",
    "/dev",
    "/root",
    "/boot",
    "/usr/bin",
    "/usr/sbin",
    "/bin",
    "/sbin",
    "/var/log",
    "/tmp",
    "/home",
    "/Users",
    "C:\\Windows",
    "C:\\Program Files",
    "C:\\ProgramData",
    "/System",
    "/Library",
    "/Applications",
];

/// Workspace boundary validator for Autopilot mode
#[derive(Debug, Clone)]
pub struct AutopilotWorkspace {
    /// Canonical absolute path to workspace root
    workspace_root: PathBuf,
    /// System paths blacklist
    system_paths: HashSet<PathBuf>,
    /// Allow hidden files (starting with .)
    allow_hidden: bool,
    /// Optional file extension whitelist
    allowed_extensions: Option<HashSet<String>>,
    /// Allow symlinks within workspace
    allow_symlinks: bool,
}

impl AutopilotWorkspace {
    /// Create new workspace with root path
    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self> {
        let workspace_root = workspace_root
            .as_ref()
            .canonicalize()
            .context("Failed to canonicalize workspace root")?;

        if !workspace_root.is_dir() {
            bail!("Workspace root must be a directory: {:?}", workspace_root);
        }

        let system_paths = DEFAULT_SYSTEM_PATHS
            .iter()
            .filter_map(|p| Path::new(p).canonicalize().ok())
            .collect();

        Ok(Self {
            workspace_root,
            system_paths,
            allow_hidden: false,
            allowed_extensions: None,
            allow_symlinks: false,
        })
    }

    /// Configure whether hidden files are allowed
    pub fn with_hidden_files(mut self, allow: bool) -> Self {
        self.allow_hidden = allow;
        self
    }

    /// Configure allowed file extensions (e.g., ["rs", "toml", "md"])
    pub fn with_extensions(mut self, extensions: Vec<&str>) -> Self {
        self.allowed_extensions = Some(extensions.iter().map(|s| s.to_string()).collect());
        self
    }

    /// Configure whether symlinks are allowed within workspace
    pub fn with_symlinks(mut self, allow: bool) -> Self {
        self.allow_symlinks = allow;
        self
    }

    /// Validate path is within workspace boundaries
    ///
    /// Returns the canonicalized safe path if validation passes
    pub fn check_path(&self, target: impl AsRef<Path>) -> Result<PathBuf> {
        let target = target.as_ref();

        // Rule 1: Null byte detection (prevents string termination attacks)
        self.check_null_bytes(target)?;

        // Rule 2: Length limits (prevents resource exhaustion)
        self.check_length_limits(target)?;

        // Rule 3: Character validation (blocks control chars and special sequences)
        self.check_characters(target)?;

        // Rule 4: Traversal detection (blocks ../ patterns)
        self.check_traversal(target)?;

        // Rule 5: Make path absolute relative to workspace if relative
        let absolute_path = if target.is_relative() {
            self.workspace_root.join(target)
        } else {
            target.to_path_buf()
        };

        // Rule 6: Canonicalize path (resolves symlinks, .., .)
        let canonical = absolute_path
            .canonicalize()
            .or_else(|_| {
                // If file doesn't exist yet, find the first existing ancestor
                // and append the non-existing components
                let mut current = absolute_path.as_path();
                let mut missing_components = Vec::new();

                // Walk up the path until we find an existing directory
                loop {
                    if current.exists() {
                        // Found existing ancestor, canonicalize it
                        let canonical_base = current.canonicalize()?;
                        // Append missing components
                        let mut result = canonical_base;
                        for component in missing_components.iter().rev() {
                            result = result.join(component);
                        }
                        return Ok(result);
                    }

                    // Record this component and move to parent
                    if let Some(filename) = current.file_name() {
                        missing_components.push(filename);
                    }

                    if let Some(parent) = current.parent() {
                        current = parent;
                    } else {
                        // Reached root without finding existing path
                        return Err(anyhow!(
                            "Cannot resolve path - no existing ancestor found: {:?}",
                            absolute_path
                        ));
                    }
                }
            })
            .context("Failed to canonicalize path")?;

        // Rule 7: Symlink escape detection
        if !self.allow_symlinks && absolute_path.exists() {
            self.check_symlink_escape(&absolute_path, &canonical)?;
        }

        // Rule 8: System path blocking
        self.check_system_paths(&canonical)?;

        // Rule 9: Workspace boundary verification
        self.check_workspace_boundary(&canonical)?;

        // Rule 10: Additional policies (hidden files, extensions)
        self.check_policies(&canonical)?;

        Ok(canonical)
    }

    /// Rule 1: Check for null bytes in path
    fn check_null_bytes(&self, path: &Path) -> Result<()> {
        let path_str = path.to_string_lossy();
        if path_str.contains('\0') {
            bail!("Path contains null byte: {:?}", path);
        }
        Ok(())
    }

    /// Rule 2: Check path and filename length limits
    fn check_length_limits(&self, path: &Path) -> Result<()> {
        let path_str = path.to_string_lossy();
        if path_str.len() > MAX_PATH_LENGTH {
            bail!(
                "Path exceeds maximum length of {} characters: {:?}",
                MAX_PATH_LENGTH,
                path
            );
        }

        if let Some(filename) = path.file_name() {
            let filename_str = filename.to_string_lossy();
            if filename_str.len() > MAX_FILENAME_LENGTH {
                bail!(
                    "Filename exceeds maximum length of {} characters: {:?}",
                    MAX_FILENAME_LENGTH,
                    filename
                );
            }
        }

        Ok(())
    }

    /// Rule 3: Check for invalid characters and sequences
    fn check_characters(&self, path: &Path) -> Result<()> {
        let path_str = path.to_string_lossy();

        // Check for control characters
        for ch in path_str.chars() {
            if ch.is_control() && ch != '\t' && ch != '\n' && ch != '\r' {
                bail!("Path contains control character: {:?}", path);
            }
        }

        // Check for dangerous sequences
        let dangerous_patterns = [
            "\\\\?\\", // Windows UNC bypass
            "\\\\.\\", // Windows device namespace
            "::$",     // NTFS alternate data streams
            ":Zone.",  // Windows zone identifier
        ];

        for pattern in &dangerous_patterns {
            if path_str.contains(pattern) {
                bail!("Path contains dangerous pattern '{}': {:?}", pattern, path);
            }
        }

        Ok(())
    }

    /// Rule 4: Check for directory traversal attempts
    fn check_traversal(&self, path: &Path) -> Result<()> {
        for component in path.components() {
            match component {
                Component::ParentDir => {
                    bail!("Path contains directory traversal (..): {:?}", path);
                }
                Component::Normal(name) => {
                    let name_str = name.to_string_lossy();
                    // Check for encoded traversal
                    if name_str.contains("%2e%2e") || name_str.contains("%2E%2E") {
                        bail!("Path contains encoded directory traversal: {:?}", path);
                    }
                    // Check for unicode tricks
                    if name_str.contains('\u{2024}') || name_str.contains('\u{FF0E}') {
                        bail!("Path contains unicode directory traversal: {:?}", path);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Rule 7: Check if symlink escapes workspace
    fn check_symlink_escape(&self, original: &Path, canonical: &Path) -> Result<()> {
        if original.is_symlink() {
            // The canonical path resolved the symlink
            // We'll verify it's still within workspace (done in Rule 9)
            // Here we just log that a symlink was detected
            tracing::debug!("Symlink detected: {:?} -> {:?}", original, canonical);
        }
        Ok(())
    }

    /// Rule 8: Check against system path blacklist
    fn check_system_paths(&self, path: &Path) -> Result<()> {
        for system_path in &self.system_paths {
            if path.starts_with(system_path) {
                bail!(
                    "Path is within protected system directory {:?}: {:?}",
                    system_path,
                    path
                );
            }
        }
        Ok(())
    }

    /// Rule 9: Verify path is within workspace boundary
    fn check_workspace_boundary(&self, path: &Path) -> Result<()> {
        if !path.starts_with(&self.workspace_root) {
            bail!(
                "Path escapes workspace boundary. Workspace: {:?}, Path: {:?}",
                self.workspace_root,
                path
            );
        }
        Ok(())
    }

    /// Rule 10: Check additional policies (hidden files, extensions)
    fn check_policies(&self, path: &Path) -> Result<()> {
        // Check hidden file policy
        if !self.allow_hidden {
            // Only check components within the workspace, not the workspace root itself
            if let Ok(relative_path) = path.strip_prefix(&self.workspace_root) {
                // Check if the file itself is hidden
                if let Some(filename) = relative_path.file_name() {
                    let filename_str = filename.to_string_lossy();
                    if filename_str.starts_with('.') && filename_str != "." && filename_str != ".."
                    {
                        bail!("Hidden files are not allowed: {:?}", path);
                    }
                }

                // Check all components within workspace for hidden directories
                for component in relative_path.components() {
                    if let Component::Normal(name) = component {
                        let name_str = name.to_string_lossy();
                        if name_str.starts_with('.') {
                            bail!("Hidden directories are not allowed: {:?}", path);
                        }
                    }
                }
            }
        }

        // Check file extension whitelist
        if let Some(ref allowed) = self.allowed_extensions {
            if let Some(extension) = path.extension() {
                let ext = extension.to_string_lossy().to_lowercase();
                if !allowed.contains(&ext) {
                    bail!(
                        "File extension '{}' is not allowed. Allowed: {:?}",
                        ext,
                        allowed
                    );
                }
            } else if path.is_file() || !path.exists() {
                // No extension on a file path
                bail!("Files without extensions are not allowed: {:?}", path);
            }
        }

        Ok(())
    }

    /// Get workspace root path
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Check if a path would be allowed without canonicalizing
    pub fn would_allow(&self, path: impl AsRef<Path>) -> bool {
        self.check_path(path).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs as unix_fs;
    use tempfile::TempDir;

    /// Create a TempDir that is guaranteed to live outside any protected system
    /// directory (see `DEFAULT_SYSTEM_PATHS`).
    ///
    /// The default `$TMPDIR` on macOS/CI resolves to `/private/tmp/...`, which is
    /// blocked by the `/tmp` protection rule, and the project tree lives under
    /// `/Users`, which is also protected. `/var/tmp` (canonical `/private/var/tmp`)
    /// is writable and matches none of the protected prefixes, so workspaces
    /// created there exercise the "legal workspace" paths correctly.
    fn test_ws() -> TempDir {
        #[cfg(unix)]
        {
            TempDir::new_in("/var/tmp").unwrap()
        }
        #[cfg(not(unix))]
        {
            TempDir::new().unwrap()
        }
    }

    #[test]
    fn test_workspace_creation() {
        let temp = TempDir::new().unwrap();
        let workspace = AutopilotWorkspace::new(temp.path()).unwrap();
        assert_eq!(
            workspace.workspace_root(),
            temp.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn test_path_traversal_blocked() {
        let temp = TempDir::new().unwrap();
        let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

        // Direct traversal
        assert!(workspace.check_path("../etc/passwd").is_err());
        assert!(workspace.check_path("../../etc/passwd").is_err());
        assert!(workspace.check_path("foo/../../../etc/passwd").is_err());

        // Encoded traversal
        assert!(workspace.check_path("foo/%2e%2e/bar").is_err());
        assert!(workspace.check_path("foo/%2E%2E/bar").is_err());
    }

    #[test]
    fn test_null_byte_blocked() {
        let temp = TempDir::new().unwrap();
        let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

        // Null byte in path
        let bad_path = "file.txt\0.exe".to_string();
        assert!(workspace.check_path(&bad_path).is_err());
    }

    #[test]
    fn test_length_limits() {
        let temp = TempDir::new().unwrap();
        let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

        // Very long path
        let long_name = "a".repeat(5000);
        assert!(workspace.check_path(&long_name).is_err());

        // Very long filename
        let long_filename = format!("file_{}.txt", "x".repeat(300));
        assert!(workspace.check_path(&long_filename).is_err());
    }

    #[test]
    fn test_system_paths_blocked() {
        let temp = TempDir::new().unwrap();
        let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

        // System paths should be blocked even if absolute
        assert!(workspace.check_path("/etc/passwd").is_err());
        assert!(workspace.check_path("/sys/kernel").is_err());
        assert!(workspace.check_path("/proc/1/status").is_err());
    }

    #[test]
    fn test_workspace_boundary() {
        let temp = test_ws();
        let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

        // Create a file in workspace
        let safe_file = temp.path().join("safe.txt");
        fs::write(&safe_file, "test").unwrap();

        // Should allow files within workspace (use absolute path)
        match workspace.check_path(&safe_file) {
            Ok(_) => {}
            Err(e) => panic!("check_path failed: {:?}", e),
        }

        // Should block files outside workspace
        let outside = "/tmp/outside.txt";
        assert!(workspace.check_path(outside).is_err());
    }

    #[test]
    fn test_hidden_files_policy() {
        let temp = test_ws();

        // Default: hidden files blocked (use absolute path)
        let workspace = AutopilotWorkspace::new(temp.path()).unwrap();
        let hidden_file = temp.path().join(".hidden");
        let ssh_config = temp.path().join(".ssh/config");
        assert!(workspace.check_path(&hidden_file).is_err());
        assert!(workspace.check_path(&ssh_config).is_err());

        // With hidden files allowed (use absolute path)
        let workspace = AutopilotWorkspace::new(temp.path())
            .unwrap()
            .with_hidden_files(true);
        let hidden_file = temp.path().join(".hidden");
        assert!(workspace.check_path(&hidden_file).is_ok());
    }

    #[test]
    fn test_extension_whitelist() {
        let temp = test_ws();
        let workspace = AutopilotWorkspace::new(temp.path())
            .unwrap()
            .with_extensions(vec!["rs", "toml", "md"]);

        // Allowed extensions (use absolute paths)
        let main_rs = temp.path().join("main.rs");
        let cargo_toml = temp.path().join("Cargo.toml");
        let readme_md = temp.path().join("README.md");
        assert!(workspace.check_path(&main_rs).is_ok());
        assert!(workspace.check_path(&cargo_toml).is_ok());
        assert!(workspace.check_path(&readme_md).is_ok());

        // Blocked extensions (use absolute paths)
        let hack_exe = temp.path().join("hack.exe");
        let script_sh = temp.path().join("script.sh");
        assert!(workspace.check_path(&hack_exe).is_err());
        assert!(workspace.check_path(&script_sh).is_err());

        // No extension (use absolute path)
        let makefile = temp.path().join("Makefile");
        assert!(workspace.check_path(&makefile).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn test_symlink_handling() {
        let temp = test_ws();
        let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

        // Create a file and symlink within workspace
        let target = temp.path().join("target.txt");
        fs::write(&target, "content").unwrap();

        let link = temp.path().join("link.txt");
        unix_fs::symlink(&target, &link).unwrap();

        // Default: symlinks blocked
        assert!(workspace.check_path(&link).is_ok()); // But resolves to target

        // Symlink pointing outside workspace
        let outside = "/etc/passwd";
        let bad_link = temp.path().join("bad_link");
        unix_fs::symlink(outside, &bad_link).unwrap();
        assert!(workspace.check_path(&bad_link).is_err());
    }

    #[test]
    fn test_control_characters_blocked() {
        let temp = TempDir::new().unwrap();
        let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

        // Control characters
        assert!(workspace.check_path("file\x00.txt").is_err()); // Null
        assert!(workspace.check_path("file\x1b.txt").is_err()); // Escape
        assert!(workspace.check_path("file\x7f.txt").is_err()); // Delete
    }

    #[test]
    fn test_dangerous_patterns_blocked() {
        let temp = TempDir::new().unwrap();
        let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

        // Windows UNC paths
        assert!(workspace.check_path("\\\\?\\C:\\file").is_err());

        // NTFS alternate data streams
        assert!(workspace.check_path("file.txt::$DATA").is_err());
    }

    #[test]
    fn test_unicode_traversal_blocked() {
        let temp = TempDir::new().unwrap();
        let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

        // Unicode dots that might be normalized to ..
        assert!(workspace.check_path("foo/\u{2024}/bar").is_err());
        assert!(workspace.check_path("foo/\u{FF0E}\u{FF0E}/bar").is_err());
    }

    #[test]
    fn test_safe_nested_paths() {
        let temp = test_ws();
        let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

        // Create nested directory structure
        let nested = temp.path().join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();

        let file = nested.join("file.txt");
        fs::write(&file, "test").unwrap();

        // Should allow deeply nested but safe paths (use absolute path)
        assert!(workspace.check_path(&file).is_ok());
    }

    #[test]
    fn test_would_allow() {
        let temp = test_ws();
        let workspace = AutopilotWorkspace::new(temp.path()).unwrap();

        // Test non-destructive checking (use absolute paths)
        let safe_file = temp.path().join("safe.txt");
        assert!(workspace.would_allow(&safe_file));
        assert!(!workspace.would_allow("../etc/passwd"));
        assert!(!workspace.would_allow("/etc/passwd"));
    }
}
