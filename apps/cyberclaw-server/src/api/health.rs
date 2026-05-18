//! 健康检查 API

use axum::{routing::get, Router};
use std::sync::Arc;

use crate::state::AppState;

/// 创建健康检查路由
///
/// `/healthz` 是 K8s/Prometheus 习惯名（很多用户先试这个），与 `/health` 共指
/// 同一处理器；`/ready` 单独保留 readiness 语义。
pub fn create_health_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health_check))
        .route("/healthz", get(health_check))
        .route("/ready", get(readiness_check))
}

/// 健康检查端点
async fn health_check() -> &'static str {
    "OK"
}

/// 就绪检查端点
async fn readiness_check() -> &'static str {
    "READY"
}
