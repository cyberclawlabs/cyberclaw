# PTY → CLI Binary Resolution

- Status: Active
- Scope: Deployment
- Last Updated: 2026-05-15
- Audience: Operators / deployment engineers
- Related code: `apps/cyberclaw-server/src/api/pty.rs`

WebUI PTY 终端在浏览器里打开一个交互式 shell，背后由 `cyberclaw-server`
fork 出一个 `cyberclaw-cli chat` 子进程。**子进程怎么找到 cyberclaw-cli
binary** 是一个有 CVE 风险的部署细节——本文档说明默认行为、绕过方式、
以及生产部署应该怎么配置。

## 解析链（按优先级）

`pty.rs` 解析 `cli_bin` 的顺序：

1. **`$CYBERCLAW_CLI_BIN` 环境变量**（绝对路径，最高优先级）
2. **server binary 同目录下的 `cyberclaw-cli`**（生产路径）
3. **`PATH` 里的 `cyberclaw-cli`**（开发 fallback）

第 2 步**先调用 `std::path::Path::canonicalize()`**——解开所有 symlink，
再 `parent().join("cyberclaw-cli")`——这是关键的安全防御。详见下节。

## Symlink 边界（CVE 防御）

### 不带 canonicalize 的攻击面

假设 server 安装在：

```
/usr/local/bin/cyberclaw-server  →  /opt/cyberclaw-1.0/bin/cyberclaw-server  (symlink)
/opt/cyberclaw-1.0/bin/cyberclaw-cli  (真实 binary)
```

如果**不解析 symlink**：

```
current_exe()       → /usr/local/bin/cyberclaw-server   (link path)
parent()            → /usr/local/bin/
join("cyberclaw-cli") → /usr/local/bin/cyberclaw-cli    ← 错！
```

攻击者只要能在 `/usr/local/bin/` 写文件，就能植入恶意 `cyberclaw-cli`，
PTY 启动时就会以 server 进程权限执行它。

### 带 canonicalize 的正确行为

```rust
std::env::current_exe()        // /usr/local/bin/cyberclaw-server (link)
    .ok()
    .and_then(|p| p.canonicalize().ok())  // → /opt/cyberclaw-1.0/bin/cyberclaw-server
    .and_then(|p| p.parent().map(|d| d.join("cyberclaw-cli")))
    .filter(|p| p.exists())    // /opt/cyberclaw-1.0/bin/cyberclaw-cli ✓
```

**canonicalize 强制走 symlink 真实目标的目录**，让 sibling 查找在
"server 真正放在哪里" 的目录下找——不是它被链接到的目录。

### 生产部署建议

1. **首选**: 把 server + cli 都装到同一个真实目录（如
   `/opt/cyberclaw/bin/`），symlink 进 `$PATH`（如 `/usr/local/bin/`）。
2. **次选**: 直接 `$PATH` 暴露真实目录，跳过 symlink。
3. **强约束**: 显式设置 `$CYBERCLAW_CLI_BIN=/绝对/路径/cyberclaw-cli`，
   绕过所有解析逻辑。这是最强保证。

### 容器部署

容器里 server + cli 通常装在同一个 image，`current_exe()` 返回的就是
真实路径，**没有 symlink 攻击面**。但仍建议设置
`CYBERCLAW_CLI_BIN=/usr/local/bin/cyberclaw-cli`，便于审计与排错。

## PTY 子进程的 env scrubbing

PTY spawn 调用 `cmd.env_clear()`，然后**只允许** 6 个 env var 透传给
子进程：

| Var | Why |
|---|---|
| `HOME` | shell init 找 .bashrc / .zshrc |
| `PATH` | 子进程要 `cargo` / `git` / `python3` 等 |
| `TERM` | ratatui 终端识别 |
| `LANG`, `LC_ALL` | 中文 / UTF-8 |
| `USER` | 不少 tool 期望存在 |

**没有**透传：

- ❌ `JWT_SECRET` / `CYBERCLAW_CLUSTER_SHARED_TOKEN` — server secret 不暴露给 CLI
- ❌ `LLM_API_KEY` — CLI 通过 HTTP 拿 token，不靠 env
- ❌ DB credentials / TLS 私钥路径 — 全部 scrub

CLI 自己读 `~/.cyberclaw/cli-token`（用户主目录）做认证；server 重启
导致 JWT_SECRET 漂移时，CLI 收到 401 会显示 actionable hint（commit
`b40c602`）。

## 排错

| 症状 | 检查 |
|---|---|
| PTY 一连上就 `[Session ended]` | tail server log 看 `failed to spawn cyberclaw-cli`，多半是 binary 找不到。设置 `CYBERCLAW_CLI_BIN`。 |
| CLI 启动但秒挂 | 子进程 stderr 看 panic。常见：缺 `$HOME`、`$TERM=dumb` |
| CLI 401 | server JWT_SECRET 跟 `~/.cyberclaw/cli-token` 签名不匹配。让用户重新走 `cyberclaw chat` 拿新 token，或 `CYBERCLAW_TOKEN=<jwt>` 临时覆盖。 |

## 相关历史

- v1.0.0 GA — initial PTY support（一连上就 [Session ended]，因为
  没有 sibling 查找，PATH 缺 cli）
- commit `fb1d268` — 加 sibling 查找 + canonicalize
- commit `b40c602` — CLI 401 友好错误（http_client.rs::explain_status）
