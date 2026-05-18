//! 请求体大小限制中间件（Content-Length 预检）
//!
//! 注意: 主要的请求体大小限制现在由 `tower_http::limit::RequestBodyLimitLayer` 提供，
//! 它能正确处理所有传输编码（包括 chunked transfer）。
//! 本中间件作为 defense-in-depth 层保留，用于在读取 body 之前基于 Content-Length
//! header 快速拒绝明显过大的请求。

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

/// 请求体大小限制中间件
///
/// # 参数
///
/// * `max_size` - 最大请求体大小（字节）
///
/// # 返回
///
/// 返回一个中间件函数
pub fn enforce_body_limit(
    max_size: usize,
) -> impl Fn(
    Request,
    Next,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Response, StatusCode>> + Send>,
> + Clone {
    move |req: Request, next: Next| {
        let max_size = max_size;
        Box::pin(async move {
            let (parts, body) = req.into_parts();

            // 检查 Content-Length header
            if let Some(content_length) = parts.headers.get("content-length") {
                if let Ok(size_str) = content_length.to_str() {
                    if let Ok(size) = size_str.parse::<usize>() {
                        if size > max_size {
                            tracing::warn!(
                                "Request body too large: {} bytes (max: {} bytes)",
                                size,
                                max_size
                            );
                            return Err(StatusCode::PAYLOAD_TOO_LARGE);
                        }
                    }
                }
            }

            let req = Request::from_parts(parts, body);
            Ok(next.run(req).await)
        })
    }
}

/// 请求体过大错误响应
pub struct BodyTooLargeError;

impl IntoResponse for BodyTooLargeError {
    fn into_response(self) -> Response {
        (StatusCode::PAYLOAD_TOO_LARGE, "Request body too large").into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        middleware,
        routing::post,
        Router,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_body_limit_accepts_small_body() {
        let max_size = 1024; // 1 KB
        let app = Router::new()
            .route("/test", post(|| async { "OK" }))
            .layer(middleware::from_fn(enforce_body_limit(max_size)));

        let request = Request::builder()
            .uri("/test")
            .method("POST")
            .header("content-length", "100")
            .body(Body::from(vec![0u8; 100]))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_body_limit_rejects_large_body() {
        let max_size = 1024; // 1 KB
        let app = Router::new()
            .route("/test", post(|| async { "OK" }))
            .layer(middleware::from_fn(enforce_body_limit(max_size)));

        let request = Request::builder()
            .uri("/test")
            .method("POST")
            .header("content-length", "2048") // 超过限制
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
