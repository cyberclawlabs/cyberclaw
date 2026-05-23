// CyberClaw M2 Governance Core - Trait 定义草案
// 此文件包含核心接口定义，用于指导实际实现

// ============================================================================
// 核心 Trait: PolicyEngine
// ============================================================================

use async_trait::async_trait;
use anyhow::Result;
use cyberclaw_core::prelude::*;
use serde::{Deserialize, Serialize};

/// 策略评估引擎的核心 trait
///
/// 设计原则：
/// 1. 异步接口支持远程策略服务
/// 2. 细粒度评估方法便于性能优化
/// 3. 明确的错误处理
#[async_trait]
pub trait PolicyEngine: Send + Sync {
    /// 评估单个 capability 是否可以执行
    ///
    /// # Arguments
    /// * `request` - Capability 评估请求，包含完整上下文
    ///
    /// # Returns
    /// * `Ok(GovernanceDecision)` - 包含决策类型、原因、触发策略等
    /// * `Err` - 评估过程中的错误（不是拒绝）
    async fn evaluate_capability(
        &self,
        request: CapabilityEvaluationRequest,
    ) -> Result<GovernanceDecision>;

    /// 评估完整的执行计划
    ///
    /// # Note
    /// 此方法会评估计划中的所有 capabilities，
    /// 任何 Deny 都会导致整个计划被拒绝
    async fn evaluate_execution(
        &self,
        request: ExecutionEvaluationRequest,
    ) -> Result<GovernanceDecision>;

    /// 评估 review 请求（用于自动审批逻辑）
    ///
    /// # Note
    /// M2 阶段可简化实现，返回默认批准
    async fn evaluate_review(
        &self,
        request: ReviewEvaluationRequest,
    ) -> Result<ReviewDecision>;

    /// 重新加载策略配置
    ///
    /// # Note
    /// 支持热重载，不影响正在进行的评估
    async fn reload_policies(&self) -> Result<()>;

    /// 获取当前活跃的策略数量（用于监控）
    async fn policy_count(&self) -> usize;

    /// 获取引擎健康状态
    async fn health_check(&self) -> Result<HealthStatus>;
}

// ============================================================================
// 请求和响应类型
// ============================================================================

/// Capability 评估请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityEvaluationRequest {
    /// Capability 标识符
    pub capability_id: CapabilityId,
    /// Connector 标识符
    pub connector_id: ConnectorId,
    /// 风险级别
    pub risk: RiskLevel,
    /// Capability 效果（读、写、执行等）
    pub effects: Vec<CapabilityEffect>,
    /// 执行者身份
    pub actor: ActorRef,
    /// 工作空间（可选）
    pub workspace: Option<WorkspaceRef>,
    /// 会话信息（可选）
    pub session: Option<SessionRef>,
    /// Capability 输入参数（可选）
    pub input: Option<serde_json::Value>,
    /// 追踪 ID（用于关联日志）
    pub trace_id: Option<TraceId>,
}

/// 执行计划评估请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionEvaluationRequest {
    /// 执行 ID
    pub execution_id: ExecutionId,
    /// 任务详情
    pub task: Task,
    /// 执行计划
    pub plan: ExecutionPlan,
    /// 执行者身份
    pub actor: ActorRef,
    /// 工作空间（可选）
    pub workspace: Option<WorkspaceRef>,
    /// 会话信息（可选）
    pub session: Option<SessionRef>,
}

/// Review 评估请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewEvaluationRequest {
    /// Review ID
    pub review_id: ReviewId,
    /// 关联的执行 ID
    pub execution_id: ExecutionId,
    /// 审批者身份
    pub reviewer: ActorRef,
    /// 审批决策
    pub decision: ReviewDecision,
    /// 审批意见（可选）
    pub comments: Option<String>,
}

/// 治理决策结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceDecision {
    /// 决策类型
    pub decision: Decision,
    /// 决策原因（人类可读）
    pub reason: String,
    /// 触发此决策的策略列表
    pub triggered_policies: Vec<PolicyRef>,
    /// 需要的审批者（如果 review_required）
    pub required_reviewers: Vec<ActorRef>,
    /// 安全建议
    pub security_recommendations: Vec<String>,
    /// 是否需要记录审计日志
    pub audit_required: bool,
    /// 决策元数据（用于扩展）
    pub metadata: Option<serde_json::Value>,
}

/// 决策类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    /// 允许执行
    Allow,
    /// 拒绝执行
    Deny,
    /// 需要人工审批
    ReviewRequired,
}

/// Review 决策
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    /// 批准
    Approve,
    /// 拒绝
    Reject,
    /// 批准但修改参数
    ApproveWithModification {
        modifications: serde_json::Value,
        reason: String,
    },
    /// 升级到更高级别审批
    Escalate {
        escalate_to: Vec<ActorRef>,
        reason: String,
    },
}

/// 策略引用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRef {
    /// 策略唯一标识
    pub id: String,
    /// 策略名称
    pub name: String,
    /// 策略版本
    pub version: String,
    /// 策略类型
    pub policy_type: PolicyType,
}

/// 策略类型枚举
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyType {
    Capability,
    Review,
    Tenant,
    Security,
    Custom(String),
}

/// 健康状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub healthy: bool,
    pub message: String,
    pub policy_count: usize,
    pub last_reload: Option<chrono::DateTime<chrono::Utc>>,
}

// ============================================================================
// 策略 Trait
// ============================================================================

/// 策略基础 trait
#[async_trait]
pub trait Policy: Send + Sync {
    /// 策略唯一标识
    fn id(&self) -> &str;

    /// 策略名称
    fn name(&self) -> &str;

    /// 策略版本
    fn version(&self) -> &str;

    /// 策略类型
    fn policy_type(&self) -> PolicyType;

    /// 策略优先级（数字越小优先级越高）
    fn priority(&self) -> i32;

    /// 是否启用
    fn is_enabled(&self) -> bool;

    /// 评估策略
    async fn evaluate(&self, context: &PolicyContext) -> PolicyResult;

    /// 验证策略配置
    fn validate(&self) -> Result<()>;
}

/// 策略上下文
#[derive(Debug, Clone)]
pub struct PolicyContext {
    /// 执行者
    pub actor: ActorRef,
    /// 工作空间
    pub workspace: Option<WorkspaceRef>,
    /// 会话
    pub session: Option<SessionRef>,
    /// Capability 信息
    pub capability: Option<CapabilityRef>,
    /// 任务信息
    pub task: Option<Task>,
    /// 额外元数据
    pub metadata: serde_json::Value,
    /// 追踪 ID
    pub trace_id: Option<TraceId>,
}

/// 策略评估结果
#[derive(Debug, Clone)]
pub enum PolicyResult {
    /// 策略不适用于当前上下文
    NotApplicable,
    /// 允许
    Allow,
    /// 拒绝
    Deny {
        reason: String,
        /// 是否可覆盖
        overridable: bool,
    },
    /// 需要审批
    RequireReview {
        reviewers: Vec<ActorRef>,
        reason: String,
        /// 审批超时时间（秒）
        timeout_secs: Option<u64>,
    },
    /// 修改输入（用于安全脱敏）
    ModifyInput {
        modified_input: serde_json::Value,
        reason: String,
    },
}

// ============================================================================
// 策略加载器 Trait
// ============================================================================

/// 策略加载器 trait
#[async_trait]
pub trait PolicyLoader: Send + Sync {
    /// 加载所有策略
    async fn load_all(&self) -> Result<Vec<Box<dyn Policy>>>;

    /// 加载特定类型的策略
    async fn load_by_type(&self, policy_type: PolicyType) -> Result<Vec<Box<dyn Policy>>>;

    /// 验证策略配置
    async fn validate_all(&self) -> Result<ValidationReport>;
}

/// 验证报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    pub valid: bool,
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<ValidationWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationError {
    pub policy_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationWarning {
    pub policy_id: String,
    pub message: String,
}

// ============================================================================
// 审计 Trait
// ============================================================================

/// 审计记录器 trait
#[async_trait]
pub trait AuditLogger: Send + Sync {
    /// 记录治理决策
    async fn log_decision(&self, entry: AuditEntry) -> Result<()>;

    /// 查询审计日志
    async fn query(
        &self,
        filter: AuditFilter,
    ) -> Result<Vec<AuditEntry>>;
}

/// 审计条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// 时间戳
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// 执行 ID
    pub execution_id: ExecutionId,
    /// 执行者
    pub actor: ActorRef,
    /// 决策
    pub decision: Decision,
    /// 触发的策略
    pub triggered_policies: Vec<PolicyRef>,
    /// Capability（如果适用）
    pub capability: Option<CapabilityId>,
    /// 输入哈希（用于审计，不存储实际输入）
    pub input_hash: String,
    /// 工作空间
    pub workspace: Option<WorkspaceRef>,
    /// 追踪 ID
    pub trace_id: Option<TraceId>,
}

/// 审计过滤器
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditFilter {
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    pub actor: Option<ActorRef>,
    pub decision: Option<Decision>,
    pub execution_id: Option<ExecutionId>,
    pub limit: Option<usize>,
}

// ============================================================================
// 安全特性 Traits
// ============================================================================

/// Secret 脱敏器 trait
#[async_trait]
pub trait SecretRedactor: Send + Sync {
    /// 检测并脱敏输入中的敏感信息
    async fn redact(&self, input: &serde_json::Value) -> Result<serde_json::Value>;

    /// 检测输入是否包含敏感信息
    async fn contains_secrets(&self, input: &serde_json::Value) -> bool;
}

/// Prompt Injection 检测器 trait
#[async_trait]
pub trait InjectionDetector: Send + Sync {
    /// 检测输入是否包含潜在的 prompt injection
    async fn detect(&self, input: &str) -> Result<InjectionDetectionResult>;
}

/// Injection 检测结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjectionDetectionResult {
    pub detected: bool,
    pub confidence: f64,
    pub patterns_matched: Vec<String>,
    pub recommendation: SecurityRecommendation,
}

/// 安全建议
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityRecommendation {
    Allow,
    Review,
    Block,
    Sanitize { suggested_sanitization: String },
}

/// Command 安全检查器 trait
#[async_trait]
pub trait CommandSafetyChecker: Send + Sync {
    /// 检查命令是否安全
    async fn check(&self, command: &str, args: &[String]) -> Result<CommandSafetyResult>;
}

/// 命令安全检查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSafetyResult {
    pub safe: bool,
    pub risk_level: RiskLevel,
    pub violations: Vec<String>,
    pub recommendations: Vec<String>,
}

// ============================================================================
// 工厂 Trait
// ============================================================================

/// PolicyEngine 工厂 trait
pub trait PolicyEngineFactory: Send + Sync {
    /// 创建 PolicyEngine 实例
    fn create(&self, config: PolicyEngineConfig) -> Result<Box<dyn PolicyEngine>>;
}

/// PolicyEngine 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyEngineConfig {
    /// 策略配置目录
    pub policy_dir: String,
    /// 是否启用缓存
    pub enable_cache: bool,
    /// 缓存 TTL（秒）
    pub cache_ttl_secs: u64,
    /// 是否启用审计
    pub enable_audit: bool,
    /// 审计日志目录
    pub audit_dir: Option<String>,
    /// 默认决策（当没有策略匹配时）
    pub default_decision: Decision,
    /// 性能配置
    pub performance: PerformanceConfig,
}

/// 性能配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// 最大并发评估数
    pub max_concurrent_evaluations: usize,
    /// 评估超时（毫秒）
    pub evaluation_timeout_ms: u64,
    /// 是否启用短路评估
    pub enable_short_circuit: bool,
}

// ============================================================================
// 扩展点
// ============================================================================

/// 策略钩子 trait（用于扩展）
#[async_trait]
pub trait PolicyHook: Send + Sync {
    /// 评估前钩子
    async fn pre_evaluate(&self, context: &PolicyContext) -> Result<()>;

    /// 评估后钩子
    async fn post_evaluate(&self, context: &PolicyContext, result: &PolicyResult) -> Result<()>;
}

/// 决策拦截器 trait（用于监控和调试）
#[async_trait]
pub trait DecisionInterceptor: Send + Sync {
    /// 拦截决策
    async fn intercept(&self, decision: &GovernanceDecision) -> Result<()>;
}