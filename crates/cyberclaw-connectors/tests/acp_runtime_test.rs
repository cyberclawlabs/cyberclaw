//! ACP External Agent Runtime Connector 测试
//!
//! 覆盖会话生命周期、权限执行、能力分发和配置验证。

use cyberclaw_connectors::acp_runtime::{
    AcpConfig, AcpRuntimeConnector, AcpSession, ExternalRuntime, PermissionProfile, SecretRef,
    SessionState, TransportBackend,
};
use cyberclaw_connectors::acp_transport::{MockTransport, SpawnConfig};
use cyberclaw_connectors::types::{CapabilityExecutionRequest, Connector, ExecutionStatus};
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::prelude::*;

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

fn make_connector() -> AcpRuntimeConnector {
    AcpRuntimeConnector::with_defaults()
}

fn make_execution_request(
    capability_id: &str,
    input: serde_json::Value,
) -> CapabilityExecutionRequest {
    CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: uuid::Uuid::new_v4().to_string(),
        actor: cyberclaw_core::identity::Identity::System
            .to_actor_ref(None)
            .unwrap(),
        workspace: WorkspaceRef {
            id: WorkspaceId::from_string("test-workspace".to_string()).unwrap(),
            mode: WorkspaceMode::Ephemeral,
            materialization_mode: None,
            home_node_id: None,
            backing_store: None,
            root: "/tmp/test".to_string(),
            writable_roots: vec![],
        },
        connector_id: ConnectorId::from_string("acp-runtime".to_string()).unwrap(),
        capability_id: CapabilityId::from_string(capability_id.to_string()).unwrap(),
        input,
    }
}

fn make_session(state: SessionState) -> AcpSession {
    AcpSession {
        session_id: "test-session-1".to_string(),
        execution_id: "exec-1".to_string(),
        trace_id: "trace-1".to_string(),
        runtime: ExternalRuntime::ClaudeCode,
        state,
        permission_profile: PermissionProfile::AskAll,
        cwd: Some("/tmp/project".to_string()),
        model: None,
        created_at: chrono::Utc::now(),
        events: vec![],
        artifacts: vec![],
    }
}

// ===========================================================================
// Session Lifecycle Tests (8)
// ===========================================================================

/// 1. Spawning -> Active 转换
#[test]
fn test_session_spawning_to_active() {
    let mut session = make_session(SessionState::Spawning);
    assert!(session.activate().is_ok());
    assert_eq!(session.state, SessionState::Active);
}

/// 2. Active -> Completed 转换
#[test]
fn test_session_active_to_completed() {
    let mut session = make_session(SessionState::Active);
    assert!(session.complete().is_ok());
    assert_eq!(session.state, SessionState::Completed);
}

/// 3. Active -> Failed 转换
#[test]
fn test_session_active_to_failed() {
    let mut session = make_session(SessionState::Active);
    assert!(session.fail("timeout".to_string()).is_ok());
    assert_eq!(
        session.state,
        SessionState::Failed {
            reason: "timeout".to_string()
        }
    );
}

/// 4. Active -> Cancelled 转换
#[test]
fn test_session_active_to_cancelled() {
    let mut session = make_session(SessionState::Active);
    assert!(session.cancel().is_ok());
    assert_eq!(session.state, SessionState::Cancelled);
}

/// 5. Active -> Paused 转换
#[test]
fn test_session_active_to_paused() {
    let mut session = make_session(SessionState::Active);
    assert!(session.pause().is_ok());
    assert_eq!(session.state, SessionState::Paused);
}

/// 6. Paused -> Active 转换（resume）
#[test]
fn test_session_paused_to_active() {
    let mut session = make_session(SessionState::Paused);
    assert!(session.resume().is_ok());
    assert_eq!(session.state, SessionState::Active);
}

/// 7. 非法状态转换应返回错误
#[test]
fn test_session_invalid_transitions() {
    // Completed 不能 activate
    let mut session = make_session(SessionState::Completed);
    assert!(session.activate().is_err());

    // Spawning 不能 complete
    let mut session = make_session(SessionState::Spawning);
    assert!(session.complete().is_err());

    // Cancelled 不能 resume
    let mut session = make_session(SessionState::Cancelled);
    assert!(session.resume().is_err());

    // Failed 不能 pause
    let mut session = make_session(SessionState::Failed {
        reason: "err".to_string(),
    });
    assert!(session.pause().is_err());

    // Paused 不能 complete（需先 resume 到 Active）
    let mut session = make_session(SessionState::Paused);
    assert!(session.complete().is_err());
}

/// 8. 完整生命周期：Spawning -> Active -> Completed
#[test]
fn test_session_full_lifecycle() {
    let mut session = make_session(SessionState::Spawning);
    assert!(session.activate().is_ok());
    assert_eq!(session.state, SessionState::Active);
    assert!(session.complete().is_ok());
    assert_eq!(session.state, SessionState::Completed);
}

// ===========================================================================
// Permission Enforcement Tests (5)
// ===========================================================================

/// 9. AskAll 权限配置可以正常创建
#[test]
fn test_permission_ask_all() {
    let session = make_session(SessionState::Active);
    assert!(matches!(
        session.permission_profile,
        PermissionProfile::AskAll
    ));
}

/// 10. ReadOnly 权限配置
#[test]
fn test_permission_read_only() {
    let mut session = make_session(SessionState::Active);
    session.permission_profile = PermissionProfile::ReadOnly;
    assert!(matches!(
        session.permission_profile,
        PermissionProfile::ReadOnly
    ));
}

/// 11. Scoped 权限配置
#[test]
fn test_permission_scoped() {
    let mut session = make_session(SessionState::Active);
    session.permission_profile = PermissionProfile::Scoped {
        allowed_paths: vec!["/tmp/project".to_string()],
    };
    match &session.permission_profile {
        PermissionProfile::Scoped { allowed_paths } => {
            assert_eq!(allowed_paths.len(), 1);
            assert_eq!(allowed_paths[0], "/tmp/project");
        }
        _ => panic!("Expected Scoped permission"),
    }
}

/// 12. PreAuthorized 有效验证（仅 Low/Medium 风险）
#[test]
fn test_permission_pre_authorized_valid() {
    let capabilities = vec![
        "external_agent.session.status".to_string(),
        "external_agent.prompt.followup".to_string(),
    ];

    // 模拟风险查找：这些能力都是 Low 风险
    let result =
        cyberclaw_connectors::acp_runtime::validate_pre_authorized(
            &capabilities,
            |cap| match cap {
                "external_agent.session.status" => Some(RiskLevel::Low),
                "external_agent.prompt.followup" => Some(RiskLevel::Low),
                _ => None,
            },
        );
    assert!(result.is_ok());
}

/// 13. PreAuthorized 拒绝 High 风险能力
#[test]
fn test_permission_pre_authorized_rejects_high_risk() {
    let capabilities = vec![
        "external_agent.session.status".to_string(),
        "external_agent.prompt.interrupt".to_string(), // High risk
    ];

    let result =
        cyberclaw_connectors::acp_runtime::validate_pre_authorized(
            &capabilities,
            |cap| match cap {
                "external_agent.session.status" => Some(RiskLevel::Low),
                "external_agent.prompt.interrupt" => Some(RiskLevel::High),
                _ => None,
            },
        );
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("external_agent.prompt.interrupt"));
    assert!(err_msg.contains("runtime governance approval"));
}

// ===========================================================================
// Capability Dispatch Tests (4)
// ===========================================================================

/// 14. session.spawn 能力分发
#[tokio::test]
async fn test_dispatch_session_spawn() {
    let connector = make_connector();
    let request = make_execution_request(
        "external_agent.session.spawn",
        serde_json::json!({
            "runtime": "claude_code",
            "cwd": "/tmp/project"
        }),
    );
    let result = connector.execute(request).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
    assert!(result.output["session_id"].is_string());
    assert_eq!(result.output["state"], "spawning");
}

/// 15. session.status 能力分发
#[tokio::test]
async fn test_dispatch_session_status() {
    let connector = make_connector();

    // 先创建会话
    let spawn_req = make_execution_request(
        "external_agent.session.spawn",
        serde_json::json!({ "runtime": "codex" }),
    );
    let spawn_result = connector.execute(spawn_req).await.unwrap();
    let session_id = spawn_result.output["session_id"].as_str().unwrap();

    // 查询状态
    let status_req = make_execution_request(
        "external_agent.session.status",
        serde_json::json!({ "session_id": session_id }),
    );
    let status_result = connector.execute(status_req).await.unwrap();
    assert_eq!(status_result.status, ExecutionStatus::Success);
}

/// 16. session.stop 能力分发
#[tokio::test]
async fn test_dispatch_session_stop() {
    let connector = make_connector();

    // 先创建会话并激活它
    let spawn_req = make_execution_request(
        "external_agent.session.spawn",
        serde_json::json!({ "runtime": "claude_code" }),
    );
    let spawn_result = connector.execute(spawn_req).await.unwrap();
    let session_id = spawn_result.output["session_id"].as_str().unwrap();

    // 发送 prompt 使其从 Spawning -> Active
    let prompt_req = make_execution_request(
        "external_agent.prompt.send",
        serde_json::json!({
            "session_id": session_id,
            "prompt": "hello"
        }),
    );
    connector.execute(prompt_req).await.unwrap();

    // 停止会话
    let stop_req = make_execution_request(
        "external_agent.session.stop",
        serde_json::json!({ "session_id": session_id }),
    );
    let stop_result = connector.execute(stop_req).await.unwrap();
    assert_eq!(stop_result.status, ExecutionStatus::Success);
    assert_eq!(stop_result.output["state"], "cancelled");
}

/// 17. prompt.send 能力分发
#[tokio::test]
async fn test_dispatch_prompt_send() {
    let connector = make_connector();

    // 先创建会话
    let spawn_req = make_execution_request(
        "external_agent.session.spawn",
        serde_json::json!({ "runtime": "gemini_cli" }),
    );
    let spawn_result = connector.execute(spawn_req).await.unwrap();
    let session_id = spawn_result.output["session_id"].as_str().unwrap();

    // 发送 prompt
    let prompt_req = make_execution_request(
        "external_agent.prompt.send",
        serde_json::json!({
            "session_id": session_id,
            "prompt": "Fix the bug in main.rs"
        }),
    );
    let result = connector.execute(prompt_req).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Success);
    assert_eq!(result.output["accepted"], true);
}

// ===========================================================================
// Config Validation Tests (3)
// ===========================================================================

/// 18. 默认配置验证
#[test]
fn test_default_config() {
    let config = AcpConfig::default();
    assert_eq!(config.max_concurrent_sessions, 4);
    assert_eq!(config.session_timeout_secs, 3600);
    assert!(matches!(
        config.default_permission,
        PermissionProfile::AskAll
    ));
    assert!(matches!(
        config.default_transport,
        TransportBackend::Acp { acpx_path: None }
    ));
    assert!(config.runtime_transports.is_empty());
}

/// 19. 自定义运行时传输覆盖配置
#[test]
fn test_per_runtime_transport_override() {
    let mut config = AcpConfig::default();
    config.runtime_transports.insert(
        "codex".to_string(),
        TransportBackend::HeadlessCli {
            command: "codex".to_string(),
            args: vec!["--headless".to_string()],
        },
    );
    assert_eq!(config.runtime_transports.len(), 1);
    assert!(config.runtime_transports.contains_key("codex"));
    match &config.runtime_transports["codex"] {
        TransportBackend::HeadlessCli { command, args } => {
            assert_eq!(command, "codex");
            assert_eq!(args, &["--headless"]);
        }
        _ => panic!("Expected HeadlessCli transport"),
    }
}

/// 20. 无效配置（session 不存在时 status 应返回失败）
#[tokio::test]
async fn test_invalid_session_returns_error() {
    let connector = make_connector();
    let req = make_execution_request(
        "external_agent.session.status",
        serde_json::json!({ "session_id": "nonexistent-session" }),
    );
    let result = connector.execute(req).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Failed);
    assert!(result.error.as_ref().unwrap().contains("Session not found"));
}

// ===========================================================================
// 额外覆盖：Connector trait、MockTransport、序列化
// ===========================================================================

/// 21. Connector trait 实现验证
#[test]
fn test_connector_trait_impl() {
    let connector = make_connector();
    assert_eq!(connector.id().as_str(), "acp-runtime");
    assert!(matches!(connector.runtime(), ConnectorRuntime::Process));
}

/// 22. 10 个 capabilities 全部暴露
#[test]
fn test_connector_exposes_10_capabilities() {
    let connector = make_connector();
    let caps = connector.capabilities();
    assert_eq!(caps.len(), 10);

    let expected_ids = [
        "external_agent.session.spawn",
        "external_agent.session.resume",
        "external_agent.session.stop",
        "external_agent.session.status",
        "external_agent.prompt.send",
        "external_agent.prompt.followup",
        "external_agent.prompt.interrupt",
        "external_agent.artifact.collect",
        "external_agent.env.set",
        "external_agent.permission.set",
    ];
    for expected in &expected_ids {
        assert!(
            caps.iter().any(|c| c.id == *expected),
            "Missing capability: {}",
            expected
        );
    }
}

/// 23. High 风险 capabilities 标记正确
#[test]
fn test_high_risk_capabilities_marked() {
    let connector = make_connector();
    let caps = connector.capabilities();

    let interrupt = caps
        .iter()
        .find(|c| c.id == "external_agent.prompt.interrupt")
        .unwrap();
    assert_eq!(interrupt.risk, RiskLevel::High);

    let perm_set = caps
        .iter()
        .find(|c| c.id == "external_agent.permission.set")
        .unwrap();
    assert_eq!(perm_set.risk, RiskLevel::High);
}

/// 24. MockTransport spawn 和 status
#[tokio::test]
async fn test_mock_transport_spawn_and_status() {
    use cyberclaw_connectors::acp_transport::AcpTransport;

    let transport = MockTransport::new();
    let config = SpawnConfig {
        runtime: ExternalRuntime::ClaudeCode,
        cwd: Some("/tmp".to_string()),
        model: None,
        permission_profile: PermissionProfile::AskAll,
        initial_prompt: None,
        credentials: None,
    };
    let session_id = transport.spawn_session(config).await.unwrap();
    assert!(session_id.starts_with("mock-session-"));

    let state = transport.get_status(&session_id).await.unwrap();
    assert_eq!(state, SessionState::Active);
}

/// 25. MockTransport stop
#[tokio::test]
async fn test_mock_transport_stop() {
    use cyberclaw_connectors::acp_transport::AcpTransport;

    let transport = MockTransport::new();
    let config = SpawnConfig {
        runtime: ExternalRuntime::Codex,
        cwd: None,
        model: None,
        permission_profile: PermissionProfile::ReadOnly,
        initial_prompt: Some("test".to_string()),
        credentials: None,
    };
    let session_id = transport.spawn_session(config).await.unwrap();
    transport.stop_session(&session_id).await.unwrap();
    let state = transport.get_status(&session_id).await.unwrap();
    assert_eq!(state, SessionState::Cancelled);
}

/// 26. SecretRef 序列化/反序列化
#[test]
fn test_secret_ref_serialization() {
    let secret = SecretRef {
        store: "env".to_string(),
        key: "CLAUDE_API_KEY".to_string(),
    };
    let json = serde_json::to_string(&secret).unwrap();
    let deserialized: SecretRef = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.store, "env");
    assert_eq!(deserialized.key, "CLAUDE_API_KEY");
}

/// 27. AcpSession 序列化/反序列化
#[test]
fn test_acp_session_serialization() {
    let session = make_session(SessionState::Active);
    let json = serde_json::to_string(&session).unwrap();
    let deserialized: AcpSession = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.session_id, "test-session-1");
    assert_eq!(deserialized.state, SessionState::Active);
    assert_eq!(deserialized.runtime, ExternalRuntime::ClaudeCode);
}

/// 28. 未知 capability 返回 Failed
#[tokio::test]
async fn test_unknown_capability_returns_failed() {
    let connector = make_connector();
    let req = make_execution_request("external_agent.nonexistent", serde_json::json!({}));
    let result = connector.execute(req).await.unwrap();
    assert_eq!(result.status, ExecutionStatus::Failed);
    assert!(result
        .error
        .as_ref()
        .unwrap()
        .contains("Unknown capability"));
}

/// 29. PreAuthorized 拒绝 Critical 风险能力
#[test]
fn test_permission_pre_authorized_rejects_critical_risk() {
    let capabilities = vec!["dangerous.action".to_string()];
    let result =
        cyberclaw_connectors::acp_runtime::validate_pre_authorized(
            &capabilities,
            |cap| match cap {
                "dangerous.action" => Some(RiskLevel::Critical),
                _ => None,
            },
        );
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("dangerous.action"));
}

/// 30. ExternalRuntime::Custom 变体
#[test]
fn test_external_runtime_custom() {
    let runtime = ExternalRuntime::Custom("my-agent".to_string());
    let json = serde_json::to_string(&runtime).unwrap();
    let deserialized: ExternalRuntime = serde_json::from_str(&json).unwrap();
    assert_eq!(
        deserialized,
        ExternalRuntime::Custom("my-agent".to_string())
    );
}

// ===========================================================================
// Review Fix 覆盖测试
// ===========================================================================

/// 31. cancel() 从 Spawning 状态应成功
#[test]
fn test_cancel_from_spawning() {
    let mut session = make_session(SessionState::Spawning);
    assert!(session.cancel().is_ok());
    assert_eq!(session.state, SessionState::Cancelled);
}

/// 32. cancel() 从 Paused 状态应成功
#[test]
fn test_cancel_from_paused() {
    let mut session = make_session(SessionState::Paused);
    assert!(session.cancel().is_ok());
    assert_eq!(session.state, SessionState::Cancelled);
}

/// 33. cancel() 从终态 Completed 应失败
#[test]
fn test_cancel_from_completed_fails() {
    let mut session = make_session(SessionState::Completed);
    let result = session.cancel();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already terminal"));
}

/// 34. cancel() 从终态 Failed 应失败
#[test]
fn test_cancel_from_failed_fails() {
    let mut session = make_session(SessionState::Failed {
        reason: "test error".to_string(),
    });
    let result = session.cancel();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already terminal"));
}

/// 35. 达到 max_concurrent_sessions 时 spawn 应被拒绝
#[tokio::test]
async fn test_spawn_rejected_at_concurrent_limit() {
    let config = AcpConfig {
        max_concurrent_sessions: 2,
        ..Default::default()
    };
    let connector = AcpRuntimeConnector::new(config);

    // 第一个 spawn 成功
    let req1 = make_execution_request(
        "external_agent.session.spawn",
        serde_json::json!({"runtime": "claude_code", "task": "task-1"}),
    );
    let r1 = connector.execute(req1).await;
    assert!(r1.is_ok());

    // 第二个 spawn 成功
    let req2 = make_execution_request(
        "external_agent.session.spawn",
        serde_json::json!({"runtime": "codex", "task": "task-2"}),
    );
    let r2 = connector.execute(req2).await;
    assert!(r2.is_ok());

    // 第三个 spawn 应被拒绝（已达上限 2）
    let req3 = make_execution_request(
        "external_agent.session.spawn",
        serde_json::json!({"runtime": "claude_code", "task": "task-3"}),
    );
    let r3 = connector.execute(req3).await.unwrap();
    assert_eq!(r3.status, ExecutionStatus::Failed);
    let err_msg = r3.error.expect("应包含错误信息");
    assert!(err_msg.contains("concurrent session limit"));
}
