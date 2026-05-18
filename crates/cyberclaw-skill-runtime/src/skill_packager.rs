//! SkillPackager — create distributable `.skill` ZIP archives of a skill folder.
//!
//! Ported from `claude-skill/skills/skill-creator/scripts/package_skill.py`.
//!
//! # Exclusions (1:1 with the Python version)
//!
//! | Pattern | Scope |
//! |---|---|
//! | `__pycache__/` directory | anywhere in tree |
//! | `node_modules/` directory | anywhere in tree |
//! | `*.pyc` files | anywhere in tree |
//! | `.DS_Store` files | anywhere in tree |
//! | `evals/` directory | only at skill root |
//!
//! The archive layout matches the Python script: archive paths are rooted at
//! the skill folder name (e.g. `my-skill/SKILL.md`), not at the skill's parent.

use std::fs::File;
use std::path::{Path, PathBuf};

use thiserror::Error;
use walkdir::WalkDir;
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::CompressionMethod;

/// Directory names excluded anywhere in the tree.
pub const EXCLUDE_DIRS: &[&str] = &["__pycache__", "node_modules"];

/// Filename patterns excluded anywhere in the tree (literal matches only).
pub const EXCLUDE_FILES: &[&str] = &[".DS_Store"];

/// Glob-style suffix excludes.
pub const EXCLUDE_SUFFIXES: &[&str] = &[".pyc"];

/// Directories excluded **only at the skill root**.
pub const ROOT_EXCLUDE_DIRS: &[&str] = &["evals"];

/// Packaging errors.
#[derive(Debug, Error)]
pub enum PackagerError {
    /// Input skill directory does not exist.
    #[error("skill folder not found: {0}")]
    SkillDirMissing(String),
    /// Input path is not a directory.
    #[error("path is not a directory: {0}")]
    NotADirectory(String),
    /// SKILL.md is missing inside the skill directory.
    #[error("SKILL.md not found in {0}")]
    SkillMdMissing(String),
    /// I/O error during packaging.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Zip library error.
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// Directory walk error.
    #[error("walk error: {0}")]
    Walk(String),
}

/// Package a skill directory into a `.skill` ZIP archive.
///
/// * `skill_path` — the directory holding `SKILL.md`.
/// * `output_dir` — optional output directory. When `None`, the `.skill` file
///   is written to the current working directory.
///
/// Returns the absolute path of the created archive.
///
/// The produced archive's filename is `<skill_dir_name>.skill` (e.g.
/// `my-skill.skill`), matching `package_skill.py`.
pub fn package_skill(
    skill_path: &Path,
    output_dir: Option<&Path>,
) -> Result<PathBuf, PackagerError> {
    let skill_path = skill_path
        .canonicalize()
        .map_err(|_| PackagerError::SkillDirMissing(skill_path.to_string_lossy().to_string()))?;

    if !skill_path.is_dir() {
        return Err(PackagerError::NotADirectory(
            skill_path.to_string_lossy().to_string(),
        ));
    }

    let skill_md = skill_path.join("SKILL.md");
    if !skill_md.exists() {
        return Err(PackagerError::SkillMdMissing(
            skill_path.to_string_lossy().to_string(),
        ));
    }

    let skill_name = skill_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "skill".to_string());

    // Root from which we compute archive-relative paths. Python uses
    // `skill_path.parent`, so `arcname` includes the skill folder name.
    let arc_root = skill_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| skill_path.clone());

    let output_dir = match output_dir {
        Some(d) => {
            std::fs::create_dir_all(d)?;
            d.to_path_buf()
        }
        None => std::env::current_dir()?,
    };
    let archive_path = output_dir.join(format!("{skill_name}.skill"));

    let file = File::create(&archive_path)?;
    let mut writer = ZipWriter::new(file);
    let options: SimpleFileOptions = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);

    for entry in WalkDir::new(&skill_path).into_iter() {
        let entry = entry.map_err(|e| PackagerError::Walk(e.to_string()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let full_path = entry.path();
        let rel = match full_path.strip_prefix(&arc_root) {
            Ok(p) => p,
            Err(_) => continue,
        };

        if should_exclude(rel) {
            continue;
        }

        let arcname = rel.to_string_lossy().replace('\\', "/");
        writer.start_file(arcname, options)?;
        let bytes = std::fs::read(full_path)?;
        use std::io::Write;
        writer.write_all(&bytes)?;
    }

    writer.finish()?;
    Ok(archive_path)
}

/// Should the given **archive-relative** path (rooted at the parent of the
/// skill folder, so `parts[0]` is the skill folder name) be excluded?
///
/// Public so test code (and other CyberClaw tools that want to vet a skill
/// before shipping) can reuse the same rules without re-zipping.
pub fn should_exclude(rel_path: &Path) -> bool {
    let parts: Vec<String> = rel_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();

    // Anywhere-in-tree directory excludes.
    for part in &parts {
        if EXCLUDE_DIRS.iter().any(|d| d == part) {
            return true;
        }
    }

    // Root-only directory excludes: parts[0] is the skill folder name,
    // parts[1] (if present) is the first sub-directory at the skill root.
    if parts.len() > 1 && ROOT_EXCLUDE_DIRS.iter().any(|d| *d == parts[1]) {
        return true;
    }

    let name = rel_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    if EXCLUDE_FILES.iter().any(|f| *f == name) {
        return true;
    }

    if EXCLUDE_SUFFIXES
        .iter()
        .any(|s| name.to_lowercase().ends_with(s))
    {
        return true;
    }

    false
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn make_skill_dir(root: &Path) -> PathBuf {
        let skill = root.join("my-skill");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: my-skill\ndescription: d\n---\nbody\n",
        )
        .unwrap();
        skill
    }

    fn archive_entries(archive: &Path) -> Vec<String> {
        let f = File::open(archive).unwrap();
        let mut zip = zip::ZipArchive::new(f).unwrap();
        let mut names = Vec::new();
        for i in 0..zip.len() {
            let file = zip.by_index(i).unwrap();
            names.push(file.name().to_string());
        }
        names
    }

    #[test]
    fn package_produces_valid_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let skill = make_skill_dir(tmp.path());
        // add a reference file
        std::fs::create_dir_all(skill.join("references")).unwrap();
        std::fs::write(skill.join("references/notes.md"), "notes").unwrap();

        let out_dir = tmp.path().join("dist");
        let archive = package_skill(&skill, Some(&out_dir)).expect("package ok");
        assert!(archive.exists());
        assert_eq!(
            archive.file_name().unwrap().to_string_lossy(),
            "my-skill.skill"
        );

        let entries = archive_entries(&archive);
        assert!(entries.iter().any(|e| e == "my-skill/SKILL.md"));
        assert!(entries.iter().any(|e| e == "my-skill/references/notes.md"));

        // Check the SKILL.md bytes round-trip.
        let f = File::open(&archive).unwrap();
        let mut zip = zip::ZipArchive::new(f).unwrap();
        let mut skill_md = zip.by_name("my-skill/SKILL.md").unwrap();
        let mut body = String::new();
        skill_md.read_to_string(&mut body).unwrap();
        assert!(body.contains("name: my-skill"));
    }

    #[test]
    fn package_excludes_pycache() {
        let tmp = tempfile::tempdir().unwrap();
        let skill = make_skill_dir(tmp.path());
        std::fs::create_dir_all(skill.join("scripts/__pycache__")).unwrap();
        std::fs::write(skill.join("scripts/__pycache__/stale.pyc"), b"binary").unwrap();
        std::fs::write(skill.join("scripts/run.sh"), "echo").unwrap();

        let out_dir = tmp.path().join("dist");
        let archive = package_skill(&skill, Some(&out_dir)).expect("package ok");
        let entries = archive_entries(&archive);

        assert!(
            !entries.iter().any(|e| e.contains("__pycache__")),
            "entries should not include __pycache__: {entries:?}"
        );
        assert!(
            !entries.iter().any(|e| e.ends_with(".pyc")),
            "entries should not include .pyc: {entries:?}"
        );
        assert!(entries.iter().any(|e| e == "my-skill/scripts/run.sh"));
    }

    #[test]
    fn package_excludes_evals_at_root() {
        let tmp = tempfile::tempdir().unwrap();
        let skill = make_skill_dir(tmp.path());
        std::fs::create_dir_all(skill.join("evals")).unwrap();
        std::fs::write(skill.join("evals/evals.json"), "[]").unwrap();
        // Same-named dir deeper in the tree should NOT be excluded.
        std::fs::create_dir_all(skill.join("references/evals")).unwrap();
        std::fs::write(skill.join("references/evals/keep.md"), "keep").unwrap();

        let out_dir = tmp.path().join("dist");
        let archive = package_skill(&skill, Some(&out_dir)).expect("package ok");
        let entries = archive_entries(&archive);

        assert!(
            !entries.iter().any(|e| e.starts_with("my-skill/evals/")),
            "root-level evals/ should be excluded: {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|e| e == "my-skill/references/evals/keep.md"),
            "nested evals dir should be kept: {entries:?}"
        );
    }

    #[test]
    fn package_excludes_ds_store_and_node_modules() {
        let tmp = tempfile::tempdir().unwrap();
        let skill = make_skill_dir(tmp.path());
        std::fs::write(skill.join(".DS_Store"), "junk").unwrap();
        std::fs::create_dir_all(skill.join("node_modules/pkg")).unwrap();
        std::fs::write(skill.join("node_modules/pkg/index.js"), "x").unwrap();

        let out_dir = tmp.path().join("dist");
        let archive = package_skill(&skill, Some(&out_dir)).expect("package ok");
        let entries = archive_entries(&archive);

        assert!(
            !entries.iter().any(|e| e.contains(".DS_Store")),
            "entries should not include .DS_Store: {entries:?}"
        );
        assert!(
            !entries.iter().any(|e| e.contains("node_modules")),
            "entries should not include node_modules: {entries:?}"
        );
    }

    #[test]
    fn package_errors_when_skill_md_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let skill = tmp.path().join("broken");
        std::fs::create_dir_all(&skill).unwrap();
        let err = package_skill(&skill, None).unwrap_err();
        assert!(
            matches!(err, PackagerError::SkillMdMissing(_)),
            "got: {err:?}"
        );
    }

    #[test]
    fn should_exclude_rules_match_python() {
        // All paths are relative to the skill parent, so parts[0] is skill name.
        assert!(should_exclude(Path::new("s/.DS_Store")));
        assert!(should_exclude(Path::new("s/a/b/__pycache__/x.pyc")));
        assert!(should_exclude(Path::new("s/a/stale.pyc")));
        assert!(should_exclude(Path::new("s/node_modules/pkg/x.js")));
        assert!(should_exclude(Path::new("s/evals/evals.json")));
        assert!(!should_exclude(Path::new("s/references/evals/keep.md")));
        assert!(!should_exclude(Path::new("s/SKILL.md")));
        assert!(!should_exclude(Path::new("s/scripts/run.sh")));
    }
}
