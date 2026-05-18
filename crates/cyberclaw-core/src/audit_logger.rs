/// MEDIUM #9 FIX: 统一审计日志格式
use chrono::{DateTime, Utc};
use std::fmt;

#[derive(Debug, Clone)]
pub struct AuditLogEntry {
    pub timestamp: DateTime<Utc>,
    pub component: String,
    pub operation: String,
    pub message: String,
    pub trace_id: Option<String>,
    pub severity: LogSeverity,
    pub metadata: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSeverity {
    Debug,
    Info,
    Warn,
    Error,
    Critical,
}

impl AuditLogEntry {
    pub fn new(
        component: impl Into<String>,
        operation: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            component: component.into(),
            operation: operation.into(),
            message: message.into(),
            trace_id: None,
            severity: LogSeverity::Info,
            metadata: std::collections::HashMap::new(),
        }
    }

    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    pub fn with_severity(mut self, severity: LogSeverity) -> Self {
        self.severity = severity;
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// MEDIUM #9: 格式化为统一格式
    /// 格式：[{component}:{operation}] {message} | trace_id: {trace_id} | {metadata}
    pub fn format(&self) -> String {
        // 构建基本部分：[component:operation] message
        let mut result = format!("[{}:{}] {}", self.component, self.operation, self.message);

        // 添加 trace_id（如果存在）
        if let Some(trace_id) = &self.trace_id {
            result.push_str(&format!(" | trace_id: {}", trace_id));
        }

        // 添加 metadata（如果存在）
        if !self.metadata.is_empty() {
            let metadata_str = self
                .metadata
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(", ");
            result.push_str(&format!(" | {}", metadata_str));
        }

        result
    }
}

impl fmt::Display for AuditLogEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format())
    }
}

/// MEDIUM #9: 审计日志记录器 trait
pub trait AuditLogger: Send + Sync {
    fn log(&self, entry: AuditLogEntry);

    fn log_info(&self, component: &str, operation: &str, message: &str, trace_id: Option<&str>) {
        let mut entry =
            AuditLogEntry::new(component, operation, message).with_severity(LogSeverity::Info);

        if let Some(tid) = trace_id {
            entry = entry.with_trace_id(tid);
        }

        self.log(entry);
    }

    fn log_error(&self, component: &str, operation: &str, message: &str, trace_id: Option<&str>) {
        let mut entry =
            AuditLogEntry::new(component, operation, message).with_severity(LogSeverity::Error);

        if let Some(tid) = trace_id {
            entry = entry.with_trace_id(tid);
        }

        self.log(entry);
    }
}

/// 默认实现：记录到 tracing
pub struct DefaultAuditLogger;

impl AuditLogger for DefaultAuditLogger {
    fn log(&self, entry: AuditLogEntry) {
        match entry.severity {
            LogSeverity::Debug => tracing::debug!("{}", entry.format()),
            LogSeverity::Info => tracing::info!("{}", entry.format()),
            LogSeverity::Warn => tracing::warn!("{}", entry.format()),
            LogSeverity::Error => tracing::error!("{}", entry.format()),
            LogSeverity::Critical => tracing::error!("CRITICAL: {}", entry.format()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_log_format_basic() {
        let entry = AuditLogEntry::new("TestComponent", "test.operation", "Test message");

        let formatted = entry.format();
        assert_eq!(formatted, "[TestComponent:test.operation] Test message");
    }

    #[test]
    fn test_audit_log_format_with_trace_id() {
        let entry = AuditLogEntry::new("TestComponent", "test.operation", "Test message")
            .with_trace_id("trace-123");

        let formatted = entry.format();
        assert_eq!(
            formatted,
            "[TestComponent:test.operation] Test message | trace_id: trace-123"
        );
    }

    #[test]
    fn test_audit_log_format_with_metadata() {
        let entry = AuditLogEntry::new("TestComponent", "test.operation", "Test message")
            .with_trace_id("trace-123")
            .with_metadata("key1", "value1")
            .with_metadata("key2", "value2");

        let formatted = entry.format();
        assert!(formatted.contains("[TestComponent:test.operation]"));
        assert!(formatted.contains("trace_id: trace-123"));
        assert!(formatted.contains("key1=value1"));
        assert!(formatted.contains("key2=value2"));
    }

    #[test]
    fn test_log_severity() {
        let entry = AuditLogEntry::new("TestComponent", "test.operation", "Test message")
            .with_severity(LogSeverity::Critical);

        assert_eq!(entry.severity, LogSeverity::Critical);
    }

    #[test]
    fn test_audit_logger_trait() {
        let logger = DefaultAuditLogger;

        // 测试 log_info
        logger.log_info(
            "TestComponent",
            "test.info",
            "Info message",
            Some("trace-123"),
        );

        // 测试 log_error
        logger.log_error(
            "TestComponent",
            "test.error",
            "Error message",
            Some("trace-456"),
        );
    }
}
