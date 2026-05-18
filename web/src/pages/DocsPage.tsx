// DocsPage — 内置文档浏览。
//
// **Design decision**: 全部内容为静态 React 节点，**不读 disk markdown**，
// **不调 backend**。理由：
//   1. 没有 path traversal 攻击面。
//   2. 没有 backend allowlist 维护负担。
//   3. 用户进这一页是要"快速理解概念"——markdown 实时渲染 + sidebar nav
//      只在 docs-as-source-of-truth 场景（IDE 类）有意义。
//   4. 真正深入的开发者去 docs/ 目录或 GitHub，**这一页是入口、不是仓库**。
//
// 结构: 左侧 7 个主题 nav + 右侧选中主题的结构化内容（含代码示例、表格、
// 外部 docs/ 路径链接）。

import { useState } from "react";
import { type Lang } from "@/lib/i18n";

type TopicId =
  | "architecture"
  | "governance"
  | "sandbox"
  | "memory"
  | "skills"
  | "api"
  | "faq";

interface Topic {
  id: TopicId;
  titleZh: string;
  titleEn: string;
  iconChar: string;
}

const TOPICS: Topic[] = [
  { id: "architecture", titleZh: "架构概览", titleEn: "Architecture", iconChar: "□" },
  { id: "governance", titleZh: "治理与审计", titleEn: "Governance & Audit", iconChar: "◉" },
  { id: "sandbox", titleZh: "容器沙箱", titleEn: "Container Sandbox", iconChar: "▢" },
  { id: "memory", titleZh: "记忆系统", titleEn: "Memory System", iconChar: "≡" },
  { id: "skills", titleZh: "Skill 系统", titleEn: "Skill System", iconChar: "✦" },
  { id: "api", titleZh: "API 速查", titleEn: "API Reference", iconChar: "⌥" },
  { id: "faq", titleZh: "常见问题", titleEn: "FAQ", iconChar: "?" },
];

interface ContentProps {
  lang: Lang;
}

function ArchitectureContent({ lang }: ContentProps) {
  const zh = lang === "zh-CN";
  return (
    <div className="space-y-5">
      <p className="text-[13px] text-fg-2 leading-relaxed">
        {zh
          ? "CyberClaw 是一个受控智能体平台，围绕 5 个一级对象组织："
          : "CyberClaw is a governed agent platform organized around 5 first-class objects:"}
      </p>
      <div className="grid grid-cols-1 md:grid-cols-2 gap-2 text-[12px]">
        {[
          ["Agent", zh ? "角色和编排——决定做什么、调哪个 Skill" : "Roles & orchestration — decides what to do, which Skill"],
          ["Skill", zh ? "方法和知识包——Markdown + 脚本 + 模板" : "Methods & knowledge — Markdown + scripts + templates"],
          ["Connector", zh ? "唯一代码级能力接入面（本地命令、MCP、外部 API）" : "Sole code-level capability surface (local cmds, MCP, ext APIs)"],
          ["Capability", zh ? "治理与授权最小单元——每一个具体动作" : "Smallest unit of governance — every concrete action"],
          ["Platform Plugin", zh ? "平台级扩展（IM 平台、Webhook、自定义页）" : "Platform-wide extensions (IM, Webhook, custom pages)"],
        ].map(([k, v]) => (
          <div key={k} className="rounded-md border border-border bg-bg-2 p-3">
            <div className="font-semibold text-accent text-[12px]">{k}</div>
            <div className="text-fg-2 mt-1">{v}</div>
          </div>
        ))}
      </div>
      <h3 className="text-[13px] font-semibold mt-4">{zh ? "执行链" : "Execution chain"}</h3>
      <pre className="text-[11px] mono text-fg-2 bg-bg-2 border border-border rounded-md p-3 overflow-x-auto">
{`Task/Case
   ↓
Resolver         (路由到合适 Agent + Skill 组合)
   ↓
Execution        (PersistentLoop / AgenticLoop)
   ↓
Governance       (DangerousFilter + IronLaw + PolicyEngine)
   ↓
Connector        (Local / MCP / Web / Browser …)
   ↓
Capability       (cmd.run / fs.read / web.search / …)
   ↓
Artifact + Trace (审计链 SHA-256)`}
      </pre>
    </div>
  );
}

function GovernanceContent({ lang }: ContentProps) {
  const zh = lang === "zh-CN";
  return (
    <div className="space-y-5">
      <p className="text-[13px] text-fg-2 leading-relaxed">
        {zh
          ? "治理是平台内生能力，不是事后补丁。每次能力调用都经过多层检查："
          : "Governance is built-in, not bolt-on. Every capability call goes through layered checks:"}
      </p>
      <ol className="text-[12px] text-fg-2 space-y-2 list-decimal pl-5">
        <li>
          <span className="font-semibold">DangerousCapabilityFilter</span>
          —{" "}
          {zh
            ? "正则匹配 7 类危险动作（rm -rf / 凭证路径 / shell escape），命中即拒绝或转审批。"
            : "Regex match against 7 dangerous patterns (rm -rf / credential paths / shell escape). Hits are denied or routed to approval."}
        </li>
        <li>
          <span className="font-semibold">PolicyEngine</span>
          {" "}
          {zh ? "（默认风险等级评估，可切换为声明式 YAML 规则）" : "(default risk-level, switchable to declarative YAML rules)"}
        </li>
        <li>
          <span className="font-semibold">IronLaw</span>
          —{" "}
          {zh
            ? "6 条不可违反的模型层规则（含 5a/5b/5c 防 role-play / 学术借口 / 权威绑架）。"
            : "6 immutable model-layer rules (with 5a/5b/5c blocking role-play / academic excuse / authority appeal)."}
        </li>
        <li>
          <span className="font-semibold">CircuitBreaker + AutoModeGate</span>
          —{" "}
          {zh
            ? "连续 3 次治理失败 → 强制退出 autopilot；进 auto 模式自动剥离危险能力。"
            : "3 consecutive governance failures → force-exit autopilot; entering auto mode auto-strips dangerous capabilities."}
        </li>
        <li>
          <span className="font-semibold">{zh ? "审计链 (Audit Chain)" : "Audit Chain"}</span>
          —{" "}
          {zh
            ? "每条事件 SHA-256 链式哈希，append-only，无后门。可用 GET /api/v1/audit/verify 验证。"
            : "Every event SHA-256 hash-chained, append-only, no back-door. Verify via GET /api/v1/audit/verify."}
        </li>
      </ol>
      <p className="text-[11px] text-fg-3">
        {zh ? "实证：" : "Evidence:"} v1.0 GA 红队 25 vectors / behavioral redteam 40/40 (100%) refuse rate / audit chain
        8000+ events / corrupted=null
      </p>
    </div>
  );
}

function SandboxContent({ lang }: ContentProps) {
  const zh = lang === "zh-CN";
  return (
    <div className="space-y-5">
      <p className="text-[13px] text-fg-2 leading-relaxed">
        {zh
          ? "cmd.run / cmd.exec 默认在 OS-level 容器内执行，即使 LLM 绕过黑名单，读到的也是容器假数据。"
          : "cmd.run / cmd.exec run inside an OS-level container by default. Even if blacklist bypassed, the LLM only sees container data."}
      </p>
      <table className="text-[12px] w-full border border-border rounded-md overflow-hidden">
        <thead className="bg-bg-3">
          <tr>
            <th className="text-left p-2">{zh ? "维度" : "Dimension"}</th>
            <th className="text-left p-2">{zh ? "配置" : "Config"}</th>
          </tr>
        </thead>
        <tbody>
          {[
            ["Image", "python:3.12-slim"],
            ["Network", "NetworkMode::None"],
            ["Root FS", "read_only_root = true"],
            ["Memory", "512 MB cap"],
            ["Lifecycle", "auto_remove = true"],
            ["Mount", "/workspace only (write-allowed)"],
          ].map(([k, v]) => (
            <tr key={k} className="border-t border-border">
              <td className="p-2 font-semibold">{k}</td>
              <td className="p-2 mono text-fg-2">{v}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <h3 className="text-[13px] font-semibold mt-4">{zh ? "验证示例" : "Verification example"}</h3>
      <pre className="text-[11px] mono text-fg-2 bg-bg-2 border border-border rounded-md p-3 overflow-x-auto">
{`# 容器内
whoami       → nobody
hostname     → 0af7803c0941
/etc/passwd  → Linux 容器内容（不是 host macOS）

# Host
whoami       → max
hostname     → mac-pro.local
/etc/passwd  → 真实用户`}
      </pre>
    </div>
  );
}

function MemoryContent({ lang }: ContentProps) {
  const zh = lang === "zh-CN";
  return (
    <div className="space-y-5">
      <p className="text-[13px] text-fg-2 leading-relaxed">
        {zh
          ? "分层 + 按 agent_id 隔离的 4 类型记忆系统。promote API 把短期回忆升级为长期知识。"
          : "Tiered, agent_id-isolated 4-layer memory system. promote API upgrades short-term recall into long-term knowledge."}
      </p>
      <div className="grid grid-cols-1 md:grid-cols-2 gap-2 text-[12px]">
        {[
          ["L0 Working", zh ? "当前会话的工作记忆（短暂、易失）" : "Current-session working memory (volatile)"],
          ["L1 Episodic", zh ? "会话间事件记忆（「我上次和 max 聊了 X」）" : "Cross-session episodic events"],
          ["L2 Procedural", zh ? "操作规则与流程模板（「如何写测试」）" : "Procedural rules & templates"],
          ["L3 Semantic", zh ? "稳定知识（「我是研究 AI 安全的」）" : "Stable knowledge facts"],
        ].map(([k, v]) => (
          <div key={k} className="rounded-md border border-border bg-bg-2 p-3">
            <div className="font-semibold text-accent text-[12px]">{k}</div>
            <div className="text-fg-2 mt-1">{v}</div>
          </div>
        ))}
      </div>
      <h3 className="text-[13px] font-semibold">{zh ? "上下文压缩" : "Context compression"}</h3>
      <p className="text-[12px] text-fg-2 leading-relaxed">
        {zh
          ? "ContextCompressor 4 阶段（filter / summarize / dedupe / trim）+ IterationBudget 限制 token 上限。10K+ token 会话 6 秒内消化无语义损失。"
          : "ContextCompressor 4-stage (filter / summarize / dedupe / trim) + IterationBudget caps tokens. 10K+ token session digests in ~6s without semantic loss."}
      </p>
    </div>
  );
}

function SkillsContent({ lang }: ContentProps) {
  const zh = lang === "zh-CN";
  return (
    <div className="space-y-5">
      <p className="text-[13px] text-fg-2 leading-relaxed">
        {zh
          ? "Skill 是技能包：Markdown 描述 + 脚本 + 模板 + 示例。Agent 通过 skill_search 发现，通过 Capability 实际执行。"
          : "Skills are knowledge packs: Markdown + scripts + templates + examples. Agents discover via skill_search and execute via Capabilities."}
      </p>
      <h3 className="text-[13px] font-semibold">{zh ? "Skill 结构" : "Skill structure"}</h3>
      <pre className="text-[11px] mono text-fg-2 bg-bg-2 border border-border rounded-md p-3 overflow-x-auto">
{`ecosystem/skills/my-skill/
├── SKILL.md          # 入口：标题 / 用途 / 调用模式
├── scripts/          # 可执行 helper（bash / python）
├── references/       # 参考资料
└── assets/           # 模板 / 示例文件`}
      </pre>
      <h3 className="text-[13px] font-semibold">{zh ? "兼容性" : "Compatibility"}</h3>
      <p className="text-[12px] text-fg-2 leading-relaxed">
        {zh
          ? "Skill 格式兼容 Claude Code / Codex / OpenClaw 三大主流技能包生态。SkillHub 提供搜索 (FTS5) + 安装 + 隔离 + 扫描审计。"
          : "Format-compatible with Claude Code / Codex / OpenClaw skill ecosystems. SkillHub provides search (FTS5) + install + isolation + scan audit."}
      </p>
    </div>
  );
}

function ApiContent({ lang }: ContentProps) {
  const zh = lang === "zh-CN";
  return (
    <div className="space-y-5">
      <p className="text-[13px] text-fg-2 leading-relaxed">
        {zh ? "常用 API 端点速查（全部需 JWT，admin 端点需 role=admin）：" : "Common API endpoints (all need JWT; admin endpoints need role=admin):"}
      </p>
      <table className="text-[11px] mono w-full border border-border rounded-md overflow-hidden">
        <thead className="bg-bg-3">
          <tr>
            <th className="text-left p-2">{zh ? "方法" : "Method"}</th>
            <th className="text-left p-2">{zh ? "路径" : "Path"}</th>
            <th className="text-left p-2">{zh ? "用途" : "Purpose"}</th>
          </tr>
        </thead>
        <tbody>
          {[
            ["POST", "/admin/login", zh ? "登录 → JWT" : "Login → JWT"],
            ["GET", "/api/v1/chat/conversations", zh ? "会话列表" : "Conversations list"],
            ["POST", "/api/v1/chat/message", zh ? "发送消息（agentic loop）" : "Send message (agentic loop)"],
            ["GET", "/api/v1/memory?agent_id=X", zh ? "记忆列表" : "Memory list"],
            ["POST", "/api/v1/memory", zh ? "写记忆" : "Write memory"],
            ["POST", "/api/v1/memory/:id/promote", zh ? "升级记忆层级" : "Promote memory layer"],
            ["GET", "/api/v1/audit/logs?limit=50", zh ? "审计日志尾部" : "Audit log tail"],
            ["GET", "/api/v1/audit/verify", zh ? "校验审计链 SHA-256" : "Verify audit chain SHA-256"],
            ["GET", "/api/v1/plugins", zh ? "平台插件列表" : "Platform plugins list"],
            ["POST", "/api/v1/plugins/:id/invoke", zh ? "调用插件" : "Invoke plugin"],
            ["GET", "/api/v1/skills", zh ? "技能列表" : "Skills list"],
            ["GET", "/api/v1/usage", zh ? "Token 用量" : "Token usage"],
          ].map(([m, p, d]) => (
            <tr key={p} className="border-t border-border">
              <td className="p-2 text-accent">{m}</td>
              <td className="p-2">{p}</td>
              <td className="p-2 sans text-fg-2">{d}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function FaqContent({ lang }: ContentProps) {
  const zh = lang === "zh-CN";
  const faqs = zh
    ? [
        ["Q: 部署需要哪些组件？", "Server (Rust) + WebUI (React+Vite) + optional Docker（沙箱）+ Postgres/SQLite（store）。最简单：cargo run + npm run dev。"],
        ["Q: 怎么换 LLM provider？", "环境变量 LLM_PROVIDER / LLM_BASE_URL / LLM_API_KEY / LLM_DEFAULT_MODEL。重启 server 生效。"],
        ["Q: 治理太严想放宽怎么办？", "策略文件 YAML 编辑 + 重启自动热加载。或在 admin Security 页提交策略变更（走审批流，不直接生效）。"],
        ["Q: Audit chain 真有用吗？", "SHA-256 链式哈希，任何篡改 verify 立刻报告 corrupted_at。append-only，无后门。"],
        ["Q: 数字人 / Agent 同一个概念吗？", "在 CyberClaw 里 Agent 就是数字人——是角色与编排主体，决定调用哪个 Skill / 触发哪些 Capability。"],
      ]
    : [
        ["Q: What does deployment need?", "Server (Rust) + WebUI (React+Vite) + optional Docker (sandbox) + Postgres/SQLite (store). Simplest: cargo run + npm run dev."],
        ["Q: How to switch LLM provider?", "Env vars LLM_PROVIDER / LLM_BASE_URL / LLM_API_KEY / LLM_DEFAULT_MODEL. Restart server."],
        ["Q: Governance too strict, how to relax?", "Edit YAML rules + hot-reload on restart. Or submit policy change via admin Security page (goes through approval, not live)."],
        ["Q: Is the audit chain real?", "SHA-256 hash chain. Any tampering → verify endpoint reports corrupted_at. Append-only, no back-door."],
        ["Q: Is 'Agent' the same as 'digital human'?", "In CyberClaw, Agent IS the digital human — the actor role that decides which Skill / Capability to invoke."],
      ];
  return (
    <div className="space-y-4">
      {faqs.map(([q, a]) => (
        <div key={q} className="rounded-md border border-border bg-bg-2 p-4">
          <div className="font-semibold text-fg-1 text-[13px] mb-1">{q}</div>
          <div className="text-fg-2 text-[12px] leading-relaxed">{a}</div>
        </div>
      ))}
    </div>
  );
}

function renderContent(topic: TopicId, lang: Lang) {
  switch (topic) {
    case "architecture": return <ArchitectureContent lang={lang} />;
    case "governance":   return <GovernanceContent lang={lang} />;
    case "sandbox":      return <SandboxContent lang={lang} />;
    case "memory":       return <MemoryContent lang={lang} />;
    case "skills":       return <SkillsContent lang={lang} />;
    case "api":          return <ApiContent lang={lang} />;
    case "faq":          return <FaqContent lang={lang} />;
  }
}

export default function DocsPage({ lang }: { lang: Lang }) {
  const [topic, setTopic] = useState<TopicId>("architecture");
  const titleZh = "文档";
  const titleEn = "Docs";
  const intro =
    lang === "zh-CN"
      ? "管理后台内置文档——快速了解平台核心概念。深入阅读请打开仓库 docs/ 目录。"
      : "In-app docs — get oriented on platform core concepts. For deep dives, see the repo's docs/ directory.";

  return (
    <div className="space-y-4">
      <header className="space-y-1">
        <h1 className="text-lg font-semibold">{lang === "zh-CN" ? titleZh : titleEn}</h1>
        <p className="text-[12px] text-fg-3">{intro}</p>
      </header>
      <div className="grid grid-cols-[200px_1fr] gap-5">
        <nav className="space-y-1">
          {TOPICS.map((t) => {
            const active = t.id === topic;
            return (
              <button
                key={t.id}
                onClick={() => setTopic(t.id)}
                className={
                  "w-full flex items-center gap-2 px-3 py-2 rounded-md text-left text-[12px] transition-colors " +
                  (active
                    ? "bg-accent-soft text-accent border border-accent/40"
                    : "text-fg-2 hover:bg-bg-2 border border-transparent")
                }
              >
                <span className="text-accent">{t.iconChar}</span>
                <span>{lang === "zh-CN" ? t.titleZh : t.titleEn}</span>
              </button>
            );
          })}
        </nav>
        <main className="min-w-0">{renderContent(topic, lang)}</main>
      </div>
    </div>
  );
}
