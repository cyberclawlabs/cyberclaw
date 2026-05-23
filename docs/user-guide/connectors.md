# Connectors

`Connector` 是 CyberClaw 唯一的代码级能力接入面。

## Connector 的职责

- 接入外部系统
- 提供运行时执行入口
- 暴露 `Capability` 集合
- 与治理和审计链协同工作

## 为什么 Connector 很重要

在 CyberClaw 中：

- `Skill` 负责方法
- `Connector` 负责执行
- `Capability` 是最小治理单元

这让平台可以在外部接入和内部治理之间维持清晰边界。

## 继续阅读

- [Builder Guide](../builders/build-a-connector.md)
- [Architecture](../architecture/README.md)
