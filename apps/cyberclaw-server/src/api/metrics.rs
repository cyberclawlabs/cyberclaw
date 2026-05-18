//! Prometheus metrics export endpoint
//!
//! GET /metrics 返回 Prometheus text format (version 0.0.4)。
//! 该端点是公开的（无需 JWT 认证），依赖反代层做 IP 白名单控制。

use axum::{http::header, response::IntoResponse, routing::get, Router};
use std::sync::Arc;

use crate::state::AppState;

/// 创建 Prometheus metrics 路由
pub fn create_metrics_router() -> Router<Arc<AppState>> {
    Router::new().route("/metrics", get(metrics_handler))
}

/// Prometheus text format metrics handler
///
/// 返回全局 prometheus registry 中所有已注册的 metric families，
/// 格式为 Prometheus text exposition format 0.0.4。
async fn metrics_handler() -> impl IntoResponse {
    let output = cyberclaw_observability::metrics::export_metrics()
        .unwrap_or_else(|e| format!("# error collecting metrics: {e}\n"));

    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        output,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    /// metrics_handler 不依赖 AppState，直接构造独立 Router 避免重型依赖。
    fn build_app() -> axum::Router {
        axum::Router::new().route("/metrics", axum::routing::get(metrics_handler))
    }

    #[tokio::test]
    async fn test_metrics_endpoint_returns_200() {
        let app = build_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_metrics_content_type_is_prometheus() {
        let app = build_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let ct = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.contains("text/plain"),
            "expected text/plain content-type, got: {ct}"
        );
        assert!(
            ct.contains("0.0.4"),
            "expected version=0.0.4 in content-type, got: {ct}"
        );
    }

    #[tokio::test]
    async fn test_metrics_body_contains_help_line() {
        // 触发一次 metric 写入，确保全局 static 已初始化
        cyberclaw_observability::metrics::recorders::record_retry_attempt("test_metrics_ep");

        let app = build_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8_lossy(&body_bytes);

        assert!(
            body.contains("# HELP"),
            "expected Prometheus # HELP lines in /metrics output"
        );
        assert!(
            body.contains("cyberclaw_"),
            "expected cyberclaw_ prefixed metrics in output"
        );
    }
}
