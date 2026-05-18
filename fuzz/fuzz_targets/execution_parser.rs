//! Fuzz Target 2: Execution JSON 解析
//!
//! 对 `Execution`、`ExecutionBudget`、`ExecutionStatus` 等执行相关结构体
//! 进行随机 JSON 反序列化，验证不会 panic。
//!
//! 同时测试 ID 类型的 `from_string` 验证路径，以及 Serde 反序列化时的安全检查。

#![no_main]

use libfuzzer_sys::fuzz_target;

use cyberclaw_core::execution::{Execution, ExecutionBudget, ExecutionStatus};
use cyberclaw_core::ids::{ExecutionId, NodeId, TraceId};

fuzz_target!(|data: &[u8]| {
    // Test 1: Execution JSON deserialization — complex nested struct, must never panic
    let _ = serde_json::from_slice::<Execution>(data);

    // Test 2: ExecutionBudget deserialization — optional numeric fields
    let _ = serde_json::from_slice::<ExecutionBudget>(data);

    // Test 3: ExecutionStatus deserialization — enum variants
    let _ = serde_json::from_slice::<ExecutionStatus>(data);

    // Test 4: ID from_string with arbitrary UTF-8 string from fuzz data
    // IDs have strict validation: no control chars, no path traversal, length limits
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = ExecutionId::from_string(s.to_string());
        let _ = NodeId::from_string(s.to_string());
        let _ = TraceId::from_string(s.to_string());
    }

    // Test 5: Serde deserialization of IDs (bypasses from_string — tests custom Deserialize impl)
    let _ = serde_json::from_slice::<ExecutionId>(data);
    let _ = serde_json::from_slice::<NodeId>(data);

    // Test 6: ExecutionId JSON roundtrip (if data happens to be valid)
    if let Ok(exec_id) = serde_json::from_slice::<ExecutionId>(data) {
        // Roundtrip: serialize back and parse again
        if let Ok(json) = serde_json::to_string(&exec_id) {
            let _ = serde_json::from_str::<ExecutionId>(&json);
        }
    }
});
