use crate::errors::{MemoryError, MemoryResult};
use crate::execution::Execution;
use crate::ids::ExecutionId;
use crate::memory::compaction::{
    CompactionCheckpoint, CompactionResult, CompactionStrategy, SimpleCompactor,
};
use crate::memory::episodic::EpisodicContextProvider;
use crate::memory::semantic::SemanticSnapshot;
use crate::memory::working::{
    InMemoryWorkingMemory, WorkingMemory, WorkingMemoryConfig, WorkingMemoryRegistry,
};
use crate::memory_context::{
    EpisodicContextProjection, EpisodicContextRequest, MemoryContextEnvelope, MemoryContextRequest,
    ProceduralDocument, WorkingMemoryEntry,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

/// 记忆系统配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// 工作记忆最大条目数
    pub working_max_entries: usize,
    /// 工作记忆最大字符数
    pub working_max_chars: usize,
    /// 情景记忆最大执行数
    pub episodic_max_executions: usize,
    /// 情景记忆最大事件数
    pub episodic_max_events: usize,
    /// 程序性记忆最大规则数
    pub procedural_max_rules: usize,
    /// 最大上下文大小（字符数）
    pub max_context_chars: usize,
    /// 是否启用压缩
    pub enable_compaction: bool,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            working_max_entries: 100,
            working_max_chars: 50000,
            episodic_max_executions: 50,
            episodic_max_events: 200,
            procedural_max_rules: 100,
            max_context_chars: 200000,
            enable_compaction: true,
        }
    }
}

/// 简化的程序性记忆存储（暂未实现新版本，保留原始实现）
struct SimpleProceduralMemory {
    documents: Arc<RwLock<Vec<ProceduralDocument>>>,
    config: MemoryConfig,
}

impl SimpleProceduralMemory {
    fn new(config: MemoryConfig) -> Self {
        Self {
            documents: Arc::new(RwLock::new(Vec::new())),
            config,
        }
    }

    fn add_document(&self, doc: ProceduralDocument) -> Result<(), String> {
        let mut documents = self.documents.write().unwrap();
        if documents.len() >= self.config.procedural_max_rules {
            return Err(format!(
                "Maximum number of documents ({}) reached",
                self.config.procedural_max_rules
            ));
        }
        documents.push(doc);
        drop(documents);
        Ok(())
    }

    fn get_all_documents(&self) -> Vec<ProceduralDocument> {
        self.documents.read().unwrap().clone()
    }

    fn clear(&self) {
        self.documents.write().unwrap().clear();
    }
}

/// 统一记忆上下文提供者
///
/// 集成了：
/// - `WorkingMemoryRegistry`（Session 隔离的工作记忆，通过 default session 使用）
/// - `EpisodicContextProvider`（支持相关性评分的情景记忆）
/// - `SimpleProceduralMemory`（程序性记忆，暂未升级）
pub struct MemoryContextProvider {
    /// Session 隔离的工作记忆注册表（保持生命周期，避免 Arc<InMemoryWorkingMemory> 被提前释放）
    _working_registry: Arc<WorkingMemoryRegistry>,
    /// 默认 Session 的工作记忆实例（直接引用，供 compact/count 使用）
    working_default: Arc<InMemoryWorkingMemory>,
    /// 情景记忆提供者（支持相关性评分）
    episodic_provider: Arc<EpisodicContextProvider>,
    /// 情景摘要（为 compact 提供压缩支持，独立于 EpisodicContextProvider）
    episodic_summaries: Arc<RwLock<VecDeque<String>>>,
    procedural_memory: Arc<SimpleProceduralMemory>,
    config: MemoryConfig,
    context_size: Arc<RwLock<usize>>,
    /// Unified security configuration for consistent security policy enforcement.
    security_config: Arc<crate::security_config::SecurityConfigManager>,
    /// Frozen semantic-memory snapshot captured at session start. Injected as
    /// the 4th envelope section by [`MemoryContextProvider::get_context`].
    ///
    /// `None` when no semantic backend is attached.
    semantic_snapshot: Arc<RwLock<Option<SemanticSnapshot>>>,
}

impl MemoryContextProvider {
    /// 创建新的记忆上下文提供者
    pub fn new(config: MemoryConfig) -> Self {
        // 创建工作记忆注册表（单次使用足够大的 session 上限）
        let wm_config = WorkingMemoryConfig {
            capacity: config.working_max_entries,
            ttl_seconds: None,
        };
        let working_registry = Arc::new(WorkingMemoryRegistry::new(wm_config.clone(), 1024));

        // 创建默认 session 并直接持有其 Arc<InMemoryWorkingMemory>
        let default_session_id = crate::ids::SessionId::from_string("__default__".to_string())
            .expect("default session id must be valid");
        let working_default = working_registry
            .session(&default_session_id)
            .expect("default session must be created");

        let episodic_provider =
            Arc::new(EpisodicContextProvider::new(config.episodic_max_executions));
        let procedural_memory = Arc::new(SimpleProceduralMemory::new(config.clone()));

        Self {
            _working_registry: working_registry,
            working_default,
            episodic_provider,
            episodic_summaries: Arc::new(RwLock::new(VecDeque::new())),
            procedural_memory,
            config,
            context_size: Arc::new(RwLock::new(0)),
            security_config: Arc::new(crate::security_config::SecurityConfigManager::default()),
            semantic_snapshot: Arc::new(RwLock::new(None)),
        }
    }

    /// Attach a custom SecurityConfigManager for unified security policy enforcement.
    pub fn with_security_config(
        mut self,
        config: Arc<crate::security_config::SecurityConfigManager>,
    ) -> Self {
        self.security_config = config;
        self
    }

    /// Attach a frozen semantic-memory snapshot. The snapshot is captured
    /// once by the caller (typically from
    /// `LocalSemanticMemoryConnector::load_snapshot`) at session start and
    /// stays fixed for the remainder of the session — this keeps the
    /// system-prompt prefix cache stable.
    pub fn set_semantic_snapshot(&self, snapshot: SemanticSnapshot) {
        if let Ok(mut guard) = self.semantic_snapshot.write() {
            *guard = Some(snapshot);
        }
    }

    /// Clear any attached semantic snapshot.
    pub fn clear_semantic_snapshot(&self) {
        if let Ok(mut guard) = self.semantic_snapshot.write() {
            *guard = None;
        }
    }

    /// Read a clone of the current semantic snapshot (if any). Primarily for
    /// tests and diagnostics.
    pub fn semantic_snapshot(&self) -> Option<SemanticSnapshot> {
        self.semantic_snapshot.read().ok().and_then(|g| g.clone())
    }

    /// 获取记忆上下文
    pub fn get_context(&self, request: MemoryContextRequest) -> MemoryContextEnvelope {
        // 收集工作记忆（通过 InMemoryWorkingMemory 的公共 API）
        let working_items = if let Some(exec_id) = &request.execution_id {
            self.get_working_entries_by_execution(exec_id)
        } else {
            self.working_default
                .list_recent(request.max_items)
                .unwrap_or_default()
        };

        // 收集程序性文档
        let procedural_docs = self
            .procedural_memory
            .get_all_documents()
            .into_iter()
            .take(5)
            .collect();

        // 收集情景上下文（使用 EpisodicContextProvider::project）
        let episodic_request = EpisodicContextRequest {
            case_id: request.case_id.clone(),
            execution_id: request.execution_id.clone(),
            trace_id: None,
            max_events: request.max_items,
        };
        let episodic = self.build_episodic_projection(&episodic_request);

        // 创建策略说明
        let policy_notes = vec![];

        // 捕获当前挂载的语义快照（不变——session 全程保持冻结）
        let semantic_snapshot = self.semantic_snapshot.read().ok().and_then(|g| g.clone());

        // 创建上下文信封
        let envelope = MemoryContextEnvelope {
            working_items,
            episodic,
            procedural_docs,
            policy_notes,
            degraded_sources: vec![],
            degraded_source_details: vec![],
            semantic_snapshot,
        };

        // 更新上下文大小
        self.update_context_size(&envelope);

        // 如果超出限制，进行截断
        if self.is_context_too_large() {
            self.truncate_envelope(envelope, request.max_items)
        } else {
            envelope
        }
    }

    /// 添加工作记忆条目
    pub fn add_working_entry(&self, entry: WorkingMemoryEntry) {
        // 忽略 push 错误（容量超出时静默丢弃，与原行为一致）
        let _ = self.working_default.push(entry);
    }

    /// 添加执行记录
    pub fn add_execution(&self, execution: Execution) {
        self.episodic_provider.ingest(execution);
    }

    /// 添加摘要
    pub fn add_summary(&self, summary: String) {
        let mut summaries = self.episodic_summaries.write().unwrap();
        while summaries.len() >= 50 {
            summaries.pop_front();
        }
        summaries.push_back(summary);
    }

    /// 添加程序性文档
    pub fn add_procedural_document(&self, doc: ProceduralDocument) -> Result<(), String> {
        self.procedural_memory.add_document(doc)
    }

    /// 获取当前上下文大小
    pub fn get_context_size(&self) -> usize {
        *self.context_size.read().unwrap()
    }

    /// 检查上下文是否过大
    pub fn is_context_too_large(&self) -> bool {
        self.get_context_size() > self.config.max_context_chars
    }

    /// 获取当前工作记忆条目数量
    ///
    /// # Errors
    /// Returns MemoryError::LockPoisoned if the working memory read lock is poisoned
    pub fn get_working_entry_count(&self) -> MemoryResult<usize> {
        self.working_default
            .len()
            .map_err(|e| MemoryError::LockPoisoned(format!("working_memory len: {}", e)))
    }

    /// 清空所有记忆
    pub fn clear_all(&self) {
        let _ = self.working_default.clear();
        // 清空情景记忆（EpisodicContextProvider 的 ring buffer）
        self.episodic_provider.clear();
        // 清空情景摘要
        self.episodic_summaries.write().unwrap().clear();
        self.procedural_memory.clear();
        *self.context_size.write().unwrap() = 0;
    }

    /// 创建压缩检查点（为 Agent 8 预留）
    pub fn create_compaction_checkpoint(&self) -> CompactionCheckpoint {
        let working_entries = self.working_default.list_recent(1000).unwrap_or_default();
        let procedural_docs = self.procedural_memory.get_all_documents();

        CompactionCheckpoint {
            working_entries,
            episodic_summary: vec![], // Agent 8 will implement
            procedural_docs,
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// 执行压缩
    ///
    /// 使用轻量级策略压缩工作记忆、情景记忆和程序性记忆
    /// - 不调用外部 API
    /// - 纯内存操作
    /// - 目标: p95 < 100ms
    pub fn compact(&self, strategy: CompactionStrategy) -> CompactionResult {
        // 获取当前条目
        let working_before = self.working_default.list_recent(1000).unwrap_or_default();
        let episodic_summaries_before = self.episodic_summaries.read().unwrap().clone();
        let procedural_before = self.procedural_memory.get_all_documents();

        let items_before =
            working_before.len() + episodic_summaries_before.len() + procedural_before.len();

        // 执行压缩
        let working_after = SimpleCompactor::compact_working(working_before.clone(), &strategy);
        let episodic_after = SimpleCompactor::compact_episodic(
            episodic_summaries_before.iter().cloned().collect(),
            &strategy,
        );
        let procedural_after = SimpleCompactor::compact_procedural(procedural_before, &strategy);

        let items_after = working_after.len() + episodic_after.len() + procedural_after.len();

        // 计算节省的字符数
        let chars_saved = SimpleCompactor::calculate_chars_saved(
            &working_before,
            &working_after,
            &episodic_summaries_before
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            &episodic_after,
        );

        // 应用压缩结果：清空并重新填充工作记忆
        let _ = self.working_default.clear();
        for entry in working_after {
            let _ = self.working_default.push(entry);
        }

        // 应用压缩后的情景摘要
        {
            let mut summaries = self.episodic_summaries.write().unwrap();
            summaries.clear();
            for summary in episodic_after {
                summaries.push_back(summary);
            }
        }

        {
            let mut docs = self.procedural_memory.documents.write().unwrap();
            *docs = procedural_after;
        }

        CompactionResult {
            items_before,
            items_after,
            chars_saved,
            success: true,
            error: None,
        }
    }

    // --- Private helper methods ---

    /// 通过 EpisodicContextProvider::project 构建 EpisodicContextProjection
    fn build_episodic_projection(
        &self,
        request: &EpisodicContextRequest,
    ) -> EpisodicContextProjection {
        let projection = self.episodic_provider.project(request);

        // 将 ScoredExecution 映射到 Execution
        let executions: Vec<Execution> = projection
            .executions
            .into_iter()
            .map(|se| se.execution)
            .collect();

        // 使用手动添加的摘要（与原 SimpleEpisodicMemory 行为一致）
        // EpisodicContextProvider 自动生成的 summary_notes 为内部调试信息，不暴露到 envelope
        let _ = projection.summary_notes; // 保留以便未来可选地合并
        let combined_summaries: Vec<String> = self
            .episodic_summaries
            .read()
            .unwrap()
            .iter()
            .rev()
            .take(5)
            .cloned()
            .collect();

        EpisodicContextProjection {
            executions,
            reviews: vec![],
            provenance_records: vec![],
            security_events: vec![],
            artifact_refs: vec![],
            summary_notes: combined_summaries,
            foresights: vec![],
        }
    }

    /// 通过 execution_id 过滤工作记忆条目
    fn get_working_entries_by_execution(
        &self,
        execution_id: &ExecutionId,
    ) -> Vec<WorkingMemoryEntry> {
        // 获取所有条目，再按 execution_id 过滤
        let all = self
            .working_default
            .list_recent(self.config.working_max_entries)
            .unwrap_or_default();
        all.into_iter()
            .filter(|e| e.execution_id.as_ref() == Some(execution_id))
            .collect()
    }

    fn update_context_size(&self, envelope: &MemoryContextEnvelope) {
        let mut size = 0;

        // 计算工作记忆大小
        for item in &envelope.working_items {
            size += item.summary.len();
        }

        // 计算情景记忆大小
        for note in &envelope.episodic.summary_notes {
            size += note.len();
        }

        // 计算程序性文档大小
        for doc in &envelope.procedural_docs {
            size += doc.title.len() + doc.content.len();
        }

        // 计算策略说明大小
        for note in &envelope.policy_notes {
            size += note.len();
        }

        *self.context_size.write().unwrap() = size;
    }

    fn truncate_envelope(
        &self,
        mut envelope: MemoryContextEnvelope,
        max_items: usize,
    ) -> MemoryContextEnvelope {
        // 截断工作记忆
        envelope.working_items.truncate(max_items / 2);

        // 截断情景记忆
        envelope.episodic.executions.truncate(max_items / 4);
        envelope.episodic.reviews.truncate(max_items / 4);
        envelope.episodic.security_events.truncate(max_items / 4);
        envelope.episodic.summary_notes.truncate(5);

        // 截断程序性文档
        envelope.procedural_docs.truncate(3);

        // 截断策略说明
        envelope.policy_notes.truncate(3);

        envelope
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::{AgentRef, ExecutionBudget, ExecutionStatus};
    use crate::ids::{AgentId, TraceId};
    use crate::memory_context::WorkingEntryKind;

    fn make_execution_id(s: &str) -> ExecutionId {
        ExecutionId::from_string(s.to_string()).unwrap()
    }

    fn make_agent_id(s: &str) -> AgentId {
        AgentId::from_string(s.to_string()).unwrap()
    }

    fn make_trace_id(s: &str) -> TraceId {
        TraceId::from_string(s.to_string()).unwrap()
    }

    fn make_test_execution(id: &str) -> Execution {
        let exec_id = make_execution_id(id);
        let agent_id = make_agent_id("test-agent");

        Execution {
            id: exec_id.clone(),
            root_execution_id: exec_id.clone(),
            parent_execution_id: None,
            owner_node_id: None,
            scheduled_node_id: None,
            placement_group: None,
            lease_id: None,
            handoff_count: 0,
            case_id: None,
            task_id: None,
            agent: AgentRef {
                id: agent_id,
                role: "test".to_string(),
            },
            status: ExecutionStatus::Pending,
            join_strategy: None,
            budget: ExecutionBudget::default(),
            workspace: None,
            trace_id: make_trace_id("trace-001"),
            started_at: None,
            finished_at: None,
            risk_level: crate::capability::RiskLevel::Low,
            execution_mode: crate::execution::ExecutionMode::Normal,
        }
    }

    #[test]
    fn test_memory_context_provider_basic() {
        let config = MemoryConfig::default();
        let provider = MemoryContextProvider::new(config);

        // 添加工作记忆条目
        let entry = WorkingMemoryEntry {
            execution_id: Some(make_execution_id("test-exec-001")),
            kind: WorkingEntryKind::Goal,
            summary: "Test goal".to_string(),
            artifact_refs: vec![],
            trace_id: None,
            encrypted: false,
        };
        provider.add_working_entry(entry);

        // 获取上下文
        let request = MemoryContextRequest {
            case_id: None,
            execution_id: None,
            max_items: 10,
        };

        let context = provider.get_context(request);
        assert!(!context.working_items.is_empty());
        assert_eq!(context.working_items[0].summary, "Test goal");
    }

    #[test]
    fn test_memory_context_provider_with_episodic() {
        let config = MemoryConfig::default();
        let provider = MemoryContextProvider::new(config);

        // 添加执行记录
        let exec = make_test_execution("exec-001");
        provider.add_execution(exec.clone());

        // 添加摘要
        provider.add_summary("Execution completed".to_string());

        // 获取上下文
        let request = MemoryContextRequest {
            case_id: None,
            execution_id: Some(exec.id.clone()),
            max_items: 10,
        };

        let context = provider.get_context(request);
        assert_eq!(context.episodic.executions.len(), 1);
        assert_eq!(context.episodic.summary_notes.len(), 1);
    }

    #[test]
    fn test_memory_context_provider_with_procedural() {
        let config = MemoryConfig::default();
        let provider = MemoryContextProvider::new(config);

        // 添加程序性文档
        let doc = ProceduralDocument {
            source: "manual".to_string(),
            title: "Operations Guide".to_string(),
            content: "This is the operations guide content".to_string(),
        };
        provider.add_procedural_document(doc.clone()).unwrap();

        // 获取上下文
        let request = MemoryContextRequest {
            case_id: None,
            execution_id: None,
            max_items: 10,
        };

        let context = provider.get_context(request);
        assert!(!context.procedural_docs.is_empty());
        assert_eq!(context.procedural_docs[0].title, "Operations Guide");
    }

    #[test]
    fn test_memory_context_provider_size_tracking() {
        let config = MemoryConfig {
            max_context_chars: 100,
            ..Default::default()
        };
        let provider = MemoryContextProvider::new(config);

        // 添加一些内容
        let entry = WorkingMemoryEntry {
            execution_id: None,
            kind: WorkingEntryKind::ToolResult,
            summary: "This is a test summary with some length".to_string(),
            artifact_refs: vec![],
            trace_id: None,
            encrypted: false,
        };
        provider.add_working_entry(entry);

        let request = MemoryContextRequest {
            case_id: None,
            execution_id: None,
            max_items: 10,
        };

        provider.get_context(request);
        assert!(provider.get_context_size() > 0);
    }

    #[test]
    fn test_memory_context_provider_clear() {
        let config = MemoryConfig::default();
        let provider = MemoryContextProvider::new(config);

        // 添加各种内容
        provider.add_working_entry(WorkingMemoryEntry {
            execution_id: None,
            kind: WorkingEntryKind::Goal,
            summary: "Test goal".to_string(),
            artifact_refs: vec![],
            trace_id: None,
            encrypted: false,
        });

        let doc = ProceduralDocument {
            source: "test".to_string(),
            title: "Test Doc".to_string(),
            content: "Content".to_string(),
        };
        provider.add_procedural_document(doc).unwrap();

        // 清空
        provider.clear_all();
        assert_eq!(provider.get_context_size(), 0);

        let request = MemoryContextRequest {
            case_id: None,
            execution_id: None,
            max_items: 10,
        };

        let context = provider.get_context(request);
        assert!(context.working_items.is_empty());
        assert!(context.procedural_docs.is_empty());
    }

    #[test]
    fn test_compaction_checkpoint() {
        let config = MemoryConfig::default();
        let provider = MemoryContextProvider::new(config);

        // 添加一些数据
        provider.add_working_entry(WorkingMemoryEntry {
            execution_id: None,
            kind: WorkingEntryKind::Decision,
            summary: "Test decision".to_string(),
            artifact_refs: vec![],
            trace_id: None,
            encrypted: false,
        });

        let checkpoint = provider.create_compaction_checkpoint();
        assert!(!checkpoint.timestamp.is_empty());
        assert_eq!(checkpoint.working_entries.len(), 1);
    }

    #[test]
    fn test_compact_with_strategy() {
        let config = MemoryConfig::default();
        let provider = MemoryContextProvider::new(config);

        // 添加多个工作记忆条目
        for i in 0..10 {
            provider.add_working_entry(WorkingMemoryEntry {
                execution_id: None,
                kind: WorkingEntryKind::ToolResult,
                summary: format!("Entry {}", i),
                artifact_refs: vec![],
                trace_id: None,
                encrypted: false,
            });
        }

        // 添加多个摘要
        for i in 0..10 {
            provider.add_summary(format!("Summary {}", i));
        }

        let strategy = CompactionStrategy {
            keep_recent_count: 5,
            enable_deduplication: true,
            keep_procedural: true,
            similarity_threshold: 0.85,
        };

        let result = provider.compact(strategy);
        assert!(result.success);
        assert!(result.items_after <= result.items_before);
    }

    #[test]
    fn test_get_working_entry_count_returns_result() {
        let config = MemoryConfig::default();
        let provider = MemoryContextProvider::new(config);

        let count = provider.get_working_entry_count();
        assert!(count.is_ok());
        assert_eq!(count.unwrap(), 0);

        for i in 0..5 {
            provider.add_working_entry(WorkingMemoryEntry {
                execution_id: Some(make_execution_id(&format!("exec-{}", i))),
                kind: WorkingEntryKind::Goal,
                summary: format!("Goal {}", i),
                artifact_refs: vec![],
                trace_id: None,
                encrypted: false,
            });
        }

        let count = provider.get_working_entry_count();
        assert!(count.is_ok());
        assert_eq!(count.unwrap(), 5);
    }

    #[test]
    fn test_envelope_includes_semantic_snapshot_as_fourth_section() {
        use crate::memory::semantic::SemanticSnapshot;

        let provider = MemoryContextProvider::new(MemoryConfig::default());

        // Before attaching a snapshot, the envelope has None for the 4th section
        // — this proves backward compatibility.
        let envelope = provider.get_context(MemoryContextRequest {
            case_id: None,
            execution_id: None,
            max_items: 10,
        });
        assert!(
            envelope.semantic_snapshot.is_none(),
            "unattached envelope must have semantic_snapshot=None"
        );

        // Attach a frozen snapshot and re-check.
        let snap = SemanticSnapshot {
            agent_notes: "workspace: cargo workspace; clippy -D warnings".to_string(),
            user_profile: "name: alice; prefers terse explanations".to_string(),
            captured_at: "2026-04-18T00:00:00Z".to_string(),
        };
        provider.set_semantic_snapshot(snap.clone());

        // Also populate the other three sections so we verify all four exist.
        provider.add_working_entry(WorkingMemoryEntry {
            execution_id: None,
            kind: WorkingEntryKind::Goal,
            summary: "complete the memory integration".to_string(),
            artifact_refs: vec![],
            trace_id: None,
            encrypted: false,
        });
        provider.add_summary("session 1 finished".to_string());
        provider
            .add_procedural_document(ProceduralDocument {
                source: "manual".to_string(),
                title: "Memory SOP".to_string(),
                content: "always review before commit".to_string(),
            })
            .expect("add procedural doc");

        let envelope = provider.get_context(MemoryContextRequest {
            case_id: None,
            execution_id: None,
            max_items: 10,
        });

        // 1. working_items
        assert!(
            !envelope.working_items.is_empty(),
            "section 1 (working) must be populated"
        );
        // 2. episodic summaries
        assert!(
            !envelope.episodic.summary_notes.is_empty(),
            "section 2 (episodic) must be populated"
        );
        // 3. procedural_docs
        assert!(
            !envelope.procedural_docs.is_empty(),
            "section 3 (procedural) must be populated"
        );
        // 4. semantic_snapshot — the new one
        let got = envelope
            .semantic_snapshot
            .expect("section 4 (semantic) must be populated");
        assert_eq!(got, snap, "attached snapshot must roundtrip into envelope");
    }

    #[test]
    fn test_semantic_snapshot_is_frozen_across_multiple_get_context_calls() {
        use crate::memory::semantic::SemanticSnapshot;

        let provider = MemoryContextProvider::new(MemoryConfig::default());
        let snap1 = SemanticSnapshot {
            agent_notes: "v1 notes".to_string(),
            user_profile: "v1 user".to_string(),
            captured_at: "2026-04-18T00:00:00Z".to_string(),
        };
        provider.set_semantic_snapshot(snap1.clone());

        // Multiple envelope builds return the same snapshot.
        for _ in 0..3 {
            let env = provider.get_context(MemoryContextRequest {
                case_id: None,
                execution_id: None,
                max_items: 5,
            });
            assert_eq!(env.semantic_snapshot.as_ref(), Some(&snap1));
        }

        // Clearing removes it.
        provider.clear_semantic_snapshot();
        let env = provider.get_context(MemoryContextRequest {
            case_id: None,
            execution_id: None,
            max_items: 5,
        });
        assert!(env.semantic_snapshot.is_none());
    }

    #[test]
    fn test_compact_preserves_data_on_success() {
        let config = MemoryConfig::default();
        let provider = MemoryContextProvider::new(config);

        for i in 0..10 {
            provider.add_working_entry(WorkingMemoryEntry {
                execution_id: None,
                kind: WorkingEntryKind::ToolResult,
                summary: format!("Result {}", i),
                artifact_refs: vec![],
                trace_id: None,
                encrypted: false,
            });
        }

        for i in 0..5 {
            provider.add_summary(format!("Summary {}", i));
        }

        let count_before = provider.get_working_entry_count().unwrap();
        assert!(count_before > 0);

        let strategy = CompactionStrategy {
            keep_recent_count: 5,
            enable_deduplication: true,
            keep_procedural: true,
            similarity_threshold: 0.85,
        };

        let result = provider.compact(strategy);
        assert!(result.success);
        assert!(result.error.is_none());

        let count_after = provider.get_working_entry_count().unwrap();
        assert!(count_after > 0, "Data should be preserved after compaction");
    }
}
