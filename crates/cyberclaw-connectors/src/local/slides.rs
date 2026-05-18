//! # Slides rendering capability — `slides.render`
//!
//! Produces a real PowerPoint `.pptx` file from a Markdown source. Closes
//! the GA-01 gap from the 2026-05-05 grade-C eval, where the agent could
//! generate a Markdown deck source but had no production path to convert
//! it into an openable `.pptx`.
//!
//! ## Backend
//!
//! Shells out to `python3` with an inline `python-pptx` script. The
//! script reads the Markdown body, splits on `---` slide separators,
//! treats the first non-empty line of each slide as the slide title and
//! the remainder as bullet points (one per line, leading `- ` / `* `
//! markers stripped). The output is written via the standard
//! `Presentation().save(path)` call.
//!
//! ## Risk classification
//!
//! `Medium` risk because:
//!   * It writes a binary file to the workspace (not destructive but
//!     not idempotent — overwrites existing files at the same path).
//!   * It launches a Python subprocess, which under containerised
//!     execution is bounded by the runtime config but on host execution
//!     has whatever sandbox the operator has provisioned.
//!   * The MIME-typed output (`application/vnd.openxmlformats-officedocument.presentationml.presentation`)
//!     is consumed by external tooling (Keynote / PowerPoint /
//!     LibreOffice), so corruption could disrupt downstream business
//!     workflows.
//!
//! Audit + governance both sit on the standard `CapabilityDispatcher`
//! path; this module owns only the input parsing and python-pptx
//! invocation.
//!
//! ## Graceful degradation
//!
//! If `python3` or `python-pptx` is missing, the call returns
//! `Err("python3 not available" / "python-pptx not installed")` so the
//! caller (typically `chat_handler::persistent_chat_dispatch` running a
//! `PersistentLoop` with a `slides.render` story) can fall back to a
//! Markdown-only deliverable and surface the gap to the operator. The
//! capability does NOT silently write a `.pptx` shell that opens but
//! shows nothing — that would be the worst failure mode for business
//! evaluation.

use crate::types::CapabilityExecutionRequest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

/// Input: Markdown source + output path. `title` overrides the deck title
/// shown on slide 1; when omitted the title is taken from the first `# H1`
/// in the Markdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlidesRenderInput {
    pub markdown: String,
    pub output_path: String,
    #[serde(default)]
    pub title: Option<String>,
}

/// Output: filesystem path of the produced `.pptx`, the slide count
/// (computed by the python-pptx invocation), and the resolved MIME type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlidesRenderOutput {
    pub path: String,
    pub slide_count: usize,
    pub format: String,
}

/// `slides.render` dispatch entrypoint.
pub async fn render(
    _caller: &super::LocalConnector,
    req: CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: SlidesRenderInput = serde_json::from_value(req.input)
        .map_err(|e| anyhow::anyhow!("invalid SlidesRenderInput: {}", e))?;

    if input.markdown.trim().is_empty() {
        anyhow::bail!("markdown body is empty — nothing to render");
    }
    if input.output_path.trim().is_empty() {
        anyhow::bail!("output_path is required");
    }
    if !input.output_path.ends_with(".pptx") {
        anyhow::bail!(
            "output_path must end with .pptx (got '{}')",
            input.output_path
        );
    }

    // Inline python script. Reads markdown from stdin, writes pptx to the
    // path passed via argv. Slide separator is the standard Marp `---`.
    const SCRIPT: &str = r#"
import sys
try:
    from pptx import Presentation
    from pptx.util import Inches
except ImportError:
    sys.stderr.write("python-pptx not installed; install via: pip install python-pptx\n")
    sys.exit(2)

import re

out_path = sys.argv[1]
title_override = sys.argv[2] if len(sys.argv) > 2 else ""
md = sys.stdin.read()

# Split on standalone --- separators (Marp convention).
slides = [s.strip() for s in re.split(r'(?m)^---\s*$', md) if s.strip()]
if not slides:
    sys.stderr.write("no non-empty slides found in markdown\n")
    sys.exit(3)

prs = Presentation()
title_layout = prs.slide_layouts[0]
content_layout = prs.slide_layouts[1]

# First slide = title slide. Use title_override or H1 of the first slide.
first = slides[0].strip().splitlines()
deck_title = title_override
if not deck_title:
    for line in first:
        line = line.strip()
        if line.startswith('#'):
            deck_title = line.lstrip('#').strip()
            break
    if not deck_title and first:
        deck_title = first[0].strip().lstrip('#').strip()
deck_title = deck_title or "Untitled"

slide = prs.slides.add_slide(title_layout)
slide.shapes.title.text = deck_title
if len(slide.placeholders) > 1:
    slide.placeholders[1].text = "Generated by CyberClaw slides.render"

# Subsequent slides.
for body in slides[1:] if len(slides) > 1 else slides:
    slide = prs.slides.add_slide(content_layout)
    lines = [ln for ln in body.splitlines() if ln.strip()]
    title_text = lines[0].lstrip('#').strip() if lines else "Slide"
    slide.shapes.title.text = title_text
    bullets = []
    for ln in lines[1:]:
        s = ln.strip()
        if s.startswith('- '):
            bullets.append(s[2:].strip())
        elif s.startswith('* '):
            bullets.append(s[2:].strip())
        elif s and not s.startswith('#'):
            bullets.append(s)
    if bullets and len(slide.placeholders) > 1:
        tf = slide.placeholders[1].text_frame
        tf.text = bullets[0]
        for b in bullets[1:]:
            p = tf.add_paragraph()
            p.text = b

prs.save(out_path)
print(len(prs.slides))
"#;

    let mut cmd = tokio::process::Command::new("python3");
    cmd.arg("-c")
        .arg(SCRIPT)
        .arg(&input.output_path)
        .arg(input.title.clone().unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            anyhow::bail!(
                "python3 not available — install python3 to enable slides.render: {}",
                e
            );
        }
    };

    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(input.markdown.as_bytes()).await?;
        stdin.shutdown().await?;
    }

    let output = child.wait_with_output().await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        anyhow::bail!(
            "python3 slides.render exited with {}: stderr={}",
            code,
            stderr.trim()
        );
    }

    let slide_count: usize = stdout.trim().parse().unwrap_or(0);
    let result = SlidesRenderOutput {
        path: input.output_path.clone(),
        slide_count,
        format: "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            .to_string(),
    };
    Ok(serde_json::to_value(result)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_round_trips() {
        let input = SlidesRenderInput {
            markdown: "# Title\n\nbody".to_string(),
            output_path: "/tmp/test.pptx".to_string(),
            title: Some("Override".to_string()),
        };
        let v = serde_json::to_value(&input).unwrap();
        let back: SlidesRenderInput = serde_json::from_value(v).unwrap();
        assert_eq!(back.markdown, input.markdown);
        assert_eq!(back.output_path, input.output_path);
        assert_eq!(back.title.as_deref(), Some("Override"));
    }

    #[test]
    fn output_carries_correct_mime() {
        let out = SlidesRenderOutput {
            path: "/tmp/x.pptx".to_string(),
            slide_count: 5,
            format: "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                .to_string(),
        };
        assert!(
            out.format.contains("presentationml.presentation"),
            "MIME must identify pptx unambiguously"
        );
    }
}
