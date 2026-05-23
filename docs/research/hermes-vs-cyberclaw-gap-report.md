# Hermes Agent vs CyberClaw Admin Dashboard: 功能缺陷报告

**报告日期**: 2026-05-10  
**对比范围**: Hermes Agent Web UI vs CyberClaw Admin Console  
**目标**: 识别 CyberClaw 相比 Hermes 的功能缺口与架构差异

---

## 1. 页面清单与对应关系

### Hermes Agent 页面 (12 个顶级页面)

| 页面 | 主要功能 | 核心交互 |
|------|--------|--------|
| **Sessions** | 会话历史查询、搜索、消息展示 | 展开查看消息、删除会话、FTS 搜索、resume in chat |
| **Skills** | 技能启用/禁用、分类浏览 | 搜索、切换、分类过滤、工具集查看 |
| **Models** | 模型使用统计、成本分析、主/辅助模型配置 | "Use as" 菜单、token 分解条形图、period 选择器 |
| **Analytics** | 日用量统计、模型/技能排名 | 可排序表、日期范围过滤、token 分解图表 |
| **Logs** | 实时日志流、多文件/级别/组件过滤 | 自动刷新开关、行数选择、log 级别分段着色 |
| **Plugins** | 插件安装/启用/更新、提供商配置 | 从 URL 或 GitHub 安装、内存/上下文引擎选择 |
| **Config** | YAML + 表单配置编辑 | 分类侧栏、导入/导出 JSON、搜索、reset to defaults |
| **Env** | API 密钥和环境变量管理 | 分组提供商、reveal/hide、OAuth 配置面板 |
| **Profiles** | SOUL 对话配置管理、创建/重命名 | 内联编辑器、克隆默认值、删除 |
| **Cron** | 定时任务创建与执行 | 新建任务表单、启用/暂停/删除、时间表显示 |
| **Docs** | 嵌入式文档（Docusaurus iframe） | 导航链接 |
| **Chat** | 终端嵌入式 TUI (PTY via WebSocket) | xterm.js、自动尺寸调整、sidebar 频道订阅 |

### CyberClaw 页面 (28+ 文件，包含多页面合并文件)

| 文件/页面 | 主要功能 | 核心交互 |
|---------|--------|--------|
| **pages_a.jsx** | LoginPage: 认证入口 | - |
| **pages_b.jsx** | SkillSheet, InstallDialog, NewSkillDialog | 技能管理、安装对话框 |
| **pages_c.jsx** | NLApprovalBar, AuditAgentTab | 自然语言批准、审计日志 |
| **pages_admin_ops.jsx** | McpServersPanel | MCP 服务器配置 |
| **pages_browser_console.jsx** | 浏览器控制台 (JavaScript REPL) | 历史记录、多行编辑 |
| **pages_capability_monitor.jsx** | 能力监控（功能验证） | 失败/状态着色、origin 标签 |
| **pages_chat.jsx** | 聊天界面（会话存储到 localStorage） | 代理选择、对话历史 |
| **pages_clarifications.jsx** | 澄清面板 | - |
| **pages_clarify_card.jsx** | 单选项澄清卡片 | - |
| **pages_cluster.jsx** | 大脑集群注册 | 集群化多代理架构 |
| **pages_compress_summary.jsx** | （空或合并内容） | - |
| **pages_curator.jsx** | 策展人（技能使用审计） | 审计日志、verdict 着色 |
| **pages_handoff_card.jsx** | 移交卡片视图 | - |
| **pages_handoffs.jsx** | 代理间移交管理 | 工作流可视化 |
| **pages_im_platforms.jsx** | IM 平台配置 (Slack/Discord/etc) | 字段表单、platform 类型 |
| **pages_kanban.jsx** | 任务看板（拖放） | 列分类、优先级着色、drag-drop |
| **pages_learning.jsx** | 学习/培训面板 | 消息转录、提示补充 |
| **pages_memory_console.jsx** | 记忆系统控制台 | - |
| **pages_memory_panel.jsx** | 记忆配置 (ProfileTab) | - |
| **pages_moa.jsx** | 专家混合 (MoA) 配置 | 提供商选择、配置面板 |
| **pages_multimodal.jsx** | 多模态（视觉）设置 | Vision 能力配置 |
| **pages_tools.jsx** | 工具管理与暴露度 | 工具启用/禁用、governance |
| **pages_workbench.jsx** | 工作台 (多模式编辑) | 模式切换 |

---

## 2. 直接页面映射与缺口分析

| Hermes | CyberClaw 等效 | 状态 | 说明 |
|--------|--------------|------|------|
| **Sessions** | pages_chat.jsx | ✅ 基础等效 | CyberClaw 存储到 localStorage，无 FTS 搜索、无消息内容展开、无恢复功能 |
| **Skills** | pages_b.jsx (SkillSheet) | ➕ CyberClaw 更广 | CyberClaw 包含安装对话框和依赖版本管理 |
| **Models** | ❌ **MISSING** | ❌ 无等效 | CyberClaw 无模型成本/token 分析、无 "Use as" 菜单、无主/辅助任务配置 |
| **Analytics** | ❌ **MISSING** | ❌ 无等效 | CyberClaw 无日/模型/技能使用统计，无图表 |
| **Logs** | ❌ **MISSING** | ❌ 无等效 | CyberClaw 无实时日志流、无多级别过滤 |
| **Plugins** | pages_b.jsx (NewSkillDialog) | ➖ CyberClaw 较窄 | CyberClaw 有插件安装但无提供商/上下文引擎切换 |
| **Config** | 分散实现 (pages_moa.jsx, pages_im_platforms.jsx) | ➖ CyberClaw 较窄 | CyberClaw 无统一配置编辑器、无 YAML 模式、无分类导航 |
| **Env** | pages_im_platforms.jsx | ➖ CyberClaw 较窄 | CyberClaw 仅支持 IM 平台，无通用 API 密钥管理 |
| **Profiles** | ❌ **MISSING** | ❌ 无等效 | CyberClaw 无 SOUL 配置、无配置文件概念 |
| **Cron** | ❌ **MISSING** | ❌ 无等效 | CyberClaw 无定时任务UI，无调度表达式编辑 |
| **Docs** | ❌ **MISSING** | ❌ 无等效 | CyberClaw 无内嵌文档 iframe |
| **Chat** | pages_chat.jsx | ✅ 基础等效 | 两者都有聊天，但 CyberClaw 基于 localStorage，Hermes 是实时 PTY |

---

## 3. 关键功能缺口 (优先级排序)

### **第 1 级 — 运营关键**

**1. Models & Analytics 完全缺失** (~1-2 周)
- **影响**: 无法追踪模型成本、token 使用、API 调用频率
- **需求**: 新页面或右栏卡片，显示模型排名、token 分解条、成本累计
- **实现**: 新页面 `pages_analytics.jsx` + API 端点 `/api/analytics`

**2. 日志查看（Logs）缺失** (~3-5 天)
- **影响**: 无实时故障诊断、无多级别/组件过滤
- **需求**: 日志流界面，支持自动刷新、level/component/file 过滤
- **实现**: 新页面 `pages_logs.jsx` + WebSocket `/api/logs/stream`

**3. 配置管理分散** (~1 周)
- **影响**: 无统一配置入口、用户体验割裂
- **需求**: 统一 ConfigPage（分类侧栏、搜索、YAML 模式、导入/导出）
- **实现**: 整合 pages_moa.jsx + pages_im_platforms.jsx + 新分类架构

---

### **第 2 级 — 管理能力**

**4. 环境变量 & API 密钥管理** (~3-5 天)
- **影响**: 无一处查看/编辑所有提供商 API 密钥、无 reveal/hide 切换
- **需求**: EnvPage（分组提供商、搜索、mask/reveal、OAuth 链接）
- **实现**: 新页面 `pages_env.jsx` + API 端点 `/api/env-vars`

**5. 模型选择菜单（"Use as" 下拉）** (~2-3 天)
- **影响**: 无法为主/辅助任务切换模型、无视觉区分
- **需求**: ModelsPage 中的下拉菜单，支持 main + 9 个辅助任务槽位
- **实现**: 新页面 `pages_models.jsx` 或 pages_analytics 扩展

**6. 定时任务管理（Cron）** (~3-4 天)
- **影响**: 无定时执行提示、无调度可视化
- **需求**: CronPage（创建/启用/暂停/删除、cron 表达式编辑、最后运行时间）
- **实现**: 新页面 `pages_cron.jsx` + API 端点 `/api/cron-jobs`

---

### **第 3 级 — 高级功能**

**7. 配置文件 (Profiles)** (~2-3 天)
- **影响**: 无 SOUL 对话文件管理、无快速切换
- **需求**: ProfilesPage（创建/重命名、内联编辑器、克隆）
- **实现**: 新页面 `pages_profiles.jsx` + API 端点 `/api/profiles`

**8. 插件提供商配置** (~1-2 天)
- **影响**: 无法切换内存/上下文引擎
- **需求**: 在 pages_b.jsx 新增 ProvidersCard（内存提供商下拉、上下文引擎选择）
- **实现**: 扩展 pages_b.jsx + 新 ProvidersCard 组件

**9. 会话搜索 (FTS)** (~2-3 天)
- **影响**: 无全文搜索历史消息、无自动 resume in chat 按钮
- **需求**: SessionsPage/pages_chat.jsx 加 FTS 搜索栏 + resume 按钮
- **实现**: 扩展 pages_chat.jsx + 后端 FTS 端点

**10. 内嵌文档** (~1 天)
- **影响**: 无快速访问文档链接
- **需求**: DocsPage（Docusaurus iframe，类似 Hermes）
- **实现**: 新页面 `pages_docs.jsx`

---

## 4. CyberClaw 独特优势

| 功能 | 评价 | 说明 |
|------|------|------|
| **自然语言审批** (pages_c.jsx) | ✅ 有用 | Hermes 无等效；直接通过 NL 批准动作很有用 |
| **能力监控** (pages_capability_monitor.jsx) | ✅ 有用 | 验证功能状态、governance 拒绝跟踪 |
| **代理间移交** (pages_handoffs.jsx) | ✅ 有用 | 可视化多代理工作流；Hermes 无 |
| **看板任务管理** (pages_kanban.jsx) | ✅ 可选 | 拖放任务分类；对某些用户有用但不必需 |
| **策展人审计** (pages_curator.jsx) | ✅ 有用 | 技能使用审计日志；Hermes 无等效 |
| **学习/培训面板** (pages_learning.jsx) | ✅ 有用 | 消息转录/提示补充；Hermes 无 |
| **大脑集群** (pages_cluster.jsx) | ✅ 架构特定 | CyberClaw 特定，多代理编排 |
| **浏览器控制台** (pages_browser_console.jsx) | ⚠️ 高风险 | JavaScript REPL 在浏览器上；安全隐患 |

**总体**: CyberClaw 的优势主要在**多代理协调**与**审计/治理**方面，而非基础运营工具。

---

## 5. 架构差异笔记

### 页面加载与路由
- **Hermes**: React Router 驱动，`src/pages/*.tsx` 映射到路由
- **CyberClaw**: 单一文件多页面（pages_a.jsx, pages_b.jsx, pages_c.jsx），sessionStorage 驱动当前标签页

### 状态管理
- **Hermes**: 本地 useState + API 调用；无全局状态库
- **CyberClaw**: localStorage 持久化（会话、技能、配置）；sessionStorage 临时状态

### i18n 与本地化
- **Hermes**: `useI18n()` Hook + 翻译对象（t.sessions, t.models 等）
- **CyberClaw**: 类似 `useI18n()` 或 `lang` prop，部分未完全国际化

### 插件系统
- **Hermes**: `<PluginSlot name="page:top"/"page:bottom" />` 允许在页面顶/底挂钩
- **CyberClaw**: 未见明显插件架构，页面较独立

### 右栏布局
- **Hermes**: `usePageHeader()` 上下文设置 `setAfterTitle`, `setEnd` 用于页面标题后和右栏组件
- **CyberClaw**: 类似机制但页面更多地通过侧栏（left-rail）而非右栏

---

## 总结

CyberClaw 在以下方面**落后于 Hermes**：
1. **完全缺失**: Models、Analytics、Logs、Profiles、Cron、Docs 页面
2. **部分覆盖**: Config（分散）、Env（仅 IM）、Plugins（无提供商切换）
3. **使用体验**: 无 FTS 搜索会话、无 token 分解视图、无成本追踪

CyberClaw **独特优势**：
- 多代理协调（集群、移交、学习）
- 审计与治理（审批、能力监控、策展人）

**建议**:
- **优先** (< 2 周): 补充 Models、Analytics、Logs、Config 统一编辑
- **次要** (2-4 周): Env、Cron、Profiles、FTS 搜索
- **可选** (1 周): Docs iframe、插件提供商配置

---

## 文件清单

**Hermes pages 涉及文件**:
- `/Users/max/project/cyberclaw/claw-research/hermes-agent/web/src/pages/*.tsx` (12 files)

**CyberClaw pages 涉及文件**:
- `/Users/max/project/cyberclaw/web/src/pages_*.jsx` (23 files)
- `/Users/max/project/cyberclaw/web/src/pages_a.jsx` (Login + misc)
- `/Users/max/project/cyberclaw/web/src/pages_b.jsx` (Skills + Install)
- `/Users/max/project/cyberclaw/web/src/pages_c.jsx` (NL Approval + Audit)

---

**报告完成**: 2026-05-10
