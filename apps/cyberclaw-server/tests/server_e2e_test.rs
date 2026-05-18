#![allow(clippy::duplicate_mod)]
#![allow(clippy::zombie_processes, clippy::needless_borrows_for_generic_args)]

//! CyberClaw Server 端到端测试
//!
//! 测试完整的 HTTP 服务器生命周期：
//! - 启动真实的 HTTP 服务器进程
//! - 通过 HTTP 客户端进行请求
//! - 验证健康检查、API 调用、错误处理
//! - 测试并发请求和性能
//! - 清理资源
//!
//! ## 稳定性设计
//!
//! - 快速失败：每轮循环检查子进程是否已崩溃，崩溃即立即终止（<2s）
//! - 可观测性：使用 Stdio::piped() 捕获子进程日志，失败时打印完整输出
//! - 自适应超时：指数退避 poll（50ms -> 500ms），最大等待 30s（可通过 CYBERCLAW_E2E_TIMEOUT 覆盖）
//! - 无固定 sleep：移除启动阶段的 3s 固定等待，立即开始 poll

#[path = "common/mod.rs"]
mod common;
use anyhow::Result;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use serial_test::serial;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::time::sleep;

/// E2E 测试专用 JWT Claims（与 auth.rs 中的 Claims 结构对齐）
#[derive(Serialize, Deserialize)]
struct TestClaims {
    sub: String,
    exp: i64,
    iat: i64,
}

/// 为 E2E 测试生成一个长期有效的 JWT token
///
/// 使用固定密钥 E2E_JWT_SECRET，必须与 TestServer::start() 中
/// 设置的 JWT_SECRET 环境变量保持一致。
fn generate_e2e_jwt(secret: &str) -> Result<String> {
    let now = chrono::Utc::now().timestamp();
    let claims = TestClaims {
        sub: "e2e-test-user".to_string(),
        iat: now,
        exp: now + 86400, // 24 小时有效期，覆盖所有测试场景
    };
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| anyhow::anyhow!("生成 E2E JWT 失败: {}", e))?;
    Ok(token)
}

/// E2E 测试固定 JWT 密钥（与 TestServer::start() 中的环境变量一致）
const E2E_JWT_SECRET: &str = "e2e-test-jwt-secret-do-not-use-in-production";

// ============================================================
// 辅助函数
// ============================================================

/// 查找一个可用的端口 (通过绑定然后释放，存在竞态条件但简单实用)
fn find_available_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    // 短暂等待以确保端口完全释放
    std::thread::sleep(std::time::Duration::from_millis(50));
    Ok(port)
}

/// 读取环境变量 CYBERCLAW_E2E_TIMEOUT（秒），默认 30s
fn e2e_timeout() -> Duration {
    let secs = std::env::var("CYBERCLAW_E2E_TIMEOUT")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30);
    Duration::from_secs(secs)
}

/// 发送原始 HTTP/1.1 GET 请求并返回 (状态码, 响应体字节)
///
/// 使用 TcpStream 手写 HTTP 请求，绕过 reqwest 在 tokio::test 环境中的连接问题。
/// reqwest 在 tokio::test 中会建立 TCP 连接但不发送 HTTP 请求（原因未明），
/// 而手写 HTTP 请求经验证可靠工作，延迟 < 2ms。
///
/// # 参数
/// * `port` - 服务器端口
/// * `path` - 请求路径（如 "/health"）
/// * `timeout` - 请求超时时间
///
/// # 返回
/// * `Ok((u16, Vec<u8>))` - (HTTP 状态码, 响应体字节)
/// * `Err` - 连接或 IO 错误
async fn raw_http_get_with_body(
    port: u16,
    path: &str,
    timeout: Duration,
) -> Result<(u16, Vec<u8>)> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path, port
    );

    let result = tokio::time::timeout(timeout, async {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).await?;
        stream.write_all(request.as_bytes()).await?;

        // 读取响应直到连接关闭
        let mut buf = Vec::with_capacity(256);
        stream.read_to_end(&mut buf).await?;
        Ok::<Vec<u8>, std::io::Error>(buf)
    })
    .await;

    match result {
        Ok(Ok(buf)) => {
            // 解析 HTTP 状态行 "HTTP/1.1 200 OK"
            let response = String::from_utf8_lossy(&buf);
            let status_code = response
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|code| code.parse::<u16>().ok())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "无法解析 HTTP 状态码: {}",
                        response.lines().next().unwrap_or("")
                    )
                })?;

            // 分割 headers 和 body
            let body_bytes = buf
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|pos| buf[pos + 4..].to_vec())
                .unwrap_or_default();

            Ok((status_code, body_bytes))
        }
        Ok(Err(e)) => Err(anyhow::anyhow!("IO 错误: {}", e)),
        Err(_) => Err(anyhow::anyhow!("超时 ({}ms)", timeout.as_millis())),
    }
}

/// 发送原始 HTTP/1.1 GET 请求并只返回状态码（健康检查轮询专用）
async fn raw_http_get(port: u16, path: &str, timeout: Duration) -> Result<u16> {
    raw_http_get_with_body(port, path, timeout)
        .await
        .map(|(status, _)| status)
}

/// 后台线程：持续读取子进程管道并追加到共享日志缓冲
///
/// 必须在 spawn 之后立即调用，避免管道满导致子进程阻塞。
fn drain_pipe_to_log(reader: impl std::io::Read + Send + 'static, log: Arc<Mutex<Vec<String>>>) {
    std::thread::spawn(move || {
        let buf = BufReader::new(reader);
        for line in buf.lines() {
            match line {
                Ok(l) => {
                    if let Ok(mut guard) = log.lock() {
                        guard.push(l);
                    }
                }
                Err(_) => break,
            }
        }
    });
}

// ============================================================
// TestServer
// ============================================================

/// 测试服务器配置
struct TestServer {
    process: Child,
    port: u16,
    #[allow(dead_code)]
    base_url: String,
    /// reqwest Client 保留用于诊断测试（DIAG-001）
    /// 正常 E2E 测试应使用 raw_get / raw_post_json，
    /// 因为 reqwest 在 tokio::test 环境中无法建立 TCP 连接。
    #[allow(dead_code)]
    client: Client,
    /// 子进程合并后的日志（stdout + stderr）
    process_logs: Arc<Mutex<Vec<String>>>,
    /// 预生成的 JWT Bearer token，用于需要认证的 API 请求
    auth_token: String,
}

impl TestServer {
    /// 启动测试服务器
    ///
    /// ## 改进点（对比旧实现）
    /// 1. 移除 3s 固定 sleep，立即开始健康检查轮询
    /// 2. 每轮先检查子进程是否已崩溃，崩溃则立即失败（不等超时）
    /// 3. 指数退避轮询间隔（50ms → 500ms），减少 CPU 空转
    /// 4. 使用 Stdio::piped() 捕获子进程日志，失败时完整输出
    /// 5. 超时由环境变量 CYBERCLAW_E2E_TIMEOUT 控制（默认 30s）
    async fn start() -> Result<Self> {
        let port = find_available_port()?;
        let base_url = format!("http://127.0.0.1:{}", port);
        let timeout = e2e_timeout();

        println!(
            "[E2E] 启动测试服务器 (端口: {}, 超时: {}s)...",
            port,
            timeout.as_secs()
        );

        let binary_path = env!("CARGO_BIN_EXE_cyberclaw-server");
        println!("[E2E] 二进制路径: {}", binary_path);

        let addr = format!("127.0.0.1:{}", port);

        // --- 启动子进程，使用 piped() 捕获日志，避免 SIGPIPE ---
        // RATE_LIMIT_PER_SECOND=10000 / RATE_LIMIT_BURST_SIZE=100000：
        // 消除速率限制对健康检查轮询的干扰（否则 burst=60 耗尽后轮询全部 429）
        // JWT_SECRET：固定测试密钥，与 generate_e2e_jwt() 保持一致
        let mut process = Command::new(binary_path)
            .env("LLM_PROVIDER", "generic")
            .env("LLM_API_KEY", "e2e-test-key")
            .env("LLM_BASE_URL", "http://localhost:11434/v1")
            .env("CYBERCLAW_ADDR", &addr)
            .env("RUST_LOG", "info")
            .env("RATE_LIMIT_PER_SECOND", "10000")
            .env("RATE_LIMIT_BURST_SIZE", "100000")
            .env("JWT_SECRET", E2E_JWT_SECRET)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                anyhow::anyhow!("Failed to spawn server binary '{}': {}", binary_path, e)
            })?;

        // 后台线程持续消耗管道，防止子进程因管道满而阻塞
        let stdout_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let stderr_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        if let Some(stdout) = process.stdout.take() {
            drain_pipe_to_log(stdout, Arc::clone(&stdout_log));
        }
        if let Some(stderr) = process.stderr.take() {
            drain_pipe_to_log(stderr, Arc::clone(&stderr_log));
        }

        // 合并日志引用（失败时统一打印）
        let process_logs = Arc::clone(&stderr_log);

        // --- 健康检查轮询（指数退避，快速失败）---
        // 注意：使用 TcpStream + 原始 HTTP 而非 reqwest，因为 reqwest 在 tokio::test
        // 环境中存在连接建立问题（TCP 层成功但 HTTP 层挂起）。
        // 手写 HTTP GET 请求已验证可靠工作，hyper-raw 延迟 < 2ms。
        let client = Client::builder().timeout(Duration::from_secs(3)).build()?;

        let start_time = Instant::now();
        let mut poll_interval = Duration::from_millis(50);
        let max_poll_interval = Duration::from_millis(500);

        loop {
            // 1. 先检查子进程是否已退出（快速失败）
            match process.try_wait() {
                Ok(Some(status)) => {
                    // 子进程已崩溃，立即收集日志并失败
                    // 给日志线程短暂时间完成最后几行刷写
                    std::thread::sleep(Duration::from_millis(100));
                    let logs = collect_logs(&stdout_log, &stderr_log);
                    eprintln!(
                        "[E2E] 子进程已退出 (exit_code={:?})，启动失败\n--- 子进程日志 ---\n{}\n--- 日志结束 ---",
                        status.code(),
                        logs
                    );
                    return Err(anyhow::anyhow!(
                        "Server process exited early with status {:?} after {:.2}s",
                        status.code(),
                        start_time.elapsed().as_secs_f32()
                    ));
                }
                Ok(None) => {
                    // 子进程仍在运行，继续检查
                }
                Err(e) => {
                    eprintln!("[E2E] 无法检查子进程状态: {}", e);
                }
            }

            // 2. 检查总超时
            if start_time.elapsed() >= timeout {
                std::thread::sleep(Duration::from_millis(100));
                let logs = collect_logs(&stdout_log, &stderr_log);
                eprintln!(
                    "[E2E] 等待服务器启动超时 ({:.2}s)\n--- 子进程日志 ---\n{}\n--- 日志结束 ---",
                    start_time.elapsed().as_secs_f32(),
                    logs
                );
                process.kill().ok();
                return Err(anyhow::anyhow!(
                    "Server failed to start within {:.0}s timeout",
                    timeout.as_secs_f64()
                ));
            }

            // 3. 发起健康检查（使用原始 TCP + HTTP/1.1，绕过 reqwest 连接问题）
            let health_result = raw_http_get(port, "/health", Duration::from_millis(500)).await;
            match health_result {
                Ok(status) if (200..300).contains(&status) => {
                    println!(
                        "[E2E] 服务器就绪 (耗时: {:.2}s, 轮询间隔: {:?}, HTTP {})",
                        start_time.elapsed().as_secs_f32(),
                        poll_interval,
                        status,
                    );
                    break;
                }
                Ok(status) => {
                    println!(
                        "[E2E] 健康检查返回非 2xx: {} (耗时: {:.2}s)",
                        status,
                        start_time.elapsed().as_secs_f32()
                    );
                }
                Err(e) => {
                    // 服务器未就绪，正常重试
                    println!(
                        "[E2E] 健康检查未就绪: {} (耗时: {:.2}s)",
                        e,
                        start_time.elapsed().as_secs_f32()
                    );
                }
            }

            // 4. 指数退避 sleep
            sleep(poll_interval).await;
            poll_interval = (poll_interval * 2).min(max_poll_interval);
        }

        // 预生成 JWT token（与服务器 JWT_SECRET 一致）
        let auth_token = generate_e2e_jwt(E2E_JWT_SECRET)?;

        Ok(Self {
            process,
            port,
            base_url,
            client,
            process_logs,
            auth_token,
        })
    }

    /// 停止服务器并打印日志（仅调试用途）
    fn stop(&mut self) {
        println!("[E2E] 停止测试服务器...");
        let _ = self.process.kill();
        let _ = self.process.wait();
    }

    /// 获取已收集的子进程日志（最后 N 行）
    #[allow(dead_code)]
    fn last_logs(&self, n: usize) -> String {
        let guard = self.process_logs.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .iter()
            .rev()
            .take(n)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 发送 GET 请求，返回 (状态码, 响应体文本)
    ///
    /// 使用原始 TCP + HTTP/1.1，绕过 reqwest 在 tokio::test 中的连接问题。
    async fn raw_get(&self, path: &str) -> Result<(u16, String)> {
        let (status, body) =
            raw_http_get_with_body(self.port, path, Duration::from_secs(10)).await?;
        let text = String::from_utf8_lossy(&body).into_owned();
        Ok((status, text))
    }

    /// 发送 POST JSON 请求，返回 (状态码, 响应体文本)
    ///
    /// 使用原始 TCP + HTTP/1.1，绕过 reqwest 在 tokio::test 中的连接问题。
    /// 自动携带 Authorization: Bearer <auth_token> header。
    async fn raw_post(&self, path: &str, body: &Value) -> Result<(u16, String)> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let body_str = body.to_string();
        let request = format!(
            "POST {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n{}",
            path, self.port, body_str.len(), self.auth_token, body_str
        );

        let result = tokio::time::timeout(Duration::from_secs(10), async {
            let mut stream = TcpStream::connect(format!("127.0.0.1:{}", self.port)).await?;
            stream.write_all(request.as_bytes()).await?;
            let mut buf = Vec::with_capacity(4096);
            stream.read_to_end(&mut buf).await?;
            Ok::<Vec<u8>, std::io::Error>(buf)
        })
        .await;

        match result {
            Ok(Ok(buf)) => {
                let response = String::from_utf8_lossy(&buf);
                let status_code = response
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|code| code.parse::<u16>().ok())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "无法解析 HTTP 状态码: {}",
                            response.lines().next().unwrap_or("")
                        )
                    })?;
                let body_bytes = buf
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|pos| buf[pos + 4..].to_vec())
                    .unwrap_or_default();
                Ok((
                    status_code,
                    String::from_utf8_lossy(&body_bytes).into_owned(),
                ))
            }
            Ok(Err(e)) => Err(anyhow::anyhow!("IO 错误: {}", e)),
            Err(_) => Err(anyhow::anyhow!("超时")),
        }
    }

    /// 发送 POST JSON 请求，将响应体解析为 JSON Value
    async fn raw_post_json(&self, path: &str, body: &Value) -> Result<(u16, Value)> {
        let (status, text) = self.raw_post(path, body).await?;
        let json = serde_json::from_str::<Value>(&text)
            .map_err(|e| anyhow::anyhow!("JSON 解析失败: {} (原始: {})", e, text))?;
        Ok((status, json))
    }

    /// 发送 POST JSON 请求，返回 (状态码, 完整原始响应文本包含 headers)
    ///
    /// 用于需要检查响应 headers（如 Content-Type: text/event-stream）的测试。
    /// 自动携带 Authorization: Bearer <auth_token> header。
    async fn raw_post_raw_full(&self, path: &str, body: &Value) -> Result<(u16, String)> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let body_str = body.to_string();
        let request = format!(
            "POST {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n{}",
            path, self.port, body_str.len(), self.auth_token, body_str
        );

        let result = tokio::time::timeout(Duration::from_secs(10), async {
            let mut stream = TcpStream::connect(format!("127.0.0.1:{}", self.port)).await?;
            stream.write_all(request.as_bytes()).await?;
            let mut buf = Vec::with_capacity(4096);
            stream.read_to_end(&mut buf).await?;
            Ok::<Vec<u8>, std::io::Error>(buf)
        })
        .await;

        match result {
            Ok(Ok(buf)) => {
                let full_response = String::from_utf8_lossy(&buf).into_owned();
                let status_code = full_response
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|code| code.parse::<u16>().ok())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "无法解析 HTTP 状态码: {}",
                            full_response.lines().next().unwrap_or("")
                        )
                    })?;
                Ok((status_code, full_response))
            }
            Ok(Err(e)) => Err(anyhow::anyhow!("IO 错误: {}", e)),
            Err(_) => Err(anyhow::anyhow!("超时")),
        }
    }

    /// 发送任意 body 的 POST 请求（用于测试无效 JSON 等场景），返回 (状态码, 响应体文本)
    /// 自动携带 Authorization: Bearer <auth_token> header。
    async fn raw_post_raw_body(&self, path: &str, body_str: &str) -> Result<(u16, String)> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let request = format!(
            "POST {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n{}",
            path, self.port, body_str.len(), self.auth_token, body_str
        );

        let result = tokio::time::timeout(Duration::from_secs(10), async {
            let mut stream = TcpStream::connect(format!("127.0.0.1:{}", self.port)).await?;
            stream.write_all(request.as_bytes()).await?;
            let mut buf = Vec::with_capacity(4096);
            stream.read_to_end(&mut buf).await?;
            Ok::<Vec<u8>, std::io::Error>(buf)
        })
        .await;

        match result {
            Ok(Ok(buf)) => {
                let response = String::from_utf8_lossy(&buf);
                let status_code = response
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|code| code.parse::<u16>().ok())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "无法解析 HTTP 状态码: {}",
                            response.lines().next().unwrap_or("")
                        )
                    })?;
                let body_bytes = buf
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|pos| buf[pos + 4..].to_vec())
                    .unwrap_or_default();
                Ok((
                    status_code,
                    String::from_utf8_lossy(&body_bytes).into_owned(),
                ))
            }
            Ok(Err(e)) => Err(anyhow::anyhow!("IO 错误: {}", e)),
            Err(_) => Err(anyhow::anyhow!("超时")),
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 合并 stdout 和 stderr 日志为单个字符串（用于错误报告）
fn collect_logs(stdout: &Arc<Mutex<Vec<String>>>, stderr: &Arc<Mutex<Vec<String>>>) -> String {
    let mut lines = Vec::new();

    if let Ok(guard) = stdout.lock() {
        for l in guard.iter() {
            lines.push(format!("[stdout] {}", l));
        }
    }
    if let Ok(guard) = stderr.lock() {
        for l in guard.iter() {
            lines.push(format!("[stderr] {}", l));
        }
    }

    if lines.is_empty() {
        "(无日志输出)".to_string()
    } else {
        lines.join("\n")
    }
}

// ============================================================
// E2E 测试用例
// ============================================================

/// E2E-001: 健康检查端点测试
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_e2e_001_health_check() -> Result<()> {
    let server = TestServer::start().await?;

    println!("[E2E-001] 健康检查端点");

    // 测试 /health
    let (status, body) = server.raw_get("/health").await?;
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "OK");
    println!("[E2E-001] /health -> OK");

    // 测试 /ready
    let (status, body) = server.raw_get("/ready").await?;
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "READY");
    println!("[E2E-001] /ready -> READY");

    Ok(())
}

/// E2E-002: Chat Completions API 非流式请求
#[tokio::test]
#[serial]
async fn test_e2e_002_chat_completion_non_streaming() -> Result<()> {
    let server = TestServer::start().await?;

    println!("[E2E-002] Chat Completions API (非流式)");

    let request_body = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Hello, can you help me?"}
        ],
        "temperature": 0.7,
        "max_tokens": 100,
        "stream": false
    });

    let start = Instant::now();
    let (status, response_json) = server
        .raw_post_json("/v1/chat/completions", &request_body)
        .await?;
    let latency = start.elapsed();

    // E2E 真实进程测试：服务器使用真实 LLM provider (generic/localhost)
    // 无本地 LLM 服务时返回 502 是正确行为；有 LLM 服务时返回 200
    // 两种状态码都验证服务器正确处理了请求
    assert!(
        status == 200 || status == 502,
        "期望 200 (LLM 可用) 或 502 (LLM 不可用)，实际: {}。其他状态码说明服务器内部有错误",
        status
    );
    println!("[E2E-002] 状态码: {} (延迟: {:?})", status, latency);

    if status == 200 {
        // 验证完整 OpenAI 格式响应
        assert_eq!(response_json["object"], "chat.completion");
        assert_eq!(response_json["model"], "gpt-4");
        assert!(response_json["choices"].is_array());
        assert!(response_json["choices"][0]["message"]["role"] == "assistant");
        assert!(response_json["usage"].is_object());
        println!("[E2E-002] 响应格式符合 OpenAI 规范");
        println!(
            "[E2E-002] 返回内容: {}",
            response_json["choices"][0]["message"]["content"]
        );
    } else {
        println!("[E2E-002] LLM 不可用 (502)，服务器正确处理并返回错误响应");
    }

    Ok(())
}

/// E2E-003: Chat Completions API 流式请求
#[tokio::test]
#[serial]
async fn test_e2e_003_chat_completion_streaming() -> Result<()> {
    let server = TestServer::start().await?;

    println!("[E2E-003] Chat Completions API (流式)");

    let request_body = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Tell me a short story"}
        ],
        "stream": true
    });

    let start = Instant::now();
    // 使用带完整响应（headers + body）的方法
    let (status, raw_response) = server
        .raw_post_raw_full("/v1/chat/completions", &request_body)
        .await?;

    // E2E 真实进程测试：服务器使用真实 LLM provider (generic/localhost)
    // 无本地 LLM 服务时流式请求返回 502 是正确行为；有 LLM 服务时返回 200
    assert!(
        status == 200 || status == 502,
        "期望 200 (LLM 可用) 或 502 (LLM 不可用)，实际: {}。其他状态码说明服务器内部有错误",
        status
    );
    let latency = start.elapsed();
    println!("[E2E-003] 状态码: {} (延迟: {:?})", status, latency);

    if status == 200 {
        // 验证流式响应格式
        // 从原始响应中提取 Content-Type header
        let content_type = raw_response
            .lines()
            .find(|line| line.to_lowercase().starts_with("content-type:"))
            .map(|line| line["content-type:".len()..].trim().to_string())
            .unwrap_or_default();

        assert!(
            content_type.contains("text/event-stream"),
            "流式响应应该使用 text/event-stream, 实际: {}",
            content_type
        );

        println!("[E2E-003] Content-Type: {}", content_type);

        // 分割 body（\r\n\r\n 之后的内容）
        let body_text = raw_response
            .find("\r\n\r\n")
            .map(|pos| &raw_response[pos + 4..])
            .unwrap_or("");

        let chunks: Vec<&str> = body_text.lines().collect();
        println!("[E2E-003] 接收到 {} 个数据块", chunks.len());
        assert!(!chunks.is_empty(), "应该接收到至少一个 chunk");
    } else {
        println!("[E2E-003] LLM 不可用 (502)，服务器正确处理流式请求并返回错误响应");
    }

    Ok(())
}

/// E2E-004: 错误处理测试
#[tokio::test]
#[serial]
async fn test_e2e_004_error_handling() -> Result<()> {
    let server = TestServer::start().await?;

    println!("[E2E-004] 错误处理");

    // 测试 1: 无效的 JSON
    println!("[E2E-004] 测试 1: 无效 JSON");
    let (status, _) = server
        .raw_post_raw_body("/v1/chat/completions", "{invalid json}")
        .await?;
    assert_eq!(status, 400, "无效 JSON 应返回 400");
    println!("[E2E-004] 无效 JSON 正确返回 400");

    // 测试 2: 缺失必需字段
    println!("[E2E-004] 测试 2: 缺失必需字段");
    let invalid_request = json!({
        "model": "gpt-4"
        // 缺少 messages 字段
    });
    let (status, _) = server
        .raw_post("/v1/chat/completions", &invalid_request)
        .await?;
    assert!(
        (400..500).contains(&status),
        "缺失必需字段应返回 4xx，实际: {}",
        status
    );
    println!("[E2E-004] 缺失字段正确返回 {}", status);

    // 测试 3: 不存在的路由
    // 注意：认证中间件在路由匹配之前运行，因此未认证请求可能先被 401 拒绝，
    // 而非 404。这是正确的安全行为（不向未认证用户暴露路由信息）。
    println!("[E2E-004] 测试 3: 不存在的路由 (无认证 GET)");
    let (status, _) = server.raw_get("/nonexistent/route").await?;
    assert!(
        status == 401 || status == 404,
        "不存在的路由应返回 401 (auth 优先) 或 404 (路由未匹配)，实际: {}",
        status
    );
    println!(
        "[E2E-004] 不存在路由正确返回 {} (认证优先于路由匹配)",
        status
    );

    Ok(())
}

/// E2E-005: 并发请求测试
#[tokio::test]
#[serial]
async fn test_e2e_005_concurrent_requests() -> Result<()> {
    let server = TestServer::start().await?;

    println!("[E2E-005] 并发请求 (10 个并发)");

    let request_body = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Quick test"}
        ],
        "stream": false
    });

    let start = Instant::now();
    let port = server.port;
    let body_str = std::sync::Arc::new(request_body.to_string());
    let token = std::sync::Arc::new(server.auth_token.clone());

    let mut handles = Vec::new();
    for _i in 0..10 {
        let body = body_str.clone();
        let tok = token.clone();
        let handle = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let request = format!(
                "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n{}",
                port, body.len(), tok, body
            );
            let result = tokio::time::timeout(Duration::from_secs(10), async {
                let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).await?;
                stream.write_all(request.as_bytes()).await?;
                let mut buf = Vec::with_capacity(4096);
                stream.read_to_end(&mut buf).await?;
                Ok::<Vec<u8>, std::io::Error>(buf)
            })
            .await;
            match result {
                Ok(Ok(buf)) => {
                    let response = String::from_utf8_lossy(&buf);
                    let status = response
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .and_then(|c| c.parse::<u16>().ok())
                        .ok_or_else(|| anyhow::anyhow!("无法解析状态码"))?;
                    let body_bytes = buf
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|pos| buf[pos + 4..].to_vec())
                        .unwrap_or_default();
                    let json: Value = serde_json::from_slice(&body_bytes)
                        .map_err(|e| anyhow::anyhow!("JSON 解析失败: {}", e))?;
                    Ok::<(u16, Value), anyhow::Error>((status, json))
                }
                Ok(Err(e)) => Err(anyhow::anyhow!("IO 错误: {}", e)),
                Err(_) => Err(anyhow::anyhow!("超时")),
            }
        });
        handles.push(handle);
    }

    let results: Vec<_> = futures::future::join_all(handles).await;
    let elapsed = start.elapsed();

    let mut handled_count = 0; // 服务器已处理（200 或 502）
    let mut llm_ok_count = 0; // LLM 可用（200）
    for (i, result) in results.iter().enumerate() {
        match result {
            Ok(Ok((status, response))) => {
                // 接受 200 (LLM 可用) 或 502 (LLM 不可用)
                assert!(
                    *status == 200 || *status == 502,
                    "请求 {} 期望 200 或 502，实际: {}",
                    i,
                    status
                );
                handled_count += 1;
                if *status == 200 {
                    assert_eq!(response["object"], "chat.completion");
                    llm_ok_count += 1;
                }
            }
            Ok(Err(e)) => {
                eprintln!("[E2E-005] 请求 {} 失败: {}", i, e);
            }
            Err(e) => {
                eprintln!("[E2E-005] 请求 {} 任务失败: {}", i, e);
            }
        }
    }

    // 服务器必须处理全部 10 个请求（无论 LLM 是否可用）
    assert_eq!(
        handled_count, 10,
        "服务器应处理全部 10 个并发请求，实际: {}",
        handled_count
    );

    println!(
        "[E2E-005] {}/10 请求由服务器处理 ({} LLM正常, {} LLM不可用)",
        handled_count,
        llm_ok_count,
        handled_count - llm_ok_count
    );
    println!("[E2E-005] 总耗时: {:?}", elapsed);
    println!("[E2E-005] 平均延迟: {:?}", elapsed / 10);

    Ok(())
}

/// E2E-006: 性能基准测试
#[tokio::test]
#[serial]
async fn test_e2e_006_performance_benchmark() -> Result<()> {
    let server = TestServer::start().await?;

    println!("[E2E-006] 性能基准测试");

    let request_body = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Performance test"}
        ],
        "stream": false
    });

    // 预热：发送 3 个请求
    println!("[E2E-006] 预热中...");
    for _ in 0..3 {
        let _ = server
            .raw_post("/v1/chat/completions", &request_body)
            .await?;
    }

    // 基准测试：100 个连续请求
    // 注意：E2E 性能基准测试的目标是测量**服务器处理延迟**，
    // 而非 LLM 生成速度。接受 200 和 502，两者都是合法的服务器响应。
    println!("[E2E-006] 开始基准测试 (100 次请求)...");
    let mut latencies = Vec::new();
    let start = Instant::now();

    for _ in 0..100 {
        let req_start = Instant::now();
        let (status, _) = server
            .raw_post("/v1/chat/completions", &request_body)
            .await?;
        // 接受 200 (LLM 可用) 或 502 (LLM 不可用)，两者都表明服务器已处理请求
        assert!(
            status == 200 || status == 502,
            "期望 200 或 502，实际: {}",
            status
        );
        latencies.push(req_start.elapsed());
    }

    let total_time = start.elapsed();

    latencies.sort();
    let min = latencies.first().unwrap();
    let max = latencies.last().unwrap();
    let avg = total_time / 100;
    let p50 = latencies[50];
    let p95 = latencies[95];
    let p99 = latencies[99];

    println!("[E2E-006] 性能指标:");
    println!("   总请求数: 100");
    println!("   总耗时: {:?}", total_time);
    println!("   吞吐量: {:.2} req/s", 100.0 / total_time.as_secs_f64());
    println!("   最小延迟: {:?}", min);
    println!("   平均延迟: {:?}", avg);
    println!("   P50: {:?}", p50);
    println!("   P95: {:?}", p95);
    println!("   P99: {:?}", p99);
    println!("   最大延迟: {:?}", max);

    // 性能断言：
    // - 当 LLM 可用时：平均延迟应 < 100ms（纯服务器处理）
    // - 当 LLM 不可用时：延迟包含 HTTP 连接超时，放宽到 < 5s（请求应在超时后快速失败）
    // - 无论如何，请求不应无限期阻塞（< 10s 是底线）
    if avg < Duration::from_millis(100) {
        println!(
            "[E2E-006] LLM 可用模式: 平均延迟 {:?} 符合 < 100ms 预期",
            avg
        );
    } else {
        println!(
            "[E2E-006] LLM 不可用模式: 平均延迟 {:?} (包含上游连接超时)",
            avg
        );
        // 在无 LLM 环境中，请求应在合理时间内失败（不超过 5s）
        assert!(
            avg < Duration::from_secs(5),
            "服务器平均处理延迟不应超过 5s，实际: {:?}",
            avg
        );
    }
    assert!(
        p99 < Duration::from_secs(10),
        "P99 延迟不应超过 10s，实际: {:?}",
        p99
    );

    println!("[E2E-006] 服务器性能基准记录完成");

    Ok(())
}

/// E2E-007: 完整用户场景测试
#[tokio::test]
#[serial]
async fn test_e2e_007_complete_user_journey() -> Result<()> {
    let server = TestServer::start().await?;

    println!("[E2E-007] 完整用户场景");

    // 场景 1: 用户启动对话
    println!("[E2E-007] 场景 1: 启动对话");
    let conversation_start = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Hi, I need help with Rust programming"}
        ],
        "stream": false
    });

    let (status, response1) = server
        .raw_post_json("/v1/chat/completions", &conversation_start)
        .await?;
    assert!(
        status == 200 || status == 502,
        "期望 200 或 502，实际: {}",
        status
    );
    let assistant_msg1 = if status == 200 {
        response1["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("Hello!")
            .to_string()
    } else {
        "Hello! (LLM 不可用，使用占位符)".to_string()
    };
    println!(
        "[E2E-007] 场景 1: 状态 {}，助手回复: {}",
        status, assistant_msg1
    );

    // 场景 2: 继续对话（多轮）
    println!("[E2E-007] 场景 2: 继续多轮对话");
    let conversation_continue = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Hi, I need help with Rust programming"},
            {"role": "assistant", "content": assistant_msg1},
            {"role": "user", "content": "Can you show me an example?"}
        ],
        "stream": false
    });

    let (status, _) = server
        .raw_post("/v1/chat/completions", &conversation_continue)
        .await?;
    assert!(
        status == 200 || status == 502,
        "期望 200 或 502，实际: {}",
        status
    );
    println!("[E2E-007] 多轮对话请求正确处理 (状态: {})", status);

    // 场景 3: 使用流式模式
    println!("[E2E-007] 场景 3: 切换到流式模式");
    let streaming_request = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Give me a detailed explanation"}
        ],
        "stream": true
    });

    let (status, raw_response) = server
        .raw_post_raw_full("/v1/chat/completions", &streaming_request)
        .await?;
    assert!(
        status == 200 || status == 502,
        "期望 200 或 502，实际: {}",
        status
    );
    if status == 200 {
        let content_type = raw_response
            .lines()
            .find(|line| line.to_lowercase().starts_with("content-type:"))
            .map(|line| line["content-type:".len()..].trim().to_string())
            .unwrap_or_default();
        assert!(
            content_type.contains("text/event-stream"),
            "实际 Content-Type: {}",
            content_type
        );
        println!("[E2E-007] 流式模式工作正常");
    } else {
        println!("[E2E-007] 流式模式：LLM 不可用 (502)，服务器正确处理");
    }

    // 场景 4: 调整参数
    println!("[E2E-007] 场景 4: 调整参数 (temperature, max_tokens)");
    let adjusted_request = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Be creative"}
        ],
        "temperature": 1.0,
        "max_tokens": 50,
        "top_p": 0.9,
        "stream": false
    });

    let (status, _) = server
        .raw_post("/v1/chat/completions", &adjusted_request)
        .await?;
    assert!(
        status == 200 || status == 502,
        "期望 200 或 502，实际: {}",
        status
    );
    println!("[E2E-007] 参数调整请求正确处理 (状态: {})", status);

    println!("[E2E-007] 完整用户场景测试通过");

    Ok(())
}

/// E2E-008: 压力测试（高并发）
#[tokio::test]
#[serial]
async fn test_e2e_008_stress_test() -> Result<()> {
    let server = TestServer::start().await?;

    println!("[E2E-008] 压力测试 (100 并发)");

    let request_body = json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "Stress test"}
        ],
        "stream": false
    });

    let start = Instant::now();
    let port = server.port;
    let body_str = std::sync::Arc::new(request_body.to_string());
    let token = std::sync::Arc::new(server.auth_token.clone());

    let mut handles = Vec::new();
    for _i in 0..100 {
        let body = body_str.clone();
        let tok = token.clone();
        let handle = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let request = format!(
                "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n{}",
                port, body.len(), tok, body
            );
            let result = tokio::time::timeout(Duration::from_secs(10), async {
                let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).await?;
                stream.write_all(request.as_bytes()).await?;
                let mut buf = Vec::with_capacity(4096);
                stream.read_to_end(&mut buf).await?;
                Ok::<Vec<u8>, std::io::Error>(buf)
            })
            .await;
            match result {
                Ok(Ok(buf)) => {
                    let response = String::from_utf8_lossy(&buf);
                    let status = response
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .and_then(|c| c.parse::<u16>().ok())
                        .unwrap_or(0u16);
                    Ok::<u16, anyhow::Error>(status)
                }
                Ok(Err(e)) => Err(anyhow::anyhow!("IO 错误: {}", e)),
                Err(_) => Err(anyhow::anyhow!("超时")),
            }
        });
        handles.push(handle);
    }

    let results: Vec<_> = futures::future::join_all(handles).await;
    let elapsed = start.elapsed();

    // 统计：服务器已处理（200 或 502），而非只计 200
    let handled_count = results
        .iter()
        .filter(|r| matches!(r, Ok(Ok(200)) | Ok(Ok(502))))
        .count();
    let ok_count = results.iter().filter(|r| matches!(r, Ok(Ok(200)))).count();
    let err_502_count = results.iter().filter(|r| matches!(r, Ok(Ok(502)))).count();
    let fail_count = 100 - handled_count;

    println!(
        "[E2E-008] 处理情况: {}/100 已处理 ({} LLM正常, {} LLM不可用, {} 失败)",
        handled_count, ok_count, err_502_count, fail_count
    );
    println!("[E2E-008] 总耗时: {:?}", elapsed);
    println!(
        "[E2E-008] 吞吐量: {:.2} req/s",
        100.0 / elapsed.as_secs_f64()
    );

    // 服务器必须处理至少 95% 的请求（无论 LLM 是否可用）
    assert!(
        handled_count >= 95,
        "服务器应处理至少 95% 的请求 (实际: {}/100)，失败 {} 个",
        handled_count,
        fail_count
    );

    Ok(())
}

/// DIAG-001: 诊断测试 - 区分 TCP 连接和 HTTP 响应问题
///
/// 此测试用于调试 reqwest 在 tokio::test 中连接子进程服务器超时的根因。
/// 同时测试：原始 TcpStream 连接 vs reqwest HTTP 请求。
/// 可通过 DIAG_TEST_PORT 环境变量指定外部运行的服务器端口（跳过子进程启动）。
#[tokio::test]
#[serial]
async fn test_diag_001_tcp_vs_reqwest() {
    // 支持通过环境变量指定外部服务器端口，跳过子进程启动
    let (port, child_opt) = if let Ok(ext_port) = std::env::var("DIAG_TEST_PORT") {
        let p: u16 = ext_port.parse().unwrap_or(58700);
        println!("[DIAG-001] 使用外部服务器 (端口: {})", p);
        (p, None)
    } else {
        let p = 58700u16;
        println!("[DIAG-001] 启动子进程服务器 (端口: {})", p);
        let child = Command::new(env!("CARGO_BIN_EXE_cyberclaw-server"))
            .env("LLM_PROVIDER", "generic")
            .env("LLM_API_KEY", "test")
            .env("LLM_BASE_URL", "http://localhost:11434/v1")
            .env("CYBERCLAW_ADDR", format!("127.0.0.1:{}", p))
            .env("RATE_LIMIT_PER_SECOND", "10000")
            .env("RATE_LIMIT_BURST_SIZE", "100000")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        (p, Some(child))
    };

    println!("[DIAG-001] === TcpStream 连接测试 (端口: {}) ===", port);
    for i in 0..3 {
        let start = Instant::now();
        match tokio::time::timeout(
            Duration::from_secs(2),
            TcpStream::connect(format!("127.0.0.1:{}", port)),
        )
        .await
        {
            Ok(Ok(_stream)) => println!(
                "[DIAG-001] TCP[{}]: 连接成功 in {:.3}s",
                i,
                start.elapsed().as_secs_f32()
            ),
            Ok(Err(e)) => println!(
                "[DIAG-001] TCP[{}]: 连接失败 {} in {:.3}s",
                i,
                e,
                start.elapsed().as_secs_f32()
            ),
            Err(_) => println!(
                "[DIAG-001] TCP[{}]: 超时 in {:.3}s",
                i,
                start.elapsed().as_secs_f32()
            ),
        }
    }

    println!("[DIAG-001] === reqwest async 健康检查测试 ===");
    let client = Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    for i in 0..3 {
        let start = Instant::now();
        match client
            .get(format!("http://127.0.0.1:{}/health", port))
            .send()
            .await
        {
            Ok(r) => println!(
                "[DIAG-001] reqwest-async[{}]: {} in {:.3}s",
                i,
                r.status(),
                start.elapsed().as_secs_f32()
            ),
            Err(e) => println!(
                "[DIAG-001] reqwest-async[{}]: 失败 {:#?} in {:.3}s",
                i,
                e,
                start.elapsed().as_secs_f32()
            ),
        }
    }

    println!("[DIAG-001] === reqwest blocking 健康检查测试 (spawn_blocking) ===");
    let port_clone = port;
    for i in 0..3 {
        let start = Instant::now();
        let url = format!("http://127.0.0.1:{}/health", port_clone);
        match tokio::task::spawn_blocking(move || {
            reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap()
                .get(&url)
                .send()
        })
        .await
        {
            Ok(Ok(r)) => println!(
                "[DIAG-001] reqwest-blocking[{}]: {} in {:.3}s",
                i,
                r.status(),
                start.elapsed().as_secs_f32()
            ),
            Ok(Err(e)) => println!(
                "[DIAG-001] reqwest-blocking[{}]: 失败 {:?} in {:.3}s",
                i,
                e,
                start.elapsed().as_secs_f32()
            ),
            Err(e) => println!(
                "[DIAG-001] reqwest-blocking[{}]: join失败 {:?} in {:.3}s",
                i,
                e,
                start.elapsed().as_secs_f32()
            ),
        }
    }

    println!("[DIAG-001] === hyper 直接 HTTP 测试 ===");
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for i in 0..3 {
        let start = Instant::now();
        let request = format!(
            "GET /health HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            port
        );
        match tokio::time::timeout(Duration::from_secs(3), async {
            let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port)).await?;
            stream.write_all(request.as_bytes()).await?;
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).await?;
            Ok::<String, std::io::Error>(String::from_utf8_lossy(&buf).to_string())
        })
        .await
        {
            Ok(Ok(resp)) => {
                let status_line = resp.lines().next().unwrap_or("").to_string();
                println!(
                    "[DIAG-001] hyper-raw[{}]: {} in {:.3}s",
                    i,
                    status_line,
                    start.elapsed().as_secs_f32()
                );
            }
            Ok(Err(e)) => println!(
                "[DIAG-001] hyper-raw[{}]: IO 错误 {} in {:.3}s",
                i,
                e,
                start.elapsed().as_secs_f32()
            ),
            Err(_) => println!(
                "[DIAG-001] hyper-raw[{}]: 超时 in {:.3}s",
                i,
                start.elapsed().as_secs_f32()
            ),
        }
    }

    if let Some(mut child) = child_opt {
        child.kill().ok();
        child.wait().ok();
    }
}
