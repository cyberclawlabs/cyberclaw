//! Multi-Agent Handoff 模块 — Agent→Agent 会话控制权转移
//!
//! Sprint 21 (OQ answers locked: 1A/2B/3C/4C/5B/6A) 引入的 runtime
//! conversational handoff 原语。**与既有抽象的边界**:
//!
//! | 已有原语 | 位置 | 语义 |
//! |---|---|---|
//! | `SubAgentOrchestrator` | agent-runtime | 父 agent 把子 agent **当工具调**，拿结果继续跑 |
//! | `StageHandoffDocument` | control-plane | pipeline 线性 stage 间的 paseo-style markdown briefing |
//! | **本模块 `HandoffRequest`** | core | **会话中**运行时，agent_A 把对话控制权**永久**交给 agent_B |
//!
//! 关键不变式（design.md §2.1）:
//! - `from_agent_id != to_agent_id`（禁止自我 handoff）
//! - `briefing_text` ≤ 2KB（保护 agent_B 的 context window）
//! - `reason` ≤ 200 字符（单行提示）
//! - `ttl_seconds` 30..=3600（避免 Ask-path 无限挂起 / 避免 <30s 的毛刺）
//! - `context_artifacts` 可空（最小 handoff = 只有 reason + briefing）
//!
//! 对齐参考（design.md §"Referenced External Patterns"）:
//! - Claude Code `SubAgentTool`: schema field naming
//! - CrewAI `@delegate` decorator: push-model 语义
//! - S15 `ClarifyRequest`: 本模块结构完全模仿，方便团队认知迁移

use crate::artifact::ArtifactRef;
use crate::ids::{AgentId, ExecutionId, HandoffId, SessionId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// HandoffStatus
// ---------------------------------------------------------------------------

/// Handoff 生命周期状态机：
///
/// ```text
///   Initiated → Authorized → Accepted        (happy path)
///             ↘ Declined                     (policy Deny 或 user Reject)
///             ↘ Expired                      (TTL 耗尽未 accept)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStatus {
    /// 已创建 + 入队，等待 PolicyEngine 决策
    Initiated,
    /// 授权通过（Allow 或 Ask→approve），等待 executor 完成 session 切换
    Authorized,
    /// 会话切换完成，agent_B 已接管
    Accepted,
    /// 被拒绝（policy Deny 或 Ask→reject）
    Declined,
    /// TTL 到期未 accept
    Expired,
}

// ---------------------------------------------------------------------------
// HandoffRequest
// ---------------------------------------------------------------------------

/// 完整的 handoff 请求对象。由 agent_A 通过 `agent.handoff` capability 提交，
/// 经 `HandoffQueue` 存储、`PolicyEngine` 授权、`execution_service.complete_handoff`
/// 切换 session。
///
/// # 字段职责
///
/// | 字段 | 作用 |
/// |---|---|
/// | `handoff_id` | 平台生成，`Uuid::new_v4()` 对应 HandoffId |
/// | `from_agent_id` / `to_agent_id` | 从 / 到的 AgentId；两者必须不同 |
/// | `conversation_id` | 本次 handoff 发生在的会话（会被 set_active_agent） |
/// | `reason` | 一行人读描述（"B 更专长后端" 之类），UI 顶部展示 |
/// | `briefing_text` | ≤ 2KB 给 agent_B 第一轮 LLM 调用的 `<handoff_briefing>` 内容 |
/// | `context_artifacts` | 可选附加引用（工作区文件、memory 引用等） |
/// | `status` | 见 [`HandoffStatus`] |
/// | `initiated_at` | 构造时间（UTC） |
/// | `decided_at` | 状态变为 Authorized/Declined/Accepted/Expired 的时间 |
/// | `ttl_seconds` | 30..=3600，默认 300s，Initiated/Authorized 态超时后 Expired |
/// | `initiated_by_execution` | 追溯初始 agent_A 的 execution（trace 用） |
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffRequest {
    pub handoff_id: HandoffId,
    pub from_agent_id: AgentId,
    pub to_agent_id: AgentId,
    pub conversation_id: String,
    pub reason: String,
    pub briefing_text: String,
    #[serde(default)]
    pub context_artifacts: Vec<ArtifactRef>,
    pub status: HandoffStatus,
    pub initiated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<DateTime<Utc>>,
    pub ttl_seconds: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiated_by_execution: Option<ExecutionId>,
    /// Persisted SessionId allocated when this handoff was first accepted.
    ///
    /// `None` until `complete_handoff` succeeds once. Subsequent idempotent
    /// calls return the same `SessionId` rather than allocating a fresh one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_session_id: Option<SessionId>,
}

/// Constant bounds exposed so tests + validate() + call sites share one source of truth.
pub const HANDOFF_REASON_MAX: usize = 200;
pub const HANDOFF_BRIEFING_MAX_BYTES: usize = 2048;
pub const HANDOFF_TTL_MIN_SECS: u32 = 30;
pub const HANDOFF_TTL_MAX_SECS: u32 = 3600;
pub const HANDOFF_TTL_DEFAULT_SECS: u32 = 300;
pub const HANDOFF_CONTEXT_ARTIFACTS_MAX: usize = 10;

impl HandoffRequest {
    /// Fresh-construct a handoff. Status is initialized to `Initiated`;
    /// `handoff_id` must be supplied by the caller (server-side `Uuid::new_v4()`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        handoff_id: HandoffId,
        from_agent_id: AgentId,
        to_agent_id: AgentId,
        conversation_id: String,
        reason: String,
        briefing_text: String,
        context_artifacts: Vec<ArtifactRef>,
        initiated_by_execution: Option<ExecutionId>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            handoff_id,
            from_agent_id,
            to_agent_id,
            conversation_id,
            reason,
            briefing_text,
            context_artifacts,
            status: HandoffStatus::Initiated,
            initiated_at: now,
            decided_at: None,
            ttl_seconds: HANDOFF_TTL_DEFAULT_SECS,
            initiated_by_execution,
            target_session_id: None,
        }
    }

    /// Validate the invariants defined in §"关键不变式" of this module.
    ///
    /// Enforces:
    /// - `from_agent_id != to_agent_id` → [`HandoffError::SelfHandoff`]
    /// - `reason`: non-empty after trim, ≤ [`HANDOFF_REASON_MAX`] chars, single-line (no `\n`)
    /// - `briefing_text`: ≤ [`HANDOFF_BRIEFING_MAX_BYTES`] bytes (byte count tracks LLM budget)
    /// - `conversation_id`: non-empty after trim
    /// - `ttl_seconds`: [`HANDOFF_TTL_MIN_SECS`]..=[`HANDOFF_TTL_MAX_SECS`]
    /// - `context_artifacts.len()`: ≤ [`HANDOFF_CONTEXT_ARTIFACTS_MAX`]
    pub fn validate(&self) -> Result<(), HandoffError> {
        // (c) self-handoff — dedicated error variant so callers can special-case
        if self.from_agent_id == self.to_agent_id {
            return Err(HandoffError::SelfHandoff);
        }

        // (a) reason — trim-check + char count + no embedded newlines (single-line UI chip)
        let trimmed_reason = self.reason.trim();
        if trimmed_reason.is_empty() {
            return Err(HandoffError::Validation(
                "reason must not be empty".to_string(),
            ));
        }
        let reason_chars = self.reason.chars().count();
        if reason_chars > HANDOFF_REASON_MAX {
            return Err(HandoffError::Validation(format!(
                "reason exceeds {} chars (got {})",
                HANDOFF_REASON_MAX, reason_chars
            )));
        }
        if self.reason.contains('\n') {
            return Err(HandoffError::Validation(
                "reason must be single-line (no embedded newline)".to_string(),
            ));
        }

        // (b) briefing_text — byte count (LLM token budget is byte-proportional), empty OK
        let briefing_bytes = self.briefing_text.len();
        if briefing_bytes > HANDOFF_BRIEFING_MAX_BYTES {
            return Err(HandoffError::Validation(format!(
                "briefing_text exceeds {} bytes (got {})",
                HANDOFF_BRIEFING_MAX_BYTES, briefing_bytes
            )));
        }

        // (d) conversation_id — non-empty after trim (routing key, silent-break guard)
        if self.conversation_id.trim().is_empty() {
            return Err(HandoffError::Validation(
                "conversation_id must not be empty".to_string(),
            ));
        }

        // (e) ttl_seconds — bounded range (u32 so lower-bound is the meaningful check)
        if self.ttl_seconds < HANDOFF_TTL_MIN_SECS {
            return Err(HandoffError::Validation(format!(
                "ttl_seconds below minimum {} (got {})",
                HANDOFF_TTL_MIN_SECS, self.ttl_seconds
            )));
        }
        if self.ttl_seconds > HANDOFF_TTL_MAX_SECS {
            return Err(HandoffError::Validation(format!(
                "ttl_seconds above maximum {} (got {})",
                HANDOFF_TTL_MAX_SECS, self.ttl_seconds
            )));
        }

        // (f) context_artifacts — hard count cap (per-byte cap enforced by sanitizer + LLM budget downstream)
        if self.context_artifacts.len() > HANDOFF_CONTEXT_ARTIFACTS_MAX {
            return Err(HandoffError::Validation(format!(
                "context_artifacts count exceeds {} (got {})",
                HANDOFF_CONTEXT_ARTIFACTS_MAX,
                self.context_artifacts.len()
            )));
        }

        Ok(())
    }

    /// Transition status to `Authorized`. No-op if already Authorized/Accepted.
    /// Sets `decided_at = now`.
    pub fn mark_authorized(&mut self, now: DateTime<Utc>) {
        if matches!(self.status, HandoffStatus::Initiated) {
            self.status = HandoffStatus::Authorized;
            self.decided_at = Some(now);
        }
    }

    /// Transition status to `Accepted`. Called by executor after session switch.
    /// Caller is responsible for ensuring status is currently Authorized.
    pub fn mark_accepted(&mut self, now: DateTime<Utc>) {
        self.status = HandoffStatus::Accepted;
        self.decided_at = Some(now);
    }

    /// Transition status to `Declined`. Valid from Initiated or Authorized.
    pub fn mark_declined(&mut self, now: DateTime<Utc>) {
        self.status = HandoffStatus::Declined;
        self.decided_at = Some(now);
    }

    /// Transition status to `Expired`. Called by TTL sweeper.
    pub fn mark_expired(&mut self, now: DateTime<Utc>) {
        self.status = HandoffStatus::Expired;
        self.decided_at = Some(now);
    }

    /// Convenience: is this handoff still actionable (not terminal)?
    pub fn is_pending(&self) -> bool {
        matches!(
            self.status,
            HandoffStatus::Initiated | HandoffStatus::Authorized
        )
    }
}

// ---------------------------------------------------------------------------
// HandoffError
// ---------------------------------------------------------------------------

/// Errors emitted along the handoff lifecycle.
///
/// `Validation` is the catch-all for `HandoffRequest::validate()` failures;
/// other variants are raised by the connector / executor / queue layers.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum HandoffError {
    /// Structural invariant violation (see `validate()`).
    #[error("handoff validation failed: {0}")]
    Validation(String),
    /// `to_agent_id` not registered with the platform.
    #[error("target agent not found: {0}")]
    TargetNotFound(String),
    /// `from_agent_id == to_agent_id`.
    #[error("self-handoff is not allowed")]
    SelfHandoff,
    /// `PolicyEngine` returned `Deny`, or reviewer rejected an `Ask` handoff.
    #[error("governance denied: {0}")]
    Denied(String),
    /// TTL elapsed before an Authorized handoff could transition to Accepted.
    #[error("handoff request expired")]
    Expired,
    /// `HandoffQueue` lookup miss.
    #[error("handoff not found")]
    NotFound,
    /// Catch-all for storage / wiring errors.
    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Tests (non-validate-dependent smoke)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{ArtifactKind, ArtifactRef};
    use crate::ids::{AgentId, ArtifactId, HandoffId};

    fn sample_request() -> HandoffRequest {
        HandoffRequest::new(
            HandoffId::from_string("ho_01".to_string()).unwrap(),
            AgentId::from_string("agent_a".to_string()).unwrap(),
            AgentId::from_string("agent_b".to_string()).unwrap(),
            "conv_42".to_string(),
            "B is better at backend".to_string(),
            "Context: user asked for schema review; PRD at /tmp/prd.md".to_string(),
            vec![],
            None,
            Utc::now(),
        )
    }

    #[test]
    fn new_handoff_starts_initiated() {
        let r = sample_request();
        assert_eq!(r.status, HandoffStatus::Initiated);
        assert!(r.decided_at.is_none());
        assert_eq!(r.ttl_seconds, HANDOFF_TTL_DEFAULT_SECS);
        assert!(r.is_pending());
    }

    #[test]
    fn state_machine_transitions() {
        let mut r = sample_request();
        let t0 = Utc::now();
        r.mark_authorized(t0);
        assert_eq!(r.status, HandoffStatus::Authorized);
        assert_eq!(r.decided_at, Some(t0));
        assert!(r.is_pending());

        r.mark_accepted(t0);
        assert_eq!(r.status, HandoffStatus::Accepted);
        assert!(!r.is_pending());
    }

    #[test]
    fn mark_authorized_noop_if_already_authorized() {
        let mut r = sample_request();
        let t0 = Utc::now();
        r.mark_authorized(t0);
        let later = t0 + chrono::Duration::seconds(10);
        r.mark_authorized(later);
        // decided_at should still be t0 — second call is no-op
        assert_eq!(r.decided_at, Some(t0));
    }

    #[test]
    fn serde_roundtrip_empty_artifacts() {
        let r = sample_request();
        let json = serde_json::to_string(&r).unwrap();
        let back: HandoffRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.handoff_id.to_string(), r.handoff_id.to_string());
        assert_eq!(back.status, HandoffStatus::Initiated);
        assert!(back.context_artifacts.is_empty());
    }

    #[test]
    fn serde_roundtrip_with_artifacts() {
        let mut r = sample_request();
        r.context_artifacts.push(ArtifactRef {
            id: ArtifactId::from_string("art_1".to_string()).unwrap(),
            kind: ArtifactKind::Summary,
            title: "PRD summary".to_string(),
            uri: "workspace://docs/prd.md".to_string(),
            content_type: "text/markdown".to_string(),
            parent_artifact_id: None,
            base_version: None,
        });
        let json = serde_json::to_string(&r).unwrap();
        let back: HandoffRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.context_artifacts.len(), 1);
        assert_eq!(back.context_artifacts[0].title, "PRD summary");
    }

    #[test]
    fn status_serializes_snake_case() {
        let s = serde_json::to_string(&HandoffStatus::Initiated).unwrap();
        assert_eq!(s, "\"initiated\"");
        let s = serde_json::to_string(&HandoffStatus::Authorized).unwrap();
        assert_eq!(s, "\"authorized\"");
    }

    #[test]
    fn error_displays_correctly() {
        let e = HandoffError::SelfHandoff;
        assert_eq!(e.to_string(), "self-handoff is not allowed");
        let e = HandoffError::Validation("reason too long".to_string());
        assert_eq!(e.to_string(), "handoff validation failed: reason too long");
    }

    // -----------------------------------------------------------------------
    // validate() boundary tests — covers invariants (a) through (f)
    // -----------------------------------------------------------------------

    #[test]
    fn validate_ok_on_minimal_request() {
        let r = sample_request();
        assert!(r.validate().is_ok());
    }

    #[test]
    fn validate_rejects_self_handoff() {
        let mut r = sample_request();
        r.to_agent_id = r.from_agent_id.clone();
        assert_eq!(r.validate(), Err(HandoffError::SelfHandoff));
    }

    #[test]
    fn validate_rejects_empty_reason() {
        let mut r = sample_request();
        r.reason = "   ".to_string(); // whitespace-only also rejected
        let err = r.validate().unwrap_err();
        assert!(
            matches!(err, HandoffError::Validation(ref m) if m.contains("reason must not be empty"))
        );
    }

    #[test]
    fn validate_rejects_too_long_reason() {
        let mut r = sample_request();
        r.reason = "x".repeat(HANDOFF_REASON_MAX + 1);
        let err = r.validate().unwrap_err();
        assert!(matches!(err, HandoffError::Validation(ref m) if m.contains("exceeds 200 chars")));
    }

    #[test]
    fn validate_rejects_reason_with_newline() {
        let mut r = sample_request();
        r.reason = "line one\nline two".to_string();
        let err = r.validate().unwrap_err();
        assert!(matches!(err, HandoffError::Validation(ref m) if m.contains("single-line")));
    }

    #[test]
    fn validate_rejects_oversized_briefing() {
        let mut r = sample_request();
        r.briefing_text = "x".repeat(HANDOFF_BRIEFING_MAX_BYTES + 1);
        let err = r.validate().unwrap_err();
        assert!(
            matches!(err, HandoffError::Validation(ref m) if m.contains("briefing_text exceeds"))
        );
    }

    #[test]
    fn validate_accepts_empty_briefing() {
        // Minimal handoff: reason only, no briefing → should pass
        let mut r = sample_request();
        r.briefing_text = String::new();
        assert!(r.validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_conversation_id() {
        let mut r = sample_request();
        r.conversation_id = "   ".to_string();
        let err = r.validate().unwrap_err();
        assert!(matches!(err, HandoffError::Validation(ref m) if m.contains("conversation_id")));
    }

    #[test]
    fn validate_rejects_ttl_too_low() {
        let mut r = sample_request();
        r.ttl_seconds = HANDOFF_TTL_MIN_SECS - 1;
        let err = r.validate().unwrap_err();
        assert!(matches!(err, HandoffError::Validation(ref m) if m.contains("below minimum")));
    }

    #[test]
    fn validate_rejects_ttl_too_high() {
        let mut r = sample_request();
        r.ttl_seconds = HANDOFF_TTL_MAX_SECS + 1;
        let err = r.validate().unwrap_err();
        assert!(matches!(err, HandoffError::Validation(ref m) if m.contains("above maximum")));
    }

    #[test]
    fn validate_rejects_too_many_artifacts() {
        let mut r = sample_request();
        for i in 0..(HANDOFF_CONTEXT_ARTIFACTS_MAX + 1) {
            r.context_artifacts.push(ArtifactRef {
                id: ArtifactId::from_string(format!("art_{}", i)).unwrap(),
                kind: ArtifactKind::Log,
                title: format!("art {}", i),
                uri: "workspace://x".to_string(),
                content_type: "text/plain".to_string(),
                parent_artifact_id: None,
                base_version: None,
            });
        }
        let err = r.validate().unwrap_err();
        assert!(
            matches!(err, HandoffError::Validation(ref m) if m.contains("context_artifacts count exceeds"))
        );
    }

    #[test]
    fn validate_accepts_max_valid_artifacts() {
        // Exactly at cap — should pass
        let mut r = sample_request();
        for i in 0..HANDOFF_CONTEXT_ARTIFACTS_MAX {
            r.context_artifacts.push(ArtifactRef {
                id: ArtifactId::from_string(format!("art_{}", i)).unwrap(),
                kind: ArtifactKind::Log,
                title: format!("art {}", i),
                uri: "workspace://x".to_string(),
                content_type: "text/plain".to_string(),
                parent_artifact_id: None,
                base_version: None,
            });
        }
        assert!(r.validate().is_ok());
    }
}
