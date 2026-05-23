//! Admin backend console — first-party server module.
//!
//! Sprint 6. Exposes `/admin/*` HTTP surface: a thin login lane, a
//! dashboard aggregation endpoint, and a vanilla-JS SPA shell served from
//! [`ADMIN_HTML`]. All management verbs (agents/tasks/executions/reviews/
//! capabilities) continue to live in their respective routers — the SPA
//! calls those directly from the browser with a Bearer token.
//!
//! # Routes
//!
//! | Route | Auth | Purpose |
//! |---|---|---|
//! | `POST /admin/login` | public | Stub login: `user_id` must exist in `UsersConfig` |
//! | `POST /admin/logout` | JWT | No server state; echoes `{ok:true}` |
//! | `GET  /admin/me` | JWT | Returns the caller's `OperatorRecord` |
//! | `GET  /admin/dashboard` | JWT | Health + counts + recent executions |
//! | `GET  /admin` | JWT | Serves the SPA shell HTML |
//!
//! # Architectural compliance (EVOLUTION_IDIOMS §1–§6)
//!
//! - §1 Injected trait dispatcher: N/A — this module only composes existing
//!   handlers; no orchestration component added.
//! - §2 Event sink trait: N/A — no long-running component.
//! - §3 Plan-in + events-out: N/A — this is a stateless HTTP adapter.
//! - §4 Facades from owner crate: N/A — no capability added.
//! - §5 No covert sixth object: confirmed. `admin` is a plain first-party
//!   server module, same taxonomy as `wizard`. It is NOT a Platform Plugin,
//!   not a new object class, and adds no `ecosystem/` subdirectory.
//! - §6 Skill is methodology: N/A — no Skill bundle added.

pub mod browser;
pub mod capability_requests;
pub mod curator;
pub mod dashboard;
pub mod events;
pub mod im_platforms;
pub mod kanban;
pub mod llm_models;
pub mod login;
pub mod mcp;
pub mod moa;
pub mod multimodal;
pub mod onboarding;
pub mod seed;

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};

use crate::state::AppState;

/// Web root containing the Claude Design bundle (cyberclaw.html + src/*.jsx).
/// Override via `CYBERCLAW_WEB_ROOT` env var (useful for production deploys).
fn web_root() -> PathBuf {
    std::env::var("CYBERCLAW_WEB_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("web"))
}

/// Allowlist of JSX files that can be served from `web/src/`.
/// Locks down /admin/src/* against path traversal + unauthorized file exposure.
const ALLOWED_JSX: &[&str] = &[
    "api.jsx",
    "app.jsx",
    "shell.jsx",
    "pages_a.jsx",
    "pages_b.jsx",
    "pages_c.jsx",
    "ui.jsx",
    "data.jsx",
    "i18n.jsx",
    "icons.jsx",
    "user_menu.jsx",
    "onboarding.jsx",
    "agent_detail.jsx",
    "agent_wizard.jsx",
    "pages_learning.jsx",
    "pages_workbench.jsx",
    "pages_chat.jsx",
    "pages_memory_panel.jsx",
    "pages_clarify_card.jsx",
    "pages_clarifications.jsx",
    "pages_compress_summary.jsx",
    "pages_handoff_card.jsx",
    "pages_handoffs.jsx",
    "pages_memory_console.jsx",
    "pages_admin_ops.jsx",
    // Hermes v0.12 closure SPA pages.
    "pages_kanban.jsx",
    "pages_browser_console.jsx",
    "pages_multimodal.jsx",
    "pages_im_platforms.jsx",
    "pages_curator.jsx",
    "pages_moa.jsx",
    "pages_capability_monitor.jsx",
    // F4/F7 admin panels (deep-crawl 2026-05-06: index.html referenced
    // them but allowlist was missing → 404 + CSP refused → page broken).
    "pages_tools.jsx",
    "pages_cluster.jsx",
];

/// Allowlist of compiled JS files that can be served from `web/dist/`.
/// Mirrors ALLOWED_JSX but with `.js` extension — produced by
/// `npx babel web/src --out-dir web/dist --extensions .jsx --out-file-extension .js`.
const ALLOWED_JS: &[&str] = &[
    "api.js",
    "app.js",
    "shell.js",
    "pages_a.js",
    "pages_b.js",
    "pages_c.js",
    "ui.js",
    "data.js",
    "i18n.js",
    "icons.js",
    "user_menu.js",
    "onboarding.js",
    "agent_detail.js",
    "agent_wizard.js",
    "pages_learning.js",
    "pages_workbench.js",
    "pages_chat.js",
    "pages_memory_panel.js",
    "pages_clarify_card.js",
    "pages_clarifications.js",
    "pages_compress_summary.js",
    "pages_handoff_card.js",
    "pages_handoffs.js",
    "pages_memory_console.js",
    "pages_admin_ops.js",
    // Hermes v0.12 closure SPA pages.
    "pages_kanban.js",
    "pages_browser_console.js",
    "pages_multimodal.js",
    "pages_im_platforms.js",
    "pages_curator.js",
    "pages_moa.js",
    "pages_capability_monitor.js",
    // F4/F7 admin panels (deep-crawl 2026-05-06: index.html referenced
    // them but allowlist was missing → 404 + CSP refused → page broken).
    "pages_tools.js",
    "pages_cluster.js",
    // 自托管 Tailwind JIT runtime（替代 cdn.tailwindcss.com，去 console warning + 离线可用）
    "tailwind.runtime.js",
];

/// Build the public `/admin/login` + SPA shell + JSX asset router (no JWT).
/// The SPA shell is public because the React app renders the login form
/// itself; once the operator logs in, jwt is stored in sessionStorage and
/// all subsequent API calls (/admin/me, /admin/dashboard, /agents, ...) are
/// gated by the authenticated middleware lane.
pub fn create_admin_public_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/login", post(login::admin_login))
        // F6 cutover: /admin 重定向到 /admin/v2（v2 stack = Vite+TS 默认入口）。
        // 重定向比直接 serve v2 html 干净——v2 BrowserRouter basename 是
        // "/admin/v2"，如果在 /admin URL 上 serve 同样 html 会 routing
        // 错乱。/admin/v1 别名保留 babel 老栈做 fallback；下个迭代评估
        // 是否完全下线。
        .route("/admin", get(redirect_to_v2))
        .route("/admin/src/:file", get(serve_admin_jsx))
        .route("/admin/dist/:file", get(serve_admin_dist))
        // 新前端栈直访路径（与 vite.config.ts base "/admin/v2/" 对齐）
        .route("/admin/v2", get(serve_admin_v2_html))
        .route("/admin/v2/", get(serve_admin_v2_html))
        .route("/admin/v2/assets/:file", get(serve_admin_v2_asset))
        // SPA fallback：react-router client routes (/admin/v2/login,
        // /admin/v2/chat, ...) deep link 直访时返回 SPA shell，让 browser
        // history 接管。assets/ 已先匹配上面的精确路由。
        .route("/admin/v2/*rest", get(serve_admin_v2_html))
}

/// Build the authenticated `/admin/*` API router (JWT required).
pub fn create_admin_authenticated_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/logout", post(login::admin_logout))
        .route("/admin/me", get(login::admin_me))
        .route("/admin/dashboard", get(dashboard::admin_dashboard))
        // Sprint 8 Phase A — `/admin/seed-demo` for populating AdminStore.
        .merge(seed::create_admin_seed_router())
        // Sprint 8 L4 — `/admin/onboarding/*` for admin console guided tour.
        .merge(onboarding::create_admin_onboarding_router())
}

/// `GET /admin` — 301 redirect 到 v2 stack（F6 cutover 后默认入口）。
async fn redirect_to_v2() -> Response {
    (
        StatusCode::MOVED_PERMANENTLY,
        [(header::LOCATION, "/admin/v2/")],
    )
        .into_response()
}

/// `GET /admin/src/:file` — JSX module from `web/src/`.
/// Enforces an allowlist against path traversal + arbitrary file exposure.
async fn serve_admin_jsx(Path(file): Path<String>) -> Response {
    if !ALLOWED_JSX.contains(&file.as_str()) {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let path = web_root().join("src").join(&file);
    match tokio::fs::read_to_string(&path).await {
        Ok(body) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            )],
            body,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// `GET /admin/dist/:file` — Babel-compiled module from `web/dist/`.
/// Same allowlist semantics as [`serve_admin_jsx`]; the SPA loads these
/// instead of raw .jsx so the browser doesn't need babel-standalone (which
/// caused IIFE-shadow issues with cross-file globals like `tFor`).
/// `GET /admin/v2` — Vite-built SPA shell (`web/dist-vite/index.html`).
/// 独立 entry，与老 /admin 的 cyberclaw.html 完全分离。
async fn serve_admin_v2_html() -> Response {
    let path = web_root().join("dist-vite").join("index.html");
    match tokio::fs::read_to_string(&path).await {
        Ok(body) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                // index.html 引用 Vite hashed asset 名（index-XXXX.js），
                // 必须 no-cache 才能保证新 hash 立即被发现。否则浏览器
                // 拿着旧 index.html → 永远请求旧 hash → 新 build 看不见。
                (header::CACHE_CONTROL, "no-cache, no-store, must-revalidate"),
                (header::PRAGMA, "no-cache"),
                (header::EXPIRES, "0"),
            ],
            body,
        )
            .into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            format!(
                "admin v2 console not built — run: cd web && npm run build (looked at {})",
                path.display()
            ),
        )
            .into_response(),
    }
}

/// `GET /admin/v2/assets/:file` — Vite hashed bundle/css/etc.
/// 不用 allowlist：Vite 输出文件名带内容哈希（`index-Brk4kSa1.js`），无法预先列出；
/// 防越界靠 file 不能含 `..` / `/`（axum Path 解构本身约束 + 显式校验）。
async fn serve_admin_v2_asset(Path(file): Path<String>) -> Response {
    if file.contains('/') || file.contains("..") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let path = web_root().join("dist-vite").join("assets").join(&file);
    let ct = if file.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if file.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if file.ends_with(".svg") {
        "image/svg+xml"
    } else if file.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    };
    match tokio::fs::read(&path).await {
        Ok(body) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, ct),
                // Vite hashed 文件名天然带 cache buster，可长缓存。
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            body,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn serve_admin_dist(Path(file): Path<String>) -> Response {
    if !ALLOWED_JS.contains(&file.as_str()) {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let path = web_root().join("dist").join(&file);
    match tokio::fs::read_to_string(&path).await {
        Ok(body) => (
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    "application/javascript; charset=utf-8",
                ),
                // 不缓存：源 JSX 一改 babel 重编 dist/，必须每次回服务器拉新。
                // 后续若加内容哈希文件名（pages_b.{hash}.js），可换长缓存策略。
                (header::CACHE_CONTROL, "no-cache, must-revalidate"),
            ],
            body,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::admin::login::tests::build_test_state;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware as axum_middleware;
    use tower::ServiceExt;

    fn admin_authenticated_app(state: Arc<AppState>) -> Router {
        create_admin_authenticated_router()
            .layer(axum_middleware::from_fn_with_state(
                state.clone(),
                crate::middleware::jwt_auth,
            ))
            .with_state(state)
    }

    fn admin_public_app(state: Arc<AppState>) -> Router {
        create_admin_public_router().with_state(state)
    }

    #[tokio::test]
    async fn jsx_allowlist_rejects_non_listed_file() {
        let state = build_test_state();
        let app = admin_public_app(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/src/secrets.jsx")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn jsx_allowlist_rejects_path_traversal() {
        let state = build_test_state();
        let app = admin_public_app(state);
        // Axum route matching keeps the traversal attempt out of the allowlist anyway.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/admin/src/..%2F..%2Fetc%2Fpasswd")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn admin_routes_return_401_without_jwt() {
        let state = build_test_state();
        let app = admin_authenticated_app(state);
        for path in ["/admin/me", "/admin/dashboard", "/admin/logout"] {
            let method = if path == "/admin/logout" {
                "POST"
            } else {
                "GET"
            };
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "path {} should be 401 without jwt",
                path
            );
        }
    }
}
