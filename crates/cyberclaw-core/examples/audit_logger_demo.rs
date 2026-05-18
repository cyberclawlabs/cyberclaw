/// MEDIUM #9: 统一审计日志格式示例
use cyberclaw_core::audit_logger::{AuditLogEntry, AuditLogger, DefaultAuditLogger, LogSeverity};

fn main() {
    let audit_logger = DefaultAuditLogger;

    println!("=== MEDIUM #9: 统一审计日志格式示例 ===\n");

    // 示例 1: 基本日志
    println!("1. 基本日志:");
    let entry = AuditLogEntry::new("DemoComponent", "demo.operation", "Basic log message");
    println!("   格式化输出: {}", entry.format());
    audit_logger.log(entry);

    println!();

    // 示例 2: 带 trace_id 的日志
    println!("2. 带 trace_id 的日志:");
    let entry = AuditLogEntry::new(
        "ExecutionService",
        "execution.start",
        "Starting execution abc123",
    )
    .with_trace_id("trace-xyz-789");
    println!("   格式化输出: {}", entry.format());
    audit_logger.log(entry);

    println!();

    // 示例 3: 带 metadata 的日志
    println!("3. 带多个 metadata 的日志:");
    let entry = AuditLogEntry::new(
        "CapabilityDispatcher",
        "capability.dispatch",
        "Dispatching capability foo to connector bar",
    )
    .with_trace_id("trace-123-456")
    .with_metadata("execution_id", "exec-001")
    .with_metadata("capability_id", "foo")
    .with_metadata("connector_id", "bar");
    println!("   格式化输出: {}", entry.format());
    audit_logger.log(entry);

    println!();

    // 示例 4: 错误级别日志
    println!("4. 错误级别日志:");
    let entry = AuditLogEntry::new(
        "ExecutionService",
        "execution.failed",
        "Execution failed: Connection timeout",
    )
    .with_trace_id("trace-error-999")
    .with_metadata("execution_id", "exec-002")
    .with_metadata("error", "Connection timeout")
    .with_severity(LogSeverity::Error);
    println!("   格式化输出: {}", entry.format());
    audit_logger.log(entry);

    println!();

    // 示例 5: 使用辅助方法
    println!("5. 使用 log_info 辅助方法:");
    audit_logger.log_info(
        "MemoryContextProvider",
        "memory.compact",
        "Memory compaction successful",
        Some("trace-mem-777"),
    );

    println!();

    println!("=== 所有示例完成 ===");
}
