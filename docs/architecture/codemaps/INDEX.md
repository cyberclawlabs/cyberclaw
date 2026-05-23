# CyberClaw v2.0 架构总览

**最后更新:** 2024-03-18
**版本:** v2.0
**语言:** Rust
**类型:** 多层架构 Agentic 平台

## 项目结构

```
cyberclaw/
├── crates/                         # Rust 工作区
│   ├── cyberclaw-core/            # 核心类型与原语 ✅
│   ├── cyberclaw-control-plane/   # 控制平面服务 ✅
│   ├── cyberclaw-agent-runtime/   # Agent 运行时 🚧
│   ├── cyberclaw-skill-runtime/   # Skill 运行时 🚧
│   ├── cyberclaw-workflow/        # 工作流引擎 🚧
│   ├── cyberclaw-connectors/      # 连接器层 🚧
│   ├── cyberclaw-governance/      # 治理层 🚧
│   ├── cyberclaw-observability/   # 可观测性 🚧
│   └── cyberclaw-store/           # 存储层 🚧
├── apps/                          # 应用程序
│   ├── cyberclaw-server/          # API 服务器 🚧
│   └── cyberclaw-cli/             # CLI 工具 🚧
├── ecosystem/                     # 生态包
│   ├── agents/                    # Agent 包
│   ├── skills/                    # Skill 包
│   ├── connectors/                # Connector 包
│   └── platform-plugins/          # Platform Plugin 包
├── docs/                          # 项目文档
│   └── architecture/codemaps/      # 架构映射
├── Cargo.toml                     # 工作区配置
└── README.md                      # 项目说明

图例: ✅ 已实现 | 🚧 规划中
```

## 架构概览

CyberClaw 采用分层架构，从应用层到核心引擎共 6 层：

```
┌─────────────────────────────────────────────────────────────────────┐
│                     应用层 (Applications)                            │
│  • cyberclaw-server (HTTP API)  • cyberclaw-cli (命令行)          │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
┌──────────────────────────────▼──────────────────────────────────────┐
│                     控制平面层 (Control Plane)                       │
│  • Registry & Resolver       • TaskManager & CaseManager           │
│  • ReviewQueue               • SubagentScheduler                    │
│  • ArtifactStore             • EventBus                             │
│  • LeaseManager              • MembershipService                    │
│  • SharedState               • Orchestrator                         │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
┌──────────────────────────────▼──────────────────────────────────────┐
│                     运行时层 (Runtime Layers)                        │
│  • AgentRuntime              • SkillRuntime                         │
│  • WorkflowEngine            • ConnectorLayer                       │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
┌──────────────────────────────▼──────────────────────────────────────┐
│                     治理层 (Governance Gate)                         │
│  • Permission Check          • Policy Evaluation                    │
│  • Risk Assessment           • Approval Workflow                    │
│  • Audit Logging             • Provenance Tracking                  │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
┌──────────────────────────────▼──────────────────────────────────────┐
│                     存储层 (Storage Layer)                           │
│  • State Store (PostgreSQL)  • Artifact Store (File/S3)            │
│  • Event Store (Event Sourcing)  • Audit Store (不可变)            │
│  • Cache Layer (Redis)       • Memory Store (会话)                  │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
┌──────────────────────────────▼──────────────────────────────────────┐
│                     核心引擎 (Core Engine)                           │
│  • ExecutionId/NodeId (验证)  • Manifests (包定义)                 │
│  • Capability (能力系统)      • Provenance (溯源)                   │
│  • Task & Case (任务模型)    • Security (安全原语)                  │
└─────────────────────────────────────────────────────────────────────┘

横跨所有层：
• Observability (日志、追踪、指标)
• Ecosystem (Agent, Skill, Connector, Plugin 包)
```

## 模块映射

| 层级 | 模块 | 位置 | 职责 | 状态 |
|------|------|------|------|------|
| **应用层** | [Applications](./applications.md) | `apps/` | HTTP API、CLI 工具 | 🚧 规划中 |
| **控制平面** | [Control Plane](./control-plane.md) | `crates/cyberclaw-control-plane/` | 服务协调、状态管理 | ✅ 已实现 |
| **运行时层** | [Runtime Layers](./runtime-layers.md) | `crates/cyberclaw-{agent,skill,workflow,connectors}-runtime/` | Agent、Skill、工作流、连接器执行 | 🚧 规划中 |
| **治理层** | [Governance](./governance.md) | `crates/cyberclaw-governance/` | 权限、策略、审批、审计 | 🚧 规划中 |
| **存储层** | [Storage](./store.md) | `crates/cyberclaw-store/` | 状态、工件、事件、缓存 | 🚧 部分实现 |
| **核心引擎** | [Core Engine](./core.md) | `crates/cyberclaw-core/` | 执行引擎、ID验证、类型原语 | ✅ 已实现 |
| **横跨层** | [Observability](./observability.md) | `crates/cyberclaw-observability/` | 日志、追踪、指标 | 🚧 规划中 |
| **生态系统** | [Ecosystem](./ecosystem.md) | `ecosystem/` | Agent、Skill、Connector、Plugin 包 | 🚧 规划中 |
| **安全架构** | [Security Layer](./security.md) | 跨模块 | 安全验证、访问控制 | 🔒 已加固 |

## 数据流

```
请求 → 验证(Core) → 调度(Control Plane) → 执行(Core) → 持久化(Control Plane) → 响应
         ↑                    ↓                 ↑              ↓
         └────── 安全层验证 ──┴─────────────────┴──────────────┘
```

## 技术栈

### 核心技术
- **语言:** Rust (Async/Tokio)
- **序列化:** Serde (JSON/YAML)
- **错误处理:** Anyhow/thiserror

### 存储技术
- **关系数据库:** PostgreSQL (状态存储、事件存储)
- **缓存:** Redis (分布式缓存)
- **对象存储:** File System / S3 (工件存储)

### 可观测技术
- **日志:** tracing + tracing-subscriber
- **追踪:** OpenTelemetry + Jaeger
- **指标:** Prometheus

### 并发控制
- **异步运行时:** Tokio
- **并发数据结构:** DashMap
- **消息传递:** Tokio channels (bounded/unbounded)

## 快速导航

### 架构文档（按层级）

**应用层**
- [Applications](./applications.md) - HTTP API 服务器、CLI 工具设计

**控制平面层**
- [Control Plane](./control-plane.md) - 服务协调、任务管理、事件总线

**运行时层**
- [Runtime Layers](./runtime-layers.md) - Agent、Skill、工作流、连接器运行时

**治理层**
- [Governance](./governance.md) - 权限、策略、风险、审批、审计

**存储层**
- [Storage](./store.md) - 状态、工件、事件、审计、缓存存储

**核心引擎**
- [Core Engine](./core.md) - 执行引擎、ID 验证、类型原语

**横跨所有层**
- [Observability](./observability.md) - 日志、追踪、指标系统
- [Ecosystem](./ecosystem.md) - Agent、Skill、Connector、Plugin 包生态
- [Security Layer](./security.md) - 安全验证、访问控制、威胁防护

### 相关文档
- [项目 README](../../../README.md) - 项目概述与快速开始
- [Cargo.toml](../../../Cargo.toml) - 工作区依赖配置

## 维护说明

本文档反映实际代码结构。
更新频率：每次重大版本发布或架构变更后。

---

**下一步:** 查看 [控制平面架构](./control-plane.md) 了解详细实现
