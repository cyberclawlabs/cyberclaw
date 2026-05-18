# cyberclaw-core

核心类型、特征和协议定义。

## 概述

`cyberclaw-core` 提供 CyberClaw 平台的核心抽象和数据结构，包括：

- **身份与授权**: `Identity`, `ActorRef`
- **ID 系统**: `ExecutionId`, `ReviewId`, `ConnectorId`, `CapabilityId`
- **风险与治理**: `RiskLevel`, `GovernanceDecision`
- **溯源追踪**: `ProvenanceRecord`, `RuntimeProvenance`, `SecurityContext`
- **安全扫描**: `SecurityScanner`, `SecretScanner`, `PromptInjectionScanner`, `CommandSafetyScanner`, `PackageTrustScanner`
- **敏感数据**: `SensitiveString`, `RedactionStrategy`
- **密钥管理**: `SecretsManager`, `InMemorySecretsManager`
- **记忆系统** (Beta): `WorkingMemory`, `EpisodicMemory`, `ProceduralMemory`, `MemoryContextProvider`
- **Agent 信任模型**: `AgentTrustLevel` (Trusted/Standard/Restricted) 用于执行风险分层调整

## 新增模块 (2026-03-21)

### Memory 模块

实现三层记忆架构和热路径压缩：

- **Working Memory** (`memory::working`): 当前会话工作记忆实时缓存
  - 执行级别记忆隔离
  - 自动大小限制

- **Episodic Memory** (`memory::episodic`): 历史执行记录和上下文投影
  - 历史摘要存储
  - 上下文投影生成

- **Procedural Memory** (`memory::procedural`): 程序性规则和文档管理
  - 文档存储
  - 规则管理

- **Memory Context Provider** (`memory::provider`): 统一记忆上下文提供者
  - Beta 三层上下文实时组装
  - 线程安全

- **Compaction Strategy** (`memory::compaction`): LRU + 去重压缩策略
  - 轻量级压缩
  - 检查点机制

**性能指标**:
- Memory Context Query: p50=8.8µs (目标 <50ms, 超标 5000x)
- Sync Compaction: p50=32.7µs (目标 <100ms, 超标 2857x)
- Checkpoint Creation: p50=3.5-6.1µs

**基准测试**: `benches/memory_bench.rs`

**集成测试**: `tests/memory_integration.rs` (12 个测试全部通过)

### Security Scanner 模块

实现安全扫描能力最小闭环：

- **SecretScanner**: API keys, AWS keys, JWT tokens 检测
- **PromptInjectionScanner**: 角色操作、指令覆盖检测
- **CommandSafetyScanner**: 危险命令检测
- **PackageTrustScanner**: 可疑文件模式检测

文件: `src/security_scanner.rs` (12 个测试通过)

## 架构原则

- 所有业务动作最终落到 `Capability`
- `Connector` 是唯一代码级能力接入面
- `Agent` 负责角色与编排
- `Skill` 负责方法、知识、模板

## 依赖

```toml
[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
uuid = { version = "1.0", features = ["v4", "serde"] }
thiserror = "2.0"
criterion = "0.5" # benches
```

## 许可证

Apache-2.0
