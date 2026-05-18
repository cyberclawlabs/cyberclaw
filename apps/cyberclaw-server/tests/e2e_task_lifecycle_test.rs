#![allow(clippy::duplicate_mod)]
//! E2E 测试：Task 生命周期完整流程
//!
//! 测试场景：
//! - Task 创建和验证
//! - Task 列表查询
//! - Task 详情查询
//! - Task 状态跟踪
//! - Task 取消
//! - 完整生命周期流程

#[path = "common/mod.rs"]
mod common;
use axum::http::StatusCode;
use serde_json::json;

use common::assertions::*;
use common::{TestDataGenerator, TestServer};

/// E2E-TASK-001: Task 创建和验证
#[tokio::test]
async fn test_e2e_task_001_create_task() {
    let mut server = TestServer::new();

    // 创建任务
    let request = TestDataGenerator::create_task_request("E2E Test Task", "Analysis");
    let response = server.post("/api/v1/tasks", request).await;

    // 验证响应
    assert_status(&response, StatusCode::CREATED);
    assert_json_field(&response, "id");
    assert_json_field(&response, "title");
    assert_json_field(&response, "kind");
    assert_json_field(&response, "status");
    assert_json_field(&response, "priority");

    let json = response.json();
    assert_eq!(json["title"], "E2E Test Task");
    assert_eq!(json["kind"], "Analysis");
    assert_eq!(json["status"], "pending");

    println!("✅ E2E-TASK-001: Task 创建和验证测试通过");
}

/// E2E-TASK-002: Task 列表查询
#[tokio::test]
async fn test_e2e_task_002_list_tasks() {
    let mut server = TestServer::new();

    // 创建几个任务
    for i in 0..3 {
        let request = TestDataGenerator::create_task_request(&format!("Task {}", i), "Analysis");
        let response = server.post("/api/v1/tasks", request).await;
        assert_success(&response);
    }

    // 查询任务列表
    let response = server.get("/api/v1/tasks").await;

    // 验证响应
    assert_success(&response);

    let json = response.json();
    assert!(json.is_array(), "Response should be an array");

    let tasks = json.as_array().unwrap();
    assert!(tasks.len() >= 3, "Should have at least 3 tasks");

    println!(
        "✅ E2E-TASK-002: Task 列表查询测试通过 (共 {} 个任务)",
        tasks.len()
    );
}

/// E2E-TASK-003: Task 详情查询
#[tokio::test]
async fn test_e2e_task_003_get_task_details() {
    let mut server = TestServer::new();

    // 创建任务
    let request = TestDataGenerator::create_task_request("Detail Test Task", "Execution");
    let create_response = server.post("/api/v1/tasks", request).await;
    assert_status(&create_response, StatusCode::CREATED);

    let task_id = create_response.json()["id"].as_str().unwrap();

    // 查询任务详情
    let response = server.get(&format!("/api/v1/tasks/{}", task_id)).await;

    // 验证响应
    assert_success(&response);
    assert_json_value(&response, "id", &json!(task_id));
    assert_json_value(&response, "title", &json!("Detail Test Task"));
    assert_json_value(&response, "kind", &json!("Execution"));

    println!("✅ E2E-TASK-003: Task 详情查询测试通过");
}

/// E2E-TASK-004: Task 状态查询
#[tokio::test]
async fn test_e2e_task_004_get_task_status() {
    let mut server = TestServer::new();

    // 创建任务
    let request = TestDataGenerator::create_task_request("Status Test Task", "Analysis");
    let create_response = server.post("/api/v1/tasks", request).await;
    assert_status(&create_response, StatusCode::CREATED);

    let task_id = create_response.json()["id"].as_str().unwrap();

    // 查询任务状态
    let response = server
        .get(&format!("/api/v1/tasks/{}/status", task_id))
        .await;

    // 验证响应
    assert_success(&response);
    assert_json_field(&response, "id");
    assert_json_field(&response, "status");

    let json = response.json();
    assert_eq!(json["id"], task_id);
    assert_eq!(json["status"], "pending");

    println!("✅ E2E-TASK-004: Task 状态查询测试通过");
}

/// E2E-TASK-005: Task 取消
#[tokio::test]
async fn test_e2e_task_005_cancel_task() {
    let mut server = TestServer::new();

    // 创建任务
    let request = TestDataGenerator::create_task_request("Cancel Test Task", "Analysis");
    let create_response = server.post("/api/v1/tasks", request).await;
    assert_status(&create_response, StatusCode::CREATED);

    let task_id = create_response.json()["id"].as_str().unwrap();

    // 取消任务
    let response = server.delete(&format!("/api/v1/tasks/{}", task_id)).await;
    assert_status(&response, StatusCode::NO_CONTENT);

    // 验证任务已取消
    let status_response = server
        .get(&format!("/api/v1/tasks/{}/status", task_id))
        .await;
    assert_success(&status_response);

    let json = status_response.json();
    assert_eq!(json["status"], "cancelled");

    println!("✅ E2E-TASK-005: Task 取消测试通过");
}

/// E2E-TASK-006: 完整生命周期流程
#[tokio::test]
async fn test_e2e_task_006_full_lifecycle() {
    let mut server = TestServer::new();

    println!("  步骤 1: 创建任务");
    let request = TestDataGenerator::create_task_request("Lifecycle Task", "Analysis");
    let create_response = server.post("/api/v1/tasks", request).await;
    assert_status(&create_response, StatusCode::CREATED);

    let task_id = create_response.json()["id"].as_str().unwrap().to_string();
    println!("  ✓ 任务已创建: {}", task_id);

    println!("  步骤 2: 查询任务详情");
    let detail_response = server.get(&format!("/api/v1/tasks/{}", task_id)).await;
    assert_success(&detail_response);
    println!("  ✓ 任务详情查询成功");

    println!("  步骤 3: 查询任务状态");
    let status_response = server
        .get(&format!("/api/v1/tasks/{}/status", task_id))
        .await;
    assert_success(&status_response);
    assert_json_value(&status_response, "status", &json!("pending"));
    println!("  ✓ 任务状态: pending");

    println!("  步骤 4: 列出所有任务（应包含新任务）");
    let list_response = server.get("/api/v1/tasks").await;
    assert_success(&list_response);
    let tasks = list_response.json().as_array().unwrap();
    let found = tasks.iter().any(|t| t["id"] == task_id);
    assert!(found, "Task should be in the list");
    println!("  ✓ 任务列表包含新任务");

    println!("  步骤 5: 取消任务");
    let cancel_response = server.delete(&format!("/api/v1/tasks/{}", task_id)).await;
    assert_status(&cancel_response, StatusCode::NO_CONTENT);
    println!("  ✓ 任务已取消");

    println!("  步骤 6: 验证最终状态");
    let final_status_response = server
        .get(&format!("/api/v1/tasks/{}/status", task_id))
        .await;
    assert_success(&final_status_response);
    assert_json_value(&final_status_response, "status", &json!("cancelled"));
    println!("  ✓ 最终状态: cancelled");

    println!("✅ E2E-TASK-006: 完整生命周期流程测试通过");
}

/// E2E-TASK-007: 错误处理 - 空标题
#[tokio::test]
async fn test_e2e_task_007_error_empty_title() {
    let mut server = TestServer::new();

    // 创建无效任务（空标题）
    let invalid_request = json!({
        "title": "",
        "summary": "This should fail",
        "kind": "Analysis"
    });

    let response = server.post("/api/v1/tasks", invalid_request).await;

    // 应该返回 400 错误
    assert_status(&response, StatusCode::BAD_REQUEST);

    println!("✅ E2E-TASK-007: 错误处理（空标题）测试通过");
}

/// E2E-TASK-008: 错误处理 - 不存在的任务
#[tokio::test]
async fn test_e2e_task_008_error_nonexistent_task() {
    let mut server = TestServer::new();

    // 查询不存在的任务
    let response = server.get("/api/v1/tasks/nonexistent-task-id").await;

    // 应该返回 404 错误
    assert_status(&response, StatusCode::NOT_FOUND);

    println!("✅ E2E-TASK-008: 错误处理（不存在的任务）测试通过");
}

/// E2E-TASK-009: 错误处理 - 重复取消
#[tokio::test]
async fn test_e2e_task_009_error_double_cancel() {
    let mut server = TestServer::new();

    // 创建任务
    let request = TestDataGenerator::create_task_request("Double Cancel Task", "Analysis");
    let create_response = server.post("/api/v1/tasks", request).await;
    assert_status(&create_response, StatusCode::CREATED);

    let task_id = create_response.json()["id"].as_str().unwrap();

    // 第一次取消
    let cancel_response1 = server.delete(&format!("/api/v1/tasks/{}", task_id)).await;
    assert_status(&cancel_response1, StatusCode::NO_CONTENT);

    // 第二次取消（应该失败）
    let cancel_response2 = server.delete(&format!("/api/v1/tasks/{}", task_id)).await;

    // 应该返回错误（已经取消的任务不能再次取消）
    assert!(
        cancel_response2.status.is_client_error() || cancel_response2.status.is_server_error(),
        "Should return error when cancelling an already cancelled task"
    );

    println!("✅ E2E-TASK-009: 错误处理（重复取消）测试通过");
}

/// E2E-TASK-010: 并发任务创建
#[tokio::test]
async fn test_e2e_task_010_concurrent_task_creation() {
    use futures::future::join_all;

    // 创建 10 个并发任务
    let mut tasks = Vec::new();

    for i in 0..10 {
        let mut test_server = TestServer::new();

        let task = tokio::spawn(async move {
            let request = TestDataGenerator::create_task_request(
                &format!("Concurrent Task {}", i),
                "Analysis",
            );
            let response = test_server.post("/api/v1/tasks", request).await;

            response.status == StatusCode::CREATED
        });

        tasks.push(task);
    }

    // 等待所有任务完成
    let results = join_all(tasks).await;

    // 验证所有任务创建都成功
    let success_count = results.iter().filter(|r| *r.as_ref().unwrap()).count();

    assert_eq!(
        success_count, 10,
        "All 10 concurrent task creations should succeed"
    );

    println!(
        "✅ E2E-TASK-010: 并发任务创建测试通过 ({}/10 成功)",
        success_count
    );
}

/// E2E-TASK-011: 不同类型的任务
#[tokio::test]
async fn test_e2e_task_011_different_task_kinds() {
    let mut server = TestServer::new();

    let task_kinds = vec!["Analysis", "Investigation", "Review", "Execution"];

    for kind in task_kinds {
        let request = json!({
            "title": format!("{} Task", kind),
            "summary": format!("Testing {} task kind", kind),
            "kind": kind,
            "priority": "medium"
        });

        let response = server.post("/api/v1/tasks", request).await;
        assert_status(&response, StatusCode::CREATED);

        let json = response.json();
        assert_eq!(json["kind"], kind);
    }

    println!("✅ E2E-TASK-011: 不同类型的任务测试通过");
}

/// E2E-TASK-012: 不同优先级的任务
#[tokio::test]
async fn test_e2e_task_012_different_priorities() {
    let mut server = TestServer::new();

    // Priority enum is `#[serde(rename_all = "lowercase")]` — keep these
    // values lowercase or `axum` will reject the JSON body with 422.
    let priorities = vec!["critical", "high", "medium", "low"];

    for priority in priorities {
        let request = json!({
            "title": format!("{} Priority Task", priority),
            "summary": format!("Testing {} priority", priority),
            "kind": "Analysis",
            "priority": priority
        });

        let response = server.post("/api/v1/tasks", request).await;
        assert_status(&response, StatusCode::CREATED);

        let json = response.json();
        assert_eq!(json["priority"], priority);
    }

    println!("✅ E2E-TASK-012: 不同优先级的任务测试通过");
}
