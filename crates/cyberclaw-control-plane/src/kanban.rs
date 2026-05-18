//! Kanban dispatcher — task board for distributing work between
//! orchestrator and worker agents.
//!
//! Mirrors Hermes v0.12 `tools/kanban_tools.py` + `hermes kanban` CLI.
//! Backed by a single SQLite file at `<state_root>/kanban.db`.
//!
//! # Lifecycle
//!
//! ```text
//!   create_task ──► todo ──► claim ──► in_progress ──► complete ──► done
//!                                  │                ╲
//!                                  │                 ╲
//!                                  └──► block ──► blocked ──► unblock ──► todo
//! ```
//!
//! # Why SQLite
//!
//! Hermes uses `~/.hermes/kanban.db`. We follow the same shape so
//! operators can `sqlite3 kanban.db` and inspect by hand. No HTTP
//! dependency in this module — the API endpoint is layered on top.

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KanbanStatus {
    Todo,
    InProgress,
    Blocked,
    Done,
    Cancelled,
}

impl KanbanStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "todo" => Some(Self::Todo),
            "in_progress" => Some(Self::InProgress),
            "blocked" => Some(Self::Blocked),
            "done" => Some(Self::Done),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KanbanTask {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: KanbanStatus,
    /// Worker agent that claimed this task. None = unclaimed.
    pub assigned_to: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Free-form metadata (e.g. priority, labels).
    pub metadata: serde_json::Value,
}

/// Sparse field-level update for [`KanbanBoard::update_task`].
///
/// Every field is `Option`-wrapped; only `Some(_)` fields are applied
/// (COALESCE semantics). `claimed_by: Some(None)` explicitly clears the
/// `assigned_to` column; `claimed_by: None` leaves it untouched.
#[derive(Debug, Clone, Default)]
pub struct KanbanTaskUpdate {
    pub title: Option<String>,
    pub body: Option<String>,
    pub status: Option<KanbanStatus>,
    pub priority: Option<u8>,
    pub due_at: Option<i64>,
    pub claimed_by: Option<Option<String>>,
}

#[derive(Debug)]
pub struct KanbanBoard {
    conn: Arc<Mutex<Connection>>,
}

impl KanbanBoard {
    /// Open or create a board at the given path.
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        let board = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        board.init_schema()?;
        Ok(board)
    }

    /// Open an in-memory board (for tests).
    pub fn in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let board = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        board.init_schema()?;
        Ok(board)
    }

    fn init_schema(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("kanban mutex poisoned");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS kanban_tasks (
                id           TEXT PRIMARY KEY,
                title        TEXT NOT NULL,
                description  TEXT NOT NULL DEFAULT '',
                status       TEXT NOT NULL,
                assigned_to  TEXT,
                created_at   TEXT NOT NULL,
                updated_at   TEXT NOT NULL,
                metadata     TEXT NOT NULL DEFAULT '{}'
            );
            CREATE INDEX IF NOT EXISTS idx_kanban_status
                ON kanban_tasks (status);
            CREATE INDEX IF NOT EXISTS idx_kanban_assigned
                ON kanban_tasks (assigned_to);",
        )
    }

    /// Create a new task in `Todo` status. Returns the generated id.
    pub fn create_task(
        &self,
        title: impl Into<String>,
        description: impl Into<String>,
        metadata: Option<serde_json::Value>,
    ) -> rusqlite::Result<KanbanTask> {
        let now = Utc::now();
        let task = KanbanTask {
            id: format!("kt-{}", Uuid::new_v4()),
            title: title.into(),
            description: description.into(),
            status: KanbanStatus::Todo,
            assigned_to: None,
            created_at: now,
            updated_at: now,
            metadata: metadata.unwrap_or_else(|| serde_json::json!({})),
        };
        let conn = self.conn.lock().expect("kanban mutex poisoned");
        conn.execute(
            "INSERT INTO kanban_tasks
                (id, title, description, status, assigned_to, created_at, updated_at, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                task.id,
                task.title,
                task.description,
                task.status.as_str(),
                task.assigned_to,
                task.created_at.to_rfc3339(),
                task.updated_at.to_rfc3339(),
                task.metadata.to_string(),
            ],
        )?;
        Ok(task)
    }

    /// Atomically claim the oldest `Todo` task for `worker`. Returns
    /// `None` if no task is available. Used by worker agents pulling
    /// work in a loop.
    pub fn claim_next(&self, worker: &str) -> rusqlite::Result<Option<KanbanTask>> {
        let conn = self.conn.lock().expect("kanban mutex poisoned");
        let now = Utc::now();
        let tx = conn.unchecked_transaction()?;
        let id_opt: Option<String> = tx
            .query_row(
                "SELECT id FROM kanban_tasks
                 WHERE status = 'todo' AND assigned_to IS NULL
                 ORDER BY created_at ASC
                 LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let Some(id) = id_opt else {
            tx.commit()?;
            return Ok(None);
        };
        tx.execute(
            "UPDATE kanban_tasks
             SET status = 'in_progress', assigned_to = ?1, updated_at = ?2
             WHERE id = ?3 AND status = 'todo'",
            params![worker, now.to_rfc3339(), id],
        )?;
        tx.commit()?;
        drop(conn);
        self.get(&id)
    }

    /// Mark a task as complete (status=done). Idempotent.
    pub fn complete(&self, id: &str) -> rusqlite::Result<()> {
        self.update_status(id, KanbanStatus::Done)
    }

    /// Mark a task as blocked.
    pub fn block(&self, id: &str, reason: Option<&str>) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("kanban mutex poisoned");
        let now = Utc::now();
        if let Some(r) = reason {
            conn.execute(
                "UPDATE kanban_tasks
                 SET status = 'blocked', updated_at = ?1,
                     metadata = json_set(metadata, '$.block_reason', ?2)
                 WHERE id = ?3",
                params![now.to_rfc3339(), r, id],
            )?;
        } else {
            self.do_update_status_locked(&conn, id, KanbanStatus::Blocked, now)?;
        }
        Ok(())
    }

    /// Move a blocked task back to todo.
    pub fn unblock(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("kanban mutex poisoned");
        let now = Utc::now();
        conn.execute(
            "UPDATE kanban_tasks
             SET status = 'todo', assigned_to = NULL, updated_at = ?1
             WHERE id = ?2 AND status = 'blocked'",
            params![now.to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Cancel a task (terminal state, distinct from done).
    pub fn cancel(&self, id: &str) -> rusqlite::Result<()> {
        self.update_status(id, KanbanStatus::Cancelled)
    }

    fn update_status(&self, id: &str, status: KanbanStatus) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("kanban mutex poisoned");
        self.do_update_status_locked(&conn, id, status, Utc::now())
    }

    fn do_update_status_locked(
        &self,
        conn: &Connection,
        id: &str,
        status: KanbanStatus,
        now: DateTime<Utc>,
    ) -> rusqlite::Result<()> {
        conn.execute(
            "UPDATE kanban_tasks
             SET status = ?1, updated_at = ?2
             WHERE id = ?3",
            params![status.as_str(), now.to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Fetch a task by id.
    pub fn get(&self, id: &str) -> rusqlite::Result<Option<KanbanTask>> {
        let conn = self.conn.lock().expect("kanban mutex poisoned");
        conn.query_row(
            "SELECT id, title, description, status, assigned_to, created_at, updated_at, metadata
             FROM kanban_tasks WHERE id = ?1",
            params![id],
            row_to_task,
        )
        .optional()
    }

    /// List all tasks with optional status filter, sorted oldest first.
    pub fn list(&self, status: Option<KanbanStatus>) -> rusqlite::Result<Vec<KanbanTask>> {
        let conn = self.conn.lock().expect("kanban mutex poisoned");
        let mut stmt;
        let mut rows = if let Some(s) = status {
            stmt = conn.prepare(
                "SELECT id, title, description, status, assigned_to, created_at, updated_at, metadata
                 FROM kanban_tasks WHERE status = ?1 ORDER BY created_at ASC",
            )?;
            stmt.query(params![s.as_str()])?
        } else {
            stmt = conn.prepare(
                "SELECT id, title, description, status, assigned_to, created_at, updated_at, metadata
                 FROM kanban_tasks ORDER BY created_at ASC",
            )?;
            stmt.query([])?
        };
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_task(row)?);
        }
        Ok(out)
    }

    /// Patch arbitrary fields on an existing task. Each `Some` field is
    /// applied; each `None` field leaves the existing value untouched
    /// (`COALESCE` semantics). Returns the post-update task on success,
    /// or `Ok(None)` when no row matched `id`.
    ///
    /// `priority` and `due_at` live under the freeform `metadata` JSON
    /// column (the historical schema has no dedicated columns) and are
    /// merged with `json_set`. `status` strings must round-trip through
    /// [`KanbanStatus::from_str`].
    ///
    /// F-4 (commits aabcf34 / d9582c7 H-8 follow-up): the previous admin
    /// PUT handler refused these fields with `InvalidInput` because the
    /// board offered no real update path. This method closes that gap.
    pub fn update_task(
        &self,
        id: &str,
        fields: KanbanTaskUpdate,
    ) -> rusqlite::Result<Option<KanbanTask>> {
        let conn = self.conn.lock().expect("kanban mutex poisoned");
        // First confirm the row exists; otherwise return None (callers map
        // this to NotFound).
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM kanban_tasks WHERE id = ?1",
                params![id],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !exists {
            return Ok(None);
        }

        let now = Utc::now();
        let status_str = fields.status.map(|s| s.as_str().to_string());
        // claimed_by is a 3-state input: None → leave alone, Some(Some(s)) →
        // set, Some(None) → clear. We collapse to (apply?, value).
        let (claim_apply, claim_value): (i64, Option<String>) = match fields.claimed_by {
            Some(opt) => (1, opt),
            None => (0, None),
        };
        // Build a single UPDATE that COALESCEs every nullable input. For
        // priority + due_at we use json_set on the metadata column so
        // unspecified keys stay where they were.
        conn.execute(
            "UPDATE kanban_tasks SET
                title       = COALESCE(?1, title),
                description = COALESCE(?2, description),
                status      = COALESCE(?3, status),
                assigned_to = CASE WHEN ?4 = 1 THEN ?5 ELSE assigned_to END,
                metadata    = (
                    SELECT
                        CASE
                            WHEN ?6 IS NOT NULL AND ?7 IS NOT NULL THEN
                                json_set(json_set(metadata, '$.priority', ?6), '$.due_at', ?7)
                            WHEN ?6 IS NOT NULL THEN
                                json_set(metadata, '$.priority', ?6)
                            WHEN ?7 IS NOT NULL THEN
                                json_set(metadata, '$.due_at', ?7)
                            ELSE metadata
                        END
                ),
                updated_at  = ?8
             WHERE id = ?9",
            params![
                fields.title,
                fields.body,
                status_str,
                claim_apply,
                claim_value,
                fields.priority.map(|p| p as i64),
                fields.due_at,
                now.to_rfc3339(),
                id,
            ],
        )?;

        // Re-read the row so callers always see the canonical post-write
        // state (avoids parsing the same JSON on the way out).
        drop(conn);
        self.get(id)
    }

    /// Aggregate counts per status — used by the orchestrator dashboard.
    pub fn stats(&self) -> rusqlite::Result<std::collections::HashMap<String, i64>> {
        let conn = self.conn.lock().expect("kanban mutex poisoned");
        let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM kanban_tasks GROUP BY status")?;
        let mut rows = stmt.query([])?;
        let mut out = std::collections::HashMap::new();
        while let Some(row) = rows.next()? {
            let status: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            out.insert(status, count);
        }
        Ok(out)
    }
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<KanbanTask> {
    let id: String = row.get(0)?;
    let title: String = row.get(1)?;
    let description: String = row.get(2)?;
    let status_str: String = row.get(3)?;
    let assigned_to: Option<String> = row.get(4)?;
    let created_at_str: String = row.get(5)?;
    let updated_at_str: String = row.get(6)?;
    let metadata_str: String = row.get(7)?;

    let status = KanbanStatus::from_str(&status_str).unwrap_or(KanbanStatus::Todo);
    let created_at: DateTime<Utc> = created_at_str.parse::<DateTime<Utc>>().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            Box::new(std::fmt::Error),
        )
    })?;
    let updated_at: DateTime<Utc> = updated_at_str.parse::<DateTime<Utc>>().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Text,
            Box::new(std::fmt::Error),
        )
    })?;
    let metadata: serde_json::Value =
        serde_json::from_str(&metadata_str).unwrap_or(serde_json::json!({}));

    Ok(KanbanTask {
        id,
        title,
        description,
        status,
        assigned_to,
        created_at,
        updated_at,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_task_starts_in_todo_unclaimed() {
        let board = KanbanBoard::in_memory().unwrap();
        let t = board.create_task("review PR", "PR #42", None).unwrap();
        assert_eq!(t.status, KanbanStatus::Todo);
        assert!(t.assigned_to.is_none());
        assert!(t.id.starts_with("kt-"));
    }

    #[test]
    fn claim_next_returns_oldest_todo_and_marks_in_progress() {
        let board = KanbanBoard::in_memory().unwrap();
        let _a = board.create_task("a", "", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let _b = board.create_task("b", "", None).unwrap();

        let claimed = board.claim_next("worker-1").unwrap().expect("got task");
        assert_eq!(claimed.title, "a", "should claim oldest first");
        assert_eq!(claimed.status, KanbanStatus::InProgress);
        assert_eq!(claimed.assigned_to.as_deref(), Some("worker-1"));
    }

    #[test]
    fn claim_next_on_empty_returns_none() {
        let board = KanbanBoard::in_memory().unwrap();
        assert!(board.claim_next("worker-1").unwrap().is_none());
    }

    #[test]
    fn complete_moves_task_to_done() {
        let board = KanbanBoard::in_memory().unwrap();
        let t = board.create_task("a", "", None).unwrap();
        board.claim_next("w1").unwrap();
        board.complete(&t.id).unwrap();
        let after = board.get(&t.id).unwrap().unwrap();
        assert_eq!(after.status, KanbanStatus::Done);
    }

    #[test]
    fn block_then_unblock_restores_todo() {
        let board = KanbanBoard::in_memory().unwrap();
        let t = board.create_task("a", "", None).unwrap();
        board.claim_next("w1").unwrap();
        board.block(&t.id, Some("waiting on review")).unwrap();
        let blocked = board.get(&t.id).unwrap().unwrap();
        assert_eq!(blocked.status, KanbanStatus::Blocked);
        assert_eq!(
            blocked
                .metadata
                .get("block_reason")
                .and_then(|v| v.as_str()),
            Some("waiting on review")
        );

        board.unblock(&t.id).unwrap();
        let after = board.get(&t.id).unwrap().unwrap();
        assert_eq!(after.status, KanbanStatus::Todo);
        assert!(after.assigned_to.is_none());
    }

    #[test]
    fn cancel_is_terminal_and_distinct_from_done() {
        let board = KanbanBoard::in_memory().unwrap();
        let t = board.create_task("a", "", None).unwrap();
        board.cancel(&t.id).unwrap();
        let after = board.get(&t.id).unwrap().unwrap();
        assert_eq!(after.status, KanbanStatus::Cancelled);
    }

    #[test]
    fn list_filters_by_status() {
        let board = KanbanBoard::in_memory().unwrap();
        let a = board.create_task("a", "", None).unwrap();
        let _b = board.create_task("b", "", None).unwrap();
        board.complete(&a.id).unwrap();

        let todos = board.list(Some(KanbanStatus::Todo)).unwrap();
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].title, "b");

        let dones = board.list(Some(KanbanStatus::Done)).unwrap();
        assert_eq!(dones.len(), 1);
        assert_eq!(dones[0].title, "a");

        let all = board.list(None).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn stats_aggregates_status_counts() {
        let board = KanbanBoard::in_memory().unwrap();
        let a = board.create_task("a", "", None).unwrap();
        let _b = board.create_task("b", "", None).unwrap();
        let _c = board.create_task("c", "", None).unwrap();
        board.complete(&a.id).unwrap();

        let stats = board.stats().unwrap();
        assert_eq!(stats.get("todo").copied(), Some(2));
        assert_eq!(stats.get("done").copied(), Some(1));
    }

    #[test]
    fn metadata_round_trips() {
        let board = KanbanBoard::in_memory().unwrap();
        let t = board
            .create_task(
                "a",
                "",
                Some(serde_json::json!({"priority": "high", "labels": ["bug", "p0"]})),
            )
            .unwrap();
        let after = board.get(&t.id).unwrap().unwrap();
        assert_eq!(after.metadata["priority"], "high");
        assert_eq!(after.metadata["labels"][0], "bug");
    }

    /// F-4: partial update — only title set; body, status, priority,
    /// due_at, claimed_by remain at their pre-update values.
    #[test]
    fn update_task_partial_only_applies_set_fields() {
        let board = KanbanBoard::in_memory().unwrap();
        let t = board
            .create_task(
                "before",
                "body-before",
                Some(serde_json::json!({"priority": 3, "due_at": 1234})),
            )
            .unwrap();
        let updated = board
            .update_task(
                &t.id,
                KanbanTaskUpdate {
                    title: Some("after".to_string()),
                    ..Default::default()
                },
            )
            .unwrap()
            .expect("row exists");
        assert_eq!(updated.title, "after");
        assert_eq!(updated.description, "body-before");
        assert_eq!(updated.status, KanbanStatus::Todo);
        assert_eq!(
            updated.metadata.get("priority").and_then(|v| v.as_i64()),
            Some(3)
        );
        assert_eq!(
            updated.metadata.get("due_at").and_then(|v| v.as_i64()),
            Some(1234)
        );
    }

    /// F-4: status-only update goes through the same path; the board's
    /// transition methods (complete/block/...) are NOT bypassed for
    /// invariants — they're independent helpers, and update_task is the
    /// generic field-patch surface.
    #[test]
    fn update_task_status_only() {
        let board = KanbanBoard::in_memory().unwrap();
        let t = board.create_task("a", "", None).unwrap();
        let updated = board
            .update_task(
                &t.id,
                KanbanTaskUpdate {
                    status: Some(KanbanStatus::InProgress),
                    ..Default::default()
                },
            )
            .unwrap()
            .expect("row exists");
        assert_eq!(updated.status, KanbanStatus::InProgress);
    }

    /// F-4: full multi-field update with priority, due_at, claimed_by.
    #[test]
    fn update_task_full_update_applies_all() {
        let board = KanbanBoard::in_memory().unwrap();
        let t = board.create_task("a", "old", None).unwrap();
        let updated = board
            .update_task(
                &t.id,
                KanbanTaskUpdate {
                    title: Some("new title".to_string()),
                    body: Some("new body".to_string()),
                    status: Some(KanbanStatus::Blocked),
                    priority: Some(9),
                    due_at: Some(987654),
                    claimed_by: Some(Some("worker-x".to_string())),
                },
            )
            .unwrap()
            .expect("row exists");
        assert_eq!(updated.title, "new title");
        assert_eq!(updated.description, "new body");
        assert_eq!(updated.status, KanbanStatus::Blocked);
        assert_eq!(
            updated.metadata.get("priority").and_then(|v| v.as_i64()),
            Some(9)
        );
        assert_eq!(
            updated.metadata.get("due_at").and_then(|v| v.as_i64()),
            Some(987654)
        );
        assert_eq!(updated.assigned_to.as_deref(), Some("worker-x"));
    }

    /// F-4: unknown id → Ok(None). Callers map this to NotFound.
    #[test]
    fn update_task_unknown_id_returns_none() {
        let board = KanbanBoard::in_memory().unwrap();
        let res = board
            .update_task(
                "kt-does-not-exist",
                KanbanTaskUpdate {
                    title: Some("x".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(res.is_none());
    }

    #[test]
    fn persistent_open_recovers_previous_data() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let board1 = KanbanBoard::open(&path).unwrap();
        let t = board1.create_task("persistent", "x", None).unwrap();
        drop(board1);

        let board2 = KanbanBoard::open(&path).unwrap();
        let restored = board2.get(&t.id).unwrap().expect("task should persist");
        assert_eq!(restored.title, "persistent");
    }
}
