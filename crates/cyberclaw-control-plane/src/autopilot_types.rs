//! Autopilot V2 核心状态模型
//!
//! 定义 Autopilot 运行时所需的所有状态类型，包括：
//! - AutopilotJob: 长时运行的自动化任务
//! - AutopilotRunState: 单次 Run 的完整状态
//! - IterationState: 单次迭代的完整记录
//! - AutopilotStep: 9步循环枚举
//! - AutopilotStatus: Run级状态
//!
//! # 设计原则
//!
//! 1. 所有类型可序列化为 JSON（通过 serde）
//! 2. 状态可通过哈希检测无进展（no-progress detection）
//! 3. 与现有 ExecutionId/ExecutionStatus 集成
//! 4. 支持版本演进和向后兼容
//!
//! # 示例
//!
//! ```rust
//! use cyberclaw_control_plane::autopilot_types::*;
//! use cyberclaw_core::ids::ExecutionId;
//! use chrono::Utc;
//!
//! let job = AutopilotJob {
//!     job_id: "job-001".to_string(),
//!     goal: "自动化部署".to_string(),
//!     max_iterations: 50,
//!     review_gates: vec![],
//!     created_at: Utc::now(),
//! };
//!
//! let run = AutopilotRunState::new(
//!     ExecutionId::new(),
//!     job.job_id.clone(),
//! );
//!
//! assert_eq!(run.current_iteration, 0);
//! assert_eq!(run.status, AutopilotStatus::Initialized);
//! ```

use chrono::{DateTime, Utc};
use cyberclaw_core::execution::ExecutionStatus;
use cyberclaw_core::ids::ExecutionId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 审批门类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReviewGate {
    /// 执行前审批
    BeforeExecution,
    /// 执行后审批
    AfterExecution,
    /// 高风险动作审批
    HighRisk,
    /// 自定义门（包含条件描述）
    Custom(String),
}

/// 长时运行的 Autopilot 任务
///
/// 一个 Job 可以产生多个 Run，每个 Run 是一次完整的自动化执行周期
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotJob {
    /// 任务唯一标识
    pub job_id: String,
    /// 任务目标描述
    pub goal: String,
    /// 最大迭代次数（防止无限循环）
    pub max_iterations: u32,
    /// 需要通过的审批门列表
    pub review_gates: Vec<ReviewGate>,
    /// 任务创建时间
    pub created_at: DateTime<Utc>,
}

/// Plan 模式权限快照的可序列化投影（Sprint 10 L3）
///
/// 与 `plan_mode_gate::PlanPermissionSnapshot` 一一对应，但不含
/// `CapabilityRef` 列表（`CapabilityRef` 语义上属于运行时视图，跨 restart
/// 无法原样复用：connector 句柄、placement 等必须由当前进程重新解析）。
///
/// Plan 阶段 enter 时，runtime 把 gate 返回的 `PlanPermissionSnapshot` 按
/// 公开字段投影成此结构体并写入 `V2IterationState::plan_mode_snapshot`，
/// 保证跨 restart 仍可知晓：哪些 capability 原本可用、哪些被剥离、哪些
/// 仍在 plan 窗口内保持可用。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PlanModeSnapshotData {
    /// 进入 plan 模式前的完整 capability ID 列表（顺序保留）
    pub original: Vec<String>,
    /// 被剥离的 capability ID 列表，附带 strip 原因文本
    ///
    /// 使用字符串化的 reason 保持与 `plan_mode_gate::StripReason`
    /// 的解耦（后者未派生 Serialize）。
    pub stripped: Vec<(String, String)>,
    /// plan 模式下依旧可用的 capability ID 列表
    pub kept: Vec<String>,
}

/// 单次迭代的完整状态记录（Autopilot V2）
///
/// 记录一次 9-Step 循环的所有执行细节
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V2IterationState {
    /// 迭代序号（从 1 开始）
    pub iteration_id: u32,
    /// 当前步骤
    pub step: AutopilotStep,
    /// 迭代开始时间
    pub start_time: DateTime<Utc>,
    /// 迭代结束时间（如果已完成）
    pub end_time: Option<DateTime<Utc>>,
    /// 状态哈希（用于 no-progress 检测）
    pub state_hash: String,
    /// 相对上次迭代的进展增量（0.0-1.0），None 表示无法计算
    pub progress_delta: Option<f64>,
    /// 本次迭代产生的执行结果
    pub execution_results: Vec<V2ExecutionResult>,
    /// 当前 5-phase 状态机所在阶段（Sprint 9 Wave 2 L1）
    ///
    /// 使用 `#[serde(default)]` 确保旧数据（缺失该字段的 JSON）反序列化时
    /// 回落到 `AutopilotPhase::Plan`，实现向后兼容。
    #[serde(default)]
    pub current_phase: AutopilotPhase,
    /// 累计进入 Fix 阶段的次数（受 `ExecutionPlan::max_fix_loops` 限制）
    #[serde(default)]
    pub fix_loop_count: u32,
    /// Plan 模式权限快照（Sprint 10 L3）
    ///
    /// 仅在 `current_phase == Plan` 期间为 `Some`，记录 `PlanModeGate`
    /// enter 时返回的只读权限快照投影。退出 Plan 阶段时清空。
    ///
    /// 使用 `#[serde(default)]` 确保旧数据（缺失该字段的 JSON）反序列化时
    /// 默认回落到 `None`，实现向后兼容。
    #[serde(default)]
    pub plan_mode_snapshot: Option<PlanModeSnapshotData>,
}

impl V2IterationState {
    /// 计算本次迭代的状态哈希
    ///
    /// 基于关键状态字段生成 SHA256 哈希，用于检测两次迭代之间是否有实质性进展
    pub fn compute_state_hash(&self) -> String {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();

        // 不哈希迭代序号，因为我们要检测的是状态变化，而非迭代次数变化

        // 哈希步骤
        hasher.update(format!("{:?}", self.step));

        // 哈希执行结果数量和状态
        hasher.update(self.execution_results.len().to_le_bytes());
        for result in &self.execution_results {
            hasher.update(result.execution_id.to_string());
            hasher.update(format!("{:?}", result.status));
        }

        // 返回十六进制字符串
        format!("{:x}", hasher.finalize())
    }

    /// 标记迭代完成
    pub fn mark_completed(&mut self) {
        self.end_time = Some(Utc::now());
        self.state_hash = self.compute_state_hash();
    }
}

/// 执行结果记录（Autopilot V2）
///
/// 记录单个 Capability 执行的结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V2ExecutionResult {
    /// 执行唯一标识
    pub execution_id: ExecutionId,
    /// 能力标识符（用于白名单验证）
    pub capability_id: String,
    /// 执行状态
    pub status: ExecutionStatus,
    /// 输出数据（可选）
    pub output: Option<serde_json::Value>,
    /// 错误信息（如果失败）
    pub error: Option<String>,
    /// 执行耗时（毫秒）
    pub duration_ms: u64,
    /// 追踪 ID（用于关联日志）
    pub trace_id: String,
}

/// Autopilot 9-Step 循环枚举
///
/// 定义 Autopilot 的标准执行步骤
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AutopilotStep {
    /// 步骤 1: 初始化（设置上下文、目标等）
    Initialize,
    /// 步骤 2: 规划（生成执行计划）
    Plan,
    /// 步骤 3: 执行（调用 Connector/Capability）
    Execute,
    /// 步骤 4: 审批（通过 Review Gate）
    Review,
    /// 步骤 5: 分析（分析执行结果）
    Analyze,
    /// 步骤 6: 决策（决定下一步行动：继续/重试/停止）
    Decide,
    /// 步骤 7: 更新（更新记忆、上下文等）
    Update,
    /// 步骤 8: 检查（检查目标完成度、预算等）
    Check,
    /// 步骤 9: 迭代（开始下一轮或结束）
    Iterate,
}

impl AutopilotStep {
    /// 获取下一步骤（循环）
    pub fn next(self) -> Self {
        match self {
            AutopilotStep::Initialize => AutopilotStep::Plan,
            AutopilotStep::Plan => AutopilotStep::Execute,
            AutopilotStep::Execute => AutopilotStep::Review,
            AutopilotStep::Review => AutopilotStep::Analyze,
            AutopilotStep::Analyze => AutopilotStep::Decide,
            AutopilotStep::Decide => AutopilotStep::Update,
            AutopilotStep::Update => AutopilotStep::Check,
            AutopilotStep::Check => AutopilotStep::Iterate,
            AutopilotStep::Iterate => AutopilotStep::Initialize, // 循环回初始化
        }
    }

    /// 判断是否为关键步骤（需要审批或高风险）
    pub fn is_critical(self) -> bool {
        matches!(self, AutopilotStep::Execute | AutopilotStep::Review)
    }
}

// ---------------------------------------------------------------------------
// Autopilot 5-Phase 显式状态机（Sprint 9 L2）
// ---------------------------------------------------------------------------

/// Autopilot 的 5 阶段显式状态机
///
/// 参考 oh-my-claudecode/skills/autopilot/SKILL.md 的 5-phase 定义，
/// 把原有隐式 9-step 流程抽象为显式可枚举的阶段序列：
///
/// ```text
///         ┌───────┐   ┌─────────┐   ┌────────┐   Pass   ┌──────┐
///   ────▶ │ Plan  │──▶│ Execute │──▶│ Verify │─────────▶│ Done │
///         └───────┘   └─────────┘   └────────┘          └──────┘
///                                       │ Fail
///                                       ▼
///                                   ┌───────┐  (max_fix_loops 超限)
///                                   │  Fix  │──────────────────┐
///                                   └───────┘                  │
///                                       │ Fix 完成             ▼
///                                       └──▶ Execute       ┌──────┐
///                                                          │ Done │
///                                                          └──────┘
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum AutopilotPhase {
    /// 规划阶段：分析目标，产出 ExecutionPlan
    #[default]
    Plan,
    /// 执行阶段：跑一轮 action（通过 ExecutionService 或 AgenticLoop）
    Execute,
    /// 校验阶段：基于 ExpectedOutcome 校验执行结果
    Verify,
    /// 修复阶段：Verify 失败后的修复迭代（受 max_fix_loops 限制）
    Fix,
    /// 终态：本轮 run 结束（成功或失败）
    Done,
}

/// Verify 阶段的裁决结果
///
/// 用于驱动 Verify -> (Done | Fix) 的状态转换
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerifyVerdict {
    /// 校验通过
    Pass,
    /// 校验失败（需要进入 Fix 阶段）
    Fail,
}

impl AutopilotPhase {
    /// 计算下一个阶段
    ///
    /// 状态转换表：
    ///
    /// | Current | Verdict       | Next             |
    /// |---------|---------------|------------------|
    /// | Plan    | N/A           | Execute          |
    /// | Execute | N/A           | Verify           |
    /// | Verify  | Some(Pass)    | Done             |
    /// | Verify  | Some(Fail)    | Fix              |
    /// | Verify  | None          | None (保持 Verify) |
    /// | Fix     | N/A           | Execute          |
    /// | Done    | N/A           | None (终态)        |
    ///
    /// 返回 `None` 表示：
    /// - 当前为终态（Done）
    /// - Verify 阶段等待 verdict
    pub fn next_phase(self, verdict: Option<VerifyVerdict>) -> Option<AutopilotPhase> {
        match (self, verdict) {
            (AutopilotPhase::Plan, _) => Some(AutopilotPhase::Execute),
            (AutopilotPhase::Execute, _) => Some(AutopilotPhase::Verify),
            (AutopilotPhase::Verify, Some(VerifyVerdict::Pass)) => Some(AutopilotPhase::Done),
            (AutopilotPhase::Verify, Some(VerifyVerdict::Fail)) => Some(AutopilotPhase::Fix),
            (AutopilotPhase::Verify, None) => None,
            (AutopilotPhase::Fix, _) => Some(AutopilotPhase::Execute),
            (AutopilotPhase::Done, _) => None,
        }
    }

    /// 是否为终态
    pub fn is_terminal(self) -> bool {
        matches!(self, AutopilotPhase::Done)
    }
}

/// Autopilot Run 级别状态
///
/// 表示整个 Run 的当前状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AutopilotStatus {
    /// 已初始化，等待开始
    Initialized,
    /// 运行中（当前步骤）
    Running { current_step: AutopilotStep },
    /// 卡住（无进展）
    Stuck { reason: String },
    /// 等待审批
    AwaitingReview { gate: ReviewGate },
    /// 已完成（总迭代次数）
    Completed { iterations: u32 },
    /// 失败（错误信息）
    Failed { error: String },
}

impl AutopilotStatus {
    /// 转换为 ExecutionStatus
    ///
    /// 用于与现有 Execution 系统集成
    pub fn to_execution_status(&self) -> ExecutionStatus {
        match self {
            AutopilotStatus::Initialized => ExecutionStatus::Pending,
            AutopilotStatus::Running { .. } => ExecutionStatus::Running,
            AutopilotStatus::Stuck { .. } => ExecutionStatus::Running,
            AutopilotStatus::AwaitingReview { .. } => ExecutionStatus::WaitingReview,
            AutopilotStatus::Completed { .. } => ExecutionStatus::Completed,
            AutopilotStatus::Failed { .. } => ExecutionStatus::Failed,
        }
    }

    /// 从 ExecutionStatus 转换（尽力而为）
    pub fn from_execution_status(status: ExecutionStatus) -> Self {
        match status {
            ExecutionStatus::Pending => AutopilotStatus::Initialized,
            ExecutionStatus::Running => AutopilotStatus::Running {
                current_step: AutopilotStep::Initialize,
            },
            ExecutionStatus::WaitingReview => AutopilotStatus::AwaitingReview {
                gate: ReviewGate::BeforeExecution,
            },
            ExecutionStatus::WaitingApproval => AutopilotStatus::AwaitingReview {
                gate: ReviewGate::BeforeExecution,
            },
            ExecutionStatus::Completed => AutopilotStatus::Completed { iterations: 0 },
            ExecutionStatus::Failed => AutopilotStatus::Failed {
                error: "Unknown error".to_string(),
            },
            ExecutionStatus::Cancelled => AutopilotStatus::Failed {
                error: "Cancelled".to_string(),
            },
            ExecutionStatus::TimedOut => AutopilotStatus::Failed {
                error: "Timed out".to_string(),
            },
        }
    }
}

/// Autopilot Run 的完整状态
///
/// 记录一次 Autopilot Run 的所有状态信息，支持暂停/恢复、进展检测等
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutopilotRunState {
    /// Run 唯一标识（复用 ExecutionId）
    pub run_id: ExecutionId,
    /// 所属 Job ID
    pub job_id: String,
    /// 当前迭代次数（从 0 开始）
    pub current_iteration: u32,
    /// 所有迭代历史
    pub iterations: Vec<V2IterationState>,
    /// 当前状态
    pub status: AutopilotStatus,
    /// 连续卡住次数（用于触发终止）
    pub stuck_count: u32,
    /// 上次状态哈希（用于 no-progress 检测）
    pub last_state_hash: Option<String>,
    /// 元数据（自定义字段）
    pub metadata: HashMap<String, serde_json::Value>,
    /// 当前 5-phase 状态机所在阶段（Sprint 9 L2）
    ///
    /// 使用 `#[serde(default)]` 保证旧数据（缺失该字段的 JSON）反序列化时
    /// 默认回落到 `AutopilotPhase::Plan`，实现向后兼容。
    #[serde(default)]
    pub current_phase: AutopilotPhase,
    /// 当前进入 Fix 阶段累计迭代次数（用于 `max_fix_loops` 限制）
    #[serde(default)]
    pub fix_loop_count: u32,
    /// Run 创建时间
    pub created_at: DateTime<Utc>,
    /// 最后更新时间
    pub updated_at: DateTime<Utc>,
}

impl AutopilotRunState {
    /// 创建新的 Run 状态
    ///
    /// # 示例
    ///
    /// ```
    /// use cyberclaw_control_plane::autopilot_types::*;
    /// use cyberclaw_core::ids::ExecutionId;
    ///
    /// let run_id = ExecutionId::new();
    /// let job_id = "job-001".to_string();
    ///
    /// let run = AutopilotRunState::new(run_id, job_id);
    /// assert_eq!(run.current_iteration, 0);
    /// assert!(run.iterations.is_empty());
    /// ```
    pub fn new(run_id: ExecutionId, job_id: String) -> Self {
        let now = Utc::now();
        Self {
            run_id,
            job_id,
            current_iteration: 0,
            iterations: Vec::new(),
            status: AutopilotStatus::Initialized,
            stuck_count: 0,
            last_state_hash: None,
            metadata: HashMap::new(),
            current_phase: AutopilotPhase::default(),
            fix_loop_count: 0,
            created_at: now,
            updated_at: now,
        }
    }

    /// 开始新的迭代
    ///
    /// 创建新的 V2IterationState 并添加到历史中
    pub fn start_iteration(&mut self, step: AutopilotStep) {
        self.current_iteration += 1;

        let iteration = V2IterationState {
            iteration_id: self.current_iteration,
            step,
            start_time: Utc::now(),
            end_time: None,
            state_hash: String::new(),
            progress_delta: None,
            execution_results: Vec::new(),
            current_phase: AutopilotPhase::Plan,
            fix_loop_count: 0,
            plan_mode_snapshot: None,
        };

        self.iterations.push(iteration);
        self.status = AutopilotStatus::Running { current_step: step };
        self.updated_at = Utc::now();
    }

    /// 完成当前迭代
    ///
    /// 计算状态哈希，检测 no-progress
    pub fn complete_iteration(&mut self) -> bool {
        if let Some(iteration) = self.iterations.last_mut() {
            iteration.mark_completed();

            // 计算进展
            let new_hash = iteration.state_hash.clone();
            let has_progress = match &self.last_state_hash {
                Some(old_hash) if old_hash == &new_hash => {
                    // 状态哈希未变化，无进展
                    self.stuck_count += 1;
                    iteration.progress_delta = Some(0.0);
                    false
                }
                _ => {
                    // 状态哈希变化，有进展
                    self.stuck_count = 0;
                    iteration.progress_delta = Some(1.0);
                    true
                }
            };

            self.last_state_hash = Some(new_hash);
            self.updated_at = Utc::now();

            has_progress
        } else {
            false
        }
    }

    /// 添加执行结果到当前迭代
    pub fn add_execution_result(&mut self, result: V2ExecutionResult) {
        if let Some(iteration) = self.iterations.last_mut() {
            iteration.execution_results.push(result);
            self.updated_at = Utc::now();
        }
    }

    /// 转换步骤
    pub fn transition_step(&mut self, new_step: AutopilotStep) {
        if let Some(iteration) = self.iterations.last_mut() {
            iteration.step = new_step;
            self.status = AutopilotStatus::Running {
                current_step: new_step,
            };
            self.updated_at = Utc::now();
        }
    }

    /// 标记为卡住
    pub fn mark_stuck(&mut self, reason: String) {
        self.status = AutopilotStatus::Stuck { reason };
        self.updated_at = Utc::now();
    }

    /// 标记为等待审批
    pub fn mark_awaiting_review(&mut self, gate: ReviewGate) {
        self.status = AutopilotStatus::AwaitingReview { gate };
        self.updated_at = Utc::now();
    }

    /// 标记为已完成
    pub fn mark_completed(&mut self) {
        self.status = AutopilotStatus::Completed {
            iterations: self.current_iteration,
        };
        self.updated_at = Utc::now();
    }

    /// 标记为失败
    pub fn mark_failed(&mut self, error: String) {
        self.status = AutopilotStatus::Failed { error };
        self.updated_at = Utc::now();
    }

    /// 检查是否应该停止（基于卡住次数）
    pub fn should_stop_due_to_stuck(&self, max_stuck: u32) -> bool {
        self.stuck_count >= max_stuck
    }

    /// 获取当前迭代的可变引用
    pub fn current_iteration_mut(&mut self) -> Option<&mut V2IterationState> {
        self.iterations.last_mut()
    }

    /// 获取当前迭代的不可变引用
    pub fn current_iteration(&self) -> Option<&V2IterationState> {
        self.iterations.last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_autopilot_step_next() {
        let step = AutopilotStep::Initialize;
        assert_eq!(step.next(), AutopilotStep::Plan);
        assert_eq!(AutopilotStep::Iterate.next(), AutopilotStep::Initialize);
    }

    #[test]
    fn test_autopilot_step_is_critical() {
        assert!(AutopilotStep::Execute.is_critical());
        assert!(AutopilotStep::Review.is_critical());
        assert!(!AutopilotStep::Plan.is_critical());
    }

    #[test]
    fn test_autopilot_status_conversion() {
        let status = AutopilotStatus::Running {
            current_step: AutopilotStep::Execute,
        };
        assert_eq!(status.to_execution_status(), ExecutionStatus::Running);

        let status = AutopilotStatus::AwaitingReview {
            gate: ReviewGate::BeforeExecution,
        };
        assert_eq!(status.to_execution_status(), ExecutionStatus::WaitingReview);

        let exec_status = ExecutionStatus::Completed;
        let auto_status = AutopilotStatus::from_execution_status(exec_status);
        assert!(matches!(auto_status, AutopilotStatus::Completed { .. }));
    }

    #[test]
    fn test_autopilot_run_state_new() {
        let run_id = ExecutionId::new();
        let job_id = "test-job".to_string();
        let run = AutopilotRunState::new(run_id.clone(), job_id.clone());

        assert_eq!(run.run_id, run_id);
        assert_eq!(run.job_id, job_id);
        assert_eq!(run.current_iteration, 0);
        assert_eq!(run.status, AutopilotStatus::Initialized);
        assert_eq!(run.stuck_count, 0);
    }

    #[test]
    fn test_start_and_complete_iteration() {
        let run_id = ExecutionId::new();
        let mut run = AutopilotRunState::new(run_id, "test-job".to_string());

        // 开始第一次迭代
        run.start_iteration(AutopilotStep::Initialize);
        assert_eq!(run.current_iteration, 1);
        assert_eq!(run.iterations.len(), 1);
        assert!(matches!(
            run.status,
            AutopilotStatus::Running {
                current_step: AutopilotStep::Initialize
            }
        ));

        // 完成第一次迭代（第一次迭代必然有进展）
        let has_progress = run.complete_iteration();
        assert!(has_progress);
        assert_eq!(run.stuck_count, 0);

        // 开始第二次迭代，但状态不变
        run.start_iteration(AutopilotStep::Initialize);
        let has_progress = run.complete_iteration();
        assert!(!has_progress); // 状态哈希相同，无进展
        assert_eq!(run.stuck_count, 1);
    }

    #[test]
    fn test_iteration_state_hash_stability() {
        let execution_id = ExecutionId::new();
        let result = V2ExecutionResult {
            execution_id: execution_id.clone(),
            capability_id: "fs:read".to_string(),
            status: ExecutionStatus::Completed,
            output: None,
            error: None,
            duration_ms: 100,
            trace_id: "trace-001".to_string(),
        };

        let iteration1 = V2IterationState {
            iteration_id: 1,
            step: AutopilotStep::Execute,
            start_time: Utc::now(),
            end_time: None,
            state_hash: String::new(),
            progress_delta: None,
            execution_results: vec![result.clone()],
            current_phase: AutopilotPhase::Plan,
            fix_loop_count: 0,
            plan_mode_snapshot: None,
        };

        let mut iteration2 = V2IterationState {
            iteration_id: 1,
            step: AutopilotStep::Execute,
            start_time: Utc::now(),
            end_time: None,
            state_hash: String::new(),
            progress_delta: None,
            execution_results: vec![result],
            current_phase: AutopilotPhase::Plan,
            fix_loop_count: 0,
            plan_mode_snapshot: None,
        };

        let hash1 = iteration1.compute_state_hash();
        let hash2 = iteration2.compute_state_hash();

        // 相同内容应产生相同哈希
        assert_eq!(hash1, hash2);
        assert!(!hash1.is_empty());

        // 修改步骤后哈希应改变（验证哈希对状态变化敏感）
        iteration2.step = AutopilotStep::Review;
        let hash3 = iteration2.compute_state_hash();
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_serialization() {
        let run_id = ExecutionId::new();
        let mut run = AutopilotRunState::new(run_id, "test-job".to_string());
        run.start_iteration(AutopilotStep::Initialize);

        // 序列化
        let json = serde_json::to_string(&run).expect("Failed to serialize");
        assert!(!json.is_empty());

        // 反序列化
        let deserialized: AutopilotRunState =
            serde_json::from_str(&json).expect("Failed to deserialize");
        assert_eq!(deserialized.run_id, run.run_id);
        assert_eq!(deserialized.job_id, run.job_id);
        assert_eq!(deserialized.current_iteration, run.current_iteration);
    }

    #[test]
    fn test_should_stop_due_to_stuck() {
        let run_id = ExecutionId::new();
        let mut run = AutopilotRunState::new(run_id, "test-job".to_string());

        assert!(!run.should_stop_due_to_stuck(3));

        run.stuck_count = 3;
        assert!(run.should_stop_due_to_stuck(3));
    }

    #[test]
    fn test_add_execution_result() {
        let run_id = ExecutionId::new();
        let mut run = AutopilotRunState::new(run_id, "test-job".to_string());
        run.start_iteration(AutopilotStep::Execute);

        let result = V2ExecutionResult {
            execution_id: ExecutionId::new(),
            capability_id: "code:ast".to_string(),
            status: ExecutionStatus::Completed,
            output: Some(serde_json::json!({"key": "value"})),
            error: None,
            duration_ms: 200,
            trace_id: "trace-123".to_string(),
        };

        run.add_execution_result(result);

        let iteration = run.current_iteration().expect("No current iteration");
        assert_eq!(iteration.execution_results.len(), 1);
    }
}
