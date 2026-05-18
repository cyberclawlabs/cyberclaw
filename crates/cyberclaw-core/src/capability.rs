//! # 能力系统 (Capability System)
//!
//! 定义 CyberClaw 平台的能力抽象，包括风险级别、效果类型和执行请求。
//!
//! ## 核心概念
//!
//! - **Capability**: 平台可执行的最小动作单元
//! - **RiskLevel**: 能力的风险等级评估
//! - **CapabilityEffect**: 能力的副作用类型
//! - **CapabilityRef**: 能力的完整引用，包含位置和元数据

use crate::cluster::CapabilityPlacement;
use crate::identity::ActorRef;
use crate::ids::{CapabilityId, ConnectorId, ExecutionId};
use serde::{Deserialize, Serialize};

/// 能力的风险级别
///
/// 用于治理层进行权限控制和审批流程判断。
///
/// # 示例
///
/// ```rust
/// use cyberclaw_core::capability::RiskLevel;
///
/// let risk = RiskLevel::High;
/// assert!(risk > RiskLevel::Low);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    /// 低风险 - 只读操作，无副作用
    Low,
    /// 中等风险 - 有限的写入或状态变更
    Medium,
    /// 高风险 - 重要资源的修改或删除
    High,
    /// 关键风险 - 系统级操作或不可逆变更
    Critical,
}

/// 能力的效果类型
///
/// 描述能力执行后产生的副作用类型，用于审计和追踪。
///
/// # 示例
///
/// ```rust
/// use cyberclaw_core::capability::CapabilityEffect;
///
/// let effect = CapabilityEffect::Write;
/// let custom = CapabilityEffect::Custom("database.migrate".to_string());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityEffect {
    /// 读取数据或状态
    Read,
    /// 写入或修改数据
    Write,
    /// 执行代码或命令
    Execute,
    /// 网络访问
    Network,
    /// 创建工单或任务
    Ticket,
    /// 发送通知或消息
    Notification,
    /// 自定义效果类型
    Custom(String),
}

/// 能力引用
///
/// 包含能力的完整信息，包括所属 Connector、风险级别和执行位置。
///
/// # 示例
///
/// ```rust
/// use cyberclaw_core::capability::{CapabilityRef, RiskLevel, CapabilityEffect};
/// use cyberclaw_core::ids::{CapabilityId, ConnectorId};
///
/// let cap_ref = CapabilityRef {
///     id: CapabilityId::from_string("file.read".to_string()).unwrap(),
///     connector_id: ConnectorId::from_string("filesystem".to_string()).unwrap(),
///     risk: RiskLevel::Low,
///     effects: vec![CapabilityEffect::Read],
///     placement: None,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityRef {
    /// 能力的唯一标识符
    pub id: CapabilityId,
    /// 提供此能力的 Connector ID
    pub connector_id: ConnectorId,
    /// 能力的风险级别
    pub risk: RiskLevel,
    /// 能力产生的效果列表
    pub effects: Vec<CapabilityEffect>,
    /// 能力的执行位置（用于分布式场景）
    pub placement: Option<CapabilityPlacement>,
}

/// 动作执行请求
///
/// 包含执行能力所需的完整上下文信息。
///
/// # 示例
///
/// ```rust
/// use cyberclaw_core::capability::ActionRequest;
/// use cyberclaw_core::ids::{ExecutionId, CapabilityId};
/// use cyberclaw_core::identity::{ActorRef, Identity};
/// use serde_json::json;
///
/// let request = ActionRequest {
///     execution_id: ExecutionId::new(),
///     requested_by: Identity::System.to_actor_ref(None).unwrap(),
///     capability: CapabilityId::from_string("github.create_issue".to_string()).unwrap(),
///     input: json!({
///         "repo": "cyberclaw/cyberclaw",
///         "title": "Bug report",
///         "body": "Description"
///     }),
///     reason: "Automated bug reporting".to_string(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRequest {
    /// 执行会话的唯一标识
    pub execution_id: ExecutionId,
    /// 请求发起者
    pub requested_by: ActorRef,
    /// 要执行的能力 ID
    pub capability: CapabilityId,
    /// 能力执行的输入参数
    pub input: serde_json::Value,
    /// 执行原因说明（用于审计）
    pub reason: String,
}
