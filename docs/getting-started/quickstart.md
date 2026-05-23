# Quickstart

5 分钟在本机跑起一个完整的 cyberclaw-server，并用 CLI 完成一次 chat + workflow chain。

## 前置条件

- 已完成 [Installation](installation.md)
- 已编译 `target/release/cyberclaw-server` 和 `target/release/cyberclaw-cli`

## 1. 准备最小 runtime 配置

CyberClaw 启动需要 3 个东西：

1. JWT secret（必须，至少 32 字符）
2. workspace 目录（首次启动会自动创建）
3. 一个 admin 用户（写在 `users.toml`）

```bash
# 1.1 生成 JWT secret
export JWT_SECRET=$(openssl rand -hex 32)

# 1.2 创建工作目录 + 用户
mkdir -p ~/.cyberclaw/workspace
PASSWORD_HASH=$(./target/release/cyberclaw-cli onboard --hash-password 'admin123')
cat > ~/.cyberclaw/users.toml <<EOF
[[users]]
user_id = "admin"
password_hash = "$PASSWORD_HASH"
role = "admin"
EOF

# 1.3 （可选）配置 LLM
export OPENAI_API_KEY=sk-...                # 走 OpenAI 兼容端点
# 或者用 MiniMax / 本地模型，参考 docs/architecture/runtime/

# 1.4 （可选）启用真实 Web 搜索（关闭 BT-06 🟡）
export WEB_SEARCH_PROVIDER=exa
export EXA_API_KEY=<your-exa-api-key>
```

## 2. 启动 server

```bash
./target/release/cyberclaw-server &
# 默认监听 127.0.0.1:38090
```

观察日志中应该看到：

```
INFO cyberclaw_server: starting on 127.0.0.1:38090
INFO cyberclaw_server: workspace = /Users/<you>/.cyberclaw/workspace
INFO cyberclaw_server: ecosystem = ./ecosystem
```

## 3. 用 CLI 登录 + 测试

CLI 通过 `~/.cyberclaw/cli-token` 持久化 JWT。第一次调用会询问凭证：

```bash
# 触发登录流程（会提示输入 user_id + password）
./target/release/cyberclaw-cli chat
# user_id: admin
# password: admin123
# Token 已保存到 ~/.cyberclaw/cli-token

# 之后所有命令直接复用 token
./target/release/cyberclaw-cli status
```

## 4. 跑一次端到端

### 4.1 列出已注册技能

```bash
cyberclaw skill list
```

### 4.2 写一条 memory（含 BT-09 tag）

```bash
cyberclaw memory set --agent-id default \
  --level L1 \
  --tag performance --tag rust \
  "Tokio runtime benchmark: 1M concurrent tasks @ 256 KB stack"
```

### 4.3 按 tag 检索

```bash
cyberclaw memory search "tokio" --tag performance
```

### 4.4 创建一个链式工作流（BT-40）

```bash
cyberclaw workflow chain --name nightly \
  --task gen:Generate-code \
  --task test:Run-tests \
  --task report:Send-report
# ✓ Chain registered: workflow_id=wf-chain-..., 3 task(s): gen → test → report
```

### 4.5 注册一个 MCP server（BT-37 热加载）

```bash
# 先在另一个 terminal 起一个 reference MCP server
npx -y @modelcontextprotocol/server-filesystem /tmp &

cyberclaw mcp register --name fs-tmp --url http://localhost:3000/mcp
cyberclaw mcp list
cyberclaw mcp unregister fs-tmp
```

### 4.6 OSV 依赖扫描（BT-04）

```bash
# 通过 capability 直接调用（需要 cargo-audit 已安装）
cyberclaw capability call security.osv_scan \
  --input '{"lockfile_path": "Cargo.lock"}'
```

### 4.7 审批一个待处理的 review

```bash
cyberclaw review list                          # 列出 pending
cyberclaw review approve rev_<uuid> --comment "ok"
```

## 5. （可选）打开 Web UI

如果你已经跑过 `npm run build:web`：

```
浏览器访问  http://127.0.0.1:38090/admin
登录: admin / admin123
左侧导航 → "Admin Ops" → 三个子 tab：MCP Servers / Workflow Chain / Search Provider
```

## 故障排查

| 现象 | 修复 |
|---|---|
| server panic `JWT_SECRET environment variable must be set` | `export JWT_SECRET=$(openssl rand -hex 32)` 后再启动 |
| CLI 报 `auth expired` | 删 `~/.cyberclaw/cli-token` 重新登录 |
| `mcp register` 报 `failed to construct MCP connector` | 真实 MCP server 没起，或 URL 错 |
| `workflow chain` 报 `chain must contain at least one task` | 至少要传一个 `--task id:Name` |
| Web UI 加载白屏 | `web/dist/` 缺失，跑 `npm run build:web` |

## 下一步

- [Deployment](deployment.md) — 把单机版搬到 K8s / Podman 生产环境
- [Reference / API](../reference/README.md) — 全部 HTTP 端点 + 工具签名
