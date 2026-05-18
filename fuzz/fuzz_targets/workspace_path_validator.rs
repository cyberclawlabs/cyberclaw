//! Fuzz Target 4: 工作空间路径验证与 ID 路径遍历检查
//!
//! 测试：
//! - `WorkspaceRef` JSON 反序列化（包含 root 路径字段）
//! - ID 类型（NodeId、ExecutionId 等）的路径遍历防护
//! - 路径字符串的边界情况：Unicode、控制字符、空字节
//!
//! 这是安全关键 target：发现的任何崩溃都可能表明路径遍历漏洞。

#![no_main]

use libfuzzer_sys::fuzz_target;

use cyberclaw_core::ids::{AgentId, ConnectorId, NodeId, WorkspaceId};
use cyberclaw_core::workspace::WorkspaceRef;

fuzz_target!(|data: &[u8]| {
    // Test 1: WorkspaceRef JSON deserialization
    // WorkspaceRef.root and WorkspaceRef.writable_roots are path strings
    // Must not panic even with path traversal payloads in JSON
    let _ = serde_json::from_slice::<WorkspaceRef>(data);

    // Test 2: Test ID types with arbitrary string paths
    // These IDs are used as file system keys and must reject traversal
    if let Ok(s) = std::str::from_utf8(data) {
        // All ID types share the same validation via the id_type! macro
        let node_result = NodeId::from_string(s.to_string());
        let workspace_result = WorkspaceId::from_string(s.to_string());
        let agent_result = AgentId::from_string(s.to_string());
        let connector_result = ConnectorId::from_string(s.to_string());

        // Key invariant: if string contains ".." (path traversal), must be rejected
        if s.contains("..") {
            assert!(
                node_result.is_err(),
                "NodeId must reject path traversal: {:?}",
                s
            );
            assert!(
                workspace_result.is_err(),
                "WorkspaceId must reject path traversal: {:?}",
                s
            );
            assert!(
                agent_result.is_err(),
                "AgentId must reject path traversal: {:?}",
                s
            );
            assert!(
                connector_result.is_err(),
                "ConnectorId must reject path traversal: {:?}",
                s
            );
        }

        // Key invariant: if string contains backslash, must be rejected
        if s.contains('\\') {
            assert!(
                node_result.is_err(),
                "NodeId must reject backslash: {:?}",
                s
            );
        }

        // Key invariant: if string is empty, must be rejected
        if s.is_empty() {
            assert!(
                node_result.is_err(),
                "NodeId must reject empty string"
            );
        }

        // Key invariant: if string contains control characters, must be rejected
        if s.chars().any(|c| c.is_control() || c == '\0') {
            assert!(
                node_result.is_err(),
                "NodeId must reject control characters: {:?}",
                s
            );
        }

        // Key invariant: if string length > 128, must be rejected
        if s.len() > 128 {
            assert!(
                node_result.is_err(),
                "NodeId must reject strings > 128 chars (len={})",
                s.len()
            );
        }
    }

    // Test 3: Serde deserialization of ID types with arbitrary bytes
    // Custom Deserialize impl must enforce all validations
    let _ = serde_json::from_slice::<NodeId>(data);
    let _ = serde_json::from_slice::<WorkspaceId>(data);
    let _ = serde_json::from_slice::<AgentId>(data);
});
