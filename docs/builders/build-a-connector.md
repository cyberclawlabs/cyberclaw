# Build a Connector

如果你要接入外部系统、协议、服务或运行时，优先构建 Connector。

## Connector 适合承载什么

- 外部系统接入
- 协议调用
- 风险动作执行
- Capability 暴露

## 设计要求

- 明确执行边界
- 显式声明 Capability
- 不绕过治理链
- 在高风险路径上保留审计与追踪

## 继续阅读

- [Connector 开发指南](../guides/CONNECTOR_DEVELOPMENT.md)
- [开发者版 Connector 指引](../guides/developers/CONNECTOR_DEVELOPMENT.md)
