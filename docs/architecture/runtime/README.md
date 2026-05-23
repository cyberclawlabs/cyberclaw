# CyberClaw 运行时文档

本目录收敛 CyberClaw 的运行时与控制平面执行设计，包括 Registry、解析器、运行蓝图和调度边界。

## 文档

1. [Registry Runtime](REGISTRY_RUNTIME_V2.0.md)
2. [Runtime Blueprint](RUNTIME_BLUEPRINT_V2.0.md)
3. [CyberClaw Autopilot Architecture](CYBERCLAW_AUTOPILOT_ARCHITECTURE_V1.md)
4. [Web3 Connector Pack Architecture](WEB3_CONNECTOR_PACK_V1.md)
5. [CyberClaw HFT Control Plane Architecture](CYBERCLAW_HFT_CONTROL_PLANE_ARCHITECTURE_V1.md)

## 适用场景

- 理解包发现、加载、注册和解析流程
- 理解运行蓝图、运行时约束和执行前准备
- 评估 Control Plane 与 Runtime 的边界设计
- 评估 Autopilot、Loop Runtime、调度和恢复策略
- 评估高风险业务场景下的 Connector 执行平面设计（如 Web3 交易）
- 评估控制平面如何管理外部低延迟系统（如 `HFT Gateway` 与 `Matching Core`）

## 阅读顺序

1. [Registry Runtime](REGISTRY_RUNTIME_V2.0.md)
2. [Runtime Blueprint](RUNTIME_BLUEPRINT_V2.0.md)
3. [CyberClaw Autopilot Architecture](CYBERCLAW_AUTOPILOT_ARCHITECTURE_V1.md)
4. [Web3 Connector Pack Architecture](WEB3_CONNECTOR_PACK_V1.md)
5. [CyberClaw HFT Control Plane Architecture](CYBERCLAW_HFT_CONTROL_PLANE_ARCHITECTURE_V1.md)
