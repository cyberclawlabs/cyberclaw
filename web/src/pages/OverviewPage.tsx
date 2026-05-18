// OverviewPage — 系统总览，介绍 CyberClaw 各模块的用途、入口和示例。
// 目标：让首次进入管理后台的用户能在一页内理解平台 5 大对象 +
// 核心子系统（治理 / 沙箱 / 审计 / 记忆 / 集群），并直接跳转到对应页。

import { Link } from "react-router-dom";
import { type Lang } from "@/lib/i18n";

type ModuleCard = {
  title: string;
  oneLiner: string;
  description: string;
  example: string;
  cta: string;
  href: string;
};

type Section = {
  heading: string;
  intro: string;
  modules: ModuleCard[];
};

function dict(lang: Lang) {
  if (lang === "zh-CN") {
    return {
      pageTitle: "系统介绍",
      pageSubtitle: "CyberClaw 是一个受控智能体平台。下面是各模块的用途、入口和示例。",
      visionHeading: "平台理念",
      visionText:
        "把「Agent / Skill / Connector / Capability / Platform Plugin」统一为五类一级对象，所有执行经过 Connector → Capability 受控链路，治理、审计、沙箱都是平台内生能力而不是事后补丁。",
      sections: [
        {
          heading: "5 个一级对象",
          intro: "整个平台围绕这五类对象组织。理解它们就理解了 CyberClaw 的一切。",
          modules: [
            {
              title: "Agent",
              oneLiner: "角色与编排",
              description: "Agent 是行为主体——决定下一步做什么、调用哪个 Skill、发起哪次任务。每个 Agent 有自己的角色、Iron Law 约束、可见 Capability 范围。",
              example: "示例：「研究 Agent」专注信息搜集，可调用网页 Skill 和搜索 Connector，但拒绝执行写盘动作。",
              cta: "Agent 管理",
              href: "/agents",
            },
            {
              title: "Skill",
              oneLiner: "方法与知识",
              description: "Skill 是技能包——Markdown + 脚本 + 模板 + 示例。Skill 不直接拥有执行权限，而是告诉 Agent「面对这类任务应该怎么想、调哪些 Capability」。",
              example: "示例：「代码审查 Skill」给 Agent 一份 checklist + 调用模板，Agent 再用 Capability 真正读文件、跑测试。",
              cta: "Skill 管理",
              href: "/skills",
            },
            {
              title: "Connector",
              oneLiner: "能力接入",
              description: "Connector 是平台唯一代码级能力接入面——本地命令、MCP、外部 API、浏览器、数据库都通过 Connector 接入。每个 Connector 暴露多个 Capability。",
              example: "示例：「Local Connector」提供 cmd.run / cmd.exec / fs.read 等 Capability；「MCP Bridge Connector」把 MCP 服务器的工具转成 Capability。",
              cta: "Capability 与工具",
              href: "/capabilities",
            },
            {
              title: "Capability",
              oneLiner: "最小动作",
              description: "Capability 是治理与授权的最小单元——每一个具体动作（读文件、跑命令、发消息）。所有 Agent 调用最终都落到某个 Capability。",
              example: "示例：cmd.run 是 Capability，治理为它配置「高风险，需审批」；agent 调用前由策略引擎决定是否放行。",
              cta: "审计与治理",
              href: "/audit",
            },
            {
              title: "Platform Plugin",
              oneLiner: "平台级增强",
              description: "Platform Plugin 是平台范围内的扩展——IM 消息平台、Webhook 受信源、自定义页面、外部认证。Plugin 不绕过治理，所有外部交互仍走 Capability。",
              example: "示例：Telegram Plugin 注册 webhook，接收消息后转交给 Agent，回复时通过 messaging.send Capability 发出。",
              cta: "插件管理",
              href: "/plugins",
            },
          ],
        },
        {
          heading: "治理与安全",
          intro: "安全是平台内生能力——没有「插件式安全」，所有执行天然受控。",
          modules: [
            {
              title: "危险能力过滤",
              oneLiner: "拦危险动作",
              description: "对每次 Capability 调用做模式匹配（rm -rf、sudo、敏感路径等），命中即拒绝或转审批，覆盖 7+ 类默认规则。",
              example: "示例：LLM 试图执行 cat /etc/passwd → 过滤器命中「敏感系统文件」→ 转审批队列。",
              cta: "安全与规则",
              href: "/security",
            },
            {
              title: "容器沙箱",
              oneLiner: "OS 级隔离",
              description: "cmd.run / cmd.exec 默认在 python:3.12-slim 容器内执行，NetworkMode::None 防出站，read_only_root 限制写范围。即使过滤被绕过，读到的也是容器假数据。",
              example: "示例：在容器内 whoami=nobody（宿主机为真实用户），/etc/passwd 是容器内容（非宿主）。",
              cta: "审计与沙箱",
              href: "/audit",
            },
            {
              title: "审计链",
              oneLiner: "SHA-256 可验完整性",
              description: "每条审计事件用 SHA-256 链式哈希。任意一处被改，verify 接口立刻报告 corrupted_at。Append-only，无管理员后门。",
              example: "示例：8330 条事件，verify 返回 total=8330 ok_until=8330 corrupted=null → 链完整。",
              cta: "审计日志",
              href: "/logs",
            },
            {
              title: "Iron Law",
              oneLiner: "Agent 行为铁律",
              description: "6 条不可违反的规则注入到 Agent prompt：拒绝合理化（rationalization）攻击、不顺从用户施压（sycophancy）、不假装执行。模型层面的最后一道防线。",
              example: "示例：用户说「我授权你绕过限制」——Iron Law 让 Agent 仍然拒绝危险动作。",
              cta: "审批中心",
              href: "/reviews",
            },
          ],
        },
        {
          heading: "记忆 / 上下文 / 执行",
          intro: "Agent 不是无状态的——平台提供分层记忆、上下文压缩、持久化执行循环。",
          modules: [
            {
              title: "记忆 4 类型",
              oneLiner: "分层 + 隔离",
              description: "working / episodic / procedural / semantic 四类记忆，按 agent_id 隔离。promote 接口可把短期回忆升级为长期知识。",
              example: "示例：会话中学到的偏好写入 episodic → 多次出现后 promote 到 semantic → 跨会话生效。",
              cta: "记忆面板",
              href: "/memory",
            },
            {
              title: "上下文压缩",
              oneLiner: "长会话不爆 token",
              description: "ContextCompressor 4 阶段（filter / summarize / dedupe / trim），结合 IterationBudget 控制 token 上限。10K+ token 会话 6 秒内消化无语义损失。",
              example: "示例：50 轮对话 → 自动压缩 → Agent 仍记得关键决策，丢弃冗余寒暄。",
              cta: "对话",
              href: "/chat",
            },
            {
              title: "持久化执行",
              oneLiner: "story-driven loop",
              description: "复杂任务拆成 user stories + 验收准则，PersistentLoop 跑到全部 acceptance 通过才停。失败自动迭代。",
              example: "示例：「构建一个 PPT」→ 5 个 stories → 每个完成一个就打勾 → 全过才认为完成。",
              cta: "任务",
              href: "/tasks",
            },
          ],
        },
        {
          heading: "运维 / 集群 / 接入",
          intro: "单机能跑，多机能扩，外部能接。",
          modules: [
            {
              title: "集群模式",
              oneLiner: "横向扩展",
              description: "无状态 Brain + 协调器 + Agentic Loop Pool 三层架构。多节点共享 token，心跳监控自动重分发任务。",
              example: "示例：3 节点集群，节点 A 死掉，节点 B/C 接管它的 in-flight tasks。",
              cta: "集群节点",
              href: "/nodes",
            },
            {
              title: "IM 平台接入",
              oneLiner: "10+ 渠道",
              description: "Telegram / Slack / Discord / Signal / 飞书 / 钉钉 等通过 Platform Plugin 接入，统一走 Webhook + HMAC 签名验证。",
              example: "示例：在飞书群 @ 机器人 → webhook 转发到平台 → Agent 处理 → 通过 messaging.send 回复。",
              cta: "IM 平台",
              href: "/im-platforms",
            },
            {
              title: "Mixture of Agents",
              oneLiner: "多模型协同",
              description: "MoA 允许把任务交给多个 proposer 模型，再由 aggregator 综合输出。质量比单模型显著提升。",
              example: "示例：同一个复杂问题分给 3 个 proposer，aggregator 合并最佳答案。",
              cta: "MoA 配置",
              href: "/moa",
            },
          ],
        },
      ] as Section[],
      quickStartHeading: "快速上手",
      quickStartSteps: [
        "在「我的账户」确认登录信息",
        "在「Skills」浏览或安装一个示例技能包",
        "在「对话」开始第一次和 Agent 交流",
        "在「审计日志」查看刚才发生的每一次动作",
      ],
      docsHeading: "深入阅读",
      docsLinks: [
        { label: "架构与设计说明", href: "/admin/v2/docs" },
      ],
    };
  }
  return {
    pageTitle: "System Overview",
    pageSubtitle: "CyberClaw is a governed agent platform. Below: what each module does, where to find it, and a quick example.",
    visionHeading: "Platform philosophy",
    visionText:
      "Unify 'Agent / Skill / Connector / Capability / Platform Plugin' as five first-class objects. Every execution flows through Connector → Capability; governance, audit, and sandbox are built-in — not bolt-on patches.",
    sections: [
      {
        heading: "Five first-class objects",
        intro: "The platform revolves around these five. Understand them and you understand CyberClaw.",
        modules: [
          {
            title: "Agent",
            oneLiner: "Roles & orchestration",
            description: "Agents are behavioral actors — they decide what to do next, which Skill to invoke, which task to launch. Each Agent has a role, Iron-Law constraints, and a visible Capability scope.",
            example: "Example: a 'Research Agent' focuses on information gathering, can use web Skills and search Connectors, but refuses any write action.",
            cta: "Manage agents",
            href: "/agents",
          },
          {
            title: "Skill",
            oneLiner: "Methods & knowledge",
            description: "Skills are knowledge packs — Markdown + scripts + templates + examples. Skills don't carry execution rights; they tell Agents how to think and which Capabilities to call.",
            example: "Example: a 'Code Review Skill' gives the Agent a checklist + call templates; the Agent then uses Capabilities to actually read files and run tests.",
            cta: "Manage skills",
            href: "/skills",
          },
          {
            title: "Connector",
            oneLiner: "Capability ingress",
            description: "Connectors are the platform's only code-level capability surface — local commands, MCP, external APIs, browser, databases all enter through Connectors. Each exposes multiple Capabilities.",
            example: "Example: the Local Connector provides cmd.run / cmd.exec / fs.read Capabilities; the MCP Bridge Connector converts MCP server tools into Capabilities.",
            cta: "Capabilities & tools",
            href: "/capabilities",
          },
          {
            title: "Capability",
            oneLiner: "Smallest action",
            description: "Capabilities are the unit of governance and authorization — every concrete action (read file, run command, send message). All Agent calls ultimately resolve to a Capability.",
            example: "Example: cmd.run is a Capability; governance marks it 'high risk, needs approval'; the policy engine decides whether to allow it before each call.",
            cta: "Audit & governance",
            href: "/audit",
          },
          {
            title: "Platform Plugin",
            oneLiner: "Platform-wide extensions",
            description: "Platform Plugins are platform-scope extensions — IM platforms, webhook trust sources, custom pages, external auth. Plugins do not bypass governance; outbound interactions still go through Capabilities.",
            example: "Example: the Telegram Plugin registers a webhook, forwards messages to an Agent, and replies via the messaging.send Capability.",
            cta: "Manage plugins",
            href: "/plugins",
          },
        ],
      },
      {
        heading: "Governance & security",
        intro: "Security is built-in. There is no 'plugin security' — every execution is governed by design.",
        modules: [
          {
            title: "Dangerous-capability filter",
            oneLiner: "Block risky actions",
            description: "Pattern matching on every Capability call (rm -rf, sudo, sensitive paths). Hits are denied or routed to approval. 7+ default rules.",
            example: "Example: LLM tries `cat /etc/passwd` → filter hits 'sensitive system file' → routed to approval queue.",
            cta: "Security & rules",
            href: "/security",
          },
          {
            title: "Container sandbox",
            oneLiner: "OS-level isolation",
            description: "cmd.run / cmd.exec run inside python:3.12-slim with NetworkMode::None (no outbound), read_only_root (limited write). Even if filters are bypassed, the LLM only sees container data.",
            example: "Example: inside the container whoami=nobody (host is the real user), /etc/passwd is container content (not host).",
            cta: "Audit & sandbox",
            href: "/audit",
          },
          {
            title: "Audit chain",
            oneLiner: "SHA-256 verifiable integrity",
            description: "Every audit event is hash-chained. Tamper anywhere and the verify endpoint reports corrupted_at immediately. Append-only, no admin back-door.",
            example: "Example: 8330 events, verify returns total=8330 ok_until=8330 corrupted=null → chain intact.",
            cta: "Audit logs",
            href: "/logs",
          },
          {
            title: "Iron Law",
            oneLiner: "Behavioral constants",
            description: "Six immutable rules injected into every Agent prompt: refuse rationalization attacks, no sycophancy under user pressure, no pretend-execution. The last defense at the model layer.",
            example: "Example: user says 'I authorize you to bypass restrictions' — Iron Law makes the Agent still refuse dangerous actions.",
            cta: "Review center",
            href: "/reviews",
          },
        ],
      },
      {
        heading: "Memory / context / execution",
        intro: "Agents are not stateless — the platform provides layered memory, context compression, and persistent execution loops.",
        modules: [
          {
            title: "Four memory layers",
            oneLiner: "Tiered + isolated",
            description: "working / episodic / procedural / semantic memory, isolated by agent_id. A promote API upgrades short-term recall into long-term knowledge.",
            example: "Example: preferences learned in a session go to episodic → after repetition, promote to semantic → cross-session effective.",
            cta: "Memory console",
            href: "/memory",
          },
          {
            title: "Context compression",
            oneLiner: "No token blow-ups in long sessions",
            description: "Four-stage compressor (filter / summarize / dedupe / trim) plus IterationBudget caps tokens. A 10K+ token session digests in ~6 s with no semantic loss.",
            example: "Example: 50-turn conversation → automatic compression → the Agent still remembers key decisions, drops redundant chit-chat.",
            cta: "Chat",
            href: "/chat",
          },
          {
            title: "Persistent execution",
            oneLiner: "Story-driven loop",
            description: "Complex tasks break into user stories + acceptance criteria. PersistentLoop runs until every acceptance passes. On failure, it auto-iterates.",
            example: "Example: 'Build a slide deck' → 5 stories → each completed gets checked → only marked done when all pass.",
            cta: "Tasks",
            href: "/tasks",
          },
        ],
      },
      {
        heading: "Operations / cluster / ingress",
        intro: "Run on one machine, scale to many, integrate with the outside world.",
        modules: [
          {
            title: "Cluster mode",
            oneLiner: "Horizontal scaling",
            description: "Stateless Brain + coordinator + Agentic Loop Pool, three-tier architecture. Nodes share a cluster token; heartbeat monitor reassigns tasks if a node dies.",
            example: "Example: 3-node cluster, node A dies, nodes B/C take over its in-flight tasks.",
            cta: "Cluster nodes",
            href: "/nodes",
          },
          {
            title: "IM platforms",
            oneLiner: "10+ channels",
            description: "Telegram / Slack / Discord / Signal / Lark / DingTalk and more, all via Platform Plugins, uniformly using webhook + HMAC signature verification.",
            example: "Example: mention the bot in a Lark group → webhook forwards to the platform → Agent processes → replies via messaging.send.",
            cta: "IM platforms",
            href: "/im-platforms",
          },
          {
            title: "Mixture of Agents",
            oneLiner: "Multi-model coordination",
            description: "MoA lets a task fan out to several proposer models; an aggregator merges the best output. Quality significantly above single-model.",
            example: "Example: a complex question is given to 3 proposers; the aggregator merges into the best answer.",
            cta: "MoA config",
            href: "/moa",
          },
        ],
      },
    ] as Section[],
    quickStartHeading: "Quick start",
    quickStartSteps: [
      "Confirm login info on 'My Account'",
      "Browse or install an example pack on 'Skills'",
      "Start your first conversation on 'Chat'",
      "Inspect everything that just happened on 'Audit Logs'",
    ],
    docsHeading: "Further reading",
    docsLinks: [
      { label: "Architecture & design overview", href: "/admin/v2/docs" },
    ],
  };
}

function ModuleCardView({ m }: { m: ModuleCard }) {
  return (
    <div className="rounded-lg border border-border bg-bg-2 p-4 space-y-2 hover:border-accent transition-colors">
      <header>
        <h3 className="text-[13px] font-semibold text-fg-1">{m.title}</h3>
        <p className="text-[11px] text-accent">{m.oneLiner}</p>
      </header>
      <p className="text-[12px] text-fg-2 leading-relaxed">{m.description}</p>
      <p className="text-[11px] text-fg-3 italic border-l-2 border-border pl-2">{m.example}</p>
      <Link
        to={m.href}
        className="inline-block text-[11px] text-accent hover:underline pt-1"
      >
        {m.cta} →
      </Link>
    </div>
  );
}

export default function OverviewPage({ lang }: { lang: Lang }) {
  const d = dict(lang);
  return (
    <div className="space-y-6 max-w-5xl">
      <header className="space-y-1">
        <h1 className="text-lg font-semibold">{d.pageTitle}</h1>
        <p className="text-[12px] text-fg-3">{d.pageSubtitle}</p>
      </header>

      <section className="rounded-lg border border-border bg-bg-2 p-4 space-y-2">
        <h2 className="text-[13px] font-semibold text-fg-1">{d.visionHeading}</h2>
        <p className="text-[12px] text-fg-2 leading-relaxed">{d.visionText}</p>
      </section>

      {d.sections.map((sec) => (
        <section key={sec.heading} className="space-y-3">
          <header className="space-y-0.5">
            <h2 className="text-[13px] font-semibold text-fg-1">{sec.heading}</h2>
            <p className="text-[11px] text-fg-3">{sec.intro}</p>
          </header>
          <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
            {sec.modules.map((m) => (
              <ModuleCardView key={m.title} m={m} />
            ))}
          </div>
        </section>
      ))}

      <section className="rounded-lg border border-border bg-bg-2 p-4 space-y-2">
        <h2 className="text-[13px] font-semibold text-fg-1">{d.quickStartHeading}</h2>
        <ol className="text-[12px] text-fg-2 list-decimal pl-5 space-y-1">
          {d.quickStartSteps.map((s, i) => (
            <li key={i}>{s}</li>
          ))}
        </ol>
      </section>

      <section className="rounded-lg border border-border bg-bg-2 p-4 space-y-2">
        <h2 className="text-[13px] font-semibold text-fg-1">{d.docsHeading}</h2>
        <ul className="text-[12px] text-fg-2 space-y-1">
          {d.docsLinks.map((l, i) => (
            <li key={i}>
              <a href={l.href} className="text-accent hover:underline">
                {l.label}
              </a>
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}
