//! 从 frontmatter / metadata 推断 skill 的 source ecosystem。
//!
//! 推断顺序（按优先级，命中即返回）：
//!
//! 1. `metadata.hermes` 字段存在 → `Hermes`
//! 2. `metadata.anthropic` 字段存在 → `Anthropic`
//! 3. `metadata.openclaw` 字段存在 → `OpenClaw`
//! 4. `metadata.superpowers` 字段存在 → `Superpowers`
//! 5. `metadata.cyberclaw.source` 路径前缀
//!    - `hermes-agent/` 或 `hermes-agent-self-evolution/` → `Hermes`
//!    - 含 `anthropic` 或 `claude-skill` → `Anthropic`
//!    - 含 `openclaw` → `OpenClaw`
//!    - 含 `superpowers` → `Superpowers`
//!    - 含 `cyberclaw` → `Native`
//! 6. frontmatter `name` 在 cyberclaw 自家 21 个 skill 内 → `Native`
//! 7. fallback → `Unknown`

use serde_json::Value;

use super::SourceEcosystem;

/// cyberclaw 自家 21 个 skill 的 canonical 名（用于在没有显式 source 标签时识别 Native）。
const NATIVE_SKILL_NAMES: &[&str] = &[
    "calculator",
    "echo",
    "design-md",
    "code-reviewer",
    "data-analysis",
    "feature-implementation",
    "git-workflow",
    "test-writing",
    "performance-tuning",
    "refactoring",
    "documentation-writer",
    "security-audit",
    "api-design",
    "schema-design",
    "infrastructure-as-code",
    "ci-cd",
    "monitoring",
    "incident-response",
    "release-management",
    "knowledge-base",
    "onboarding",
];

/// 从 frontmatter 的 JSON 表示推断 source ecosystem。
///
/// `frontmatter` 应是 SKILL.md 顶部 YAML/TOML 解析后的 JSON Value。
/// 调用方传 `Value::Null` 时会退化到 `Unknown`。
pub fn detect_source_ecosystem(frontmatter: &Value) -> SourceEcosystem {
    // 1-4: metadata.<ecosystem> 字段直接命中
    if let Some(meta) = frontmatter.get("metadata") {
        if meta.get("hermes").is_some() {
            return SourceEcosystem::Hermes;
        }
        if meta.get("anthropic").is_some() {
            return SourceEcosystem::Anthropic;
        }
        if meta.get("openclaw").is_some() {
            return SourceEcosystem::OpenClaw;
        }
        if meta.get("superpowers").is_some() {
            return SourceEcosystem::Superpowers;
        }
        // 5: cyberclaw.source 路径前缀
        if let Some(src) = meta
            .get("cyberclaw")
            .and_then(|c| c.get("source"))
            .and_then(|s| s.as_str())
        {
            if let Some(eco) = infer_from_source_path(src) {
                return eco;
            }
        }
    }

    // 6: name 在 NATIVE_SKILL_NAMES 列表
    if let Some(name) = frontmatter.get("name").and_then(|n| n.as_str()) {
        if NATIVE_SKILL_NAMES.contains(&name) {
            return SourceEcosystem::Native;
        }
    }

    SourceEcosystem::Unknown
}

/// 从 source 字符串前缀推断生态。返回 `None` 表示无规则匹配。
fn infer_from_source_path(src: &str) -> Option<SourceEcosystem> {
    let lower = src.to_ascii_lowercase();
    if lower.starts_with("hermes-agent/") || lower.starts_with("hermes-agent-self-evolution/") {
        Some(SourceEcosystem::Hermes)
    } else if lower.contains("claude-skill") || lower.contains("anthropic") {
        Some(SourceEcosystem::Anthropic)
    } else if lower.contains("openclaw") {
        Some(SourceEcosystem::OpenClaw)
    } else if lower.contains("superpowers") {
        Some(SourceEcosystem::Superpowers)
    } else if lower.contains("cyberclaw") {
        Some(SourceEcosystem::Native)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn source_detector_recognizes_hermes_via_metadata() {
        let fm = json!({
            "name": "jupyter-live-kernel",
            "metadata": {
                "hermes": { "tags": ["jupyter"] }
            }
        });
        assert_eq!(detect_source_ecosystem(&fm), SourceEcosystem::Hermes);
    }

    #[test]
    fn source_detector_recognizes_anthropic_via_metadata() {
        let fm = json!({
            "name": "pptx",
            "metadata": {
                "anthropic": { "version": "1.0.0" }
            }
        });
        assert_eq!(detect_source_ecosystem(&fm), SourceEcosystem::Anthropic);
    }

    #[test]
    fn source_detector_recognizes_openclaw_via_metadata() {
        let fm = json!({
            "name": "obsidian",
            "metadata": {
                "openclaw": { "emoji": "💎" }
            }
        });
        assert_eq!(detect_source_ecosystem(&fm), SourceEcosystem::OpenClaw);
    }

    #[test]
    fn source_detector_recognizes_superpowers_via_metadata() {
        let fm = json!({
            "metadata": {
                "superpowers": {}
            }
        });
        assert_eq!(detect_source_ecosystem(&fm), SourceEcosystem::Superpowers);
    }

    #[test]
    fn source_detector_falls_back_to_path_inference_hermes() {
        let fm = json!({
            "name": "x",
            "metadata": {
                "cyberclaw": {
                    "source": "hermes-agent/skills/data-science/jupyter-live-kernel"
                }
            }
        });
        assert_eq!(detect_source_ecosystem(&fm), SourceEcosystem::Hermes);
    }

    #[test]
    fn source_detector_falls_back_to_path_inference_anthropic() {
        let fm = json!({
            "metadata": {
                "cyberclaw": {
                    "source": "anthropics/claude-skill/pptx"
                }
            }
        });
        assert_eq!(detect_source_ecosystem(&fm), SourceEcosystem::Anthropic);
    }

    #[test]
    fn source_detector_recognizes_native_via_name() {
        let fm = json!({"name": "echo"});
        assert_eq!(detect_source_ecosystem(&fm), SourceEcosystem::Native);
    }

    #[test]
    fn source_detector_unknown_when_no_signals() {
        let fm = json!({"name": "some-random-third-party-skill"});
        assert_eq!(detect_source_ecosystem(&fm), SourceEcosystem::Unknown);
    }

    #[test]
    fn source_detector_metadata_takes_priority_over_path() {
        // metadata.hermes 应优先于 cyberclaw.source 中的 "openclaw"
        let fm = json!({
            "metadata": {
                "hermes": {},
                "cyberclaw": { "source": "openclaw/skills/foo" }
            }
        });
        assert_eq!(detect_source_ecosystem(&fm), SourceEcosystem::Hermes);
    }
}
