//! Core process-sandbox logic (synchronous).
//!
//! This file contains the machinery that:
//!
//! 1. Creates a fresh tempdir as the child's working directory.
//! 2. Builds the child [`Command`] with a scrubbed environment (allowlist
//!    only) and optional stdin.
//! 3. Spawns the child and enforces a hard timeout via a watcher thread that
//!    kills the process if it runs over.
//! 4. Captures stdout/stderr with a per-stream cap of [`MAX_OUTPUT_BYTES`]
//!    (64 KiB) so misbehaving variants cannot flood memory.
//!
//! On Linux, memory limits are enforced via `RLIMIT_AS` (virtual address space)
//! set inside a `pre_exec` hook before the child process image is loaded.
//! On macOS and non-Linux platforms, `memory_limit_mb` remains advisory because
//! `RLIMIT_AS` on macOS caps the entire virtual address space including
//! memory-mapped dylibs, making it impractical for normal process execution.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Per-stream cap on captured output (64 KiB).
///
/// Output beyond this is discarded and the response field is truncated.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Lower bound we refuse to drop below when killing on timeout.
const KILL_GRACE_MS: u64 = 50;

/// Fully resolved sandbox request passed into [`run_sandboxed`].
///
/// Higher layers (`SandboxConnector::execute`) are responsible for resolving
/// `Option<...>` request overrides against connector defaults before building
/// this struct.
#[derive(Debug, Clone)]
pub struct SandboxRequest {
    /// Program to execute.
    pub command: String,
    /// CLI arguments (passed verbatim; no shell interpretation).
    pub args: Vec<String>,
    /// Optional stdin data.
    pub stdin: Option<String>,
    /// Hard timeout. Child is killed if still alive when it expires.
    pub timeout: Duration,
    /// Virtual address space cap in MiB. `0` means no cap.
    ///
    /// On Linux this is enforced via `RLIMIT_AS` in a `pre_exec` hook.
    /// On macOS and other non-Linux platforms the value is advisory (ignored
    /// at runtime) because `RLIMIT_AS` there caps the entire VA space
    /// including dylib mappings.
    pub memory_limit_mb: u64,
    /// Additional env vars. Keys still filtered through `env_allowlist`.
    pub env: HashMap<String, String>,
    /// Allowlist of env var keys that may reach the child.
    pub env_allowlist: Vec<String>,
    /// Parent dir for the per-request tempdir; `None` = system default.
    pub tempdir_root: Option<PathBuf>,
}

/// Result of a sandbox run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxResponse {
    /// Exit status code. `None` if the process was killed by a signal (unix)
    /// or by our timeout watcher.
    pub exit_code: Option<i32>,
    /// Captured stdout, UTF-8 lossy, truncated to [`MAX_OUTPUT_BYTES`].
    pub stdout: String,
    /// Captured stderr, UTF-8 lossy, truncated to [`MAX_OUTPUT_BYTES`].
    pub stderr: String,
    /// Wall-clock elapsed time in milliseconds.
    pub elapsed_ms: u64,
    /// `true` if the process was killed because it exceeded `timeout`.
    pub timed_out: bool,
    /// `true` if the advisory memory cap was exceeded. Always `false` in v1.
    pub memory_exceeded: bool,
}

/// Run `req` in a fresh sandbox.
///
/// The caller is expected to invoke this from a blocking context (either
/// a plain thread or `tokio::task::spawn_blocking`) — it uses synchronous
/// [`std::process::Command`] and its own timeout watcher thread.
///
/// On Linux, when `req.memory_limit_mb > 0`, the child's virtual address
/// space is capped to `memory_limit_mb * 1024 * 1024` bytes via `RLIMIT_AS`
/// set in a `pre_exec` hook. The limit fires before the child process image
/// is loaded, so it is not bypassable by the child. On macOS and other
/// non-Linux platforms the field is advisory (ignored at runtime).
///
/// # Errors
///
/// Returns `Err` only for fatal setup failures (tempdir creation, process
/// spawn). A non-zero exit code, a timeout, or captured stderr is still
/// considered a successful *run* and is reported via `SandboxResponse`.
pub fn run_sandboxed(req: SandboxRequest) -> anyhow::Result<SandboxResponse> {
    // 1. Fresh isolated working directory. `TempDir` is cleaned up on drop
    //    (including on error paths via the `?` operator below).
    let mut builder = tempfile::Builder::new();
    builder.prefix("cyberclaw-sandbox-");
    let tempdir = match req.tempdir_root.as_ref() {
        Some(root) => {
            std::fs::create_dir_all(root)
                .map_err(|e| anyhow::anyhow!("failed to create tempdir root {:?}: {}", root, e))?;
            builder.tempdir_in(root)
        }
        None => builder.tempdir(),
    }
    .map_err(|e| anyhow::anyhow!("failed to create sandbox tempdir: {}", e))?;
    let workdir = tempdir.path().to_path_buf();
    debug!("sandbox workdir: {:?}", workdir);

    // 2. Build command with scrubbed env.
    let allowlist: std::collections::HashSet<String> = req.env_allowlist.iter().cloned().collect();
    let mut cmd = Command::new(&req.command);
    cmd.args(&req.args);
    cmd.current_dir(&workdir);

    // env_clear() drops *all* inherited env vars. We then re-add only those
    // whose keys are in the allowlist.
    cmd.env_clear();
    for (k, v) in std::env::vars() {
        if allowlist.contains(&k) {
            cmd.env(k, v);
        }
    }
    // Explicit request env overrides inherited allowlisted values, but keys
    // are still required to be in the allowlist — no unbounded injection.
    for (k, v) in &req.env {
        if allowlist.contains(k) {
            cmd.env(k, v);
        } else {
            debug!("scrubbed non-allowlisted env key from request: {}", k);
        }
    }

    cmd.stdin(if req.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // Enforce memory limit via RLIMIT_AS on Linux only.
    //
    // On macOS, RLIMIT_AS caps the entire virtual address space including
    // memory-mapped dylibs, making even a 512 MiB limit too tight for normal
    // processes. Linux's virtual address layout is more predictable and the
    // limit correctly bounds heap + stack without breaking dylib loading.
    //
    // The limit is set in pre_exec so it takes effect before the child process
    // image is loaded and cannot be bypassed by the child.
    #[cfg(target_os = "linux")]
    if req.memory_limit_mb > 0 {
        use std::os::unix::process::CommandExt;
        let limit_bytes: libc::rlim_t = req.memory_limit_mb * 1024 * 1024;
        let rlim = libc::rlimit {
            rlim_cur: limit_bytes,
            rlim_max: limit_bytes,
        };
        // SAFETY: pre_exec runs in the child after fork() but before exec().
        // Only async-signal-safe functions may be called here. setrlimit(2)
        // is listed as async-signal-safe on Linux (signal-safety(7)).
        unsafe {
            cmd.pre_exec(move || {
                if libc::setrlimit(libc::RLIMIT_AS, &rlim) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = req.memory_limit_mb;

    // 3. Spawn.
    let started = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn sandbox command {:?}: {}", req.command, e))?;

    // 4. Write stdin if provided.
    if let Some(stdin_data) = req.stdin.as_ref() {
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = stdin.write_all(stdin_data.as_bytes()) {
                warn!("failed to write sandbox stdin: {}", e);
            }
            // Explicit drop closes the pipe so children blocked on read() exit.
            drop(stdin);
        }
    }

    // 5. Timeout watcher. Poll a cancel flag in small slices so the main
    //    thread can dismiss us once `child.wait()` returns, preventing a
    //    post-exit spurious kill that corrupts exit_code.
    let timed_out = Arc::new(Mutex::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let timeout = req.timeout.max(Duration::from_millis(KILL_GRACE_MS));
    let watcher_handle = {
        let timed_out = Arc::clone(&timed_out);
        let done = Arc::clone(&done);
        let pid = child.id();
        thread::spawn(move || {
            let slice = Duration::from_millis(25);
            let mut remaining = timeout;
            loop {
                let step = remaining.min(slice);
                thread::sleep(step);
                if done.load(Ordering::Acquire) {
                    return;
                }
                remaining = remaining.saturating_sub(step);
                if remaining.is_zero() {
                    *timed_out.lock().unwrap() = true;
                    kill_by_pid(pid);
                    return;
                }
            }
        })
    };

    // 6. Capture stdout/stderr on dedicated threads up to MAX_OUTPUT_BYTES.
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();
    let stdout_thread =
        stdout_handle.map(|mut h| thread::spawn(move || read_capped(&mut h, MAX_OUTPUT_BYTES)));
    let stderr_thread =
        stderr_handle.map(|mut h| thread::spawn(move || read_capped(&mut h, MAX_OUTPUT_BYTES)));

    // 7. Wait for exit. If the watcher killed us, `wait()` returns immediately
    //    with the signaled status.
    let status = child
        .wait()
        .map_err(|e| anyhow::anyhow!("failed to wait for sandbox child: {}", e))?;
    let elapsed = started.elapsed();
    // Cancel the watcher so it doesn't fire a spurious kill on a reused pid
    // after our child exited normally.
    done.store(true, Ordering::Release);
    let _ = watcher_handle.join();

    let stdout = stdout_thread
        .and_then(|t| t.join().ok())
        .unwrap_or_default();
    let stderr = stderr_thread
        .and_then(|t| t.join().ok())
        .unwrap_or_default();

    let timed_out = *timed_out.lock().unwrap();
    let exit_code = if timed_out { None } else { status.code() };

    // `tempdir` is dropped here, which recursively removes the workdir. We
    // make that explicit in the log so failures are debuggable.
    let workdir_for_log = workdir.clone();
    drop(tempdir);
    debug!("sandbox tempdir cleaned up: {:?}", workdir_for_log);

    Ok(SandboxResponse {
        exit_code,
        stdout,
        stderr,
        elapsed_ms: elapsed.as_millis() as u64,
        timed_out,
        memory_exceeded: false,
    })
}

/// Read from `src` into a `String` (UTF-8 lossy), stopping after `cap` bytes.
/// Extra bytes are drained and discarded so the child doesn't block on a full
/// pipe.
fn read_capped<R: Read>(src: &mut R, cap: usize) -> String {
    let mut buf = Vec::with_capacity(cap.min(8192));
    let mut chunk = [0u8; 4096];
    let mut remaining = cap;
    loop {
        match src.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if remaining > 0 {
                    let take = n.min(remaining);
                    buf.extend_from_slice(&chunk[..take]);
                    remaining -= take;
                }
                // Keep draining until EOF even once we're full, so the child
                // doesn't block writing to a full pipe buffer.
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(unix)]
fn kill_by_pid(pid: u32) {
    // Best-effort. If the pid already reaped this just returns ESRCH.
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_by_pid(_pid: u32) {
    // On non-unix platforms we skip the explicit kill; the `Child` drop on
    // tempdir cleanup won't help (since we waited first), and the watcher
    // thread's exit will at least flip `timed_out`. Windows TerminateProcess
    // would require a handle, which we don't have here.
}

// `libc` is a direct unix-only dependency declared in Cargo.toml under
// `[target.'cfg(unix)'.dependencies]` so we can send SIGKILL to a child by
// pid from the timeout watcher thread.

#[cfg(test)]
mod tests {
    use super::*;

    fn base_request(command: &str, args: &[&str]) -> SandboxRequest {
        SandboxRequest {
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            stdin: None,
            timeout: Duration::from_secs(5),
            // 0 means no cap; existing tests do not exercise the memory limit.
            memory_limit_mb: 0,
            env: HashMap::new(),
            env_allowlist: super::super::DEFAULT_ENV_ALLOWLIST
                .iter()
                .map(|s| s.to_string())
                .collect(),
            tempdir_root: None,
        }
    }

    #[test]
    #[cfg(unix)]
    fn simple_echo_succeeds() {
        let resp = run_sandboxed(base_request("echo", &["hello"])).expect("run ok");
        assert_eq!(resp.exit_code, Some(0));
        assert_eq!(resp.stdout.trim(), "hello");
        assert!(!resp.timed_out);
    }

    #[test]
    #[cfg(unix)]
    fn nonzero_exit_captured() {
        let resp = run_sandboxed(base_request("false", &[])).expect("run ok");
        assert!(matches!(resp.exit_code, Some(code) if code != 0));
        assert!(!resp.timed_out);
    }

    #[test]
    #[cfg(unix)]
    fn timeout_kills_long_running() {
        let mut req = base_request("sleep", &["5"]);
        req.timeout = Duration::from_millis(200);
        let resp = run_sandboxed(req).expect("run ok");
        assert!(resp.timed_out, "expected timeout flag: {:?}", resp);
        // Elapsed should be close to the timeout, definitely < 5s.
        assert!(
            resp.elapsed_ms < 2_000,
            "elapsed_ms too high: {}",
            resp.elapsed_ms
        );
    }

    #[test]
    #[cfg(unix)]
    fn stdin_piped_to_child() {
        let mut req = base_request("cat", &[]);
        req.stdin = Some("test-payload".to_string());
        let resp = run_sandboxed(req).expect("run ok");
        assert_eq!(resp.exit_code, Some(0));
        assert_eq!(resp.stdout, "test-payload");
    }

    #[test]
    #[cfg(unix)]
    fn env_allowlist_filters_custom_var() {
        // Set a var NOT in the default allowlist.
        let key = "CYBERCLAW_SANDBOX_TEST_SHOULD_NOT_LEAK";
        // SAFETY of set_var: tests here run in the same process; we use an
        // unusual key that no other test touches. Accept the theoretical race.
        // SAFETY: std::env::set_var is safe for single-threaded tests that
        // don't conflict on keys. This key is unique to this test.
        unsafe {
            std::env::set_var(key, "leaked");
        }

        // Use `sh -c "echo $VAR"` — if the var leaked through, stdout will be
        // "leaked"; if it was scrubbed it will be empty.
        let resp = run_sandboxed(base_request(
            "sh",
            &["-c", &format!("printf '%s' \"${{{}:-EMPTY}}\"", key)],
        ))
        .expect("run ok");
        assert_eq!(resp.exit_code, Some(0));
        assert_eq!(
            resp.stdout, "EMPTY",
            "env var was not scrubbed: stdout={:?}",
            resp.stdout
        );

        unsafe {
            std::env::remove_var(key);
        }
    }

    #[test]
    #[cfg(unix)]
    fn tempdir_cleaned_up_after_run() {
        // Use a dedicated tempdir root so we can inspect it.
        let root = tempfile::tempdir().expect("root ok");
        let root_path = root.path().to_path_buf();

        let before: Vec<_> = std::fs::read_dir(&root_path)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(before.is_empty());

        let mut req = base_request("echo", &["hi"]);
        req.tempdir_root = Some(root_path.clone());
        let resp = run_sandboxed(req).expect("run ok");
        assert_eq!(resp.exit_code, Some(0));

        let after: Vec<_> = std::fs::read_dir(&root_path)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            after.is_empty(),
            "per-request tempdir was not cleaned up: {:?}",
            after.iter().map(|e| e.path()).collect::<Vec<_>>()
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn memory_limit_enforced_via_rlimit_as() {
        // Spawn a trivial shell command under an extremely tight memory cap
        // (1 MiB) — far below what any real process needs to load. The kernel
        // will either refuse exec() with ENOMEM or the process will be killed
        // by SIGSEGV / SIGBUS when it tries to map its own text segment.
        // Either way the child must not exit with code 0.
        //
        // We use `sh -c 'true'` rather than a bare binary so the test stays
        // portable across macOS (where `true` lives under /usr/bin and pulls
        // in dylibs) and Linux.
        let mut req = base_request("sh", &["-c", "true"]);
        req.memory_limit_mb = 1; // 1 MiB — triggers RLIMIT_AS enforcement
        req.timeout = Duration::from_secs(5);

        // The spawn itself might fail (pre_exec error propagated), or the
        // child might exit non-zero. Both outcomes prove the limit was wired.
        match run_sandboxed(req) {
            Err(_) => {
                // spawn/exec failed — limit rejected the child at exec time
            }
            Ok(resp) => {
                // Child ran but exited non-zero (killed by signal or OOM)
                assert!(
                    resp.exit_code != Some(0) || resp.timed_out,
                    "child exited 0 under 1 MiB RLIMIT_AS — limit not enforced: {:?}",
                    resp
                );
            }
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn memory_limit_zero_means_no_cap() {
        // memory_limit_mb = 0 must skip pre_exec entirely; a normal echo
        // command must still succeed.
        let mut req = base_request("echo", &["no-cap"]);
        req.memory_limit_mb = 0;
        let resp = run_sandboxed(req).expect("run ok");
        assert_eq!(resp.exit_code, Some(0));
        assert_eq!(resp.stdout.trim(), "no-cap");
    }

    #[test]
    #[cfg(unix)]
    fn output_truncated_at_cap() {
        // `yes` prints infinitely; we rely on the per-stream cap + kill on
        // timeout to bound the run. `head` would be cleaner but we want to
        // prove truncation against a flooding child.
        let mut req = base_request("sh", &["-c", "yes really-long-line"]);
        req.timeout = Duration::from_millis(500);
        let resp = run_sandboxed(req).expect("run ok");
        assert!(resp.timed_out);
        assert!(
            resp.stdout.len() <= MAX_OUTPUT_BYTES,
            "stdout exceeded cap: {} > {}",
            resp.stdout.len(),
            MAX_OUTPUT_BYTES
        );
    }
}
