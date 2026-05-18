//! Tool visibility filter helpers.

/// 判断 tool 名称是否命中 pattern。
///
/// 支持：
/// - `*`：匹配全部
/// - `prefix*`：前缀匹配
/// - `exact`：精确匹配
pub(crate) fn matches_tool_pattern(tool_name: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return tool_name.starts_with(prefix);
    }
    tool_name == pattern
}

/// 根据 allow/deny 规则判断 tool 是否可见。
///
/// 规则：
/// - allow 为空：默认全部允许
/// - allow 非空：必须命中 allow 中任一 pattern
/// - deny 命中优先级高于 allow（命中即拒绝）
pub(crate) fn is_tool_enabled(
    tool_name: &str,
    allow_patterns: &[String],
    deny_patterns: &[String],
) -> bool {
    let allow_hit = if allow_patterns.is_empty() {
        true
    } else {
        allow_patterns
            .iter()
            .any(|pattern| matches_tool_pattern(tool_name, pattern))
    };

    let deny_hit = deny_patterns
        .iter()
        .any(|pattern| matches_tool_pattern(tool_name, pattern));

    allow_hit && !deny_hit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_tool_pattern() {
        assert!(matches_tool_pattern("Read", "*"));
        assert!(matches_tool_pattern("WebSearch", "Web*"));
        assert!(matches_tool_pattern("Read", "Read"));
        assert!(!matches_tool_pattern("Write", "Read"));
        assert!(!matches_tool_pattern("Read", ""));
    }

    #[test]
    fn test_is_tool_enabled() {
        // allow 为空时默认允许
        assert!(is_tool_enabled("Read", &[], &[]));

        // allow 非空，需命中 allow
        assert!(is_tool_enabled("Read", &[String::from("R*")], &[]));
        assert!(!is_tool_enabled("Write", &[String::from("R*")], &[]));

        // deny 优先于 allow
        assert!(!is_tool_enabled(
            "Read",
            &[String::from("Read")],
            &[String::from("Read")]
        ));
    }
}
