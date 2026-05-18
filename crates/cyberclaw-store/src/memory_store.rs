//! L0/L1/L2 分层记忆持久化存储
//!
//! 提供三级记忆层次：
//! - L0 (Full): 完整上下文，当前对话所有消息
//! - L1 (Summary): 关键信息摘要，由 LLM 生成
//! - L2 (Metadata): 结构化元数据，JSON 键值提取

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Result, StoreError};

/// 记忆读取记录（S18 R4：trace 端点用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryReadRecord {
    /// 被读取的记忆 ID
    pub memory_id: String,
    /// 发起读取的执行 ID
    pub execution_id: String,
    /// 读取时间
    pub read_at: DateTime<Utc>,
}

/// 记忆层级
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryLevel {
    /// L0: Full context (current conversation, all messages)
    L0Full,
    /// L1: Key info summary (LLM-generated summaries)
    L1Summary,
    /// L2: Structured metadata (JSON key-value extraction)
    L2Metadata,
}

impl std::fmt::Display for MemoryLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryLevel::L0Full => write!(f, "L0Full"),
            MemoryLevel::L1Summary => write!(f, "L1Summary"),
            MemoryLevel::L2Metadata => write!(f, "L2Metadata"),
        }
    }
}

impl MemoryLevel {
    /// 返回该层级的默认 TTL（秒）
    ///
    /// - L0: 3600 秒（1 小时）
    /// - L1: 86400 秒（24 小时）
    /// - L2: None（永久）
    pub fn default_ttl_seconds(&self) -> Option<i64> {
        match self {
            MemoryLevel::L0Full => Some(3600),
            MemoryLevel::L1Summary => Some(86400),
            MemoryLevel::L2Metadata => None,
        }
    }
}

/// 分层记忆记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeveledMemoryRecord {
    /// 记录唯一标识
    pub id: String,
    /// 所属会话 ID
    pub session_id: String,
    /// 所属 Agent ID
    pub agent_id: String,
    /// 记忆层级
    pub level: MemoryLevel,
    /// 记录键名
    pub key: String,
    /// 记录内容（JSON）
    pub content: Value,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 最后更新时间
    pub updated_at: DateTime<Utc>,
    /// 过期时间（秒），None 表示永不过期
    pub ttl_seconds: Option<i64>,
    /// 写入该记录的执行 ID（S18 R4 trace）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_execution_id: Option<String>,
    /// Optional embedding vector for semantic search (Sprint 25).
    /// `None` records are still searchable via BM25; `Some(vec)` enables cosine ranking.
    /// Vector length must match the platform's embed dimension or it will be skipped at search time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    /// User-supplied tags for filtered retrieval (Hermes BT-09).
    /// Empty vec is the default for backward compatibility with rows written
    /// before this field existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// 分层记忆存储 trait
#[async_trait]
pub trait LeveledMemoryStore: Send + Sync {
    /// 存储一条分层记忆记录
    async fn store_leveled(&self, record: LeveledMemoryRecord) -> Result<()>;

    /// 按层级查询某会话的所有记录
    async fn query_by_level(
        &self,
        session_id: &str,
        level: MemoryLevel,
    ) -> Result<Vec<LeveledMemoryRecord>>;

    /// 按键名查询某会话的记录
    async fn query_by_key(
        &self,
        session_id: &str,
        key: &str,
    ) -> Result<Option<LeveledMemoryRecord>>;

    /// 按标签查询某会话的记录（Hermes BT-09）。
    /// 返回包含 *任意* 给定 tag 的记录（OR 语义）。空 tags 列表返回空结果。
    /// 默认实现遍历所有 level 然后内存过滤；存储后端可覆盖以使用索引。
    async fn query_by_tags(
        &self,
        session_id: &str,
        tags: &[String],
    ) -> Result<Vec<LeveledMemoryRecord>> {
        if tags.is_empty() {
            return Ok(Vec::new());
        }
        let mut results = Vec::new();
        for level in [
            MemoryLevel::L0Full,
            MemoryLevel::L1Summary,
            MemoryLevel::L2Metadata,
        ] {
            let rows = self.query_by_level(session_id, level).await?;
            for row in rows {
                if row.tags.iter().any(|t| tags.contains(t)) {
                    results.push(row);
                }
            }
        }
        Ok(results)
    }

    /// 提升记录层级（例如 L0 -> L1）
    async fn promote(&self, id: &str, new_level: MemoryLevel) -> Result<()>;

    /// 降低记录层级（例如 L1 -> L0）
    async fn demote(&self, id: &str, new_level: MemoryLevel) -> Result<()>;

    /// 清除超过最大年龄的过期记录，返回删除数量
    async fn expire_stale(&self, max_age: chrono::Duration) -> Result<u64>;

    /// 统计某会话各层级的记录数量
    async fn count_by_level(&self, session_id: &str) -> Result<HashMap<MemoryLevel, usize>>;

    // ─── S18 R4: trace 支持（默认 no-op，不强制已有 impl 实现）────────────────

    /// 记录一次读取事件（execution_id 读取了 memory_id）。
    /// 默认实现为 no-op，支持只读取不追踪的存储后端。
    async fn record_read(&self, _memory_id: &str, _execution_id: &str) -> Result<()> {
        Ok(())
    }

    /// 返回指定 memory_id 的所有读取记录。
    /// 默认实现返回空列表。
    async fn get_reads(&self, _memory_id: &str) -> Result<Vec<MemoryReadRecord>> {
        Ok(Vec::new())
    }

    // ─── S19 F: delete 支持（默认 not-supported，不强制已有 impl 实现）──────────

    /// 删除指定 id 的记忆记录。
    /// 默认实现返回 not-supported 错误，允许只读后端保持不变。
    async fn delete(&self, _memory_id: &str) -> Result<()> {
        Err(StoreError::InternalError(
            "delete not supported by this store backend".to_string(),
        ))
    }
}

/// 基于内存的分层记忆存储实现
pub struct InMemoryLeveledStore {
    records: RwLock<HashMap<String, LeveledMemoryRecord>>,
    /// S18 R4：读取事件日志
    reads: RwLock<Vec<MemoryReadRecord>>,
}

impl InMemoryLeveledStore {
    /// 创建新的内存分层存储实例
    pub fn new() -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
            reads: RwLock::new(Vec::new()),
        }
    }
}

impl Default for InMemoryLeveledStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LeveledMemoryStore for InMemoryLeveledStore {
    async fn store_leveled(&self, record: LeveledMemoryRecord) -> Result<()> {
        let mut records = self.records.write().unwrap();
        records.insert(record.id.clone(), record);
        Ok(())
    }

    async fn query_by_level(
        &self,
        session_id: &str,
        level: MemoryLevel,
    ) -> Result<Vec<LeveledMemoryRecord>> {
        let records = self.records.read().unwrap();
        let mut results: Vec<LeveledMemoryRecord> = records
            .values()
            .filter(|r| r.session_id == session_id && r.level == level)
            .cloned()
            .collect();
        results.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        Ok(results)
    }

    async fn query_by_key(
        &self,
        session_id: &str,
        key: &str,
    ) -> Result<Option<LeveledMemoryRecord>> {
        let records = self.records.read().unwrap();
        let result = records
            .values()
            .find(|r| r.session_id == session_id && r.key == key)
            .cloned();
        Ok(result)
    }

    async fn promote(&self, id: &str, new_level: MemoryLevel) -> Result<()> {
        let mut records = self.records.write().unwrap();
        let record = records
            .get_mut(id)
            .ok_or_else(|| StoreError::NotFound(format!("LeveledMemoryRecord {} not found", id)))?;
        record.level = new_level;
        record.ttl_seconds = new_level.default_ttl_seconds();
        record.updated_at = Utc::now();
        Ok(())
    }

    async fn demote(&self, id: &str, new_level: MemoryLevel) -> Result<()> {
        let mut records = self.records.write().unwrap();
        let record = records
            .get_mut(id)
            .ok_or_else(|| StoreError::NotFound(format!("LeveledMemoryRecord {} not found", id)))?;
        record.level = new_level;
        record.ttl_seconds = new_level.default_ttl_seconds();
        record.updated_at = Utc::now();
        Ok(())
    }

    async fn expire_stale(&self, max_age: chrono::Duration) -> Result<u64> {
        let mut records = self.records.write().unwrap();
        let now = Utc::now();
        let mut removed = 0u64;

        records.retain(|_, record| {
            // L2 永不过期
            if record.level == MemoryLevel::L2Metadata {
                return true;
            }

            // 检查 TTL
            if let Some(ttl) = record.ttl_seconds {
                let age = now.signed_duration_since(record.created_at).num_seconds();
                if age > ttl {
                    removed += 1;
                    return false;
                }
            }

            // 检查 max_age
            let age = now.signed_duration_since(record.created_at);
            if age > max_age {
                removed += 1;
                return false;
            }

            true
        });

        Ok(removed)
    }

    async fn count_by_level(&self, session_id: &str) -> Result<HashMap<MemoryLevel, usize>> {
        let records = self.records.read().unwrap();
        let mut counts = HashMap::new();

        for record in records.values() {
            if record.session_id == session_id {
                *counts.entry(record.level).or_insert(0) += 1;
            }
        }

        Ok(counts)
    }

    // S18 R4: 覆盖 trait 默认实现，追踪实际读取
    async fn record_read(&self, memory_id: &str, execution_id: &str) -> Result<()> {
        let entry = MemoryReadRecord {
            memory_id: memory_id.to_string(),
            execution_id: execution_id.to_string(),
            read_at: Utc::now(),
        };
        self.reads
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(entry);
        Ok(())
    }

    async fn get_reads(&self, memory_id: &str) -> Result<Vec<MemoryReadRecord>> {
        let reads = self.reads.read().unwrap_or_else(|e| e.into_inner());
        Ok(reads
            .iter()
            .filter(|r| r.memory_id == memory_id)
            .cloned()
            .collect())
    }

    async fn delete(&self, memory_id: &str) -> Result<()> {
        let mut records = self.records.write().unwrap_or_else(|e| e.into_inner());
        if records.remove(memory_id).is_some() {
            Ok(())
        } else {
            Err(StoreError::NotFound(format!(
                "LeveledMemoryRecord {} not found",
                memory_id
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_record(
        id: &str,
        session_id: &str,
        agent_id: &str,
        level: MemoryLevel,
        key: &str,
    ) -> LeveledMemoryRecord {
        let now = Utc::now();
        LeveledMemoryRecord {
            id: id.to_string(),
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            level,
            key: key.to_string(),
            content: json!({"data": key}),
            created_at: now,
            updated_at: now,
            ttl_seconds: level.default_ttl_seconds(),
            source_execution_id: None,
            embedding: None,
            tags: Vec::new(),
        }
    }

    fn make_record_with_time(
        id: &str,
        session_id: &str,
        level: MemoryLevel,
        key: &str,
        created_at: DateTime<Utc>,
    ) -> LeveledMemoryRecord {
        LeveledMemoryRecord {
            id: id.to_string(),
            session_id: session_id.to_string(),
            agent_id: "agent-1".to_string(),
            level,
            key: key.to_string(),
            content: json!({"data": key}),
            created_at,
            updated_at: created_at,
            ttl_seconds: level.default_ttl_seconds(),
            source_execution_id: None,
            embedding: None,
            tags: Vec::new(),
        }
    }

    #[tokio::test]
    async fn query_by_tags_returns_matching_records() {
        // BT-09: tag-based filtering for memory retrieval.
        let store = InMemoryLeveledStore::new();

        let mut r0 = make_record("r0", "s1", "a1", MemoryLevel::L0Full, "k0");
        r0.tags = vec!["performance".to_string(), "metric".to_string()];

        let mut r1 = make_record("r1", "s1", "a1", MemoryLevel::L1Summary, "k1");
        r1.tags = vec!["performance".to_string()];

        let r2 = make_record("r2", "s1", "a1", MemoryLevel::L2Metadata, "k2");
        // r2 has no tags

        store.store_leveled(r0).await.unwrap();
        store.store_leveled(r1).await.unwrap();
        store.store_leveled(r2).await.unwrap();

        // Query for "performance" tag returns r0 + r1, not r2.
        let perf = store
            .query_by_tags("s1", &["performance".to_string()])
            .await
            .unwrap();
        assert_eq!(perf.len(), 2);
        assert!(perf.iter().any(|r| r.id == "r0"));
        assert!(perf.iter().any(|r| r.id == "r1"));
        assert!(!perf.iter().any(|r| r.id == "r2"));

        // Query for unmatched tag returns empty.
        let none = store
            .query_by_tags("s1", &["nonexistent".to_string()])
            .await
            .unwrap();
        assert!(none.is_empty());

        // Empty tags list returns empty.
        let empty = store.query_by_tags("s1", &[]).await.unwrap();
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn test_store_and_retrieve_by_level() {
        let store = InMemoryLeveledStore::new();

        let r0 = make_record("r0", "sess-1", "agent-1", MemoryLevel::L0Full, "conv-1");
        let r1 = make_record(
            "r1",
            "sess-1",
            "agent-1",
            MemoryLevel::L1Summary,
            "summary-1",
        );
        let r2 = make_record("r2", "sess-1", "agent-1", MemoryLevel::L2Metadata, "meta-1");

        store.store_leveled(r0).await.unwrap();
        store.store_leveled(r1).await.unwrap();
        store.store_leveled(r2).await.unwrap();

        let l0_records = store
            .query_by_level("sess-1", MemoryLevel::L0Full)
            .await
            .unwrap();
        assert_eq!(l0_records.len(), 1);
        assert_eq!(l0_records[0].id, "r0");

        let l1_records = store
            .query_by_level("sess-1", MemoryLevel::L1Summary)
            .await
            .unwrap();
        assert_eq!(l1_records.len(), 1);
        assert_eq!(l1_records[0].id, "r1");

        let l2_records = store
            .query_by_level("sess-1", MemoryLevel::L2Metadata)
            .await
            .unwrap();
        assert_eq!(l2_records.len(), 1);
        assert_eq!(l2_records[0].id, "r2");
    }

    #[tokio::test]
    async fn test_query_by_key() {
        let store = InMemoryLeveledStore::new();

        let r = make_record("r1", "sess-1", "agent-1", MemoryLevel::L0Full, "my-key");
        store.store_leveled(r).await.unwrap();

        let found = store.query_by_key("sess-1", "my-key").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "r1");

        let not_found = store.query_by_key("sess-1", "missing").await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_promote_l0_to_l1() {
        let store = InMemoryLeveledStore::new();

        let r = make_record("r1", "sess-1", "agent-1", MemoryLevel::L0Full, "conv-1");
        store.store_leveled(r).await.unwrap();

        store.promote("r1", MemoryLevel::L1Summary).await.unwrap();

        let l0 = store
            .query_by_level("sess-1", MemoryLevel::L0Full)
            .await
            .unwrap();
        assert!(l0.is_empty());

        let l1 = store
            .query_by_level("sess-1", MemoryLevel::L1Summary)
            .await
            .unwrap();
        assert_eq!(l1.len(), 1);
        assert_eq!(l1[0].level, MemoryLevel::L1Summary);
        // TTL should update to L1 default (24h)
        assert_eq!(l1[0].ttl_seconds, Some(86400));
    }

    #[tokio::test]
    async fn test_demote_l1_to_l0() {
        let store = InMemoryLeveledStore::new();

        let r = make_record(
            "r1",
            "sess-1",
            "agent-1",
            MemoryLevel::L1Summary,
            "summary-1",
        );
        store.store_leveled(r).await.unwrap();

        store.demote("r1", MemoryLevel::L0Full).await.unwrap();

        let l1 = store
            .query_by_level("sess-1", MemoryLevel::L1Summary)
            .await
            .unwrap();
        assert!(l1.is_empty());

        let l0 = store
            .query_by_level("sess-1", MemoryLevel::L0Full)
            .await
            .unwrap();
        assert_eq!(l0.len(), 1);
        assert_eq!(l0[0].level, MemoryLevel::L0Full);
        // TTL should update to L0 default (1h)
        assert_eq!(l0[0].ttl_seconds, Some(3600));
    }

    #[tokio::test]
    async fn test_expire_stale_removes_old_l0() {
        let store = InMemoryLeveledStore::new();

        // Create an L0 record with created_at 2 hours ago
        let two_hours_ago = Utc::now() - chrono::Duration::hours(2);
        let old_r =
            make_record_with_time("old-r", "sess-1", MemoryLevel::L0Full, "old", two_hours_ago);
        store.store_leveled(old_r).await.unwrap();

        // Create a fresh L0 record
        let fresh_r = make_record("fresh-r", "sess-1", "agent-1", MemoryLevel::L0Full, "fresh");
        store.store_leveled(fresh_r).await.unwrap();

        // Expire with max_age of 3 hours -- the old record has TTL=3600s (1h) and is 2h old
        let removed = store
            .expire_stale(chrono::Duration::hours(3))
            .await
            .unwrap();
        assert_eq!(removed, 1);

        let remaining = store
            .query_by_level("sess-1", MemoryLevel::L0Full)
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "fresh-r");
    }

    #[tokio::test]
    async fn test_l2_never_expires() {
        let store = InMemoryLeveledStore::new();

        // Create an L2 record with created_at 30 days ago
        let old_time = Utc::now() - chrono::Duration::days(30);
        let r = make_record_with_time(
            "l2-old",
            "sess-1",
            MemoryLevel::L2Metadata,
            "meta",
            old_time,
        );
        store.store_leveled(r).await.unwrap();

        // Try to expire with a very short max_age
        let removed = store
            .expire_stale(chrono::Duration::seconds(1))
            .await
            .unwrap();
        assert_eq!(removed, 0);

        let l2 = store
            .query_by_level("sess-1", MemoryLevel::L2Metadata)
            .await
            .unwrap();
        assert_eq!(l2.len(), 1);
        assert_eq!(l2[0].id, "l2-old");
    }

    #[tokio::test]
    async fn test_count_by_level_accuracy() {
        let store = InMemoryLeveledStore::new();

        store
            .store_leveled(make_record("a", "sess-1", "ag", MemoryLevel::L0Full, "k1"))
            .await
            .unwrap();
        store
            .store_leveled(make_record("b", "sess-1", "ag", MemoryLevel::L0Full, "k2"))
            .await
            .unwrap();
        store
            .store_leveled(make_record(
                "c",
                "sess-1",
                "ag",
                MemoryLevel::L1Summary,
                "k3",
            ))
            .await
            .unwrap();
        store
            .store_leveled(make_record(
                "d",
                "sess-1",
                "ag",
                MemoryLevel::L2Metadata,
                "k4",
            ))
            .await
            .unwrap();
        // Different session -- should not be counted
        store
            .store_leveled(make_record("e", "sess-2", "ag", MemoryLevel::L0Full, "k5"))
            .await
            .unwrap();

        let counts = store.count_by_level("sess-1").await.unwrap();
        assert_eq!(counts.get(&MemoryLevel::L0Full), Some(&2));
        assert_eq!(counts.get(&MemoryLevel::L1Summary), Some(&1));
        assert_eq!(counts.get(&MemoryLevel::L2Metadata), Some(&1));
    }

    #[tokio::test]
    async fn test_empty_store_returns_empty() {
        let store = InMemoryLeveledStore::new();

        let l0 = store
            .query_by_level("nonexistent", MemoryLevel::L0Full)
            .await
            .unwrap();
        assert!(l0.is_empty());

        let key = store.query_by_key("nonexistent", "nokey").await.unwrap();
        assert!(key.is_none());

        let counts = store.count_by_level("nonexistent").await.unwrap();
        assert!(counts.is_empty());
    }

    #[tokio::test]
    async fn test_promote_nonexistent_returns_not_found() {
        let store = InMemoryLeveledStore::new();
        let result = store.promote("missing-id", MemoryLevel::L1Summary).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_demote_nonexistent_returns_not_found() {
        let store = InMemoryLeveledStore::new();
        let result = store.demote("missing-id", MemoryLevel::L0Full).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_default_ttl_values() {
        assert_eq!(MemoryLevel::L0Full.default_ttl_seconds(), Some(3600));
        assert_eq!(MemoryLevel::L1Summary.default_ttl_seconds(), Some(86400));
        assert_eq!(MemoryLevel::L2Metadata.default_ttl_seconds(), None);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SQLite-backed LeveledMemoryStore
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "sqlite")]
pub mod sqlite_leveled {
    use super::*;
    use crate::error::StoreError;
    use rusqlite::{params, Connection};
    use std::sync::Mutex;

    /// SQLite-backed [`LeveledMemoryStore`].
    ///
    /// Uses WAL + NORMAL-sync pattern consistent with other SQLite stores in this crate.
    /// Schema is created on first open. Records survive process restarts.
    pub struct SqliteLeveledStore {
        conn: Mutex<Connection>,
    }

    impl SqliteLeveledStore {
        /// Open (or create) a store at `path`.
        pub fn new(path: &str) -> crate::error::Result<Self> {
            let conn = Connection::open(path)
                .map_err(|e| StoreError::ConnectionError(format!("SQLite open: {}", e)))?;
            Self::configure_and_migrate(conn)
        }

        /// Open an in-memory store (tests / ephemeral).
        pub fn in_memory() -> crate::error::Result<Self> {
            let conn = Connection::open_in_memory()
                .map_err(|e| StoreError::ConnectionError(format!("SQLite in-memory: {}", e)))?;
            Self::configure_and_migrate(conn)
        }

        fn configure_and_migrate(conn: Connection) -> crate::error::Result<Self> {
            conn.execute_batch("PRAGMA journal_mode = WAL;")
                .map_err(|e| StoreError::ConnectionError(format!("WAL pragma: {}", e)))?;
            conn.execute_batch("PRAGMA synchronous = NORMAL;")
                .map_err(|e| StoreError::ConnectionError(format!("synchronous pragma: {}", e)))?;
            conn.execute_batch("PRAGMA busy_timeout = 5000;")
                .map_err(|e| StoreError::ConnectionError(format!("busy_timeout pragma: {}", e)))?;

            let store = Self {
                conn: Mutex::new(conn),
            };
            store.create_schema()?;
            Ok(store)
        }

        fn create_schema(&self) -> crate::error::Result<()> {
            let conn = self
                .conn
                .lock()
                .map_err(|e| StoreError::InternalError(format!("mutex poisoned: {}", e)))?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS leveled_memory (
                    id               TEXT NOT NULL PRIMARY KEY,
                    session_id       TEXT NOT NULL,
                    agent_id         TEXT NOT NULL,
                    level            TEXT NOT NULL,
                    key              TEXT NOT NULL,
                    content          TEXT NOT NULL,
                    created_at       TEXT NOT NULL,
                    updated_at       TEXT NOT NULL,
                    ttl_seconds      INTEGER,
                    source_execution_id TEXT,
                    embedding        BLOB,
                    tags             TEXT NOT NULL DEFAULT '[]'
                );
                CREATE INDEX IF NOT EXISTS idx_lm_session_level
                    ON leveled_memory (session_id, level);
                CREATE INDEX IF NOT EXISTS idx_lm_session_key
                    ON leveled_memory (session_id, key);
                CREATE TABLE IF NOT EXISTS leveled_memory_reads (
                    memory_id    TEXT NOT NULL,
                    execution_id TEXT NOT NULL,
                    read_at      TEXT NOT NULL
                );",
            )
            .map_err(|e| StoreError::InternalError(format!("schema creation: {}", e)))?;

            // S25 migration: add embedding BLOB to existing DBs that were created before this column.
            // SQLite does not support ADD COLUMN IF NOT EXISTS; ignore "duplicate column" error.
            match conn.execute_batch("ALTER TABLE leveled_memory ADD COLUMN embedding BLOB") {
                Ok(_) => tracing::info!("S25: embedding column added to leveled_memory"),
                Err(e) if e.to_string().contains("duplicate column") => {
                    // Column already exists (fresh DB created with the new schema above).
                }
                Err(e) => {
                    return Err(StoreError::InternalError(format!(
                        "S25 embedding migration: {}",
                        e
                    )));
                }
            }

            // BT-09 (Hermes benchmark) migration: add tags column to existing DBs.
            match conn.execute_batch(
                "ALTER TABLE leveled_memory ADD COLUMN tags TEXT NOT NULL DEFAULT '[]'",
            ) {
                Ok(_) => tracing::info!("BT-09: tags column added to leveled_memory"),
                Err(e) if e.to_string().contains("duplicate column") => {}
                Err(e) => {
                    return Err(StoreError::InternalError(format!(
                        "BT-09 tags migration: {}",
                        e
                    )));
                }
            }

            Ok(())
        }

        fn lock(&self) -> crate::error::Result<std::sync::MutexGuard<'_, Connection>> {
            self.conn
                .lock()
                .map_err(|e| StoreError::InternalError(format!("mutex poisoned: {}", e)))
        }

        fn level_to_str(level: MemoryLevel) -> &'static str {
            match level {
                MemoryLevel::L0Full => "L0Full",
                MemoryLevel::L1Summary => "L1Summary",
                MemoryLevel::L2Metadata => "L2Metadata",
            }
        }

        fn level_from_str(s: &str) -> crate::error::Result<MemoryLevel> {
            match s {
                "L0Full" => Ok(MemoryLevel::L0Full),
                "L1Summary" => Ok(MemoryLevel::L1Summary),
                "L2Metadata" => Ok(MemoryLevel::L2Metadata),
                other => Err(StoreError::InternalError(format!(
                    "unknown MemoryLevel: {}",
                    other
                ))),
            }
        }

        fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<LeveledMemoryRecord> {
            let id: String = row.get(0)?;
            let session_id: String = row.get(1)?;
            let agent_id: String = row.get(2)?;
            let level_str: String = row.get(3)?;
            let key: String = row.get(4)?;
            let content_str: String = row.get(5)?;
            let created_at_str: String = row.get(6)?;
            let updated_at_str: String = row.get(7)?;
            let ttl_seconds: Option<i64> = row.get(8)?;
            let source_execution_id: Option<String> = row.get(9)?;
            let embedding_bytes: Option<Vec<u8>> = row.get(10)?;
            let tags_json: String = row
                .get::<_, Option<String>>(11)?
                .unwrap_or_else(|| "[]".to_string());

            let level = Self::level_from_str(&level_str).map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(std::fmt::Error),
                )
            })?;
            let content: serde_json::Value = serde_json::from_str(&content_str).map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(std::fmt::Error),
                )
            })?;
            let created_at: chrono::DateTime<chrono::Utc> =
                created_at_str.parse().map_err(|_| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Text,
                        Box::new(std::fmt::Error),
                    )
                })?;
            let updated_at: chrono::DateTime<chrono::Utc> =
                updated_at_str.parse().map_err(|_| {
                    rusqlite::Error::FromSqlConversionFailure(
                        7,
                        rusqlite::types::Type::Text,
                        Box::new(std::fmt::Error),
                    )
                })?;

            let embedding = match embedding_bytes {
                Some(bytes) if !bytes.is_empty() => {
                    bincode::deserialize::<Vec<f32>>(&bytes)
                        .map(Some)
                        .unwrap_or_else(|e| {
                            tracing::warn!(error = %e, "failed to deserialize embedding; treating as None");
                            None
                        })
                }
                _ => None,
            };

            let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();

            Ok(LeveledMemoryRecord {
                id,
                session_id,
                agent_id,
                level,
                key,
                content,
                created_at,
                updated_at,
                ttl_seconds,
                source_execution_id,
                embedding,
                tags,
            })
        }
    }

    #[async_trait::async_trait]
    impl LeveledMemoryStore for SqliteLeveledStore {
        async fn store_leveled(&self, record: LeveledMemoryRecord) -> crate::error::Result<()> {
            let embedding_blob: Option<Vec<u8>> = record
                .embedding
                .as_ref()
                .map(bincode::serialize)
                .transpose()
                .map_err(|e| StoreError::InternalError(format!("embed serialize: {}", e)))?;

            let conn = self.lock()?;
            let tags_json = serde_json::to_string(&record.tags)
                .map_err(|e| StoreError::InternalError(format!("tags json: {}", e)))?;
            conn.execute(
                "INSERT INTO leveled_memory
                    (id, session_id, agent_id, level, key, content, created_at, updated_at, ttl_seconds, source_execution_id, embedding, tags)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id) DO UPDATE SET
                    level = excluded.level,
                    key = excluded.key,
                    content = excluded.content,
                    updated_at = excluded.updated_at,
                    ttl_seconds = excluded.ttl_seconds,
                    source_execution_id = excluded.source_execution_id,
                    embedding = excluded.embedding,
                    tags = excluded.tags",
                params![
                    record.id,
                    record.session_id,
                    record.agent_id,
                    Self::level_to_str(record.level),
                    record.key,
                    serde_json::to_string(&record.content)
                        .map_err(|e| StoreError::InternalError(format!("json: {}", e)))?,
                    record.created_at.to_rfc3339(),
                    record.updated_at.to_rfc3339(),
                    record.ttl_seconds,
                    record.source_execution_id,
                    embedding_blob,
                    tags_json,
                ],
            )
            .map_err(|e| StoreError::InternalError(format!("insert leveled_memory: {}", e)))?;
            Ok(())
        }

        async fn query_by_level(
            &self,
            session_id: &str,
            level: MemoryLevel,
        ) -> crate::error::Result<Vec<LeveledMemoryRecord>> {
            let conn = self.lock()?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, agent_id, level, key, content, created_at, updated_at, ttl_seconds, source_execution_id, embedding, tags
                     FROM leveled_memory
                     WHERE session_id = ?1 AND level = ?2
                     ORDER BY created_at DESC",
                )
                .map_err(|e| StoreError::InternalError(format!("prepare: {}", e)))?;
            let rows = stmt
                .query_map(
                    params![session_id, Self::level_to_str(level)],
                    Self::row_to_record,
                )
                .map_err(|e| StoreError::InternalError(format!("query: {}", e)))?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(|e| StoreError::InternalError(format!("row: {}", e)))?);
            }
            Ok(records)
        }

        async fn query_by_key(
            &self,
            session_id: &str,
            key: &str,
        ) -> crate::error::Result<Option<LeveledMemoryRecord>> {
            let conn = self.lock()?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, session_id, agent_id, level, key, content, created_at, updated_at, ttl_seconds, source_execution_id, embedding, tags
                     FROM leveled_memory
                     WHERE session_id = ?1 AND key = ?2
                     LIMIT 1",
                )
                .map_err(|e| StoreError::InternalError(format!("prepare: {}", e)))?;
            let mut rows = stmt
                .query_map(params![session_id, key], Self::row_to_record)
                .map_err(|e| StoreError::InternalError(format!("query: {}", e)))?;
            match rows.next() {
                Some(row) => {
                    Ok(Some(row.map_err(|e| {
                        StoreError::InternalError(format!("row: {}", e))
                    })?))
                }
                None => Ok(None),
            }
        }

        async fn promote(&self, id: &str, new_level: MemoryLevel) -> crate::error::Result<()> {
            let now = chrono::Utc::now().to_rfc3339();
            let ttl = new_level.default_ttl_seconds();
            let conn = self.lock()?;
            let affected = conn
                .execute(
                    "UPDATE leveled_memory SET level = ?1, ttl_seconds = ?2, updated_at = ?3 WHERE id = ?4",
                    params![Self::level_to_str(new_level), ttl, now, id],
                )
                .map_err(|e| StoreError::InternalError(format!("promote: {}", e)))?;
            if affected == 0 {
                return Err(StoreError::NotFound(format!(
                    "LeveledMemoryRecord {} not found",
                    id
                )));
            }
            Ok(())
        }

        async fn demote(&self, id: &str, new_level: MemoryLevel) -> crate::error::Result<()> {
            let now = chrono::Utc::now().to_rfc3339();
            let ttl = new_level.default_ttl_seconds();
            let conn = self.lock()?;
            let affected = conn
                .execute(
                    "UPDATE leveled_memory SET level = ?1, ttl_seconds = ?2, updated_at = ?3 WHERE id = ?4",
                    params![Self::level_to_str(new_level), ttl, now, id],
                )
                .map_err(|e| StoreError::InternalError(format!("demote: {}", e)))?;
            if affected == 0 {
                return Err(StoreError::NotFound(format!(
                    "LeveledMemoryRecord {} not found",
                    id
                )));
            }
            Ok(())
        }

        async fn expire_stale(&self, max_age: chrono::Duration) -> crate::error::Result<u64> {
            let cutoff = (chrono::Utc::now() - max_age).to_rfc3339();
            let conn = self.lock()?;
            let deleted = conn
                .execute(
                    "DELETE FROM leveled_memory
                     WHERE level != 'L2Metadata'
                       AND (
                           (ttl_seconds IS NOT NULL AND
                            CAST(strftime('%s', 'now') AS INTEGER) - CAST(strftime('%s', created_at) AS INTEGER) > ttl_seconds)
                           OR created_at < ?1
                       )",
                    params![cutoff],
                )
                .map_err(|e| StoreError::InternalError(format!("expire_stale: {}", e)))?;
            Ok(deleted as u64)
        }

        async fn count_by_level(
            &self,
            session_id: &str,
        ) -> crate::error::Result<HashMap<MemoryLevel, usize>> {
            let conn = self.lock()?;
            let mut stmt = conn
                .prepare(
                    "SELECT level, COUNT(*) FROM leveled_memory WHERE session_id = ?1 GROUP BY level",
                )
                .map_err(|e| StoreError::InternalError(format!("prepare: {}", e)))?;
            let mut counts = HashMap::new();
            let rows = stmt
                .query_map(params![session_id], |row| {
                    let level_str: String = row.get(0)?;
                    let count: i64 = row.get(1)?;
                    Ok((level_str, count))
                })
                .map_err(|e| StoreError::InternalError(format!("query: {}", e)))?;
            for row in rows {
                let (level_str, count) =
                    row.map_err(|e| StoreError::InternalError(format!("row: {}", e)))?;
                if let Ok(level) = Self::level_from_str(&level_str) {
                    counts.insert(level, count as usize);
                }
            }
            Ok(counts)
        }

        async fn record_read(
            &self,
            memory_id: &str,
            execution_id: &str,
        ) -> crate::error::Result<()> {
            let conn = self.lock()?;
            conn.execute(
                "INSERT INTO leveled_memory_reads (memory_id, execution_id, read_at) VALUES (?1, ?2, ?3)",
                params![memory_id, execution_id, chrono::Utc::now().to_rfc3339()],
            )
            .map_err(|e| StoreError::InternalError(format!("record_read: {}", e)))?;
            Ok(())
        }

        async fn get_reads(&self, memory_id: &str) -> crate::error::Result<Vec<MemoryReadRecord>> {
            let conn = self.lock()?;
            let mut stmt = conn
                .prepare(
                    "SELECT memory_id, execution_id, read_at FROM leveled_memory_reads WHERE memory_id = ?1",
                )
                .map_err(|e| StoreError::InternalError(format!("prepare: {}", e)))?;
            let rows = stmt
                .query_map(params![memory_id], |row| {
                    let mid: String = row.get(0)?;
                    let eid: String = row.get(1)?;
                    let rat: String = row.get(2)?;
                    Ok((mid, eid, rat))
                })
                .map_err(|e| StoreError::InternalError(format!("query: {}", e)))?;
            let mut result = Vec::new();
            for row in rows {
                let (mid, eid, rat) =
                    row.map_err(|e| StoreError::InternalError(format!("row: {}", e)))?;
                let read_at: chrono::DateTime<chrono::Utc> = rat
                    .parse()
                    .map_err(|e| StoreError::InternalError(format!("parse read_at: {}", e)))?;
                result.push(MemoryReadRecord {
                    memory_id: mid,
                    execution_id: eid,
                    read_at,
                });
            }
            Ok(result)
        }

        async fn delete(&self, memory_id: &str) -> crate::error::Result<()> {
            let conn = self.lock()?;
            let affected = conn
                .execute(
                    "DELETE FROM leveled_memory WHERE id = ?1",
                    params![memory_id],
                )
                .map_err(|e| StoreError::InternalError(format!("delete leveled_memory: {}", e)))?;
            if affected == 0 {
                return Err(StoreError::NotFound(format!(
                    "LeveledMemoryRecord {} not found",
                    memory_id
                )));
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        fn make_record(
            id: &str,
            session_id: &str,
            level: MemoryLevel,
            key: &str,
        ) -> LeveledMemoryRecord {
            let now = chrono::Utc::now();
            LeveledMemoryRecord {
                id: id.to_string(),
                session_id: session_id.to_string(),
                agent_id: "agent-test".to_string(),
                level,
                key: key.to_string(),
                content: json!({"data": key}),
                created_at: now,
                updated_at: now,
                ttl_seconds: level.default_ttl_seconds(),
                source_execution_id: None,
                embedding: None,
                tags: Vec::new(),
            }
        }

        #[allow(dead_code)]
        fn sample_record() -> LeveledMemoryRecord {
            make_record(
                "sample-id",
                "sess-sample",
                MemoryLevel::L1Summary,
                "sample-key",
            )
        }

        #[tokio::test]
        async fn test_sqlite_leveled_store_persists() {
            // Use a unique temp path without the tempfile crate.
            let path = format!(
                "{}/cyberclaw-leveled-test-{}.db",
                std::env::temp_dir().display(),
                uuid::Uuid::new_v4()
            );

            // Write
            {
                let store = SqliteLeveledStore::new(&path).unwrap();
                let r = make_record("r1", "sess-1", MemoryLevel::L1Summary, "key-a");
                store.store_leveled(r).await.unwrap();
            }

            // Re-open same file — record must survive
            {
                let store = SqliteLeveledStore::new(&path).unwrap();
                let records = store
                    .query_by_level("sess-1", MemoryLevel::L1Summary)
                    .await
                    .unwrap();
                assert_eq!(records.len(), 1, "record must persist across reopens");
                assert_eq!(records[0].key, "key-a");
            }

            // Cleanup
            let _ = std::fs::remove_file(&path);
        }

        #[tokio::test]
        async fn test_sqlite_leveled_store_promote_demote() {
            let store = SqliteLeveledStore::in_memory().unwrap();
            let r = make_record("r2", "sess-2", MemoryLevel::L0Full, "key-b");
            store.store_leveled(r).await.unwrap();

            store.promote("r2", MemoryLevel::L1Summary).await.unwrap();
            let records = store
                .query_by_level("sess-2", MemoryLevel::L1Summary)
                .await
                .unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].id, "r2");

            store.demote("r2", MemoryLevel::L0Full).await.unwrap();
            let records_l0 = store
                .query_by_level("sess-2", MemoryLevel::L0Full)
                .await
                .unwrap();
            assert_eq!(records_l0.len(), 1);
        }

        #[tokio::test]
        async fn test_sqlite_leveled_store_count_by_level() {
            let store = SqliteLeveledStore::in_memory().unwrap();
            store
                .store_leveled(make_record("c1", "sess-c", MemoryLevel::L0Full, "k1"))
                .await
                .unwrap();
            store
                .store_leveled(make_record("c2", "sess-c", MemoryLevel::L1Summary, "k2"))
                .await
                .unwrap();
            store
                .store_leveled(make_record("c3", "sess-c", MemoryLevel::L1Summary, "k3"))
                .await
                .unwrap();
            let counts = store.count_by_level("sess-c").await.unwrap();
            assert_eq!(*counts.get(&MemoryLevel::L0Full).unwrap_or(&0), 1);
            assert_eq!(*counts.get(&MemoryLevel::L1Summary).unwrap_or(&0), 2);
        }

        #[tokio::test]
        async fn test_sqlite_leveled_store_record_read() {
            let store = SqliteLeveledStore::in_memory().unwrap();
            store.record_read("mem-1", "exec-1").await.unwrap();
            store.record_read("mem-1", "exec-2").await.unwrap();
            let reads = store.get_reads("mem-1").await.unwrap();
            assert_eq!(reads.len(), 2);
        }
    }
}
