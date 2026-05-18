//! CyberClaw P2 阶段端到端测试套件
//!
//! 测试涵盖:
//! - Plugin 加载 -> Hook 触发 -> Capability 执行
//! - MCP Connector 完整流程
//! - External Connectors 集成测试
//! - Cron 调度 -> Task 执行 -> 结果记录
//! - Skill 加载 -> 执行

mod common;

use common::*;
use anyhow::Result;
use std::time::Duration;

// ==================== Plugin 系统 E2E 测试 ====================

#[tokio::test]
async fn test_plugin_lifecycle_e2e() -> Result<()> {
    init_test_env();

    // 1. 创建测试 Plugin
    let (temp_dir, plugin_path) = common::plugin::create_test_plugin("test-plugin").await?;

    // 2. 初始化 Plugin 运行时
    let plugin_runtime = cyberclaw_plugin_runtime::PluginRuntime::new(
        cyberclaw_plugin_runtime::PluginRuntimeConfig::default()
    ).await?;

    // 3. 加载 Plugin
    let plugin_id = plugin_runtime.load_plugin(&plugin_path).await?;
    assert!(!plugin_id.is_empty(), "Plugin ID should not be empty");

    // 4. 验证 Plugin 状态
    let plugin_info = plugin_runtime.get_plugin_info(&plugin_id).await?;
    assert_eq!(plugin_info.state, cyberclaw_plugin_runtime::PluginState::Enabled);

    // 5. 触发 Hook
    let hook_context = cyberclaw_plugin_runtime::HookContext {
        execution_id: "test-exec-001".to_string(),
        phase: cyberclaw_plugin_runtime::HookType::BeforeExecution,
        params: serde_json::json!({"test": "data"}),
    };

    let hook_result = plugin_runtime
        .dispatch_hook(cyberclaw_plugin_runtime::HookType::BeforeExecution, &hook_context)
        .await?;

    assert!(hook_result.success, "Hook should execute successfully");

    // 6. 禁用 Plugin
    plugin_runtime.disable_plugin(&plugin_id).await?;
    let plugin_info = plugin_runtime.get_plugin_info(&plugin_id).await?;
    assert_eq!(plugin_info.state, cyberclaw_plugin_runtime::PluginState::Disabled);

    // 7. 卸载 Plugin
    plugin_runtime.unload_plugin(&plugin_id).await?;

    // 验证卸载
    let result = plugin_runtime.get_plugin_info(&plugin_id).await;
    assert!(result.is_err(), "Plugin should be unloaded");

    Ok(())
}

#[tokio::test]
async fn test_hook_priority_and_ordering() -> Result<()> {
    init_test_env();

    // 创建多个带有不同优先级 Hook 的 Plugin
    let hooks = vec![
        common::plugin::HookConfig {
            hook_type: "before_execution".to_string(),
            handler_name: "high_priority_hook".to_string(),
            priority: 10,
            timeout_ms: 1000,
        },
        common::plugin::HookConfig {
            hook_type: "before_execution".to_string(),
            handler_name: "low_priority_hook".to_string(),
            priority: 100,
            timeout_ms: 1000,
        },
    ];

    let (_temp, plugin_path) = common::plugin::create_plugin_with_hooks("priority-test", hooks).await?;

    let plugin_runtime = cyberclaw_plugin_runtime::PluginRuntime::new(
        cyberclaw_plugin_runtime::PluginRuntimeConfig::default()
    ).await?;

    plugin_runtime.load_plugin(&plugin_path).await?;

    // 触发 Hooks 并验证执行顺序
    let context = cyberclaw_plugin_runtime::HookContext {
        execution_id: "test-002".to_string(),
        phase: cyberclaw_plugin_runtime::HookType::BeforeExecution,
        params: serde_json::json!({}),
    };

    let result = plugin_runtime
        .dispatch_hook(cyberclaw_plugin_runtime::HookType::BeforeExecution, &context)
        .await?;

    // 验证高优先级 Hook 先执行
    assert!(result.execution_order[0].contains("high_priority"));
    assert!(result.execution_order[1].contains("low_priority"));

    Ok(())
}

#[tokio::test]
async fn test_failure_policy_handling() -> Result<()> {
    init_test_env();

    // 测试不同的失败策略
    let policies = vec![
        cyberclaw_plugin_runtime::FailurePolicy::Ignore,
        cyberclaw_plugin_runtime::FailurePolicy::Retry { max_attempts: 3 },
        cyberclaw_plugin_runtime::FailurePolicy::Abort,
    ];

    for policy in policies {
        let plugin_runtime = cyberclaw_plugin_runtime::PluginRuntime::new(
            cyberclaw_plugin_runtime::PluginRuntimeConfig {
                failure_policy: policy.clone(),
                ..Default::default()
            }
        ).await?;

        // 创建会失败的 Plugin
        let (_temp, plugin_path) = common::plugin::create_test_plugin("failing-plugin").await?;
        plugin_runtime.load_plugin(&plugin_path).await?;

        let context = cyberclaw_plugin_runtime::HookContext {
            execution_id: "fail-test".to_string(),
            phase: cyberclaw_plugin_runtime::HookType::BeforeExecution,
            params: serde_json::json!({"force_fail": true}),
        };

        let result = plugin_runtime
            .dispatch_hook(cyberclaw_plugin_runtime::HookType::BeforeExecution, &context)
            .await;

        match policy {
            cyberclaw_plugin_runtime::FailurePolicy::Ignore => {
                assert!(result.is_ok(), "Ignore policy should not fail");
            }
            cyberclaw_plugin_runtime::FailurePolicy::Retry { .. } => {
                // 重试后可能成功或失败
            }
            cyberclaw_plugin_runtime::FailurePolicy::Abort => {
                if context.params["force_fail"].as_bool().unwrap_or(false) {
                    assert!(result.is_err(), "Abort policy should fail on error");
                }
            }
        }
    }

    Ok(())
}

// ==================== MCP Connector E2E 测试 ====================

#[tokio::test]
async fn test_mcp_connector_tool_discovery() -> Result<()> {
    init_test_env();

    // 1. 启动 Mock MCP Server
    let mcp_server = common::mcp::MockMcpServer::start().await?;

    // 2. 添加测试工具
    mcp_server.add_tool(common::mcp::McpTool {
        name: "calculator".to_string(),
        description: "A simple calculator".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "operation": { "type": "string" },
                "a": { "type": "number" },
                "b": { "type": "number" }
            }
        }),
    }).await;

    // 3. 初始化 MCP Connector
    let mcp_connector = cyberclaw_connectors::mcp::McpConnector::new(
        cyberclaw_connectors::mcp::McpConfig {
            server_url: mcp_server.url(),
            timeout: Duration::from_secs(10),
        }
    ).await?;

    // 4. 发现 Capabilities
    let capabilities = mcp_connector.discover_capabilities().await?;

    assert!(!capabilities.is_empty(), "Should discover at least one capability");
    assert!(capabilities.iter().any(|c| c.id.contains("calculator")));

    // 5. 关闭服务器
    mcp_server.shutdown().await;

    Ok(())
}

#[tokio::test]
async fn test_mcp_connector_tool_execution() -> Result<()> {
    init_test_env();

    let mcp_server = common::mcp::MockMcpServer::start().await?;

    let mcp_connector = cyberclaw_connectors::mcp::McpConnector::new(
        cyberclaw_connectors::mcp::McpConfig {
            server_url: mcp_server.url(),
            timeout: Duration::from_secs(10),
        }
    ).await?;

    // 执行 Tool
    let request = cyberclaw_core::CapabilityRequest {
        params: serde_json::json!({
            "message": "Hello MCP"
        }),
        metadata: Default::default(),
    };

    let response = mcp_connector.execute("mcp.tool.test_tool", request).await?;

    assert!(response.output["output"].as_str().is_some());
    assert_eq!(response.output["output"], "Tool executed successfully");

    mcp_server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn test_mcp_connector_resource_reading() -> Result<()> {
    init_test_env();

    let mcp_server = common::mcp::MockMcpServer::start().await?;

    // 添加测试资源
    mcp_server.add_resource(common::mcp::McpResource {
        uri: "file://test.txt".to_string(),
        name: "Test File".to_string(),
        mime_type: Some("text/plain".to_string()),
    }).await;

    let mcp_connector = cyberclaw_connectors::mcp::McpConnector::new(
        cyberclaw_connectors::mcp::McpConfig {
            server_url: mcp_server.url(),
            timeout: Duration::from_secs(10),
        }
    ).await?;

    // 读取 Resource
    let request = cyberclaw_core::CapabilityRequest::default();
    let response = mcp_connector.execute("mcp.resource.file://test.txt", request).await?;

    assert_eq!(response.output["content"], "Test resource content");

    mcp_server.shutdown().await;
    Ok(())
}

// ==================== External Connectors E2E 测试 ====================

#[tokio::test]
async fn test_github_connector_create_issue() -> Result<()> {
    init_test_env();

    // 1. 启动 Mock GitHub Server
    let github_server = common::github::MockGitHubServer::start().await?;

    // 2. 添加测试仓库
    github_server.add_repo(common::github::Repository {
        owner: "cyberclaw".to_string(),
        name: "test-repo".to_string(),
        description: Some("Test repository".to_string()),
    }).await;

    // 3. 初始化 GitHub Connector
    let github_connector = cyberclaw_connectors::github::GitHubConnector::new(
        cyberclaw_connectors::github::GitHubConfig {
            api_url: github_server.url(),
            token: "test-token".to_string(),
        }
    )?;

    // 4. 创建 Issue
    let request = cyberclaw_core::CapabilityRequest {
        params: serde_json::json!({
            "owner": "cyberclaw",
            "repo": "test-repo",
            "title": "Test Issue",
            "body": "This is a test issue"
        }),
        metadata: Default::default(),
    };

    let response = github_connector.execute("github.create_issue", request).await?;

    assert!(response.output["id"].as_u64().is_some());
    assert_eq!(response.output["title"], "Test Issue");
    assert_eq!(response.output["state"], "open");

    github_server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn test_database_connector_query() -> Result<()> {
    init_test_env();

    let mock_db = common::database::MockDatabase::new();

    // 插入测试数据
    mock_db.execute("INSERT INTO users VALUES ('test', 'user')").await?;

    // 查询数据
    let results = mock_db.execute("SELECT * FROM users").await?;

    assert_eq!(results.len(), 1);
    assert!(results[0]["id"].as_str().is_some());

    Ok(())
}

#[tokio::test]
async fn test_slack_connector_send_message() -> Result<()> {
    init_test_env();

    // 1. 启动 Mock Slack Server
    let slack_server = common::slack::MockSlackServer::start().await?;

    // 2. 初始化 Slack Connector
    let slack_connector = cyberclaw_connectors::slack::SlackConnector::new(
        cyberclaw_connectors::slack::SlackConfig {
            api_url: slack_server.url(),
            bot_token: "xoxb-test-token".to_string(),
        }
    )?;

    // 3. 发送消息
    let request = cyberclaw_core::CapabilityRequest {
        params: serde_json::json!({
            "channel": "#general",
            "template": "notification",
            "data": {
                "title": "Test Notification",
                "message": "This is a test message"
            }
        }),
        metadata: Default::default(),
    };

    let response = slack_connector.execute("slack.send_message", request).await?;

    assert!(response.output["ok"].as_bool().unwrap_or(false));
    assert!(response.output["ts"].as_str().is_some());

    // 4. 验证消息已发送
    let messages = slack_server.get_messages().await;
    assert_eq!(messages.len(), 1);
    assert!(messages[0].text.contains("Test"));

    slack_server.shutdown().await;
    Ok(())
}

// ==================== Scheduler E2E 测试 ====================

#[tokio::test]
async fn test_heartbeat_monitor_node_health() -> Result<()> {
    init_test_env();

    // 1. 创建 Heartbeat Monitor
    let monitor = cyberclaw_scheduler::heartbeat::HeartbeatMonitor::new(
        cyberclaw_scheduler::heartbeat::HeartbeatConfig {
            interval: Duration::from_secs(1),
            timeout: Duration::from_secs(3),
        }
    )?;

    // 2. 注册节点
    let node_id = monitor.register_node(cyberclaw_scheduler::NodeInfo {
        id: "node-001".to_string(),
        address: "127.0.0.1:8080".to_string(),
    }).await?;

    // 3. 启动监控
    let monitor_handle = tokio::spawn(async move {
        monitor.start().await
    });

    // 4. 发送心跳
    for _ in 0..3 {
        cyberclaw_scheduler::heartbeat::send_heartbeat(&node_id, cyberclaw_scheduler::HeartbeatData {
            cpu_usage: 50.0,
            memory_usage: 60.0,
            disk_usage: 70.0,
        }).await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // 5. 检查节点状态
    let status = cyberclaw_scheduler::heartbeat::get_node_status(&node_id).await?;
    assert_eq!(status, cyberclaw_scheduler::NodeStatus::Healthy);

    // 6. 停止发送心跳，等待超时
    tokio::time::sleep(Duration::from_secs(4)).await;

    let status = cyberclaw_scheduler::heartbeat::get_node_status(&node_id).await?;
    assert_eq!(status, cyberclaw_scheduler::NodeStatus::Offline);

    monitor_handle.abort();
    Ok(())
}

#[tokio::test]
async fn test_cron_scheduler_task_execution() -> Result<()> {
    init_test_env();

    // 1. 创建 Cron Scheduler
    let scheduler = cyberclaw_scheduler::cron::CronScheduler::new()?;

    // 2. 添加定时任务 (每秒执行)
    let task_id = scheduler.schedule(
        "test-task".to_string(),
        "* * * * * *".to_string(), // 每秒
        cyberclaw_scheduler::TaskAction::ExecuteCapability {
            connector_id: "test-connector".to_string(),
            capability: "test.capability".to_string(),
            params: serde_json::json!({"test": "data"}),
        },
    ).await?;

    // 3. 启动调度器
    let scheduler_handle = tokio::spawn(async move {
        scheduler.start().await
    });

    // 4. 等待任务执行
    tokio::time::sleep(Duration::from_secs(3)).await;

    // 5. 检查执行历史
    let history = cyberclaw_scheduler::cron::get_execution_history(&task_id).await?;
    assert!(history.len() >= 2, "Should have at least 2 executions");

    scheduler_handle.abort();
    Ok(())
}

#[tokio::test]
async fn test_scheduler_integration_with_task_executor() -> Result<()> {
    init_test_env();

    // 1. 初始化完整的执行环境
    let control_plane = cyberclaw_control_plane::ControlPlane::new(
        cyberclaw_control_plane::Config::default()
    ).await?;

    // 2. 创建 Scheduler
    let scheduler = cyberclaw_scheduler::cron::CronScheduler::with_executor(
        control_plane.task_executor()
    )?;

    // 3. 添加任务
    let task_id = scheduler.schedule(
        "integrated-task".to_string(),
        "*/5 * * * * *".to_string(), // 每5秒
        cyberclaw_scheduler::TaskAction::ExecuteAgent {
            agent_id: "test-agent".to_string(),
            task: "perform test action".to_string(),
        },
    ).await?;

    // 4. 启动并运行
    let handle = tokio::spawn(async move {
        scheduler.start().await
    });

    tokio::time::sleep(Duration::from_secs(10)).await;

    // 5. 验证任务进入了主执行链路
    let executions = control_plane.get_recent_executions(10).await?;
    assert!(executions.iter().any(|e| e.metadata.contains_key("scheduled_task_id")));

    handle.abort();
    Ok(())
}

// ==================== Skill Loader E2E 测试 ====================

#[tokio::test]
async fn test_skill_loader_claude_code_format() -> Result<()> {
    init_test_env();

    // 1. 创建 Claude Code 格式的 Skill
    let temp_dir = tempfile::TempDir::new()?;
    let skill_dir = temp_dir.path().join("test-skill");
    std::fs::create_dir_all(&skill_dir)?;

    // 创建 SKILL.md
    let skill_md = r#"---
name: Test Skill
version: 1.0.0
description: A test skill for E2E testing
author: CyberClaw Test
---

# Test Skill

This skill provides testing capabilities.

## Capabilities

- `test.echo`: Echo back the input
- `test.calculate`: Perform calculations
"#;

    std::fs::write(skill_dir.join("SKILL.md"), skill_md)?;

    // 创建 scripts 目录
    let scripts_dir = skill_dir.join("scripts");
    std::fs::create_dir_all(&scripts_dir)?;

    let echo_script = r#"#!/bin/bash
echo "Echo: $1"
"#;
    std::fs::write(scripts_dir.join("echo.sh"), echo_script)?;

    // 2. 加载 Skill
    let skill_loader = cyberclaw_skill_runtime::UnifiedSkillLoader::new();
    let skill = skill_loader.load_skill(&skill_dir).await?;

    // 3. 验证 Skill 元数据
    assert_eq!(skill.id(), "test-skill");
    assert_eq!(skill.metadata().name, "Test Skill");
    assert_eq!(skill.metadata().version, "1.0.0");

    // 4. 执行 Skill
    let result = skill.execute(
        "test.echo",
        serde_json::json!({"message": "Hello"})
    ).await?;

    assert!(result["output"].as_str().unwrap().contains("Hello"));

    Ok(())
}

#[tokio::test]
async fn test_skill_hot_reload() -> Result<()> {
    init_test_env();

    // 1. 创建初始 Skill
    let temp_dir = tempfile::TempDir::new()?;
    let skill_dir = temp_dir.path().join("hot-reload-skill");
    std::fs::create_dir_all(&skill_dir)?;

    let initial_skill = r#"---
name: Hot Reload Skill
version: 1.0.0
---
Initial version
"#;
    std::fs::write(skill_dir.join("SKILL.md"), initial_skill)?;

    // 2. 设置热重载监控
    let skill_loader = cyberclaw_skill_runtime::UnifiedSkillLoader::new();
    let mut watcher = cyberclaw_skill_runtime::HotReloadWatcher::new(skill_loader.clone());
    watcher.watch(vec![skill_dir.clone()]).await?;

    // 3. 加载初始版本
    let skill_v1 = skill_loader.load_skill(&skill_dir).await?;
    assert_eq!(skill_v1.metadata().version, "1.0.0");

    // 4. 修改 Skill
    let updated_skill = r#"---
name: Hot Reload Skill
version: 2.0.0
---
Updated version
"#;
    std::fs::write(skill_dir.join("SKILL.md"), updated_skill)?;

    // 5. 等待热重载
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 6. 验证新版本
    let skill_v2 = skill_loader.load_skill(&skill_dir).await?;
    assert_eq!(skill_v2.metadata().version, "2.0.0");

    Ok(())
}

#[tokio::test]
async fn test_multi_format_skill_loading() -> Result<()> {
    init_test_env();

    let temp_dir = tempfile::TempDir::new()?;

    // 1. 创建 Claude Code 格式
    let claude_dir = temp_dir.path().join("claude-skill");
    std::fs::create_dir_all(&claude_dir)?;
    std::fs::write(claude_dir.join("SKILL.md"), "---\nname: Claude Skill\n---")?;

    // 2. 创建 Codex 格式
    let codex_dir = temp_dir.path().join("codex-skill");
    std::fs::create_dir_all(&codex_dir)?;
    std::fs::write(codex_dir.join("skill.json"), r#"{"name": "Codex Skill"}"#)?;

    // 3. 创建 OpenClaw 格式
    let openclaw_dir = temp_dir.path().join("openclaw-skill");
    std::fs::create_dir_all(&openclaw_dir)?;
    std::fs::write(openclaw_dir.join("manifest.yaml"), "name: OpenClaw Skill")?;

    // 4. 使用统一加载器加载所有格式
    let loader = cyberclaw_skill_runtime::UnifiedSkillLoader::new();

    let claude_skill = loader.load_skill(&claude_dir).await?;
    assert!(claude_skill.metadata().name.contains("Claude"));

    let codex_skill = loader.load_skill(&codex_dir).await?;
    assert!(codex_skill.metadata().name.contains("Codex"));

    let openclaw_skill = loader.load_skill(&openclaw_dir).await?;
    assert!(openclaw_skill.metadata().name.contains("OpenClaw"));

    Ok(())
}

// ==================== 完整集成测试 ====================

#[tokio::test]
async fn test_complete_p2_integration() -> Result<()> {
    init_test_env();

    // 1. 启动所有 Mock 服务
    let mcp_server = common::mcp::MockMcpServer::start().await?;
    let github_server = common::github::MockGitHubServer::start().await?;
    let slack_server = common::slack::MockSlackServer::start().await?;

    // 2. 初始化完整系统
    let control_plane = cyberclaw_control_plane::ControlPlane::new(
        cyberclaw_control_plane::Config::default()
    ).await?;

    // 3. 加载 Plugin
    let plugin_runtime = control_plane.plugin_runtime();
    let (_temp, plugin_path) = common::plugin::create_test_plugin("integration-plugin").await?;
    let plugin_id = plugin_runtime.load_plugin(&plugin_path).await?;

    // 4. 注册 Connectors
    control_plane.register_connector(
        cyberclaw_connectors::mcp::McpConnector::new(
            cyberclaw_connectors::mcp::McpConfig {
                server_url: mcp_server.url(),
                timeout: Duration::from_secs(10),
            }
        ).await?
    ).await?;

    control_plane.register_connector(
        cyberclaw_connectors::github::GitHubConnector::new(
            cyberclaw_connectors::github::GitHubConfig {
                api_url: github_server.url(),
                token: "test-token".to_string(),
            }
        )?
    ).await?;

    control_plane.register_connector(
        cyberclaw_connectors::slack::SlackConnector::new(
            cyberclaw_connectors::slack::SlackConfig {
                api_url: slack_server.url(),
                bot_token: "xoxb-test".to_string(),
            }
        )?
    ).await?;

    // 5. 加载 Skill
    let skill_loader = control_plane.skill_loader();
    let temp_dir = tempfile::TempDir::new()?;
    let skill_dir = temp_dir.path().join("integration-skill");
    std::fs::create_dir_all(&skill_dir)?;
    std::fs::write(skill_dir.join("SKILL.md"), "---\nname: Integration Skill\n---")?;
    let skill = skill_loader.load_skill(&skill_dir).await?;

    // 6. 创建定时任务
    let scheduler = control_plane.scheduler();
    let task_id = scheduler.schedule(
        "integration-task".to_string(),
        "*/10 * * * * *".to_string(),
        cyberclaw_scheduler::TaskAction::ExecuteCapability {
            connector_id: "github".to_string(),
            capability: "github.create_issue".to_string(),
            params: serde_json::json!({
                "owner": "cyberclaw",
                "repo": "test",
                "title": "Scheduled Issue",
                "body": "Created by scheduler"
            }),
        },
    ).await?;

    // 7. 执行一个完整的流程
    let execution_id = control_plane.execute(cyberclaw_core::Execution {
        id: "integration-exec".to_string(),
        agent_id: "test-agent".to_string(),
        task: "Create issue and notify".to_string(),
        capabilities: vec![
            cyberclaw_core::CapabilityInvocation {
                connector_id: "github".to_string(),
                capability: "github.create_issue".to_string(),
                params: serde_json::json!({
                    "owner": "cyberclaw",
                    "repo": "test",
                    "title": "Integration Test Issue",
                    "body": "Full integration test"
                }),
            },
            cyberclaw_core::CapabilityInvocation {
                connector_id: "slack".to_string(),
                capability: "slack.send_message".to_string(),
                params: serde_json::json!({
                    "channel": "#notifications",
                    "text": "Issue created successfully"
                }),
            },
        ],
        ..Default::default()
    }).await?;

    // 8. 等待执行完成
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 9. 验证结果
    let execution_result = control_plane.get_execution(&execution_id).await?;
    assert_eq!(execution_result.status, cyberclaw_core::ExecutionStatus::Completed);

    // 验证 Hook 被触发
    let hook_events = plugin_runtime.get_hook_events(&plugin_id).await?;
    assert!(hook_events.iter().any(|e| e.execution_id == execution_id));

    // 10. 清理
    scheduler.cancel_task(&task_id).await?;
    plugin_runtime.unload_plugin(&plugin_id).await?;

    mcp_server.shutdown().await;
    github_server.shutdown().await;
    slack_server.shutdown().await;

    Ok(())
}

// ==================== 性能和压力测试 ====================

#[tokio::test]
async fn test_high_load_concurrent_execution() -> Result<()> {
    init_test_env();

    let control_plane = cyberclaw_control_plane::ControlPlane::new(
        cyberclaw_control_plane::Config::default()
    ).await?;

    // 并发执行 100 个任务
    let mut handles = Vec::new();
    for i in 0..100 {
        let cp = control_plane.clone();
        let handle = tokio::spawn(async move {
            cp.execute(cyberclaw_core::Execution {
                id: format!("load-test-{}", i),
                agent_id: "test-agent".to_string(),
                task: format!("Task {}", i),
                ..Default::default()
            }).await
        });
        handles.push(handle);
    }

    // 等待所有任务完成
    let results: Vec<_> = futures::future::join_all(handles).await;

    // 验证所有任务都成功执行
    for result in results {
        assert!(result.is_ok());
        assert!(result.unwrap().is_ok());
    }

    Ok(())
}

#[tokio::test]
async fn test_plugin_isolation() -> Result<()> {
    init_test_env();

    let plugin_runtime = cyberclaw_plugin_runtime::PluginRuntime::new(
        cyberclaw_plugin_runtime::PluginRuntimeConfig {
            enable_sandbox: true,
            resource_limits: cyberclaw_plugin_runtime::ResourceLimits {
                max_memory: 50 * 1024 * 1024, // 50 MB
                max_cpu_ms: 5000,
                max_file_handles: 10,
                max_network_connections: 5,
            },
            ..Default::default()
        }
    ).await?;

    // 创建消耗资源的 Plugin
    let (_temp, plugin_path) = common::plugin::create_test_plugin("resource-heavy").await?;
    let plugin_id = plugin_runtime.load_plugin(&plugin_path).await?;

    // 尝试超出资源限制的操作
    let context = cyberclaw_plugin_runtime::HookContext {
        execution_id: "resource-test".to_string(),
        phase: cyberclaw_plugin_runtime::HookType::BeforeExecution,
        params: serde_json::json!({"allocate_memory": 100_000_000}), // 100 MB
    };

    let result = plugin_runtime
        .dispatch_hook(cyberclaw_plugin_runtime::HookType::BeforeExecution, &context)
        .await;

    // 应该因为资源限制而失败
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("resource"));

    Ok(())
}