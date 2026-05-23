# Builder Guide

Builder Guide 面向想扩展 CyberClaw 的开发者和团队。

## 这组文档适合谁

- 想发布 Skill 的生态构建者
- 想接入外部系统的 Connector 开发者
- 想增强平台生命周期的 Plugin 开发者

## Builder Tracks

- [Build a Skill](build-a-skill.md)
- [Build a Connector](build-a-connector.md)
- [Build a Platform Plugin](build-a-plugin.md)

## 先理解的边界

在开始扩展前，请先确认：

1. `Skill` 负责方法和知识
2. `Connector` 负责执行和接入
3. `Capability` 是最小治理动作单元
4. `Platform Plugin` 负责平台级增强

## 最常见的选型问题

如果你不确定该做成什么：

- 做方法、知识、模板：先看 Skill
- 做真实执行和外部接入：先看 Connector
- 做平台级增强：先看 Platform Plugin

## 相关文档

- [Skill/Tool 兼容性架构设计](../architecture/overview/SKILL_TOOL_COMPATIBILITY_V1.md)
- [Reference](../reference/README.md)
