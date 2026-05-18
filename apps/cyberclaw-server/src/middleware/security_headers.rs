//! 安全响应头中间件
//!
//! 为所有响应添加安全相关的 HTTP 头部，防御多种 Web 攻击

use axum::{
    body::Body,
    http::{HeaderValue, Request},
    middleware::Next,
    response::Response,
};

/// 为所有响应添加安全响应头
///
/// 添加的安全头包括：
/// - `X-Content-Type-Options`: 防止 MIME 类型嗅探
/// - `X-Frame-Options`: 防止点击劫持攻击
/// - `X-XSS-Protection`: 启用浏览器 XSS 保护
/// - `Strict-Transport-Security`: 强制使用 HTTPS（仅在生产环境）
/// - `Content-Security-Policy`: 限制资源加载来源
pub async fn add_security_headers(req: Request<Body>, next: Next) -> Response {
    let is_admin_spa = req.uri().path().starts_with("/admin");
    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    // 防止 MIME 类型嗅探
    headers.insert(
        "X-Content-Type-Options",
        HeaderValue::from_static("nosniff"),
    );

    // 防止点击劫持
    headers.insert("X-Frame-Options", HeaderValue::from_static("DENY"));

    // XSS 保护（虽然现代浏览器已废弃，但保留以兼容旧浏览器）
    headers.insert(
        "X-XSS-Protection",
        HeaderValue::from_static("1; mode=block"),
    );

    // 严格传输安全（HTTPS）
    // 注意：仅在 HTTPS 连接上启用 HSTS
    // P1-6 fix: default to "production" to match main.rs
    if std::env::var("ENVIRONMENT")
        .unwrap_or_else(|_| "production".to_string())
        .to_lowercase()
        == "production"
    {
        headers.insert(
            "Strict-Transport-Security",
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }

    // 内容安全策略
    // /admin/* SPA uses CDN assets (React/ReactDOM/Tailwind/Google Fonts).
    // JSX is precompiled by `npm run build:web` and served from /admin/dist/*
    // (same-origin, covered by 'self'). 'unsafe-eval' kept for Tailwind JIT.
    // All other routes keep the strict default.
    let csp = if is_admin_spa {
        "default-src 'self'; \
         script-src 'self' 'unsafe-inline' 'unsafe-eval' https://unpkg.com https://cdn.tailwindcss.com https://cdn.jsdelivr.net; \
         style-src 'self' 'unsafe-inline' https://fonts.googleapis.com https://cdn.tailwindcss.com https://cdn.jsdelivr.net; \
         font-src 'self' https://fonts.gstatic.com; \
         connect-src 'self'; \
         img-src 'self' data:"
    } else {
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'"
    };
    headers.insert("Content-Security-Policy", HeaderValue::from_static(csp));

    response
}
