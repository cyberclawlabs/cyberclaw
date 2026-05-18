//! Fuzz Target 3: Prompt Injection & Secret 安全检测器
//!
//! 对 `PromptInjectionScanner`、`SecretScanner`、`CommandSafetyScanner`
//! 的扫描方法进行随机输入测试，确保：
//! - 正则匹配不会 panic（ReDoS 防护验证）
//! - 任意 UTF-8/二进制输入不会导致崩溃
//! - `redact_all` 方法对任意输入安全处理

#![no_main]

use libfuzzer_sys::fuzz_target;

use cyberclaw_core::ids::TraceId;
use cyberclaw_core::security_scanner::{
    CommandSafetyScanner, PromptInjectionScanner, SecretScanner,
};

fuzz_target!(|data: &[u8]| {
    // Only process valid UTF-8 (security scanners work on text)
    // For non-UTF-8, use lossy conversion to test scanner robustness
    let lossy_string;
    let text: &str = match std::str::from_utf8(data) {
        Ok(s) => s,
        Err(_) => {
            lossy_string = String::from_utf8_lossy(data).into_owned();
            &lossy_string
        }
    };

    let trace_id = TraceId::new();

    // Test 1: PromptInjectionScanner — must never panic on arbitrary text
    // This uses regex internally; test for ReDoS vulnerabilities
    let scanner = PromptInjectionScanner::new();
    let events = scanner.scan(text, trace_id.clone(), None, None);
    // Verify scan result is consistent (no panics during result inspection)
    let _ = events.len();

    // Test 2: SecretScanner.scan — must never panic on arbitrary text
    let secret_scanner = SecretScanner::new();
    let secret_events = secret_scanner.scan(text, trace_id.clone(), None);
    let _ = secret_events.len();

    // Test 3: SecretScanner.redact_all — must never panic or return corrupt data
    let redacted = secret_scanner.redact_all(text);
    // Redacted output must be valid UTF-8 (no corruption)
    assert!(std::str::from_utf8(redacted.as_bytes()).is_ok());

    // Test 4: CommandSafetyScanner — must never panic on arbitrary command strings
    let cmd_scanner = CommandSafetyScanner::new();
    let cmd_events = cmd_scanner.scan(text, trace_id.clone(), None, None);
    let _ = cmd_events.len();

    // Test 5: Verify that scanning a string twice gives the same result (determinism)
    let events_repeat = scanner.scan(text, trace_id.clone(), None, None);
    assert_eq!(events.len(), events_repeat.len());
});
