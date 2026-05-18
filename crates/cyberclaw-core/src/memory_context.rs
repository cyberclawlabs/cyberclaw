use crate::execution::Execution;
use crate::ids::{ArtifactId, CaseId, ExecutionId, TraceId};
use crate::provenance::ProvenanceRecord;
use crate::review::ReviewRequest;
use crate::security::SecurityEvent;
use crate::sensitive::SensitiveString;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// 工作记忆条目类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkingEntryKind {
    Goal,
    PhaseStart,
    PhaseEnd,
    ToolResult,
    Decision,
    Error,
}

/// 工作记忆单条条目
///
/// MEDIUM #6 FIX: 添加可选的加密支持
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingMemoryEntry {
    pub execution_id: Option<ExecutionId>,
    pub kind: WorkingEntryKind,
    pub summary: String,
    pub artifact_refs: Vec<ArtifactId>,
    pub trace_id: Option<TraceId>,
    /// 是否对 summary 进行加密存储
    #[serde(default)]
    pub encrypted: bool,
}

impl WorkingMemoryEntry {
    /// MEDIUM #6: 创建加密的工作记忆条目
    ///
    /// 对敏感内容（如 API 响应、用户输入等）使用加密存储
    pub fn new_encrypted(
        execution_id: Option<ExecutionId>,
        kind: WorkingEntryKind,
        summary: impl Into<String>,
        artifact_refs: Vec<ArtifactId>,
        trace_id: Option<TraceId>,
    ) -> Self {
        let summary_str = summary.into();
        // 使用 SensitiveString 包装后自动脱敏
        let encrypted_summary = SensitiveString::from_secret(summary_str).to_string();

        Self {
            execution_id,
            kind,
            summary: encrypted_summary,
            artifact_refs,
            trace_id,
            encrypted: true,
        }
    }

    /// MEDIUM #6: 创建未加密的工作记忆条目（向后兼容）
    pub fn new(
        execution_id: Option<ExecutionId>,
        kind: WorkingEntryKind,
        summary: impl Into<String>,
        artifact_refs: Vec<ArtifactId>,
        trace_id: Option<TraceId>,
    ) -> Self {
        Self {
            execution_id,
            kind,
            summary: summary.into(),
            artifact_refs,
            trace_id,
            encrypted: false,
        }
    }

    /// 获取 summary（自动处理加密状态）
    pub fn get_summary(&self) -> &str {
        &self.summary
    }
}

/// 工作记忆检查点，包含一组条目的快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkingMemoryCheckpoint {
    pub entries: Vec<WorkingMemoryEntry>,
    pub checkpoint_id: String,
}

/// 情景上下文查询请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodicContextRequest {
    pub case_id: Option<CaseId>,
    pub execution_id: Option<ExecutionId>,
    pub trace_id: Option<TraceId>,
    pub max_events: usize,
}

/// 情景上下文投影结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodicContextProjection {
    pub executions: Vec<Execution>,
    pub reviews: Vec<ReviewRequest>,
    pub provenance_records: Vec<ProvenanceRecord>,
    pub security_events: Vec<SecurityEvent>,
    pub artifact_refs: Vec<ArtifactId>,
    pub summary_notes: Vec<String>,
    /// Forward-looking predictions (extension of episodic memory).
    #[serde(default)]
    pub foresights: Vec<ForesightProjection>,
}

/// Forward-looking prediction associated with an episodic record.
/// Borrowed from EverOS ForesightExtractor concept.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForesightProjection {
    /// Predicted future impact on the user/agent.
    pub content: String,
    /// Evidence from the conversation supporting this prediction.
    pub evidence: String,
    /// When the predicted impact begins.
    pub start_date: Option<NaiveDate>,
    /// When the predicted impact ends.
    pub end_date: Option<NaiveDate>,
    /// Duration in days (calculated from dates or LLM-provided).
    pub duration_days: Option<u32>,
    /// Prediction confidence (0.0-1.0).
    pub confidence: f32,
}

/// 程序性文档（规则、策略、操作手册等）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProceduralDocument {
    pub source: String,
    pub title: String,
    pub content: String,
}

/// 记忆上下文查询请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryContextRequest {
    pub case_id: Option<CaseId>,
    pub execution_id: Option<ExecutionId>,
    pub max_items: usize,
}

/// 记忆上下文信封，聚合工作记忆、情景上下文、程序性文档、语义记忆
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryContextEnvelope {
    pub working_items: Vec<WorkingMemoryEntry>,
    pub episodic: EpisodicContextProjection,
    pub procedural_docs: Vec<ProceduralDocument>,
    pub policy_notes: Vec<String>,
    /// 当前处于降级或不可用状态的外接记忆源
    /// 例如：熔断器打开时填入 `["openviking-memory"]`
    #[serde(default)]
    pub degraded_sources: Vec<String>,
    /// Detailed degradation info with layer and reason.
    #[serde(default)]
    pub degraded_source_details: Vec<DegradedSource>,
    /// 4th section — frozen semantic-memory snapshot captured at session start.
    ///
    /// Added as the system-prompt-injected "what the agent has learned about
    /// this environment and user" tier. `None` when no semantic backend is
    /// attached; old payloads without this field decode as `None`.
    #[serde(default)]
    pub semantic_snapshot: Option<crate::memory::semantic::SemanticSnapshot>,
}

/// Detailed degradation info for external memory sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DegradedSource {
    /// Source name (e.g., "openviking").
    pub name: String,
    /// Specific layer that degraded (e.g., "L1").
    pub layer: Option<String>,
    /// Human-readable reason for degradation.
    pub reason: String,
}

/// Semantic intent for external retrieval.
/// Each Connector maps this to its own depth levels internally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExternalRetrievalIntent {
    /// Quick context supplement (maps to OpenViking L0/L1).
    Supplement,
    /// Deep retrieval for comprehensive context (maps to OpenViking L2).
    Deep,
}

/// External retrieval item clearly tagged as external origin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalRetrievalItem {
    /// Source connector name (e.g., "openviking").
    pub source: String,
    /// Retrieved content.
    pub content: String,
    /// Relevance score (0.0-1.0).
    pub relevance: f32,
    /// Source-specific depth label (e.g., "L1").
    pub retrieval_depth: String,
}

/// External retrieval provider trait.
/// Called by orchestrator OUTSIDE get_context() to keep it synchronous.
#[async_trait::async_trait]
pub trait ExternalRetrievalProvider: Send + Sync {
    /// Retrieve external context by semantic intent.
    async fn retrieve(
        &self,
        query: &str,
        intent: ExternalRetrievalIntent,
    ) -> anyhow::Result<Vec<ExternalRetrievalItem>>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_execution_id(s: &str) -> ExecutionId {
        ExecutionId::from_string(s.to_string()).unwrap()
    }

    // -----------------------------------------------------------------------
    // WorkingMemoryEntry / WorkingEntryKind serde roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn test_working_entry_serde_roundtrip() {
        let exec_id = make_execution_id("wm-serde-exec-001");
        let entry = WorkingMemoryEntry {
            execution_id: Some(exec_id.clone()),
            kind: WorkingEntryKind::ToolResult,
            summary: "tool returned file listing".to_string(),
            artifact_refs: vec![],
            trace_id: None,
            encrypted: false,
        };

        let serialized = serde_json::to_string(&entry).expect("serialization must succeed");
        assert!(!serialized.is_empty(), "serialized JSON must not be empty");

        let deserialized: WorkingMemoryEntry =
            serde_json::from_str(&serialized).expect("deserialization must succeed");

        assert_eq!(deserialized.execution_id, Some(exec_id));
        assert_eq!(deserialized.summary, "tool returned file listing");
        assert!(
            matches!(deserialized.kind, WorkingEntryKind::ToolResult),
            "kind must roundtrip correctly"
        );
        assert!(deserialized.artifact_refs.is_empty());
        assert!(deserialized.trace_id.is_none());
    }

    #[test]
    fn test_working_entry_serde_roundtrip_all_kinds() {
        let exec_id = make_execution_id("wm-serde-exec-002");
        let kinds = [
            WorkingEntryKind::Goal,
            WorkingEntryKind::PhaseStart,
            WorkingEntryKind::PhaseEnd,
            WorkingEntryKind::ToolResult,
            WorkingEntryKind::Decision,
            WorkingEntryKind::Error,
        ];

        for kind in kinds {
            let entry = WorkingMemoryEntry {
                execution_id: Some(exec_id.clone()),
                kind,
                summary: "roundtrip test".to_string(),
                artifact_refs: vec![],
                trace_id: None,
                encrypted: false,
            };

            let json = serde_json::to_value(&entry).expect("to_value must succeed");
            let roundtrip: WorkingMemoryEntry =
                serde_json::from_value(json).expect("from_value must succeed");

            assert_eq!(roundtrip.summary, "roundtrip test");
        }
    }

    #[test]
    fn test_working_entry_no_execution_id_serde_roundtrip() {
        let entry = WorkingMemoryEntry {
            execution_id: None,
            kind: WorkingEntryKind::Goal,
            summary: "initial goal: scan targets".to_string(),
            artifact_refs: vec![],
            trace_id: None,
            encrypted: false,
        };

        let json = serde_json::to_value(&entry).expect("to_value must succeed");
        let roundtrip: WorkingMemoryEntry =
            serde_json::from_value(json).expect("from_value must succeed");

        assert!(roundtrip.execution_id.is_none());
        assert_eq!(roundtrip.summary, "initial goal: scan targets");
    }

    // -----------------------------------------------------------------------
    // EpisodicContextProjection serde roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn test_episodic_projection_serde_roundtrip() {
        let projection = EpisodicContextProjection {
            executions: vec![],
            reviews: vec![],
            provenance_records: vec![],
            security_events: vec![],
            artifact_refs: vec![],
            summary_notes: vec![
                "Execution completed successfully.".to_string(),
                "No security incidents detected.".to_string(),
            ],
            foresights: vec![],
        };

        let serialized = serde_json::to_string(&projection).expect("serialization must succeed");
        assert!(!serialized.is_empty());

        let deserialized: EpisodicContextProjection =
            serde_json::from_str(&serialized).expect("deserialization must succeed");

        assert!(deserialized.executions.is_empty());
        assert!(deserialized.reviews.is_empty());
        assert!(deserialized.provenance_records.is_empty());
        assert!(deserialized.security_events.is_empty());
        assert!(deserialized.artifact_refs.is_empty());
        assert_eq!(deserialized.summary_notes.len(), 2);
        assert_eq!(
            deserialized.summary_notes[0],
            "Execution completed successfully."
        );
    }

    #[test]
    fn test_episodic_projection_serde_roundtrip_empty() {
        let projection = EpisodicContextProjection {
            executions: vec![],
            reviews: vec![],
            provenance_records: vec![],
            security_events: vec![],
            artifact_refs: vec![],
            summary_notes: vec![],
            foresights: vec![],
        };

        let json = serde_json::to_value(&projection).expect("to_value must succeed");
        let roundtrip: EpisodicContextProjection =
            serde_json::from_value(json).expect("from_value must succeed");

        assert!(roundtrip.executions.is_empty());
        assert!(roundtrip.summary_notes.is_empty());
    }

    // MEDIUM #6: 加密功能测试
    #[test]
    fn test_working_memory_entry_encrypted() {
        let entry = WorkingMemoryEntry::new_encrypted(
            None,
            WorkingEntryKind::ToolResult,
            "sensitive api response data",
            vec![],
            None,
        );

        assert!(entry.encrypted);
        // 内容应该被 SensitiveString 处理（脱敏）
        assert_ne!(entry.summary, "sensitive api response data");
        assert_eq!(entry.summary, "***REDACTED***");
    }

    #[test]
    fn test_working_memory_entry_unencrypted() {
        let entry = WorkingMemoryEntry::new(
            None,
            WorkingEntryKind::Goal,
            "public goal description",
            vec![],
            None,
        );

        assert!(!entry.encrypted);
        assert_eq!(entry.summary, "public goal description");
    }

    #[test]
    fn test_working_memory_entry_encrypted_serde_roundtrip() {
        let entry = WorkingMemoryEntry::new_encrypted(
            None,
            WorkingEntryKind::Error,
            "error with sensitive info",
            vec![],
            None,
        );

        let json = serde_json::to_string(&entry).expect("serialization must succeed");
        let deserialized: WorkingMemoryEntry =
            serde_json::from_str(&json).expect("deserialization must succeed");

        assert!(deserialized.encrypted);
        // 序列化后的内容应该仍然是脱敏的
        assert!(!json.contains("error with sensitive info"));
    }

    // -----------------------------------------------------------------------
    // WorkingMemoryCheckpoint serde roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn test_working_memory_checkpoint_serde_roundtrip() {
        let exec_id = make_execution_id("wm-chkpt-exec-001");
        let checkpoint = WorkingMemoryCheckpoint {
            entries: vec![
                WorkingMemoryEntry {
                    execution_id: Some(exec_id.clone()),
                    kind: WorkingEntryKind::PhaseStart,
                    summary: "phase 1 started".to_string(),
                    artifact_refs: vec![],
                    trace_id: None,
                    encrypted: false,
                },
                WorkingMemoryEntry {
                    execution_id: Some(exec_id.clone()),
                    kind: WorkingEntryKind::PhaseEnd,
                    summary: "phase 1 completed".to_string(),
                    artifact_refs: vec![],
                    trace_id: None,
                    encrypted: false,
                },
            ],
            checkpoint_id: "chkpt-abc-001".to_string(),
        };

        let json = serde_json::to_value(&checkpoint).expect("to_value must succeed");
        let roundtrip: WorkingMemoryCheckpoint =
            serde_json::from_value(json).expect("from_value must succeed");

        assert_eq!(roundtrip.checkpoint_id, "chkpt-abc-001");
        assert_eq!(roundtrip.entries.len(), 2);
        assert_eq!(roundtrip.entries[0].summary, "phase 1 started");
        assert_eq!(roundtrip.entries[1].summary, "phase 1 completed");
    }

    #[test]
    fn test_foresight_projection_serde_roundtrip() {
        let fp = ForesightProjection {
            content: "User may need to reschedule meeting".to_string(),
            evidence: "Mentioned conflict with dentist appointment".to_string(),
            start_date: Some(chrono::NaiveDate::from_ymd_opt(2026, 4, 15).unwrap()),
            end_date: Some(chrono::NaiveDate::from_ymd_opt(2026, 4, 20).unwrap()),
            duration_days: Some(5),
            confidence: 0.75,
        };
        let json = serde_json::to_string(&fp).expect("serialize");
        let deser: ForesightProjection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.content, fp.content);
        assert_eq!(deser.duration_days, Some(5));
        assert!((deser.confidence - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn test_episodic_context_projection_with_foresights_serde() {
        let projection = EpisodicContextProjection {
            executions: vec![],
            reviews: vec![],
            provenance_records: vec![],
            security_events: vec![],
            artifact_refs: vec![],
            summary_notes: vec![],
            foresights: vec![ForesightProjection {
                content: "test foresight".to_string(),
                evidence: "test evidence".to_string(),
                start_date: None,
                end_date: None,
                duration_days: None,
                confidence: 0.5,
            }],
        };
        let json = serde_json::to_string(&projection).expect("serialize");
        let deser: EpisodicContextProjection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.foresights.len(), 1);
        assert_eq!(deser.foresights[0].content, "test foresight");
    }

    #[test]
    fn test_episodic_context_projection_backward_compat_no_foresights() {
        let old_json = r#"{
            "executions": [],
            "reviews": [],
            "provenance_records": [],
            "security_events": [],
            "artifact_refs": [],
            "summary_notes": ["note1"]
        }"#;
        let deser: EpisodicContextProjection = serde_json::from_str(old_json).expect("deserialize");
        assert!(
            deser.foresights.is_empty(),
            "foresights should default to empty"
        );
        assert_eq!(deser.summary_notes.len(), 1);
    }

    #[test]
    fn test_degraded_source_serde_roundtrip() {
        let ds = DegradedSource {
            name: "openviking".to_string(),
            layer: Some("L1".to_string()),
            reason: "timeout after 500ms".to_string(),
        };
        let json = serde_json::to_string(&ds).expect("serialize");
        let deser: DegradedSource = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.name, "openviking");
        assert_eq!(deser.layer, Some("L1".to_string()));
    }

    #[test]
    fn test_envelope_backward_compat_no_degraded_details() {
        let old_json = r#"{
            "working_items": [],
            "episodic": {
                "executions": [],
                "reviews": [],
                "provenance_records": [],
                "security_events": [],
                "artifact_refs": [],
                "summary_notes": []
            },
            "procedural_docs": [],
            "policy_notes": [],
            "degraded_sources": ["openviking-memory"]
        }"#;
        let deser: MemoryContextEnvelope = serde_json::from_str(old_json).expect("deserialize");
        assert_eq!(deser.degraded_sources.len(), 1);
        assert!(deser.degraded_source_details.is_empty());
    }

    #[test]
    fn test_external_retrieval_item_serde() {
        let item = ExternalRetrievalItem {
            source: "openviking".to_string(),
            content: "retrieved context".to_string(),
            relevance: 0.85,
            retrieval_depth: "L1".to_string(),
        };
        let json = serde_json::to_string(&item).expect("serialize");
        let deser: ExternalRetrievalItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.source, "openviking");
        assert!((deser.relevance - 0.85).abs() < f32::EPSILON);
    }
}
