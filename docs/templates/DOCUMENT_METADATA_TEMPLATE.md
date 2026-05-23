# CyberClaw 文档元信息模板

本模板用于统一 CyberClaw 仓库中的入口文档、正式文档和 crate 本地文档页头。

## 标准字段

推荐在文档标题后增加以下元信息：

- `Status`: `Active` / `Draft` / `Scaffold` / `Archived`
- `Scope`: `Repository` / `Docs` / `Crate` / `Business`
- `Owner`: 负责维护该文档的团队或角色
- `Last Updated`: `YYYY-MM-DD`

## 标准示例

```md
# Document Title

- Status: Active
- Scope: Repository
- Owner: CyberClaw Maintainers
- Last Updated: 2026-03-20
```

## 推荐用法

### 根目录入口与政策文档

适用：

- `README.md`
- `CHANGELOG.md`
- `DEVELOPMENT.md`
- `DOCUMENTATION_SYSTEM.md`
- `CLAUDE.md`
- `AGENTS.md`

推荐：

- `Status: Active`
- `Scope: Repository`

### `docs/` 正式文档

适用：

- `docs/INDEX.md`
- 各目录 `README.md`
- 长期维护的架构、实现、业务文档

推荐：

- `Scope: Docs` 或 `Scope: Business`

### crate 本地文档

适用：

- `crates/*/README.md`
- `crates/*/CHANGELOG.md`

推荐：

- `Scope: Crate`
- `Owner`: 对应 crate maintainers

## 使用规则

1. 入口页、规则页、crate README、crate CHANGELOG 建议强制使用该模板。
2. 普通技术长文可按需要使用，但至少应保证 `Last Updated` 可追踪。
3. 若文档已经明显过期，优先修正文档正文，不要只改日期。
