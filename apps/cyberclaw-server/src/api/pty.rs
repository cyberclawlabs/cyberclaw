//! PTY WebSocket endpoint — `/api/v1/pty/ws?token={jwt}`
//!
//! Spawns a `cyberclaw-cli chat` subprocess inside a PTY and bridges it
//! to the browser over a WebSocket connection.
//!
//! # Auth
//!
//! JWT is validated from the `?token=` query parameter because the browser
//! WebSocket API cannot set arbitrary request headers (same pattern as
//! `/admin/events`). Requires `admin` role (PTY is a privileged shell).
//!
//! # Security
//!
//! - Subprocess environment is scrubbed: only a minimal allow-list of env
//!   vars is forwarded so server secrets (JWT_SECRET, CYBERCLAW_*, API keys)
//!   are never visible to the spawned shell.
//! - At most `MAX_PTY` concurrent PTY sessions are permitted. Connections
//!   beyond that limit receive 429 Too Many Requests.
//! - Every session open/close is emitted as an `AuditKind::Mutation` entry.
//!
//! # Protocol
//!
//! - Binary WebSocket frames → raw bytes written to PTY master stdin.
//! - PTY master stdout → binary WebSocket frames sent to client.
//! - Text frame `{"type":"resize","cols":N,"rows":N}` → PTY resize.
//! - On WebSocket close or PTY process exit → PTY is killed and tasks stop.
//!
//! # Lifecycle
//!
//! Each WebSocket connection owns exactly one PTY subprocess. No multiplexing.
//! Each connect = fresh PTY (no session resumption in v1).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::audit::{AuditEntry, AuditKind, AuditResult, AuditSink};
use crate::middleware::auth::{require_admin, verify_jwt, Claims};
use crate::state::AppState;

/// Hard cap on concurrent PTY sessions.
const MAX_PTY: usize = 8;

/// Global counter of active PTY sessions.
static ACTIVE_PTY_COUNT: AtomicUsize = AtomicUsize::new(0);

/// RAII guard that decrements `ACTIVE_PTY_COUNT` on drop.
struct PtyGuard;

impl Drop for PtyGuard {
    fn drop(&mut self) {
        ACTIVE_PTY_COUNT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Query params: JWT token for auth (WebSocket can't set headers).
#[derive(Debug, Deserialize)]
pub struct PtyQuery {
    #[serde(default)]
    token: Option<String>,
}

/// Resize control frame sent by the frontend xterm.js.
#[derive(Debug, Deserialize)]
struct ResizeFrame {
    #[serde(rename = "type")]
    kind: String,
    cols: u16,
    rows: u16,
}

/// Build the PTY router. Not JWT-gated at middleware level — handler
/// self-validates `?token=` (same pattern as `/admin/events`).
pub fn create_pty_router() -> Router<Arc<AppState>> {
    Router::new().route("/api/v1/pty/ws", get(pty_ws_handler))
}

/// `GET /api/v1/pty/ws` — upgrade to WebSocket then bridge PTY.
async fn pty_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(query): Query<PtyQuery>,
) -> Response {
    // ── Auth: require valid JWT ──────────────────────────────────────────────
    let Some(token) = query.token else {
        return (StatusCode::UNAUTHORIZED, "missing token").into_response();
    };

    let claims = match verify_jwt(&token, state.jwt_secret.as_bytes()) {
        Ok(c) => c,
        Err(err) => {
            warn!(%err, "pty ws rejected: invalid jwt");
            return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
        }
    };

    // ── Auth: require admin role ─────────────────────────────────────────────
    if let Err(err) = require_admin(&claims).await {
        warn!(user_id = %claims.sub, %err, "pty ws rejected: non-admin role");
        return (StatusCode::FORBIDDEN, "PTY access requires admin role").into_response();
    }

    // ── Concurrency cap ──────────────────────────────────────────────────────
    if ACTIVE_PTY_COUNT.fetch_add(1, Ordering::SeqCst) >= MAX_PTY {
        ACTIVE_PTY_COUNT.fetch_sub(1, Ordering::SeqCst);
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "PTY concurrent limit reached",
        )
            .into_response();
    }
    // Guard decrements counter when the connection ends (moved into handle_socket).
    let guard = PtyGuard;

    ws.on_upgrade(move |socket| handle_socket(socket, claims, state, guard))
}

/// Handle the upgraded WebSocket: spawn PTY subprocess and bridge I/O.
async fn handle_socket(
    mut socket: WebSocket,
    claims: Claims,
    state: Arc<AppState>,
    _guard: PtyGuard,
) {
    let session_start = Instant::now();
    let actor = claims.sub.as_str().to_string();

    // ── Audit: session start ─────────────────────────────────────────────────
    emit_audit(
        &state.audit,
        &actor,
        "pty.session_start",
        serde_json::json!({}),
    )
    .await;

    // ── Spawn PTY ────────────────────────────────────────────────────────────
    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            warn!(%e, "failed to open PTY");
            let _ = socket
                .send(Message::Text(format!("PTY open failed: {e}\r\n")))
                .await;
            emit_audit(
                &state.audit,
                &actor,
                "pty.session_end",
                serde_json::json!({"reason": "pty_open_failed", "duration_secs": session_start.elapsed().as_secs()}),
            )
            .await;
            return;
        }
    };

    // Resolve the CLI binary path: explicit env var → server-binary-sibling →
    // PATH fallback. The sibling lookup is the production path: when the server
    // is installed alongside cyberclaw-cli (cargo build --release puts both in
    // target/release/), spawning by absolute path avoids depending on PATH
    // being set up correctly for the server process.
    let cli_bin = std::env::var("CYBERCLAW_CLI_BIN").unwrap_or_else(|_| {
        // canonicalize() resolves symlinks first so the sibling lookup follows
        // the installed binary's real directory, not a launcher symlink — which
        // would otherwise let an attacker plant a fake cyberclaw-cli next to
        // a /usr/local/bin shim.
        std::env::current_exe()
            .ok()
            .and_then(|p| p.canonicalize().ok())
            .and_then(|p| p.parent().map(|d| d.join("cyberclaw-cli")))
            .filter(|p| p.exists())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "cyberclaw-cli".to_string())
    });

    let mut cmd = CommandBuilder::new(&cli_bin);
    cmd.arg("chat");

    // ── CRITICAL: scrub environment — allow-list only ────────────────────────
    // env_clear() removes all inherited vars including JWT_SECRET,
    // CYBERCLAW_CLUSTER_SHARED_TOKEN, ANTHROPIC_API_KEY, DB credentials, etc.
    cmd.env_clear();
    for var in &["HOME", "PATH", "TERM", "LANG", "LC_ALL", "USER"] {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
    }
    // Ensure a sane terminal type even if TERM was unset.
    if std::env::var("TERM").is_err() {
        cmd.env("TERM", "xterm-256color");
    }

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            warn!(%e, cli=%cli_bin, "failed to spawn cyberclaw-cli chat");
            let _ = socket
                .send(Message::Text(format!("spawn failed: {e}\r\n")))
                .await;
            emit_audit(
                &state.audit,
                &actor,
                "pty.session_end",
                serde_json::json!({"reason": "spawn_failed", "duration_secs": session_start.elapsed().as_secs()}),
            )
            .await;
            return;
        }
    };

    debug!("PTY subprocess spawned, cli={}", cli_bin);

    // PTY master read/write handles. portable-pty returns Box<dyn …> which is
    // not Send, so we perform all PTY I/O on a dedicated blocking thread via
    // mpsc channels.
    let master_writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(e) => {
            warn!(%e, "failed to get PTY writer");
            return;
        }
    };
    let master_reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            warn!(%e, "failed to clone PTY reader");
            return;
        }
    };

    // Channel: ws → pty (bytes to write)
    let (ws_to_pty_tx, ws_to_pty_rx) = mpsc::channel::<Vec<u8>>(64);
    // Channel: pty → ws (bytes to send)
    let (pty_to_ws_tx, mut pty_to_ws_rx) = mpsc::channel::<Vec<u8>>(64);
    // Resize channel: send new PtySize to the master resize task
    let (resize_tx, mut resize_rx) = mpsc::channel::<PtySize>(8);

    // ── Blocking thread: PTY reader ──────────────────────────────────────────
    let pty_to_ws_tx2 = pty_to_ws_tx.clone();
    std::thread::spawn(move || {
        let mut reader = master_reader;
        let mut buf = [0u8; 4096];
        loop {
            use std::io::Read;
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if pty_to_ws_tx2.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // ── Blocking thread: PTY writer ──────────────────────────────────────────
    std::thread::spawn(move || {
        let mut writer = master_writer;
        let mut rx = ws_to_pty_rx;
        while let Some(bytes) = rx.blocking_recv() {
            use std::io::Write;
            if writer.write_all(&bytes).is_err() {
                break;
            }
        }
    });

    // ── Blocking thread: PTY resize ──────────────────────────────────────────
    // The master resize handle must be used from a thread because it's not Send.
    // We use a std mpsc to bridge from async.
    let (resize_std_tx, resize_std_rx) = std::sync::mpsc::channel::<PtySize>();
    std::thread::spawn(move || {
        while let Ok(size) = resize_std_rx.recv() {
            let _ = pair.master.resize(size);
        }
    });

    // Bridge async resize channel → std channel
    tokio::spawn(async move {
        while let Some(size) = resize_rx.recv().await {
            let _ = resize_std_tx.send(size);
        }
    });

    // ── Async: bridge WebSocket ⇄ channels ──────────────────────────────────
    loop {
        tokio::select! {
            // PTY output → WebSocket
            maybe_bytes = pty_to_ws_rx.recv() => {
                match maybe_bytes {
                    Some(bytes) => {
                        if socket.send(Message::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                    None => break, // PTY reader closed
                }
            }

            // WebSocket input → PTY or resize
            maybe_msg = socket.recv() => {
                match maybe_msg {
                    Some(Ok(Message::Binary(bytes))) => {
                        let _ = ws_to_pty_tx.send(bytes).await;
                    }
                    Some(Ok(Message::Text(text))) => {
                        // Try to parse as resize frame; silently ignore unknown text frames.
                        if let Ok(r) = serde_json::from_str::<ResizeFrame>(&text) {
                            if r.kind == "resize" && r.cols > 0 && r.rows > 0 {
                                let _ = resize_tx
                                    .send(PtySize {
                                        rows: r.rows,
                                        cols: r.cols,
                                        pixel_width: 0,
                                        pixel_height: 0,
                                    })
                                    .await;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // ping/pong handled by axum
                    Some(Err(_)) => break,
                }
            }
        }
    }

    // ── Cleanup: kill PTY subprocess ─────────────────────────────────────────
    debug!("PTY WebSocket closed, killing subprocess");
    let _ = child.kill();

    // ── Audit: session end ───────────────────────────────────────────────────
    emit_audit(
        &state.audit,
        &actor,
        "pty.session_end",
        serde_json::json!({"duration_secs": session_start.elapsed().as_secs()}),
    )
    .await;
    // _guard drops here → ACTIVE_PTY_COUNT decremented.
}

/// Emit a `AuditKind::Mutation` entry if the audit sink is configured.
async fn emit_audit(
    audit: &Option<Arc<AuditSink>>,
    actor: &str,
    action: &str,
    detail: serde_json::Value,
) {
    if let Some(sink) = audit.as_ref() {
        sink.record(AuditEntry::now(
            actor,
            AuditKind::Mutation,
            action,
            None,
            detail,
            AuditResult::Success,
        ))
        .await;
    }
}
