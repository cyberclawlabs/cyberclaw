//! # CyberClaw Core
//!
//! `cyberclaw-core` 提供 CyberClaw 平台的核心类型定义和基础抽象。
//!
//! ## 模块组织
//!
//! - **执行核心**: `execution`, `task`, `workflow` - 任务执行和工作流管理
//! - **能力系统**: `capability`, `manifests` - 能力定义和清单管理
//! - **安全治理**: `security`, `audit_logger`, `validation` - 安全策略和审计
//! - **内存管理**: `memory`, `memory_context` - 记忆系统和上下文管理
//! - **身份认证**: `identity`, `secrets`, `sensitive` - 身份和凭证管理
//! - **追踪审计**: `provenance`, `artifact`, `review` - 执行追踪和评审
//! - **分布式**: `cluster`, `protocol` - 集群和协议定义
//!
//! ## 快速开始
//!
//! ```rust
//! use cyberclaw_core::prelude::*;
//!
//! // 创建执行 ID
//! let exec_id = ExecutionId::new();
//!
//! // 创建能力引用
//! let cap_ref = CapabilityRef {
//!     id: CapabilityId::from_string("file.read".to_string()).unwrap(),
//!     connector_id: ConnectorId::from_string("filesystem".to_string()).unwrap(),
//!     risk: RiskLevel::Low,
//!     effects: vec![CapabilityEffect::Read],
//!     placement: None,
//! };
//! ```

/// Agent 信任级别模块 - 定义 Agent 信任等级和风险调整
pub mod agent;
/// 工件管理模块 - 执行产物的存储和追踪
pub mod artifact;
/// 审计日志模块 - 统一的审计日志记录 // MEDIUM #9 FIX: 统一审计日志格式
pub mod audit_logger;
/// Autopilot 模块 - 自动驾驶执行模式
pub mod autopilot;
/// 能力定义模块 - Capability 类型和元数据
pub mod capability;
/// Capability behavior contract - parallelism and safety metadata
pub mod capability_contract;
/// 用例管理模块 - 测试用例和场景定义
pub mod case;
/// Clarify 交互模块 - Agent→User 结构化澄清请求（对齐 Claude Code AskUserQuestionTool）
pub mod clarify;
/// 集群支持模块 - 分布式节点管理
pub mod cluster;
/// 枚举定义模块 - 通用枚举类型
pub mod enums;
/// 错误处理模块 - 统一错误类型定义
pub mod errors;
/// 执行引擎模块 - 核心执行逻辑
pub mod execution;
/// Capability facade — LLM-facing projection of a `Capability`.
///
/// Lives in core (not agent-runtime) so connectors can declare their
/// own facades without reverse-importing agent-runtime, fulfilling the
/// EVOLUTION_IDIOMS §4 "Facades From Owner Crate" idiom. Moved here
/// 2026-05-06 after F8 architecture review (Claude + GPT consensus).
pub mod facade;
/// Orchestrator gateway trait - agent-runtime to control-plane bridge
pub mod gateway;
/// Handoff 模块 - Agent→Agent 运行时会话控制权转移（Sprint 21）
pub mod handoff;
/// 国际化支持模块 — 编译时字符串表，支持 en / zh 两种语言（v1）
pub mod i18n;
/// 身份管理模块 - 用户和服务身份
pub mod identity;
/// ID 生成模块 - 各类唯一标识符
pub mod ids;
/// 清单定义模块 - Agent/Skill/Plugin 清单
pub mod manifests;
/// 记忆系统模块 - 长期记忆管理
pub mod memory;
/// 记忆上下文模块 - 执行上下文管理
pub mod memory_context;
/// 协议定义模块 - 通信协议抽象
pub mod protocol;
/// 溯源追踪模块 - 执行链追踪
pub mod provenance;
/// 评审系统模块 - 执行评审和反馈
pub mod review;
/// 密钥管理模块 - 安全凭证存储
pub mod secrets;
/// 安全核心模块 - 安全策略和检查
pub mod security;
/// 安全配置模块 - 安全策略配置
pub mod security_config;
/// 安全错误模块 - 安全相关错误
pub mod security_error;
/// 安全扫描模块 - 代码和依赖扫描
pub mod security_scanner;
/// 敏感数据模块 - 敏感信息处理
pub mod sensitive;
/// 任务管理模块 - 异步任务调度
pub mod task;
/// Trait 定义模块 - 核心 trait 抽象
pub mod traits;
/// 用户身份模块 - 最小操作员身份持久化 (Sprint 5)
pub mod users;
/// 验证模块 - 输入验证和校验
pub mod validation;
/// 工作流模块 - 工作流定义和执行
pub mod workflow;
/// 工作空间模块 - 项目工作空间管理
pub mod workspace;

/// Prelude 模块 - 常用类型的便捷导入
///
/// # 使用示例
///
/// ```rust
/// use cyberclaw_core::prelude::*;
///
/// // 现在可以直接使用所有核心类型
/// let exec_id = ExecutionId::new();
/// let actor = Identity::System.to_actor_ref(None).unwrap();
/// ```
pub mod prelude {
    #[allow(ambiguous_glob_reexports)]
    pub use crate::agent::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::artifact::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::audit_logger::*; // MEDIUM #9 FIX
    #[allow(ambiguous_glob_reexports)]
    pub use crate::autopilot::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::capability::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::case::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::clarify::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::cluster::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::enums::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::errors::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::execution::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::identity::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::ids::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::manifests::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::protocol::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::provenance::*;
    #[allow(ambiguous_glob_reexports)]
    pub use crate::review::*;
    pub use crate::secrets::*;
    pub use crate::security::*;
    pub use crate::security_config::*;
    pub use crate::security_error::*;
    pub use crate::security_scanner::*;
    pub use crate::sensitive::*;
    pub use crate::task::*;
    pub use crate::traits::*;
    pub use crate::validation::*;
    pub use crate::workflow::*;
    pub use crate::workspace::*;
}
