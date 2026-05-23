# Installation

完成这页后你将拥有：可运行的 `cyberclaw-server` 二进制 + `cyberclaw-cli` 二进制，以及（可选）已编译的 admin SPA。

## 环境要求

| 必需 | 工具 | 最低版本 | 说明 |
|---|---|---|---|
| ✅ | Rust + Cargo | 1.75 | edition = "2021"，workspace 编译 |
| ✅ | OpenSSL / libssl-dev | system | reqwest TLS 后端 |
| ✅ | sqlite3 | system | rusqlite + sqlx FFI |
| 🟡 | Node.js + npm | 18+ | 仅在需要 admin Web UI 时用（Babel 编译 JSX）|
| 🟡 | podman / docker | 任意 | 仅 Container 运行时需要（cmd.exec 隔离）|
| 🟡 | gpg | 任意 | 仅 audit 归档签名需要 |
| 🟡 | cargo-audit | 任意 | 仅 BT-04/25 OSV 扫描真实运行需要 |

## 1. 拉取代码

```bash
git clone https://git.nextcyber.cn/cyberclaw/cyberclaw.git
cd cyberclaw
```

## 2. 编译 Rust workspace

### 开发构建（快速）

```bash
cargo build --workspace
# → target/debug/cyberclaw-server
# → target/debug/cyberclaw-cli
```

### 发布构建（生产）

```bash
cargo build --release -p cyberclaw-server -p cyberclaw-cli
# → target/release/cyberclaw-server
# → target/release/cyberclaw-cli
```

发布构建启用 LTO 和 strip，二进制约 60-80 MB。

### 单独编译某个 crate（增量验证）

```bash
cargo build -p cyberclaw-control-plane
cargo build -p cyberclaw-connectors
```

## 3. （可选）编译 Admin Web UI

只有需要 SPA 时才执行。CLI + API 不依赖此步骤。

```bash
npm install              # 一次性安装 babel
npm run build:web        # 把 web/src/*.jsx → web/dist/*.js
```

构建产物 `web/dist/*.js` 已在 `.gitignore` 中；server 启动后会通过 `/admin/dist/:file` 路由提供这些文件。

**校验**:
```bash
ls web/dist/                              # 应看到 ~25 个 .js 文件
ls web/dist/pages_admin_ops.js            # BT-37/40/06 admin 页面
```

## 4. 验证编译产物

```bash
./target/release/cyberclaw-cli --help     # 应列出全部命令组
./target/release/cyberclaw-server --version
```

CLI 命令组（2026-05-04 版本）：

```
status / inspect / chat / task / connector / package / agent / skill /
capability / audit / review / memory / workflow / mcp / onboard
```

## 5. 跑测试套件（推荐）

```bash
cargo test --workspace --no-fail-fast
# 当前基线（2026-05-04）: 4062 passed / 0 failed / 14 ignored / 102 suites
```

## 故障排查

| 现象 | 原因 | 修复 |
|---|---|---|
| `error: linker `cc` not found` | 缺 C 工具链 | macOS: `xcode-select --install` / Linux: `apt install build-essential` |
| `failed to run custom build command for ring` | OpenSSL 缺失 | macOS: 已含 / Linux: `apt install libssl-dev pkg-config` |
| `failed to compile sqlx-sqlite` | sqlite headers 缺失 | macOS: 已含 / Linux: `apt install libsqlite3-dev` |
| `cargo audit` 命令找不到 | 仅运行 BT-04 osv_scan 需要 | `cargo install cargo-audit` |

## 下一步

- [Quickstart](quickstart.md) — 5 分钟跑起一个本地 server + CLI 调用
- [Deployment](deployment.md) — 生产部署（K8s / Podman / 环境变量 / Secret 管理）
