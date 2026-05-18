//! Project-wide invariant: every user-facing chat handler that sends a
//! `Message::system(...)` to the LLM MUST either
//!
//! (a) build the system content from `cyberclaw_constitution_text(...)` /
//!     `cyberclaw_constitution_template(...)`, OR
//!
//! (b) be explicitly allowlisted via a `// CONSTITUTION-BYPASS-OK: <reason>`
//!     comment in the same function (e.g. read-only diagnostic modes that
//!     intentionally use narrow mode-specific prompts).
//!
//! Why this test exists:
//!
//! In 2026-05-17 a user reported the WebUI chat asks A/B/C clarification
//! questions for clear "帮我写一个 Rust 脚本" requests. Root-cause audit found
//! TWO bypass sites (`agents.rs::test_agent`, `workbench.rs::chat`) that
//! never inherited the constitution — meaning IRON LAWs (governance, anti-
//! fabrication, action-bias) were silently absent on those paths. After the
//! fix, this test exists so the **next** chat handler someone adds cannot
//! silently bypass governance — either it gets the constitution, or it
//! explicitly documents why it's an exception.
//!
//! Catches: forgotten constitution wiring, accidental override-without-prepend,
//! new chat surfaces that copy from older code without the IRON LAWs.

use std::fs;
use std::path::PathBuf;

/// All files under `src/api/` that send any `Message::system(...)` to the LLM.
/// Discovered via filesystem walk so newly-added chat surfaces are auto-included.
fn api_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/api")
}

fn collect_api_rs_files() -> Vec<PathBuf> {
    fn walk(dir: &PathBuf, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read api dir").flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(&api_dir(), &mut out);
    out
}

/// Find each line that has `Message::system(` outside of comments / strings /
/// tests. Returns `(file, line_number, line_text)`.
fn find_system_message_call_sites() -> Vec<(PathBuf, usize, String)> {
    let mut hits = Vec::new();
    for file in collect_api_rs_files() {
        let content = fs::read_to_string(&file).unwrap_or_default();
        let mut in_test_module = false;
        let mut test_brace_depth = 0i32;

        for (idx, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();

            // Track entry into / exit from `#[cfg(test)] mod tests { ... }`
            if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[cfg(any(test")
            {
                in_test_module = true;
                test_brace_depth = 0;
                continue;
            }
            if in_test_module {
                test_brace_depth += line.matches('{').count() as i32;
                test_brace_depth -= line.matches('}').count() as i32;
                if test_brace_depth <= 0 && line.contains('}') {
                    in_test_module = false;
                }
                continue;
            }

            // Skip pure comments
            if trimmed.starts_with("//") || trimmed.starts_with("///")
                || trimmed.starts_with("//!")
            {
                continue;
            }
            if !line.contains("Message::system(") {
                continue;
            }
            hits.push((file.clone(), idx + 1, line.to_string()));
        }
    }
    hits
}

/// Read the surrounding function body for a hit and check whether it satisfies
/// either branch of the invariant. We look in a generous window (80 lines back,
/// 5 lines forward) to cover prompt assembly that happens earlier in the
/// function.
fn function_window_satisfies_invariant(file: &PathBuf, line_no: usize) -> Result<(), String> {
    let content = fs::read_to_string(file).map_err(|e| format!("read {file:?}: {e}"))?;
    let lines: Vec<&str> = content.lines().collect();
    let start = line_no.saturating_sub(80);
    let end = (line_no + 5).min(lines.len());
    let window: String = lines[start..end].join("\n");

    let uses_constitution = window.contains("cyberclaw_constitution_text(")
        || window.contains("cyberclaw_constitution_template(");
    let explicitly_allowed = window.contains("CONSTITUTION-BYPASS-OK");

    if uses_constitution || explicitly_allowed {
        Ok(())
    } else {
        Err(format!(
            "{}:{} — Message::system() call site has no nearby \
             `cyberclaw_constitution_text(...)` reference and no \
             `// CONSTITUTION-BYPASS-OK: <reason>` allowlist comment. \
             Either prefix the constitution OR add an explicit bypass-OK \
             comment with a reason (see workbench.rs for example).",
            file.display(),
            line_no
        ))
    }
}

#[test]
fn every_user_facing_chat_handler_includes_constitution_or_is_explicitly_allowlisted() {
    let hits = find_system_message_call_sites();
    assert!(
        !hits.is_empty(),
        "expected at least one Message::system() call site in src/api/ — \
         test may be looking at the wrong directory or the project layout changed"
    );

    let mut failures = Vec::new();
    for (file, line_no, line_text) in &hits {
        if let Err(msg) = function_window_satisfies_invariant(file, *line_no) {
            failures.push(format!("  • {msg}\n    line: {}", line_text.trim()));
        }
    }

    assert!(
        failures.is_empty(),
        "\n\nCONSTITUTION COVERAGE INVARIANT VIOLATED\n\n\
         {} chat handler(s) build system prompts without the CyberClaw \
         constitution and without an explicit `// CONSTITUTION-BYPASS-OK: \
         <reason>` allowlist comment.\n\nThis means IRON LAWs (governance, \
         anti-fabrication, action-bias) are silently absent on those paths. \
         Either prefix the constitution via `cyberclaw_constitution_text(...)`, \
         or add an explicit bypass comment with a reason.\n\nViolations:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn known_user_facing_chat_handlers_use_constitution() {
    // Positive control — these specific files MUST contain the constitution
    // reference. Catches the case where someone accidentally replaces the
    // constitution call with something else.
    for (file, label) in &[
        ("chat.rs", "OpenAI-compat /v1/chat/completions"),
        ("chat_handler.rs", "/v1/agent/chat/completions"),
        ("chat_conversations.rs", "WebUI /api/v1/chat/message"),
        ("agents.rs", "/api/agents/{name}/test + preview-prompt"),
    ] {
        let path = api_dir().join(file);
        let content = fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("failed to read {}", path.display()));
        assert!(
            content.contains("cyberclaw_constitution_text(")
                || content.contains("cyberclaw_constitution_template("),
            "{file} ({label}) lost its `cyberclaw_constitution_text(...)` reference — \
             user-facing chat path is now ungoverned"
        );
    }
}
