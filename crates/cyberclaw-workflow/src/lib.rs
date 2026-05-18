//! cyberclaw-workflow: 工作流引擎与编排能力
//!
//! 提供工作流定义、执行、监控和状态持久化功能。

pub mod daily_digest_trigger;
mod engine;
pub mod osv_scan_trigger;
pub mod trigger;

// Re-export public API
pub use daily_digest_trigger::{
    agent_id_from_workflow_id, fire_daily_digest, matched_daily_digest_workflows,
    register_daily_digest_cron, DailyDigestRunner, DAILY_DIGEST_CRON,
};
pub use engine::{
    RetryPolicy, StepType, WorkflowContext, WorkflowDefinition, WorkflowEngine, WorkflowEngineApi,
    WorkflowError, WorkflowEvent, WorkflowInstance, WorkflowStatus, WorkflowStep,
};
pub use osv_scan_trigger::{
    agent_id_from_osv_workflow_id, fire_osv_scan_fanout, matched_osv_scan_workflows,
    register_osv_scan_cron, DefaultOsvScanRunner, OsvScanError, OsvScanOutcome, OsvScanRunner,
    OSV_SCAN_CRON,
};
pub use trigger::{
    TriggerError, TriggerEvent, TriggerRegistration, TriggerRegistry, WorkflowTrigger,
};
