// AgentsPage — 3 tabs: Agents / Handoffs / Handoff Audit Trail
// Merges: HandoffsPage into this host.

import { useCallback, useEffect, useRef, useState } from "react";
import { type Agent, type Handoff, type Skill, type Tool, createAgent, fetchAgents, fetchHandoffs, fetchSkills, fetchTools, patchAgent } from "@/lib/api";
import { type Lang } from "@/lib/i18n";

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        source: "来源",
        pageTitle: "智能体与协作",
        tabAgents: "智能体",
        tabHandoffs: "协作",
        tabAudit: "协作审计",
        newAgent: "+ 新建智能体",
        total: (n: number) => `共 ${n} 个`,
        search: "搜索",
        searchPlaceholder: "名称 / 描述",
        trust: "信任等级",
        status: "状态",
        all: "全部",
        high: "高",
        medium: "中",
        low: "低",
        active: "运行中",
        idle: "空闲",
        paused: "已暂停",
        noAgents: "暂无智能体",
        noAgentsFilter: "当前筛选条件下无 Agent",
        noAgentsBody: "点击 '+ 新建智能体' 创建一个，或设置 CYBERCLAW_ADMIN_SEED_DEMO=1 导入演示数据。",
        colAgentId: "agent_id",
        colName: "名称",
        colDesc: "描述",
        colTrust: "信任",
        colStatus: "状态",
        colTools: "工具数",
        colSkills: "Skill 数",
        colRuns: "执行/24h",
        colActions: "操作",
        edit: "编辑",
        editTitle: (name: string) => `编辑智能体 · ${name}`,
        editHint: "可调整描述 / 信任等级 / 运行状态。名称、Skill、工具在创建后不可改。",
        statusLabel: "状态",
        statusActive: "active",
        statusIdle: "idle",
        statusPaused: "paused",
        update: "保存",
        updating: "保存中…",
        updatedFmt: (name: string) => `已更新 ${name}`,
        // Wizard
        wizardTitle: "新建智能体",
        wizardStep: (cur: number, total: number, label: string) => `步骤 ${cur}/${total} — ${label}`,
        stepLabels: ["基础信息", "信任等级", "工具", "Skill", "确认"],
        cancel: "取消",
        back: "← 返回",
        next: "下一步 →",
        creating: "创建中…",
        create: "创建",
        fieldName: "名称",
        fieldDesc: "描述",
        namePlaceholder: "e.g. deploy-bot",
        descPlaceholder: "这个 Agent 做什么？",
        nameRequired: "Agent 名称必填。",
        trustRequired: "请选择信任等级。",
        trustPrompt: "选择该 Agent 的信任等级，这决定了它能自主执行哪些 Capability。",
        trustHigh: "高",
        trustHighDesc: "完全访问 — 受信任的 Agent，拥有广泛的自主 Capability。",
        trustMedium: "中",
        trustMediumDesc: "标准访问 — 可自主执行常见 Capability。",
        trustLow: "低",
        trustLowDesc: "受限访问 — 受监控，大多数动作需要审批。",
        toolsPrompt: "选择授予该 Agent 的工具（Capability facade）。之后可修改。",
        loadingTools: "加载工具中…",
        noTools: "没有可用工具。",
        skillsPrompt: "选择要挂载的 Skill。之后可修改。",
        loadingSkills: "加载 Skill 中…",
        noSkills: "没有已安装的 Skill。",
        reviewPrompt: "创建前请确认 Agent 配置。",
        // Handoffs
        handoffSearch: "搜索",
        handoffSearchPh: "id / 来源 Agent / 目标 Agent",
        handoffStatus: "状态",
        handoffAll: "全部",
        handoffPending: "待处理",
        handoffAccepted: "已接受",
        handoffRejected: "已拒绝",
        noHandoffs: "暂无交接记录",
        noHandoffsFilter: "当前筛选条件下无交接记录",
        colHandoffId: "handoff_id",
        colFrom: "来源 Agent",
        colTo: "目标 Agent",
        colHandoffStatus: "状态",
        colReason: "原因",
        colCreated: "创建时间",
        handoffCount: (n: number) => `已显示 ${n} 条`,
        // Audit Trail
        auditTitle: "交接审计追踪",
        auditKpiCount: "24h 交接次数",
        auditKpiAgents: "涉及 Agent 数",
        auditKpiLatest: "最近交接",
        auditKpiNone: "—",
        auditSearch: "搜索",
        auditSearchPh: "来源 / 目标 / 原因",
        auditNoData: "暂无交接记录",
        auditNoDataFilter: "当前筛选条件下无记录",
        auditDegrade: "端点暂不可用，无法加载交接审计数据。",
        colTs: "时间",
        colFromAgent: "来源 Agent",
        colToAgent: "目标 Agent",
        colReason2: "原因",
        colKind: "类型",
        colExcerpt: "消息摘要",
        drawerTitle: "交接详情",
        drawerClose: "关闭",
        drawerHandoffId: "handoff_id",
        drawerFrom: "来源",
        drawerTo: "目标",
        drawerKind: "类型",
        drawerTs: "时间",
        drawerDecidedAt: "决定时间",
        drawerReason: "原因",
        drawerBriefing: "交接简报",
        drawerBriefingEmpty: "—",
        auditCount: (n: number) => `已显示 ${n} 条`,
      }
    : {
        source: "Source",
        pageTitle: "Agents & Handoffs",
        tabAgents: "Agents",
        tabHandoffs: "Handoffs",
        tabAudit: "Audit Trail",
        newAgent: "+ New agent",
        total: (n: number) => `${n} total`,
        search: "search",
        searchPlaceholder: "name / description",
        trust: "trust",
        status: "status",
        all: "all",
        high: "high",
        medium: "medium",
        low: "low",
        active: "active",
        idle: "idle",
        paused: "paused",
        noAgents: "No agents configured",
        noAgentsFilter: "No agents match the current filters",
        noAgentsBody: "Click '+ New agent' to create one, or run with CYBERCLAW_ADMIN_SEED_DEMO=1 for demo seed.",
        colAgentId: "agent_id",
        colName: "name",
        colDesc: "description",
        colTrust: "trust",
        colStatus: "status",
        colTools: "tools",
        colSkills: "skills",
        colRuns: "runs/24h",
        colActions: "actions",
        edit: "edit",
        editTitle: (name: string) => `Edit agent · ${name}`,
        editHint: "Adjust description / trust level / runtime status. Name, skills, and tools are immutable after creation.",
        statusLabel: "Status",
        statusActive: "active",
        statusIdle: "idle",
        statusPaused: "paused",
        update: "Save",
        updating: "Saving…",
        updatedFmt: (name: string) => `Updated ${name}`,
        // Wizard
        wizardTitle: "New agent",
        wizardStep: (cur: number, total: number, label: string) => `Step ${cur} of ${total} — ${label}`,
        stepLabels: ["Basics", "Trust", "Tools", "Skills", "Review"],
        cancel: "Cancel",
        back: "← Back",
        next: "Next →",
        creating: "Creating…",
        create: "Create",
        fieldName: "Name",
        fieldDesc: "Description",
        namePlaceholder: "e.g. deploy-bot",
        descPlaceholder: "What does this agent do?",
        nameRequired: "Agent name is required.",
        trustRequired: "Please select a trust level.",
        trustPrompt: "Select the trust level for this agent. This controls what capabilities it can execute autonomously.",
        trustHigh: "High",
        trustHighDesc: "Full access — trusted agent with broad autonomous capabilities.",
        trustMedium: "Medium",
        trustMediumDesc: "Standard access — can execute common capabilities autonomously.",
        trustLow: "Low",
        trustLowDesc: "Restricted access — monitored, requires approval for most actions.",
        toolsPrompt: "Select tools to grant this agent. You can modify this later.",
        loadingTools: "Loading tools…",
        noTools: "No tools available.",
        skillsPrompt: "Select skills to attach. You can modify this later.",
        loadingSkills: "Loading skills…",
        noSkills: "No skills installed.",
        reviewPrompt: "Review the agent configuration before creating.",
        // Handoffs
        handoffSearch: "search",
        handoffSearchPh: "id / from / to agent",
        handoffStatus: "status",
        handoffAll: "all",
        handoffPending: "pending",
        handoffAccepted: "accepted",
        handoffRejected: "rejected",
        noHandoffs: "No handoffs found",
        noHandoffsFilter: "No handoffs match the current filters",
        colHandoffId: "handoff_id",
        colFrom: "from_agent",
        colTo: "to_agent",
        colHandoffStatus: "status",
        colReason: "reason",
        colCreated: "created_at",
        handoffCount: (n: number) => `${n} shown`,
        // Audit Trail
        auditTitle: "Handoff Audit Trail",
        auditKpiCount: "Handoffs (24h)",
        auditKpiAgents: "Agents involved",
        auditKpiLatest: "Latest handoff",
        auditKpiNone: "—",
        auditSearch: "search",
        auditSearchPh: "from / to / reason",
        auditNoData: "No handoff records found",
        auditNoDataFilter: "No records match the current filters",
        auditDegrade: "Endpoint unavailable — handoff audit data could not be loaded.",
        colTs: "timestamp",
        colFromAgent: "from_agent",
        colToAgent: "to_agent",
        colReason2: "reason",
        colKind: "kind",
        colExcerpt: "message_excerpt",
        drawerTitle: "Handoff detail",
        drawerClose: "Close",
        drawerHandoffId: "handoff_id",
        drawerFrom: "from",
        drawerTo: "to",
        drawerKind: "kind",
        drawerTs: "timestamp",
        drawerDecidedAt: "decided_at",
        drawerReason: "reason",
        drawerBriefing: "Briefing",
        drawerBriefingEmpty: "—",
        auditCount: (n: number) => `${n} shown`,
      };
}
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import { ArrowRight, Bot } from "@/components/icons";
import Modal from "@/components/Modal";
import Field, { TextArea, TextInput } from "@/components/Field";
import PageHeader from "@/components/PageHeader";
import { useToast } from "@/components/ToastBar";
import { useModalA11y } from "@/components/useModalA11y";

// ─── Shared ──────────────────────────────────────────────────────────────────

type Tab = "agents" | "handoffs" | "audit";

function TabBtn({ active, onClick, children }: { active: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      onClick={onClick}
      className={[
        "px-3 py-1.5 text-[12px] border-b-2 -mb-px transition-colors",
        active
          ? "border-accent text-fg font-medium"
          : "border-transparent text-fg-3 hover:text-fg",
      ].join(" ")}
    >
      {children}
    </button>
  );
}

// ─── Wizard ──────────────────────────────────────────────────────────────────

const TRUST_TONE: Record<string, string> = {
  high: "bg-emerald-500/15 text-emerald-300",
  medium: "bg-amber-500/15 text-amber-300",
  low: "bg-rose-500/15 text-rose-300",
};
const AGENT_STATUS_TONE: Record<string, string> = {
  active: "bg-emerald-500/15 text-emerald-300",
  idle: "bg-white/10 text-fg-3",
  paused: "bg-amber-500/15 text-amber-300",
};

type WizardState = {
  name: string;
  description: string;
  trust: "low" | "medium" | "high" | "";
  toolIds: string[];
  skillIds: string[];
};

function StepDots({ step, labels }: { step: number; labels: string[] }) {
  return (
    <div className="flex items-center justify-center gap-2 mb-5">
      {labels.map((label, i) => (
        <div key={i} className="flex flex-col items-center gap-1">
          <div
            className={`w-2 h-2 rounded-full transition-colors ${
              i === step ? "bg-accent" : i < step ? "bg-accent/40" : "bg-border"
            }`}
          />
          <span className={`text-[9px] ${i === step ? "text-accent" : "text-fg-4"}`}>{label}</span>
        </div>
      ))}
    </div>
  );
}

function CreateAgentWizard({
  onSubmit,
  onClose,
  lang,
}: {
  onSubmit: (agent: Agent) => void;
  onClose: () => void;
  lang: Lang;
}) {
  const L = dict(lang);
  const [step, setStep] = useState(0);
  const [form, setForm] = useState<WizardState>({
    name: "",
    description: "",
    trust: "",
    toolIds: [],
    skillIds: [],
  });
  const [tools, setTools] = useState<Tool[]>([]);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [loadingTools, setLoadingTools] = useState(false);
  const [loadingSkills, setLoadingSkills] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [stepErr, setStepErr] = useState<string | null>(null);
  const toolsLoaded = useRef(false);
  const skillsLoaded = useRef(false);

  useEffect(() => {
    if (step === 2 && !toolsLoaded.current) {
      toolsLoaded.current = true;
      setLoadingTools(true);
      // P1 fix (silent-failure hunt): previously swallowed fetch errors,
      // so a 401 / 500 / network error looked identical to an empty
      // catalog. Surface via setStepErr so operator knows the list is
      // incomplete and can retry.
      fetchTools().then(
        (t) => setTools(t.slice(0, 20)),
        (e: unknown) => {
          const msg = e instanceof Error ? e.message : String(e);
          setStepErr(`Failed to load tools (showing empty list — retry or check connectivity): ${msg}`);
        },
      ).finally(() => setLoadingTools(false));
    }
    if (step === 3 && !skillsLoaded.current) {
      skillsLoaded.current = true;
      setLoadingSkills(true);
      fetchSkills().then(
        (s) => setSkills(s.slice(0, 20)),
        (e: unknown) => {
          const msg = e instanceof Error ? e.message : String(e);
          setStepErr(`Failed to load skills (showing empty list — retry or check connectivity): ${msg}`);
        },
      ).finally(() => setLoadingSkills(false));
    }
  }, [step]);

  const validate = (): boolean => {
    setStepErr(null);
    if (step === 0 && !form.name.trim()) {
      setStepErr(L.nameRequired);
      return false;
    }
    if (step === 1 && !form.trust) {
      setStepErr(L.trustRequired);
      return false;
    }
    return true;
  };

  const next = () => {
    if (!validate()) return;
    setStep((s) => Math.min(s + 1, 4));
  };
  const back = () => {
    setStepErr(null);
    setStep((s) => Math.max(s - 1, 0));
  };

  const toggleId = (id: string, list: string[], set: (v: string[]) => void) => {
    set(list.includes(id) ? list.filter((x) => x !== id) : [...list, id]);
  };

  const submit = async () => {
    setSubmitting(true);
    setErr(null);
    try {
      const agent = await createAgent({
        name: form.name.trim(),
        description: form.description.trim() || undefined,
        trust_level: form.trust || undefined,
        tools: form.toolIds.length ? form.toolIds : undefined,
        skills: form.skillIds.length ? form.skillIds : undefined,
      });
      onSubmit(agent);
    } catch (e: unknown) {
      const ae = e as { status?: number; body?: string };
      setErr(`HTTP ${ae?.status} ${ae?.body ?? ""}`);
    } finally {
      setSubmitting(false);
    }
  };

  const reviewPayload = {
    name: form.name,
    description: form.description || undefined,
    trust_level: form.trust || undefined,
    tools: form.toolIds,
    skills: form.skillIds,
  };

  const footer = (
    <div className="flex items-center justify-between w-full">
      <button
        onClick={step === 0 ? onClose : back}
        className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg bg-bg-3 border border-border"
      >
        {step === 0 ? L.cancel : L.back}
      </button>
      {step < 4 ? (
        <button
          onClick={next}
          className="px-3 h-8 rounded-md text-xs bg-accent text-accent-fg hover:opacity-90"
        >
          {L.next}
        </button>
      ) : (
        <button
          onClick={submit}
          disabled={submitting}
          className="px-3 h-8 rounded-md text-xs bg-accent text-accent-fg hover:opacity-90 disabled:opacity-50"
        >
          {submitting ? L.creating : L.create}
        </button>
      )}
    </div>
  );

  const trustOptions: { value: "low" | "medium" | "high"; label: string; desc: string }[] = [
    { value: "high", label: L.trustHigh, desc: L.trustHighDesc },
    { value: "medium", label: L.trustMedium, desc: L.trustMediumDesc },
    { value: "low", label: L.trustLow, desc: L.trustLowDesc },
  ];

  return (
    <Modal
      open
      onClose={onClose}
      title={L.wizardTitle}
      subtitle={L.wizardStep(step + 1, 5, L.stepLabels[step] ?? "")}
      width={560}
      footer={footer}
    >
      <StepDots step={step} labels={L.stepLabels} />

      {stepErr && (
        <p className="text-[11px] text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded mb-3">{stepErr}</p>
      )}
      {err && (
        <p className="text-[11px] text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded mb-3">{err}</p>
      )}

      {step === 0 && (
        <div className="space-y-3">
          <Field label={L.fieldName} required error={stepErr && !form.name.trim() ? stepErr : undefined}>
            <TextInput
              value={form.name}
              onChange={(v) => setForm((f) => ({ ...f, name: v }))}
              placeholder={L.namePlaceholder}
            />
          </Field>
          <Field label={L.fieldDesc}>
            <TextArea
              value={form.description}
              onChange={(v) => setForm((f) => ({ ...f, description: v }))}
              placeholder={L.descPlaceholder}
              rows={3}
            />
          </Field>
        </div>
      )}

      {step === 1 && (
        <div className="space-y-2">
          <p className="text-xs text-fg-3 mb-3">{L.trustPrompt}</p>
          {trustOptions.map(({ value, label, desc }) => (
            <button
              key={value}
              onClick={() => setForm((f) => ({ ...f, trust: value }))}
              className={`w-full text-left px-4 py-3 rounded-lg border transition-colors ${
                form.trust === value
                  ? "border-accent bg-accent-soft"
                  : "border-border bg-bg-3 hover:bg-hover"
              }`}
            >
              <div className="flex items-center gap-2 mb-0.5">
                <span
                  className={`w-3 h-3 rounded-full border-2 shrink-0 ${
                    form.trust === value ? "border-accent bg-accent" : "border-fg-4 bg-transparent"
                  }`}
                />
                <span className="text-sm font-medium">{label}</span>
              </div>
              <p className="text-[11px] text-fg-3 pl-5">{desc}</p>
            </button>
          ))}
        </div>
      )}

      {step === 2 && (
        <div className="space-y-2">
          <p className="text-xs text-fg-3 mb-3">{L.toolsPrompt}</p>
          {loadingTools && <p className="text-xs text-fg-4">{L.loadingTools}</p>}
          {!loadingTools && tools.length === 0 && (
            <p className="text-xs text-fg-4">{L.noTools}</p>
          )}
          <div className="space-y-1 max-h-64 overflow-y-auto pr-1">
            {tools.map((t) => (
              <label
                key={t.tool_id}
                className="flex items-start gap-2.5 px-3 py-2 rounded-md bg-bg-3 hover:bg-hover cursor-pointer"
              >
                <input
                  type="checkbox"
                  checked={form.toolIds.includes(t.tool_id)}
                  onChange={() =>
                    toggleId(t.tool_id, form.toolIds, (v) =>
                      setForm((f) => ({ ...f, toolIds: v })),
                    )
                  }
                  className="mt-0.5 accent-[hsl(var(--color-accent))]"
                />
                <div className="min-w-0">
                  <div className="text-xs font-medium">{t.name}</div>
                  {t.description && (
                    <div className="text-[11px] text-fg-3 truncate">{t.description}</div>
                  )}
                </div>
              </label>
            ))}
          </div>
        </div>
      )}

      {step === 3 && (
        <div className="space-y-2">
          <p className="text-xs text-fg-3 mb-3">{L.skillsPrompt}</p>
          {loadingSkills && <p className="text-xs text-fg-4">{L.loadingSkills}</p>}
          {!loadingSkills && skills.length === 0 && (
            <p className="text-xs text-fg-4">{L.noSkills}</p>
          )}
          <div className="space-y-1 max-h-64 overflow-y-auto pr-1">
            {skills.map((s) => (
              <label
                key={s.skill_id}
                className="flex items-start gap-2.5 px-3 py-2 rounded-md bg-bg-3 hover:bg-hover cursor-pointer"
              >
                <input
                  type="checkbox"
                  checked={form.skillIds.includes(s.skill_id)}
                  onChange={() =>
                    toggleId(s.skill_id, form.skillIds, (v) =>
                      setForm((f) => ({ ...f, skillIds: v })),
                    )
                  }
                  className="mt-0.5 accent-[hsl(var(--color-accent))]"
                />
                <div className="min-w-0">
                  <div className="text-xs font-medium">{s.name}</div>
                  {s.description && (
                    <div className="text-[11px] text-fg-3 truncate">{s.description}</div>
                  )}
                </div>
              </label>
            ))}
          </div>
        </div>
      )}

      {step === 4 && (
        <div className="space-y-3">
          <p className="text-xs text-fg-3">{L.reviewPrompt}</p>
          <pre className="text-[11px] font-mono bg-bg-3 border border-border rounded-md p-3 overflow-x-auto whitespace-pre-wrap break-words">
            {JSON.stringify(reviewPayload, null, 2)}
          </pre>
        </div>
      )}
    </Modal>
  );
}

// ─── Tab: Agents ─────────────────────────────────────────────────────────────

function AgentsTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [agents, setAgents] = useState<Agent[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [q, setQ] = useState("");
  const [trust, setTrust] = useState("");
  const [status, setStatus] = useState("");
  const [showWizard, setShowWizard] = useState(false);
  const newAgentRowRef = useRef<string | null>(null);
  // Edit-modal state. Backend PATCH /api/v1/agents/:id accepts only
  // description / trust_level / status — name + skills + tools are
  // immutable. Reflected in the UI as a narrow "tweak metadata" modal
  // rather than the full create wizard.
  const [editing, setEditing] = useState<Agent | null>(null);
  const [editDesc, setEditDesc] = useState("");
  const [editTrust, setEditTrust] = useState<string>("");
  const [editStatus, setEditStatus] = useState<string>("");
  const [editBusy, setEditBusy] = useState(false);
  const toast = useToast();

  // P1-E fix: stabilise loadAgents via useCallback so its reference can be
  // safely depended-on by other hooks and so handleCreated doesn't close
  // over a stale state snapshot.
  const loadAgents = useCallback(() => {
    setLoading(true);
    fetchAgents().then(
      (a) => setAgents(a),
      (e) => setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => setLoading(false));
  }, []);

  useEffect(() => { loadAgents(); }, [loadAgents]);

  const handleCreated = (agent: Agent) => {
    setShowWizard(false);
    newAgentRowRef.current = agent.agent_id;
    loadAgents();
    setTimeout(() => {
      if (newAgentRowRef.current) {
        document.getElementById(`agent-row-${newAgentRowRef.current}`)?.scrollIntoView({ behavior: "smooth", block: "center" });
        newAgentRowRef.current = null;
      }
    }, 300);
  };

  const filtered = agents.filter((a) => {
    if (q && !(a.name + " " + (a.description ?? "")).toLowerCase().includes(q.toLowerCase())) return false;
    if (trust && a.trust_level !== trust) return false;
    if (status && a.status !== status) return false;
    return true;
  });

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between gap-3">
        <p className="text-xs text-fg-3">{L.total(agents.length)}</p>
        <button
          onClick={() => setShowWizard(true)}
          className="px-3 h-8 rounded-md text-xs bg-accent text-accent-fg hover:opacity-90"
        >
          {L.newAgent}
        </button>
      </div>

      <div className="flex flex-wrap items-end gap-2 text-xs">
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.search}</span>
          <input
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder={L.searchPlaceholder}
            className="h-8 px-3 rounded-md bg-bg-3 border border-border outline-none focus-ring w-56"
          />
        </label>
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.trust}</span>
          <select
            value={trust}
            onChange={(e) => setTrust(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-28"
          >
            <option value="">{L.all}</option>
            <option value="high">{L.high}</option>
            <option value="medium">{L.medium}</option>
            <option value="low">{L.low}</option>
          </select>
        </label>
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.status}</span>
          <select
            value={status}
            onChange={(e) => setStatus(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-28"
          >
            <option value="">{L.all}</option>
            <option value="active">{L.active}</option>
            <option value="idle">{L.idle}</option>
            <option value="paused">{L.paused}</option>
          </select>
        </label>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={8} />}

      {!loading && filtered.length === 0 && (
        <EmptyState
          icon={Bot}
          title={agents.length === 0 ? L.noAgents : L.noAgentsFilter}
          body={agents.length === 0 ? L.noAgentsBody : undefined}
        />
      )}

      {!loading && filtered.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colAgentId}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colName}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colDesc}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colTrust}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colStatus}</th>
                <th className="px-3 py-2 font-medium text-fg-3 mono text-right">{L.colTools}</th>
                <th className="px-3 py-2 font-medium text-fg-3 mono text-right">{L.colSkills}</th>
                <th className="px-3 py-2 font-medium text-fg-3 mono text-right">{L.colRuns}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[1%]">{L.colActions}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((a) => (
                <tr
                  key={a.agent_id}
                  id={`agent-row-${a.agent_id}`}
                  className="border-t border-border hover:bg-hover"
                >
                  <td className="px-3 py-2 mono text-fg-4">{a.agent_id}</td>
                  <td className="px-3 py-2 font-medium">{a.name}</td>
                  <td className="px-3 py-2 text-fg-3 max-w-xs truncate" title={a.description ?? ""}>{a.description ?? "—"}</td>
                  <td className="px-3 py-2">
                    {a.trust_level && (
                      <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${TRUST_TONE[a.trust_level] ?? "bg-white/10"}`}>
                        {a.trust_level}
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2">
                    {a.status && (
                      <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${AGENT_STATUS_TONE[a.status] ?? "bg-white/10"}`}>
                        {a.status}
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2 mono text-right text-fg-3">{a.tools ?? "—"}</td>
                  <td className="px-3 py-2 mono text-right text-fg-3">{a.skills ?? "—"}</td>
                  <td className="px-3 py-2 mono text-right text-fg-3">{a.executions_24h ?? "—"}</td>
                  <td className="px-3 py-2">
                    <button
                      onClick={() => {
                        setEditing(a);
                        setEditDesc(a.description ?? "");
                        setEditTrust(a.trust_level ?? "");
                        setEditStatus(a.status ?? "");
                      }}
                      className="px-2 h-6 rounded text-[11px] text-fg-3 hover:text-fg hover:bg-hover border border-border"
                    >
                      {L.edit}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/agents</code>
      </p>

      {showWizard && (
        <CreateAgentWizard
          onSubmit={handleCreated}
          onClose={() => setShowWizard(false)}
          lang={lang}
        />
      )}

      <Modal
        open={!!editing}
        onClose={() => setEditing(null)}
        title={editing ? L.editTitle(editing.name) : ""}
        width={460}
        footer={
          <>
            <button
              onClick={() => setEditing(null)}
              className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg hover:bg-hover"
              disabled={editBusy}
            >
              {L.cancel}
            </button>
            <button
              onClick={async () => {
                if (!editing) return;
                setEditBusy(true);
                try {
                  await patchAgent(editing.agent_id, {
                    description: editDesc,
                    trust_level: editTrust || undefined,
                    status: editStatus || undefined,
                  });
                  toast({ tone: "success", msg: L.updatedFmt(editing.name) });
                  setEditing(null);
                  loadAgents();
                } catch (e: unknown) {
                  const ae = e as { status?: number; body?: string };
                  setErr(`HTTP ${ae?.status ?? "?"} ${ae?.body ?? ""}`);
                } finally {
                  setEditBusy(false);
                }
              }}
              disabled={editBusy}
              className="px-3 h-8 rounded-md bg-accent text-accent-fg text-xs font-medium hover:opacity-90 disabled:opacity-50"
            >
              {editBusy ? L.updating : L.update}
            </button>
          </>
        }
      >
        <div className="space-y-4">
          <p className="text-[11px] text-amber-300/90 px-2 py-1.5 bg-amber-500/10 rounded leading-relaxed">
            {L.editHint}
          </p>
          <Field label={L.fieldDesc}>
            <TextArea value={editDesc} onChange={setEditDesc} rows={3} placeholder={L.descPlaceholder} />
          </Field>
          <Field label={L.colTrust}>
            <select
              value={editTrust}
              onChange={(e) => setEditTrust(e.target.value)}
              className="h-9 px-2 rounded-md bg-bg-3 border border-border text-[13px] outline-none focus-ring w-full"
            >
              <option value="">—</option>
              <option value="high">{L.trustHigh}</option>
              <option value="medium">{L.trustMedium}</option>
              <option value="low">{L.trustLow}</option>
            </select>
          </Field>
          <Field label={L.statusLabel}>
            <select
              value={editStatus}
              onChange={(e) => setEditStatus(e.target.value)}
              className="h-9 px-2 rounded-md bg-bg-3 border border-border text-[13px] outline-none focus-ring w-full"
            >
              <option value="">—</option>
              <option value="active">{L.statusActive}</option>
              <option value="idle">{L.statusIdle}</option>
              <option value="paused">{L.statusPaused}</option>
            </select>
          </Field>
        </div>
      </Modal>
    </div>
  );
}

// ─── Tab: Handoffs ───────────────────────────────────────────────────────────

const HANDOFF_STATUS_TONE: Record<string, string> = {
  pending: "bg-amber-500/15 text-amber-300",
  accepted: "bg-emerald-500/15 text-emerald-300",
  rejected: "bg-rose-500/15 text-rose-300",
};

function HandoffsTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [items, setItems] = useState<Handoff[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [q, setQ] = useState("");
  const [status, setStatus] = useState("");

  useEffect(() => {
    let cancelled = false;
    fetchHandoffs().then(
      (d) => !cancelled && setItems(d),
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, []);

  const filtered = items.filter((h) => {
    if (q && !(h.handoff_id + " " + (h.from_agent ?? "") + " " + (h.to_agent ?? "")).toLowerCase().includes(q.toLowerCase())) return false;
    if (status && h.status !== status) return false;
    return true;
  });

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-end gap-2 text-xs">
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.handoffSearch}</span>
          <input
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder={L.handoffSearchPh}
            className="h-8 px-3 rounded-md bg-bg-3 border border-border outline-none focus-ring w-56"
          />
        </label>
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.handoffStatus}</span>
          <select
            value={status}
            onChange={(e) => setStatus(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-28"
          >
            <option value="">{L.handoffAll}</option>
            <option value="pending">{L.handoffPending}</option>
            <option value="accepted">{L.handoffAccepted}</option>
            <option value="rejected">{L.handoffRejected}</option>
          </select>
        </label>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={6} />}

      {!loading && filtered.length === 0 && (
        <EmptyState
          icon={ArrowRight}
          title={items.length === 0 ? L.noHandoffs : L.noHandoffsFilter}
        />
      )}

      {!loading && filtered.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colHandoffId}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colFrom}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colTo}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colHandoffStatus}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colReason}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colCreated}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((h) => (
                <tr key={h.handoff_id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 mono text-fg-4">{h.handoff_id}</td>
                  <td className="px-3 py-2 text-fg-3">{h.from_agent ?? "—"}</td>
                  <td className="px-3 py-2 text-fg-3">{h.to_agent ?? "—"}</td>
                  <td className="px-3 py-2">
                    <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${HANDOFF_STATUS_TONE[h.status ?? ""] ?? "bg-white/10 text-fg-3"}`}>
                      {h.status}
                    </span>
                  </td>
                  <td className="px-3 py-2 text-fg-3 max-w-xs truncate" title={h.reason ?? ""}>{h.reason ?? "—"}</td>
                  <td className="px-3 py-2 mono text-fg-4">{h.created_at ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/handoffs</code>. {L.handoffCount(filtered.length)}
      </p>
    </div>
  );
}

// ─── Tab: Handoff Audit Trail ─────────────────────────────────────────────────

function fmtTs(iso: string | undefined): string {
  if (!iso) return "—";
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

function HandoffDrawer({
  item,
  onClose,
  lang,
}: {
  item: Handoff;
  onClose: () => void;
  lang: Lang;
}) {
  const L = dict(lang);
  const dialogRef = useRef<HTMLDivElement>(null);
  // B-10 fix: shared a11y hook — Escape + focus trap + restore. Replaces
  // the manual `keydown→Escape` useEffect that lived here before.
  useModalA11y(dialogRef, true, onClose);

  const ts = item.ts ?? item.initiated_at ?? item.created_at;
  const fromAgent = item.from_agent ?? item.from_agent_id;
  const toAgent = item.to_agent ?? item.to_agent_id;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/45"
      onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label={L.drawerTitle}
        tabIndex={-1}
        className="bg-bg-2 border border-border rounded-lg shadow-xl w-full max-w-[560px] max-h-[85vh] flex flex-col anim-slide-up outline-none"
      >
        {/* Header */}
        <div className="flex items-center justify-between gap-3 px-5 py-4 border-b border-border shrink-0">
          <h2 className="text-sm font-medium text-fg">{L.drawerTitle}</h2>
          <button onClick={onClose} className="text-fg-3 hover:text-fg transition-colors">
            <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
              <path d="M12 4L4 12M4 4l8 8" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            </svg>
          </button>
        </div>

        {/* Meta grid */}
        <div className="flex-1 overflow-y-auto px-5 py-4 space-y-4 min-h-0">
          <div className="grid grid-cols-2 gap-x-4 gap-y-2 text-[11px] font-mono">
            <span className="text-fg-3">{L.drawerHandoffId}</span>
            <span className="text-fg truncate">{item.handoff_id || "—"}</span>
            <span className="text-fg-3">{L.drawerFrom}</span>
            <span className="text-fg truncate">{fromAgent || "—"}</span>
            <span className="text-fg-3">{L.drawerTo}</span>
            <span className="text-fg truncate">{toAgent || "—"}</span>
            <span className="text-fg-3">{L.drawerKind}</span>
            <span className="text-fg">{item.handoff_kind || "—"}</span>
            <span className="text-fg-3">{L.drawerTs}</span>
            <span className="text-fg">{fmtTs(ts)}</span>
            {item.decided_at && (
              <>
                <span className="text-fg-3">{L.drawerDecidedAt}</span>
                <span className="text-fg">{fmtTs(item.decided_at)}</span>
              </>
            )}
            {item.reason && (
              <>
                <span className="text-fg-3 col-span-2 mt-1">{L.drawerReason}</span>
                <span className="text-fg col-span-2 whitespace-pre-wrap break-words">{item.reason}</span>
              </>
            )}
          </div>

          {/* Briefing */}
          <div>
            <p className="text-[10px] font-mono text-fg-4 uppercase tracking-wide mb-2">{L.drawerBriefing}</p>
            {item.briefing_text ? (
              <pre className="text-[11px] font-mono bg-bg-3 border border-border rounded-md p-3 whitespace-pre-wrap break-words leading-relaxed text-fg">
                {item.briefing_text}
              </pre>
            ) : (
              <div className="text-[11px] italic text-fg-4 bg-bg-3 border border-border rounded-md p-3">
                {L.drawerBriefingEmpty}
              </div>
            )}
          </div>
        </div>

        {/* Footer */}
        <div className="shrink-0 px-5 py-3 border-t border-border flex justify-end">
          <button
            onClick={onClose}
            className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg bg-bg-3 border border-border"
          >
            {L.drawerClose}
          </button>
        </div>
      </div>
    </div>
  );
}

function HandoffAuditTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [items, setItems] = useState<Handoff[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [degraded, setDegraded] = useState(false);
  const [loading, setLoading] = useState(true);
  const [q, setQ] = useState("");
  const [selected, setSelected] = useState<Handoff | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    fetchHandoffs(50).then(
      (d) => { if (!cancelled) { setItems(d); setErr(null); setDegraded(false); } },
      (e: unknown) => {
        if (cancelled) return;
        const status = (e as { status?: number }).status;
        if (status === 404) {
          setDegraded(true);
        } else {
          setErr(`HTTP ${status ?? "?"} ${(e as { body?: string }).body ?? ""}`);
        }
      },
    ).finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, []);

  // KPI calculations
  const now = Date.now();
  const oneDayMs = 24 * 60 * 60 * 1000;
  const count24h = items.filter((h) => {
    const ts = h.ts ?? h.initiated_at ?? h.created_at;
    if (!ts) return false;
    try { return now - new Date(ts).getTime() < oneDayMs; } catch { return false; }
  }).length;

  const agentSet = new Set<string>();
  items.forEach((h) => {
    const f = h.from_agent ?? h.from_agent_id;
    const t = h.to_agent ?? h.to_agent_id;
    if (f) agentSet.add(f);
    if (t) agentSet.add(t);
  });
  const agentCount = agentSet.size;

  const latestTs = items.reduce<string | null>((best, h) => {
    const ts = h.ts ?? h.initiated_at ?? h.created_at;
    if (!ts) return best;
    if (!best) return ts;
    return ts > best ? ts : best;
  }, null);

  const filtered = items.filter((h) => {
    if (!q) return true;
    const hay = [
      h.from_agent, h.from_agent_id, h.to_agent, h.to_agent_id, h.reason, h.handoff_kind,
    ].filter(Boolean).join(" ").toLowerCase();
    return hay.includes(q.toLowerCase());
  });

  return (
    <div className="space-y-4">
      {/* Degraded banner */}
      {degraded && (
        <div className="flex items-center gap-2 px-3 py-2 rounded-md bg-amber-500/10 border border-amber-500/30 text-amber-300 text-xs">
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" className="shrink-0">
            <path d="M8 2L14 13H2L8 2Z" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" />
            <path d="M8 7v3M8 11.5v.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
          {L.auditDegrade}
        </div>
      )}

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}

      {/* KPI cards */}
      {!degraded && (
        <div className="grid grid-cols-3 gap-3">
          {[
            { label: L.auditKpiCount, value: loading ? "…" : String(count24h) },
            { label: L.auditKpiAgents, value: loading ? "…" : String(agentCount) },
            { label: L.auditKpiLatest, value: loading ? "…" : (latestTs ? fmtTs(latestTs) : L.auditKpiNone) },
          ].map(({ label, value }) => (
            <div key={label} className="rounded-lg border border-border bg-bg-2 px-4 py-3">
              <p className="text-[10px] font-medium text-fg-4 uppercase tracking-wide">{label}</p>
              <p className="text-sm font-medium text-fg mt-1 truncate">{value}</p>
            </div>
          ))}
        </div>
      )}

      {/* Search */}
      {!degraded && (
        <div className="flex flex-wrap items-end gap-2 text-xs">
          <label className="flex flex-col gap-0.5">
            <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.auditSearch}</span>
            <input
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder={L.auditSearchPh}
              className="h-8 px-3 rounded-md bg-bg-3 border border-border outline-none focus-ring w-64"
            />
          </label>
        </div>
      )}

      {loading && !degraded && <TableSkeleton cols={6} />}

      {!loading && !degraded && filtered.length === 0 && (
        <EmptyState
          icon={ArrowRight}
          title={items.length === 0 ? L.auditNoData : L.auditNoDataFilter}
        />
      )}

      {!loading && !degraded && filtered.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colTs}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colFromAgent}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colToAgent}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colReason2}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colKind}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colExcerpt}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((h) => {
                const ts = h.ts ?? h.initiated_at ?? h.created_at;
                const fromAgent = h.from_agent ?? h.from_agent_id;
                const toAgent = h.to_agent ?? h.to_agent_id;
                return (
                  <tr
                    key={h.handoff_id}
                    onClick={() => setSelected(h)}
                    className="border-t border-border hover:bg-hover cursor-pointer"
                  >
                    <td className="px-3 py-2 mono text-fg-4 whitespace-nowrap">{fmtTs(ts)}</td>
                    <td className="px-3 py-2 mono text-fg-3 max-w-[120px] truncate">{fromAgent ?? "—"}</td>
                    <td className="px-3 py-2 mono text-fg-3 max-w-[120px] truncate">{toAgent ?? "—"}</td>
                    <td className="px-3 py-2 text-fg-3 max-w-[160px] truncate">{h.reason ?? "—"}</td>
                    <td className="px-3 py-2 text-fg-3">{h.handoff_kind ?? "—"}</td>
                    <td className="px-3 py-2 text-fg-4 max-w-[200px] truncate italic">{h.message_excerpt ?? "—"}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {!degraded && (
        <p className="text-[10px] text-fg-4">
          {L.source}: <code>/api/v1/chat/handoff</code>. {L.auditCount(filtered.length)}
        </p>
      )}

      {selected && (
        <HandoffDrawer item={selected} onClose={() => setSelected(null)} lang={lang} />
      )}
    </div>
  );
}

// ─── Page ────────────────────────────────────────────────────────────────────

export default function AgentsPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [tab, setTab] = useState<Tab>("agents");

  const tabs = (
    <>
      <TabBtn active={tab === "agents"} onClick={() => setTab("agents")}>{L.tabAgents}</TabBtn>
      <TabBtn active={tab === "handoffs"} onClick={() => setTab("handoffs")}>{L.tabHandoffs}</TabBtn>
      <TabBtn active={tab === "audit"} onClick={() => setTab("audit")}>{L.tabAudit}</TabBtn>
    </>
  );

  return (
    <section className="space-y-4">
      <PageHeader title={L.pageTitle} tabs={tabs} />
      {tab === "agents" && <AgentsTab lang={lang} />}
      {tab === "handoffs" && <HandoffsTab lang={lang} />}
      {tab === "audit" && <HandoffAuditTab lang={lang} />}
    </section>
  );
}
