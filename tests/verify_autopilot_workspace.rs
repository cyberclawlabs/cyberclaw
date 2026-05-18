#!/usr/bin/env rust-script

//! Verification script for AutopilotWorkspace security module
//!
//! ```cargo
//! [dependencies]
//! anyhow = "1"
//! tempfile = "3"
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use anyhow::{Result, bail};

// Simplified version of AutopilotWorkspace for verification
struct SimpleWorkspaceValidator {
    workspace_root: PathBuf,
}

impl SimpleWorkspaceValidator {
    fn new(root: &Path) -> Result<Self> {
        let workspace_root = root.canonicalize()?;
        Ok(Self { workspace_root })
    }

    fn check_path(&self, path: &Path) -> Result<PathBuf> {
        // Check for null bytes
        let path_str = path.to_string_lossy();
        if path_str.contains('\0') {
            bail!("Null byte detected");
        }

        // Check for traversal
        for component in path.components() {
            if let std::path::Component::ParentDir = component {
                bail!("Path traversal detected");
            }
        }

        // Make absolute
        let absolute = if path.is_relative() {
            self.workspace_root.join(path)
        } else {
            path.to_path_buf()
        };

        // Canonicalize
        let canonical = absolute.canonicalize()
            .or_else(|_| {
                if let Some(parent) = absolute.parent() {
                    if let Some(filename) = absolute.file_name() {
                        return parent.canonicalize().map(|p| p.join(filename));
                    }
                }
                bail!("Cannot canonicalize path")
            })?;

        // Check boundary
        if !canonical.starts_with(&self.workspace_root) {
            bail!("Path escapes workspace");
        }

        Ok(canonical)
    }
}

fn main() -> Result<()> {
    println!("=== AutopilotWorkspace Security Verification ===\n");

    let temp = TempDir::new()?;
    let validator = SimpleWorkspaceValidator::new(temp.path())?;

    println!("Workspace root: {:?}\n", temp.path());

    // Test cases
    let test_cases = vec![
        ("safe.txt", true, "Safe relative path"),
        ("dir/file.txt", true, "Safe nested path"),
        ("../etc/passwd", false, "Path traversal blocked"),
        ("../../etc/shadow", false, "Multiple traversal blocked"),
        ("/etc/passwd", false, "System path blocked"),
        ("file\0.txt", false, "Null byte blocked"),
    ];

    let mut passed = 0;
    let mut failed = 0;

    for (path, should_pass, description) in test_cases {
        print!("Testing: {} - {} ... ", path, description);
        let result = validator.check_path(Path::new(path));

        if result.is_ok() == should_pass {
            println!("✓ PASS");
            passed += 1;
        } else {
            println!("✗ FAIL");
            failed += 1;
            if should_pass {
                println!("  Expected success but got: {:?}", result.err());
            } else {
                println!("  Expected failure but got: {:?}", result.ok());
            }
        }
    }

    println!("\n=== Summary ===");
    println!("Passed: {}", passed);
    println!("Failed: {}", failed);
    println!("Total:  {}", passed + failed);

    if failed == 0 {
        println!("\n✅ All security checks passed!");
        Ok(())
    } else {
        bail!("Security validation failed")
    }
}