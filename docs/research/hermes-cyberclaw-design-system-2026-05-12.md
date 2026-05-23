# Hermes ↔ CyberClaw 设计 system 对照报告

**Date**: 2026-05-12
**Author**: analyst (Opus, read-only)
**Read-Through**: ~400 files across both codebases
**Status**: 5 critical questions need user resolution before execution

---

## 0. 5 个执行前必答问题（user 拍板）

这些问题不答，下面所有方案无法落地。各 2-3 句够。

### Q1. WebUI 着陆页：sessions（hermes 风）or status dashboard（当前 cyberclaw）？
- Sessions-first: hermes 默认 `/sessions`（[App.tsx:81](/Users/max/project/cyberclaw/claw-research/hermes-agent/web/src/App.tsx#L81)）。用户进来直奔"我的对话"
- Status-first: cyberclaw 当前 `/`（Dashboard）。用户进来看系统大盘
- 这决定整个 IA 结构

### Q2. WebUI Chat 传输：xterm PTY (hermes) or WebSocket/SSE 流式 (现状)？
- hermes ChatPage 是嵌入 xterm.js + PTY 子进程（[ChatPage.tsx 872 lines](/Users/max/project/cyberclaw/claw-research/hermes-agent/web/src/pages/ChatPage.tsx)）= 完整 shell 体验
- cyberclaw 现在是 React + SSE 渲染 markdown
- 这决定 ChatPage 整个架构（也决定 server 是否需要 PTY endpoint）

### Q3. TUI 框架：保持 ratatui (Rust) or 切 Ink (Node.js, hermes 同款)？
- hermes TUI 是 React+Ink，~3000 LOC，能 markdown 渲染/虚拟滚动/sub-agent 树
- cyberclaw TUI 是 ratatui，592 LOC，基础 send/receive
- ratatui 保留 Rust 优势 + 启动快；Ink 迭代速度 + 视觉更丰富
- 6 个月的 TUI 开发速度差

### Q4. Plugin slot 系统是 v2 初版还是后续 milestone？
- hermes 有 28 个 slot + manifest schema + SRI integrity（[slots.ts](/Users/max/project/cyberclaw/claw-research/hermes-agent/web/src/plugins/slots.ts)）
- cyberclaw 有 PlatformPlugin **概念**但没 web plugin runtime
- 6+ files 的 plugin 基础设施，是否在初版就上

### Q5. 哪些 cyberclaw 现有页面合并/砍掉？
- cyberclaw v2 现在 30 页 + hermes 想借的 12 页 = 42 页，不可持续
- 候选合并：
  - Capabilities + CapabilityMonitor + Tools → 一页 "Capabilities & Tools"
  - Agents + Handoffs → "Agent Operations"
  - Tasks + Executions + Kanban → "Work Queue"
- 目标降到 ~18 页

---

## 1. 设计哲学差异

| 维度 | hermes | cyberclaw |
|---|---|---|
| 用户类型 | 单 admin 模式 + 多平台 IM 用户共用一个 admin console | 单 admin operator + 受治理执行链 |
| 核心对象 | Session / Skill / Plugin / Profile (SOUL) / Model | Agent / Skill / Connector / Capability / PlatformPlugin |
| 一等公民选择 | **Session** 是中心（landing page、所有数据围绕它） | **Agent** + 治理执行链是中心 |
| 治理模型 | 无显式审批链，靠 platform 本身 ACL | 明确 Review/Approval/Audit 三链 + Risk levels + PolicyEngine |
| TUI 角色 | 主要交互入口（hermes TUI 是产品形态） | 辅助工具（admin console 是主入口）|
| 配置存储 | `~/.hermes/` 多 SQLite + YAML | `~/.cyberclaw/*.toml` + `.env` |
| 后端语言 | Python (FastAPI + tortoise-orm) | Rust (axum + custom stores) |

## 2. hermes 可借鉴（cyberclaw 该学）

按"能力"列，每条标 P0 / P1 / P2：

### 2.1 Session-centric IA + 数据模型 **【P0】**
- **能力**：Sessions tab 是中心，所有 chat/job/log/cost 数据围绕 session_id 聚合
- **hermes 实现**：`gateway/session.py:1249-1301` SessionDB (SQLite + JSONL dual-write)；`SessionStore` 统一 CRUD
- **cyberclaw 现状**：有 `chat_conversations.rs` 但只存 chat messages，job/log/cost 是分离表
- **借鉴方式**：扩 cyberclaw 的 conversation_id 为通用 session_id，统一聚合维度
- **风险**：动核心数据模型，影响多个 crate

### 2.2 PageHeaderProvider + usePageHeader Hook **【P0】**
- **能力**：所有页面用 React Context 统一 header 结构（title + subtitle + actions + tabs）
- **hermes 实现**：`web/src/contexts/PageHeaderProvider.tsx` 107 lines
- **cyberclaw 现状**：每页自己写 `<header>`，无统一规范，视觉发散
- **借鉴方式**：直接 port，30 页全用
- **风险**：低

### 2.3 useI18n Hook + 强类型翻译 **【P0】**
- **能力**：`t.sessions.title` 类型安全的翻译
- **hermes 实现**：`web/src/i18n/types.ts` + `useI18n.ts`，en/zh 全覆盖
- **cyberclaw 现状**：v2 有 `lib/i18n.ts` 但只覆盖 16 key
- **借鉴方式**：扩到 15 page 全翻译；不强求 30 页全 i18n
- **风险**：低

### 2.4 Models page 实时 token 用量 + 成本估算 **【P1】**
- **能力**：每模型显示 24h tokens/cost/sessions count
- **hermes 实现**：`web/src/pages/ModelsPage.tsx` 818 lines；`gateway/session.py:444-450` SessionEntry 字段
- **cyberclaw 现状**：`/api/v1/usage` 已有，ModelsPage 已显，但**没有 cost_status="estimated/exact/unknown"** 三态
- **借鉴方式**：补 3 态 + per-provider 显式 disclaimer
- **风险**：低

### 2.5 SKILL.md + scripts/ + references/ + assets/ 完整 skill 包形态 **【P1】**
- **能力**：skill 不只 manifest，是完整 directory tree + 可执行 scripts + 文档 + 资产
- **hermes 实现**：`optional-skills/` 87 个 bundled skill
- **cyberclaw 现状**：`SkillHub` 有 quarantine/install/scan 生命周期，但 manifest 字段没完整对齐
- **借鉴方式**：跟齐 hermes manifest schema 字段，optional-skills 87 个可直接 import 加测
- **风险**：medium（schema 演进）

### 2.6 Plugin slot 注入点系统 **【P1，但 Q4 决定时机】**
- **能力**：plugin 通过 manifest 声明往哪个 slot 注入组件，无需改主体代码
- **hermes 实现**：`web/src/plugins/` 6 文件 + 28 slot 名（`slots.ts:60-93`）+ SRI integrity check
- **cyberclaw 现状**：有 PlatformPlugin 概念但 web plugin runtime 0
- **借鉴方式**：port slot registry + SDK exposure；slot 名按 cyberclaw IA 重新定义
- **风险**：medium（运行时基础设施大）

### 2.7 Logs SSE 实时流 ✅ **已做**
cyberclaw 已实现（commit 353576e），但 hermes 还多 1 个细节：
- **scrollRef auto-scroll**：新 log 来时自动滚到底部（hermes `LogsPage.tsx:71-75`）
- **cyberclaw 现状**：未实现 auto-scroll，长 log 用户要手动翻
- **改法**：5 行 useEffect

### 2.8 Cron job 真 runtime ✅ **已做**
cyberclaw 已实现 scheduler + REST CRUD (commit e689d5d)。

### 2.9 /env OAuth 配置体系 **【P2】**
- **能力**：env tab 不只 key/value 编辑，还能 inline OAuth flow（提供商内置）
- **hermes 实现**：`/env` 含 OAuth panel
- **cyberclaw 现状**：v2 Settings env tab 只 key/value mask
- **借鉴方式**：常用 provider (Anthropic/OpenAI/Google) 加 OAuth button
- **风险**：低（仅前端）

### 2.10 TUI 完整交互（slash 命令/approval prompt/session resume picker）**【Q3 决定】**
- 16 个 slash 命令（hermes vs cyberclaw 4 个）
- approval/clarify/sudo/secret prompt overlay
- session resume picker（不是新建，选已有）
- model picker（不是配置，运行时切换）
- tab completion

### 2.11 Plugin 安装来源（URL/GitHub）✅ **已做**
cyberclaw v2 SkillsPage Install Modal 3 source 已实现。

## 3. cyberclaw 独有优势（保留 + 发挥）

### 3.1 五对象统一模型 + 严格边界 **【保留】**
- Agent / Skill / Connector / Capability / PlatformPlugin 边界清晰
- CLAUDE.md 写明"不把 Tool 升级为一级生态对象"
- **vs hermes**：hermes 把 skill 和 plugin 混做扩展机制；cyberclaw 边界更清晰，长期可维护性高
- **怎么发挥**：每个对象有专门 page，互不混淆；hermes 那种 Skill+Plugin 混合 IA 不抄

### 3.2 治理链 + 审计链 + Risk levels **【独有，强发挥】**
- `RiskLevel`/`DangerousCapabilityFilter`/`AutoModeGate`/`CircuitBreaker`
- 每个 action 有 risk classification + approval requirement
- audit log 不可篡改 (hash chain)
- **vs hermes**：hermes 治理是平台 ACL 层面，cyberclaw 是 capability 级别精细控制
- **怎么发挥**：Reviews / Audit / CapabilityMonitor 这三页保留 + 提升 UX

### 3.3 Multi-agent handoff + clarify card **【独有】**
- `HandoffsPage` + `ClarifyCard` 这是 hermes 没有的能力
- **怎么发挥**：v2 chat 内联渲染 handoff/clarify card（v1 已有，v2 待补）

### 3.4 多模型 MoA + 多 provider abstraction **【独有】**
- MoA config 可同时跑多个 LLM 聚合
- **vs hermes**：hermes Models 是单 model 单 task；cyberclaw 是 ensemble
- **怎么发挥**：MoA page 提升 UX (现在 read-only)

### 3.5 Memory L0/L1/L2 分层 **【独有】**
- Working / Episodic / Procedural 三层
- **vs hermes**：hermes 没有内存分层（session 即历史）
- **怎么发挥**：Memory Console 加 inline edit + 跨 session 召回

### 3.6 Persistent execution + AutopilotStepRunner **【独有】**
- 长时间任务即使 server 重启也能继续
- **vs hermes**：hermes 任务跟 process 绑定
- **怎么发挥**：Executions page 加 resume/pause/cancel 按钮（backend 已有）

### 3.7 集群分布式（StatelessBrain + BrainCoordinator）**【独有】**
- Nodes / Cluster page
- **vs hermes**：hermes 单进程
- **怎么发挥**：保留现有 page，但需要加 register/deregister 操作

### 3.8 Rust 后端类型安全 + 性能 **【保留】**
- 477 cargo tests pass，type safety 全栈
- **vs hermes Python**：编译期捕获 bug，运行时延迟低
- **怎么发挥**：不要因为前端动摇后端

## 4. 融合设计建议

### 4.1 IA 重组（v3 — Sessions-first）

如果 Q1 答 sessions-first：

```
Sidebar (1 段 flat, 12-15 项)
├── Sessions      (landing, 取代 Status)
├── Chat          (compose 新 session)
├── Skills        (含 install/manage)
├── Agents        (含 wizard)
├── Models        (含 usage stats)
├── Memory        (L0/L1/L2)
├── Cron          (jobs schedule)
├── Workflows     (含 handoffs/clarifications/reviews)
├── Channels & IM (合并)
├── Audit & Logs  (合并)
├── Cluster       (含 nodes/admin-ops)
├── Plugins       (manifest discovery)
├── Settings      (config/env/about)
└── ⌘K palette    (header right)
```

砍掉：StatusPage 主 nav（保留为 /status 副页面）/ Curator / Capability Monitor / Browser Console / Multimodal / MoA / Workbench / Kanban / Learning（merged into 其它 or 砍）

如果 Q1 答 status-first：keep 现有结构但合并到 ≤18 项。

### 4.2 通用 hook + component

- **useI18n** — port hermes 模式
- **usePageHeader** — port + 30 page 全用
- **useSystemActions** — restart/update 等系统操作（port）
- **useToast** — 替代当前 inline status hack
- **useSessionContext** — 当前 session 全局状态（依赖 Q1 结论）

新组件：
- `PageHeader` (title/subtitle/actions/tabs slot)
- `KpiCard` (Models/Status 共用)
- `EmptyState`（已有，扩 props）
- `Toast`（新增）
- `PluginSlot`（依赖 Q4）

### 4.3 数据模型对齐

Backend 加：
- `GET /api/v1/sessions?limit=20` + FTS search（依赖 Q1）
- `GET /api/v1/sessions/:id` 含 messages + costs + logs + tools used 聚合视图
- `POST /api/v1/skills/:id/toggle` enable/disable
- `GET /api/v1/profiles` + Profile concept 映射（看 Q1 + cyberclaw Agent 是否就是 Profile？）

## 5. 执行清单（按 user value × cost 排序）

待 Q1-Q5 答完后扩展。当前提议优先级：

| # | 任务 | 影响 | 涉及 | 估时 | 依赖 | 风险 |
|---|---|---|---|---|---|---|
| 1 | Sidebar IA 重组 (30→15 项) | 全局 | Sidebar/AppV2 | 2h | Q1 | low |
| 2 | usePageHeader + PageHeader 组件 | 全局 | 新 hook + 30 page 改 | 4h | - | low |
| 3 | useI18n + 15 page i18n | 全局 | i18n.ts 扩 + 改 page | 3h | - | low |
| 4 | Toast 系统替代 inline status | 全局 | 新组件 + 5 page 替换 | 2h | - | low |
| 5 | Sessions 数据模型 + page | 核心 | cyberclaw-store + new API + SessionsPage | 1d | Q1 | med |
| 6 | Skills toggle API + UI | core | backend + frontend | 4h | - | low |
| 7 | Models cost_status 三态 | low | 1 page + 1 endpoint | 2h | - | low |
| 8 | hermes 87 skill import & test | core | cyberclaw skill loader | 1d | 2.5 | med |
| 9 | Page 合并（Capabilities+CapMonitor+Tools 等） | 全局 | 多 page 重构 | 4h | Q5 | med |
| 10 | Mode switcher 删除 | 全局 | Sidebar/Topbar/AppV2 | 30min | - | low |
| 11 | Auto-scroll Logs SSE | low | LogsPage useEffect | 30min | - | low |
| 12 | Profiles concept 映射 | core | 设计决策 + 实现 | 1d | Q1+Q6 | high |
| 13 | Plugin slot system (web) | feat | 6 新文件 + manifest schema | 2d | Q4 | high |
| 14 | TUI 升级（slash/approval/resume） | core | chat_tui.rs 扩 OR 切 Ink | 3d-1w | Q3 | high |
| 15 | xterm PTY chat embed | feat | new server endpoint + ChatPage 重写 | 2d | Q2 | high |

## 6. 关键决策点（即 0 节 5 问题）

见报告顶部。

---

## 注：read-only 限制

analyst agent 是 read-only。本报告由 orchestrator (claude-code 主上下文) 整理。
未来的 ralplan / autopilot 执行需要等 Q1-Q5 答完。
