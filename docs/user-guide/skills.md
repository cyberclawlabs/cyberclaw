# Skills

在 CyberClaw 中，`Skill` 负责“怎么做”，而不是“直接执行什么”。

## Skill 的职责

- 承载知识、方法、提示词和参考资料
- 提供可复用的工作方式
- 声明工具表面和依赖信息

## Skill 的边界

- 不直接拥有执行权限
- 不替代 Connector
- 不绕过 `Connector -> Capability` 治理链

## 兼容性方向

CyberClaw 当前对外强调 Skill 兼容接入面，参考：

- [Skill/Tool 兼容性架构设计](../architecture/overview/SKILL_TOOL_COMPATIBILITY_V1.md)
- [Skill 开发指南](../guides/SKILL_DEVELOPMENT.md)
