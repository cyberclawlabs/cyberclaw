// 把 git commit + 编译时间注入为 compile-time env，让 GET /api/v1/settings/about
// 的 footer "v0.1.0 · unknown" 变成 "v0.1.0 · abc1234567ab"。
//
// 失败回退：commit 取不到 → "nogit"；build_time 始终能取（系统时间）。

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let commit = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "nogit".to_string());
    println!("cargo:rustc-env=CYBERCLAW_COMMIT={}", commit);

    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=CYBERCLAW_BUILD_TIME={}", secs);

    // .git/HEAD 变化（切分支 / 新 commit）时重跑 build script，让 commit hash 跟上。
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs");
}
