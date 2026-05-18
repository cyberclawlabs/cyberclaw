use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogEntry {
    pub index: u64,
    pub term: u64,
    pub command: Vec<u8>,
}

impl LogEntry {
    pub fn new(index: u64, term: u64, command: Vec<u8>) -> Self {
        Self {
            index,
            term,
            command,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RaftLog {
    /// Log entries
    entries: VecDeque<LogEntry>,

    /// Index of highest log entry known to be committed
    commit_index: u64,

    /// Index of highest log entry applied to state machine
    last_applied: u64,

    /// Last snapshot index (for log compaction)
    snapshot_index: u64,

    /// Last snapshot term
    snapshot_term: u64,
}

impl RaftLog {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            commit_index: 0,
            last_applied: 0,
            snapshot_index: 0,
            snapshot_term: 0,
        }
    }

    /// Append a new entry to the log
    pub fn append(&mut self, entry: LogEntry) -> u64 {
        debug!(
            "Appending log entry: index={}, term={}",
            entry.index, entry.term
        );

        // Remove any conflicting entries
        if let Some(pos) = self.find_entry_position(entry.index) {
            self.entries.truncate(pos);
        }

        let index = entry.index;
        self.entries.push_back(entry);
        index
    }

    /// Append multiple entries
    pub fn append_entries(&mut self, entries: Vec<LogEntry>) -> Option<u64> {
        if entries.is_empty() {
            return None;
        }

        let first_index = entries[0].index;

        // Find position and truncate if needed
        if let Some(pos) = self.find_entry_position(first_index) {
            self.entries.truncate(pos);
        }

        let last_index = entries.last().map(|e| e.index);
        self.entries.extend(entries);
        last_index
    }

    /// Get entry at specific index
    pub fn get(&self, index: u64) -> Option<&LogEntry> {
        if index == 0 || index <= self.snapshot_index {
            return None;
        }

        let pos = (index - self.snapshot_index - 1) as usize;
        self.entries.get(pos)
    }

    /// Get entries from index (inclusive)
    pub fn get_from(&self, from_index: u64) -> Vec<LogEntry> {
        if from_index <= self.snapshot_index {
            self.entries.iter().cloned().collect()
        } else {
            let pos = (from_index - self.snapshot_index - 1) as usize;
            self.entries.iter().skip(pos).cloned().collect()
        }
    }

    /// Truncate log from index (exclusive)
    pub fn truncate(&mut self, from_index: u64) {
        if from_index <= self.snapshot_index {
            self.entries.clear();
        } else {
            let pos = (from_index - self.snapshot_index - 1) as usize;
            self.entries.truncate(pos);
        }
    }

    /// Get last log index
    pub fn last_index(&self) -> u64 {
        self.entries
            .back()
            .map(|e| e.index)
            .unwrap_or(self.snapshot_index)
    }

    /// Get last log term
    pub fn last_term(&self) -> u64 {
        self.entries
            .back()
            .map(|e| e.term)
            .unwrap_or(self.snapshot_term)
    }

    /// Get term of entry at index
    pub fn term_at(&self, index: u64) -> Option<u64> {
        if index == self.snapshot_index {
            Some(self.snapshot_term)
        } else {
            self.get(index).map(|e| e.term)
        }
    }

    /// Check if log contains entry with given index and term
    pub fn contains(&self, index: u64, term: u64) -> bool {
        if index == self.snapshot_index {
            return term == self.snapshot_term;
        }
        self.get(index).is_some_and(|e| e.term == term)
    }

    /// Get commit index
    pub fn commit_index(&self) -> u64 {
        self.commit_index
    }

    /// Update commit index
    pub fn update_commit_index(&mut self, new_commit: u64) {
        if new_commit > self.commit_index {
            self.commit_index = new_commit;
            debug!("Updated commit index to {}", new_commit);
        }
    }

    /// Get last applied index
    pub fn last_applied(&self) -> u64 {
        self.last_applied
    }

    /// Mark entry as applied
    pub fn mark_applied(&mut self, index: u64) {
        if index > self.last_applied {
            self.last_applied = index;
        }
    }

    /// Get entries that need to be applied
    pub fn get_unapplied(&self) -> Vec<LogEntry> {
        let start = self.last_applied + 1;
        let end = self.commit_index + 1;

        (start..end)
            .filter_map(|idx| self.get(idx).cloned())
            .collect()
    }

    /// Create snapshot up to index
    pub fn create_snapshot(&mut self, index: u64, term: u64) {
        if index <= self.snapshot_index {
            return;
        }

        // Remove entries before snapshot
        let keep_from = (index - self.snapshot_index) as usize;
        self.entries = self.entries.split_off(keep_from);

        self.snapshot_index = index;
        self.snapshot_term = term;

        debug!("Created snapshot at index={}, term={}", index, term);
    }

    /// Install snapshot
    pub fn install_snapshot(&mut self, index: u64, term: u64) {
        self.entries.clear();
        self.snapshot_index = index;
        self.snapshot_term = term;
        self.commit_index = index;
        self.last_applied = index;

        debug!("Installed snapshot at index={}, term={}", index, term);
    }

    /// Find position of entry with given index
    fn find_entry_position(&self, index: u64) -> Option<usize> {
        if index <= self.snapshot_index {
            Some(0)
        } else {
            Some((index - self.snapshot_index - 1) as usize)
        }
    }
}

impl Default for RaftLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_append_and_get() {
        let mut log = RaftLog::new();

        let entry1 = LogEntry::new(1, 1, vec![1, 2, 3]);
        let entry2 = LogEntry::new(2, 1, vec![4, 5, 6]);

        log.append(entry1.clone());
        log.append(entry2.clone());

        assert_eq!(log.last_index(), 2);
        assert_eq!(log.last_term(), 1);
        assert_eq!(log.get(1), Some(&entry1));
        assert_eq!(log.get(2), Some(&entry2));
        assert_eq!(log.get(3), None);
    }

    #[test]
    fn test_truncate() {
        let mut log = RaftLog::new();

        for i in 1..=5 {
            log.append(LogEntry::new(i, 1, vec![i as u8]));
        }

        log.truncate(3);
        assert_eq!(log.last_index(), 2);
        assert!(log.get(3).is_none());
    }

    #[test]
    fn test_commit_and_apply() {
        let mut log = RaftLog::new();

        for i in 1..=5 {
            log.append(LogEntry::new(i, 1, vec![i as u8]));
        }

        log.update_commit_index(3);
        assert_eq!(log.commit_index(), 3);

        let unapplied = log.get_unapplied();
        assert_eq!(unapplied.len(), 3);

        log.mark_applied(3);
        let unapplied = log.get_unapplied();
        assert_eq!(unapplied.len(), 0);
    }

    #[test]
    fn test_snapshot() {
        let mut log = RaftLog::new();

        for i in 1..=10 {
            log.append(LogEntry::new(i, 1, vec![i as u8]));
        }

        log.create_snapshot(5, 1);
        assert_eq!(log.snapshot_index, 5);
        assert_eq!(log.last_index(), 10);
        assert!(log.get(4).is_none());
        assert!(log.get(6).is_some());
    }

    #[test]
    fn test_contains() {
        let mut log = RaftLog::new();

        log.append(LogEntry::new(1, 1, vec![1]));
        log.append(LogEntry::new(2, 2, vec![2]));

        assert!(log.contains(1, 1));
        assert!(log.contains(2, 2));
        assert!(!log.contains(1, 2));
        assert!(!log.contains(3, 1));
    }
}
