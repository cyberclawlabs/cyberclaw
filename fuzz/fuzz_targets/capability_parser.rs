//! Fuzz Target 1: Capability JSON 解析
//!
//! 对 `cyberclaw-core` 中 `CapabilityRef`、`ActionRequest`、`RiskLevel`
//! 等能力相关结构体进行随机 JSON 反序列化，确保不会发生 panic 或内存问题。
//!
//! 同时测试 `validate_json_input` 的边界行为。

#![no_main]

use libfuzzer_sys::fuzz_target;

use cyberclaw_core::capability::{ActionRequest, CapabilityRef};
use cyberclaw_core::validation::{validate_json_input, MAX_JSON_DEPTH, MAX_JSON_SIZE_BYTES};

fuzz_target!(|data: &[u8]| {
    // Test 1: CapabilityRef JSON deserialization — must never panic
    let _ = serde_json::from_slice::<CapabilityRef>(data);

    // Test 2: ActionRequest JSON deserialization — must never panic
    let _ = serde_json::from_slice::<ActionRequest>(data);

    // Test 3: validate_json_input with arbitrary input
    // This function must always return Ok or Err, never panic
    let _ = validate_json_input(data, MAX_JSON_SIZE_BYTES, MAX_JSON_DEPTH);

    // Test 4: validate_json_input with small size limit (stress the size check path)
    let small_limit = if data.len() > 0 { data.len() / 2 } else { 1 };
    let _ = validate_json_input(data, small_limit.max(1), MAX_JSON_DEPTH);

    // Test 5: validate_json_input with depth=1 (stress the depth check path)
    let _ = validate_json_input(data, MAX_JSON_SIZE_BYTES, 1);

    // Test 6: RiskLevel deserialization
    let _ = serde_json::from_slice::<cyberclaw_core::capability::RiskLevel>(data);

    // Test 7: CapabilityEffect deserialization
    let _ = serde_json::from_slice::<cyberclaw_core::capability::CapabilityEffect>(data);
});
