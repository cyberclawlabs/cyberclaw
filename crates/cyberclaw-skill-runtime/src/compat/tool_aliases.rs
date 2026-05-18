//! 共享 alias 工具：通过 `&[(&str, &str)]` 提供 alias → facade 静态映射并查询。
//!
//! 各 translator 复用 [`lookup`] 实现，避免重复的 match 分支。

/// 在静态 alias 表中查找 `llm_name` 对应的 cyberclaw facade。
pub fn lookup(table: &[(&str, &str)], llm_name: &str) -> Option<String> {
    table
        .iter()
        .find(|(alias, _)| *alias == llm_name)
        .map(|(_, facade)| (*facade).to_string())
}

/// 抽取所有 alias 名称（按表序）。表必须是 'static slice (translator 模块的 const 表)。
pub fn aliases_only(table: &'static [(&'static str, &'static str)]) -> Vec<&'static str> {
    table.iter().map(|(a, _)| *a).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_hits() {
        let t = &[("foo", "ns.foo"), ("bar", "ns.bar")];
        assert_eq!(lookup(t, "foo"), Some("ns.foo".to_string()));
        assert_eq!(lookup(t, "bar"), Some("ns.bar".to_string()));
    }

    #[test]
    fn lookup_misses_returns_none() {
        let t = &[("foo", "ns.foo")];
        assert!(lookup(t, "baz").is_none());
    }

    #[test]
    fn aliases_only_preserves_order() {
        let t = &[("a", "x"), ("b", "y"), ("c", "z")];
        assert_eq!(aliases_only(t), vec!["a", "b", "c"]);
    }
}
