//! CyberClaw Server Library
//!
//! 导出供测试使用的模块和类型

pub mod admin_store;
pub mod api;
pub mod audit;
pub mod audit_archive;
pub mod clarify_broadcast;
pub mod cluster_assignments;
pub mod config;
pub mod connector_drift;
pub mod cron;
pub mod error;
pub mod feedback_loop;
pub mod installed_packages;
pub mod memory_cleanup;
pub mod memory_connector;
pub mod middleware;
pub mod persistence;
pub mod profiles_store;
pub mod search;
pub mod services;
pub mod skill_index;
pub mod sse_replay;
pub mod state;
pub mod todo_connector;
pub mod tools_overrides;
pub mod types;
pub mod usage;

pub use config::ServerConfig;
pub use error::ApiError;
pub use state::{AppState, ControlPlaneComponents};

use axum::{
    http::{header, HeaderValue, Method},
    middleware as axum_middleware, Router,
};
use std::sync::Arc;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    limit::RequestBodyLimitLayer,
    trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer},
};

/// 创建应用路由
///
/// # 参数
///
/// * `state` - 应用状态
/// * `config` - 服务器配置（可选，用于速率限制和请求体大小限制）
///
/// # 返回
///
/// 配置好的路由
pub fn create_router(state: Arc<AppState>) -> Router {
    create_router_with_config(state, ServerConfig::default())
}

/// 创建应用路由（带配置）
///
/// # 参数
///
/// * `state` - 应用状态
/// * `config` - 服务器配置
///
/// # 返回
///
/// 配置好的路由
pub fn create_router_with_config(state: Arc<AppState>, config: ServerConfig) -> Router {
    let origin_source = if config.cors_origins.is_empty() {
        tracing::warn!("ServerConfig.cors_origins is empty, using localhost defaults");
        vec![
            "http://localhost:3000".to_string(),
            "http://localhost:5173".to_string(),
        ]
    } else {
        config.cors_origins.clone()
    };

    let allowed_origins = origin_source
        .into_iter()
        .filter_map(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() {
                return None;
            }
            match trimmed.parse::<HeaderValue>() {
                Ok(header) => Some(header),
                Err(e) => {
                    tracing::error!("Invalid CORS origin '{}': {}", trimmed, e);
                    None
                }
            }
        })
        .collect::<Vec<_>>();

    // 确保至少有一个有效的来源
    if allowed_origins.is_empty() {
        tracing::error!(
            "CORS configuration error: No valid origins. Set CORS_ALLOWED_ORIGINS env var."
        );
        std::process::exit(1);
    }

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed_origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::DELETE,
            Method::PUT,
            Method::PATCH,
            Method::OPTIONS,
        ])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, header::ACCEPT])
        .allow_credentials(true)
        .max_age(std::time::Duration::from_secs(3600));

    // 受保护的 API 路由（需要 JWT 认证）
    //
    // Sprint 7: skills / channels / settings / cluster merged in. The
    // admin-authenticated router still owns `/admin/me`, `/admin/logout`,
    // `/admin/dashboard`. `/admin/events` is deliberately NOT here —
    // EventSource cannot send an Authorization header, so that route
    // self-validates the `?token=` query parameter.
    let protected_routes = Router::new()
        .merge(api::create_chat_router())
        .merge(api::create_chat_approval_router())
        .merge(api::create_chat_clarify_router())
        .merge({
            #[cfg(debug_assertions)]
            {
                api::chat_clarify::dev::create_dev_clarify_router()
            }
            #[cfg(not(debug_assertions))]
            {
                Router::new()
            }
        })
        .merge(api::create_chat_compress_router())
        .merge(api::create_chat_conversations_router())
        .merge(api::create_agent_chat_router())
        // v1.3 WP-1 — server-side conversation session endpoint.
        .merge(api::create_agent_chat_v2_router())
        .merge(api::create_agents_router())
        .merge(api::create_tasks_router())
        .merge(api::create_executions_router())
        .merge(api::create_reviews_router())
        .merge(api::create_sessions_router())
        .merge(api::create_capabilities_router())
        .merge(api::create_workflows_router())
        // Sprint 18 W2 — observability demo endpoints (replaces remaining
        // SPA-side hardcoded MOCKs). Merged BEFORE the skills router so
        // `/api/v1/skills/poison-verdicts` doesn't get captured by the
        // `/api/v1/skills/:id` glob.
        .merge(api::create_observability_demo_router())
        .merge(connector_drift::create_connector_drift_router())
        .merge(api::create_skills_router())
        .merge(api::create_channels_router())
        .merge(api::create_settings_router())
        .merge(api::create_cluster_router())
        .merge(api::create_cluster_brain_router())
        .merge(api::create_connectors_router())
        .merge(api::create_tools_router())
        .merge(api::create_audit_router())
        .merge(api::create_plugins_router())
        // v1.x — unified package management surface (`/api/v2/packages`,
        // `/api/v2/status`). Replaces the CLI's process-local
        // InMemoryRegistry; server is now the single source of truth.
        .merge(api::create_packages_router())
        .merge(api::create_uploads_router())
        // Sprint 11 W2 L1 — admin UI aggregate endpoints.
        .merge(api::create_governance_router())
        .merge(api::create_learning_router())
        .merge(api::create_workbench_router())
        // Sprint 17 Lane B — Govern Memory Console.
        .merge(api::create_memory_router())
        // Sprint 21 — Multi-Agent Handoff HTTP surface.
        .merge(api::create_handoff_router())
        // BT-37 — Admin MCP server hot-reload (list/register/unregister).
        .merge(api::admin::mcp::create_admin_mcp_router())
        // Sprint 18 W3 admin REST surfaces — capability layer already shipped,
        // these merge the HTTP entry points (browser/multimodal/im/kanban/curator/moa).
        .merge(api::admin::browser::create_admin_browser_router())
        .merge(api::admin::multimodal::create_admin_multimodal_router())
        .merge(api::admin::im_platforms::create_admin_im_platforms_router())
        .merge(api::admin::llm_models::create_admin_llm_models_router())
        .merge(api::admin::kanban::create_admin_kanban_router())
        .merge(api::admin::curator::create_admin_curator_router())
        .merge(api::admin::moa::create_admin_moa_router())
        // A-4 (2026-05-05) — capability gap queue admin REST surface.
        .merge(api::admin::capability_requests::create_admin_capability_requests_router())
        // Usage statistics endpoint.
        .merge(api::create_usage_router())
        // Stub endpoints — empty-list 200s for v2 frontend pages that previously 404'd.
        .merge(api::create_stubs_router())
        // Cron job CRUD — replaces the /api/v1/cron stub.
        .merge(api::create_cron_router())
        // User profile store — SOUL / personality preference layer.
        .merge(api::create_profiles_router())
        .merge(api::create_admin_authenticated_router())
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::jwt_auth,
        ));

    // P1-2 fix: 内部集群路由也应用安全头和审计日志
    let internal_routes = api::create_internal_cluster_router()
        .layer(axum_middleware::from_fn(middleware::add_security_headers))
        .layer(RequestBodyLimitLayer::new(config.max_body_size))
        .layer(axum_middleware::from_fn(middleware::enforce_body_limit(
            config.max_body_size,
        )))
        .layer(middleware::create_rate_limiter(
            config.rate_limit_per_second,
            config.rate_limit_burst_size,
        ))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().include_headers(false))
                .on_request(DefaultOnRequest::new().level(tracing::Level::INFO))
                .on_response(DefaultOnResponse::new().level(tracing::Level::INFO)),
        );

    // Sprint 20 W1 — observability scrape endpoints are now a separate
    // route lane that does NOT carry the rate-limit layer. /health is
    // hit by Kubernetes liveness/readiness probes (every 5-10s per
    // replica) and /metrics by Prometheus (every 5-30s); both legitimately
    // ignore rate limits and were previously contributing scrape traffic
    // to the per-IP budget meant for user requests.
    let scrape_routes = Router::new()
        .merge(api::create_health_router())
        .merge(api::create_metrics_router());

    // 公开路由（不需要 JWT 认证）
    // Webhook 路由不需要 JWT 认证，依赖平台签名校验。
    // Wizard (`/wizard/*`) 路由是 Onboarding Web UI 的入口；Sprint 5 起由
    // `bootstrap_auth` 中间件接管：首次启动使用一次性 bootstrap token，已注册
    // 操作员可用 JWT 重入。
    let public_routes = Router::new()
        .merge(api::create_webhooks_router())
        .merge(api::wizard::create_wizard_router_with_auth(state.clone()))
        .merge(api::create_admin_public_router())
        // Sprint 7: SSE stream for live admin events. The handler
        // self-validates JWT from `Authorization: Bearer` or `?token=`
        // (EventSource can't set headers), so it lives outside the
        // JWT middleware lane.
        .merge(api::admin::events::create_admin_events_router())
        // Phase 3: PTY WebSocket endpoint. WebSocket API cannot set
        // Authorization headers, so self-validates `?token=` JWT
        // (same pattern as /admin/events).
        .merge(api::create_pty_router());

    // Rate-limited app routes (everything except scrape endpoints).
    let rate_limited_routes = Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .layer(middleware::create_rate_limiter(
            config.rate_limit_per_second,
            config.rate_limit_burst_size,
        ));

    let application_routes = Router::new()
        .merge(scrape_routes)
        .merge(rate_limited_routes)
        .layer(axum_middleware::from_fn(middleware::add_security_headers))
        // 请求体大小限制（RequestBodyLimitLayer 处理所有传输编码，包括 chunked）
        .layer(RequestBodyLimitLayer::new(config.max_body_size))
        // Content-Length 预检（defense-in-depth，在读取 body 前快速拒绝）
        .layer(axum_middleware::from_fn(middleware::enforce_body_limit(
            config.max_body_size,
        )))
        // 增强的请求/响应日志和审计
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().include_headers(false))
                .on_request(DefaultOnRequest::new().level(tracing::Level::INFO))
                .on_response(DefaultOnResponse::new().level(tracing::Level::INFO)),
        )
        .layer(cors);

    Router::new()
        .merge(internal_routes)
        .merge(application_routes)
        .with_state(state)
}
