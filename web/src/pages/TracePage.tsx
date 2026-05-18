// TracePage — CyberClaw 统一追踪看板。
// 合并 Trace Timeline / Execution Tree / Provenance 三视图于一页。
// 数据源：/api/v1/runtime/timeline（主）+ /api/v1/executions + /api/v1/audit/logs（兜底）。
// 只读视图，不做任何 mutation。

import { useEffect, useState } from "react";
import {
  type Execution,
  type RuntimeEvent,
  fetchAuditLogs,
  fetchExecutionTree,
  fetchRecentTraceIds,
  fetchTraceTimeline,
} from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import PageHeader from "@/components/PageHeader";

// ─── i18n ────────────────────────────────────────────────────────────────────

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "追踪看板",
        subtitle: "Trace · Provenance · ExecutionTree 统一视图",
        traceIdLabel: "Trace ID / Execution ID",
        traceIdPlaceholder: "输入 trace_id 或 exec_id，或从下拉选择",
        recentLabel: "最近追踪",
        tabTimeline: "时间线",
        tabTree: "执行树",
        tabProvenance: "产物溯源",
        loading: "加载中…",
        empty: "暂无数据",
        emptyTimeline: "本 trace 暂无事件，请输入有效 trace_id 或浏览全部事件。",
        emptyTree: "暂无执行记录。",
        emptyProvenance: "未发现产物记录（audit 中无 artifact 类型条目）。",
        colTs: "时间",
        colKind: "类型",
        colActor: "执行者",
        colAction: "动作",
        colTarget: "目标",
        colSeverity: "严重度",
        colStatus: "状态",
        colAgent: "Agent",
        colCapability: "Capability",
        colDuration: "耗时",
        colArtifact: "产物",
        colType: "类型",
        colSize: "大小",
        colCreated: "创建时间",
        colConsumedBy: "被消费",
        hover: "hover 查看详情",
        accentTitle: "CyberClaw 可观测性差异化",
        accentBody:
          "cyberclaw 把 Trace / Provenance / ExecutionTree 做成一等公民，每个 action 都可追溯到根 case 与所有产物 — 这是平台可解释性、可审计性、可验证性的核心保障。",
        sourceNote:
          "时间线优先使用运行时事件流；如果该端点尚未启用，自动降级到审计日志重建相同视图，业务可见结果一致。",
        root: "根",
        expand: "展开",
        collapse: "折叠",
      }
    : {
        title: "Trace Dashboard",
        subtitle: "Unified view: Timeline · ExecutionTree · Provenance",
        traceIdLabel: "Trace ID / Execution ID",
        traceIdPlaceholder: "Enter trace_id or exec_id, or pick from dropdown",
        recentLabel: "Recent traces",
        tabTimeline: "Timeline",
        tabTree: "Execution Tree",
        tabProvenance: "Provenance",
        loading: "Loading…",
        empty: "No data",
        emptyTimeline:
          "No events for this trace. Enter a valid trace_id or browse all events.",
        emptyTree: "No execution records found.",
        emptyProvenance:
          "No artifact records found (no audit entries with kind=artifact).",
        colTs: "time",
        colKind: "kind",
        colActor: "actor",
        colAction: "action",
        colTarget: "target",
        colSeverity: "severity",
        colStatus: "status",
        colAgent: "agent",
        colCapability: "capability",
        colDuration: "duration",
        colArtifact: "artifact",
        colType: "type",
        colSize: "size",
        colCreated: "created",
        colConsumedBy: "consumed by",
        hover: "hover to see detail",
        accentTitle: "CyberClaw Observability Advantage",
        accentBody:
          "cyberclaw treats Trace, Provenance, and ExecutionTree as first-class citizens — every action is traceable back to its root case and all produced artifacts, forming the foundation of platform explainability, auditability, and verifiability.",
        sourceNote:
          "The timeline uses the runtime event stream when available; if that endpoint isn't enabled, it transparently rebuilds the same view from audit logs.",
        root: "root",
        expand: "expand",
        collapse: "collapse",
      };
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

function fmtTs(ts: number | string): string {
  const d = typeof ts === "number" ? new Date(ts) : new Date(ts);
  if (isNaN(d.getTime())) return String(ts);
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

function fmtDur(secs?: number): string {
  if (secs === undefined || secs === null) return "—";
  if (secs < 60) return `${secs.toFixed(1)}s`;
  return `${Math.floor(secs / 60)}m ${(secs % 60).toFixed(0)}s`;
}

function severityColor(sev: string): string {
  switch (sev?.toLowerCase()) {
    case "critical": return "text-rose-400";
    case "high":     return "text-orange-400";
    case "medium":   return "text-amber-400";
    case "low":      return "text-emerald-400";
    default:         return "text-fg-3";
  }
}

function statusBadge(status: string): string {
  switch (status) {
    case "done":           return "bg-emerald-500/20 text-emerald-400";
    case "running":        return "bg-blue-500/20 text-blue-400";
    case "failed":         return "bg-rose-500/20 text-rose-400";
    case "pending_review": return "bg-amber-500/20 text-amber-400";
    case "cancelled":      return "bg-fg-4/20 text-fg-4";
    default:               return "bg-fg-4/20 text-fg-3";
  }
}

// ─── Tab 1: Timeline ─────────────────────────────────────────────────────────

type TimelineRowProps = { ev: RuntimeEvent; lang: Lang };

function TimelineRow({ ev }: TimelineRowProps) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <tr
        className="border-t border-border hover:bg-hover cursor-pointer select-none"
        onClick={() => setOpen((o) => !o)}
      >
        <td className="px-3 py-2 mono text-[11px] text-fg-3 whitespace-nowrap">
          {fmtTs(ev.ts)}
        </td>
        <td className="px-3 py-2 text-[11px] mono text-fg-3 truncate">
          {ev.rule}
        </td>
        <td className="px-3 py-2 text-[11px] mono text-accent truncate">
          {ev.actor}
        </td>
        <td className="px-3 py-2 text-[11px] truncate">
          {ev.action}
        </td>
        <td className="px-3 py-2 text-[11px] mono text-fg-4 truncate">
          {ev.trace_id || "—"}
        </td>
        <td className={`px-3 py-2 text-[11px] mono font-medium ${severityColor(ev.severity)}`}>
          {ev.severity}
        </td>
      </tr>
      {open && (
        <tr className="border-t border-border bg-bg-3">
          <td colSpan={6} className="px-4 py-3">
            <pre className="text-[10px] mono text-fg-3 whitespace-pre-wrap break-all">
              {JSON.stringify(ev, null, 2)}
            </pre>
          </td>
        </tr>
      )}
    </>
  );
}

function TimelineTab({
  events,
  loading,
  lang,
}: {
  events: RuntimeEvent[];
  loading: boolean;
  lang: Lang;
}) {
  const L = dict(lang);
  if (loading) return <p className="text-xs text-fg-4 py-4">{L.loading}</p>;
  if (events.length === 0)
    return <p className="text-xs text-fg-4 py-4">{L.emptyTimeline}</p>;
  return (
    <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
      <table className="w-full text-xs table-fixed">
        <thead className="bg-bg-3">
          <tr className="text-left">
            <th className="px-3 py-2 font-medium text-fg-3 w-[13%]">{L.colTs}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[20%]">{L.colKind}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[15%]">{L.colActor}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[20%]">{L.colAction}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[22%]">trace_id</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[10%]">{L.colSeverity}</th>
          </tr>
        </thead>
        <tbody>
          {events.map((ev) => (
            <TimelineRow key={ev.id} ev={ev} lang={lang} />
          ))}
        </tbody>
      </table>
      <p className="px-3 py-1.5 text-[10px] text-fg-4 border-t border-border">
        {L.hover}
      </p>
    </div>
  );
}

// ─── Tab 2: Execution Tree ────────────────────────────────────────────────────

type ExecTreeNodeProps = {
  exec: Execution & { parent_id?: string; children?: string[] };
  all: (Execution & { parent_id?: string })[];
  depth: number;
  lang: Lang;
};

function ExecTreeNode({ exec, all, depth, lang }: ExecTreeNodeProps) {
  const L = dict(lang);
  const [collapsed, setCollapsed] = useState(false);
  const children = all.filter((e) => e.parent_id === exec.exec_id);

  return (
    <div style={{ paddingLeft: depth * 20 }} className="text-xs">
      <div className="flex items-center gap-2 py-1.5 border-b border-border/50 hover:bg-hover px-2 rounded">
        {children.length > 0 && (
          <button
            onClick={() => setCollapsed((c) => !c)}
            className="text-[10px] mono text-fg-4 hover:text-fg w-4 shrink-0"
          >
            {collapsed ? "▶" : "▼"}
          </button>
        )}
        {children.length === 0 && <span className="w-4 shrink-0" />}

        {depth === 0 && (
          <span className="text-[9px] px-1 py-0.5 rounded bg-accent/20 text-accent mono shrink-0">
            {L.root}
          </span>
        )}

        <span className="mono text-fg-2 font-medium truncate flex-1">
          {exec.exec_id}
        </span>

        {exec.agent_id && (
          <span className="text-fg-4 text-[11px] mono truncate max-w-[100px]">
            {exec.agent_id}
          </span>
        )}

        {exec.capability && (
          <span className="text-fg-4 text-[11px] mono truncate max-w-[120px]">
            {exec.capability}
          </span>
        )}

        <span
          className={`px-1.5 py-0.5 text-[9px] mono rounded shrink-0 ${statusBadge(exec.status)}`}
        >
          {exec.status}
        </span>

        {exec.duration_secs !== undefined && (
          <span className="text-fg-4 text-[10px] mono shrink-0">
            {fmtDur(exec.duration_secs)}
          </span>
        )}
      </div>

      {!collapsed &&
        children.map((child) => (
          <ExecTreeNode
            key={child.exec_id}
            exec={child}
            all={all}
            depth={depth + 1}
            lang={lang}
          />
        ))}
    </div>
  );
}

function ExecutionTreeTab({
  executions,
  loading,
  lang,
}: {
  executions: (Execution & { parent_id?: string })[];
  loading: boolean;
  lang: Lang;
}) {
  const L = dict(lang);
  if (loading) return <p className="text-xs text-fg-4 py-4">{L.loading}</p>;
  if (executions.length === 0)
    return <p className="text-xs text-fg-4 py-4">{L.emptyTree}</p>;

  // Roots = executions without a parent in this set
  const allIds = new Set(executions.map((e) => e.exec_id));
  const roots = executions.filter(
    (e) => !e.parent_id || !allIds.has(e.parent_id),
  );

  return (
    <div className="rounded-lg border border-border bg-bg-2 overflow-hidden">
      <div className="px-3 py-2 bg-bg-3 border-b border-border flex items-center gap-2">
        <span className="text-[10px] mono text-fg-4 uppercase tracking-widest">
          {L.tabTree}
        </span>
        <span className="text-[10px] text-fg-4">
          {executions.length} exec{executions.length === 1 ? "" : "s"}
        </span>
      </div>
      <div className="p-2 space-y-0.5">
        {roots.map((r) => (
          <ExecTreeNode
            key={r.exec_id}
            exec={r}
            all={executions}
            depth={0}
            lang={lang}
          />
        ))}
      </div>
    </div>
  );
}

// ─── Tab 3: Provenance ────────────────────────────────────────────────────────

type ArtifactRow = {
  id: string;
  name: string;
  kind: string;
  size?: string;
  created_at?: string;
  consumed_by?: string;
  trace_id?: string;
};

function ProvenanceTab({
  artifacts,
  loading,
  lang,
}: {
  artifacts: ArtifactRow[];
  loading: boolean;
  lang: Lang;
}) {
  const L = dict(lang);
  if (loading) return <p className="text-xs text-fg-4 py-4">{L.loading}</p>;
  if (artifacts.length === 0)
    return <p className="text-xs text-fg-4 py-4">{L.emptyProvenance}</p>;
  return (
    <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
      <table className="w-full text-xs table-fixed">
        <thead className="bg-bg-3">
          <tr className="text-left">
            <th className="px-3 py-2 font-medium text-fg-3 w-[28%]">{L.colArtifact}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[12%]">{L.colType}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[10%]">{L.colSize}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[18%]">{L.colCreated}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[16%]">trace_id</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[16%]">{L.colConsumedBy}</th>
          </tr>
        </thead>
        <tbody>
          {artifacts.map((a) => (
            <tr key={a.id} className="border-t border-border hover:bg-hover">
              <td className="px-3 py-2 font-medium truncate mono text-[11px]">
                {a.name}
              </td>
              <td className="px-3 py-2 text-fg-3 text-[11px] mono truncate">
                {a.kind}
              </td>
              <td className="px-3 py-2 text-fg-4 text-[11px] mono">
                {a.size ?? "—"}
              </td>
              <td className="px-3 py-2 text-fg-3 text-[11px] mono truncate">
                {a.created_at ? fmtTs(a.created_at) : "—"}
              </td>
              <td className="px-3 py-2 text-fg-4 text-[11px] mono truncate">
                {a.trace_id || "—"}
              </td>
              <td className="px-3 py-2 text-fg-4 text-[11px] truncate">
                {a.consumed_by ?? "—"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ─── Main Page ────────────────────────────────────────────────────────────────

type Tab = "timeline" | "tree" | "provenance";

export default function TracePage({ lang }: { lang: Lang }) {
  const L = dict(lang);

  const [traceId, setTraceId] = useState("");
  const [recentIds, setRecentIds] = useState<string[]>([]);
  const [activeTab, setActiveTab] = useState<Tab>("timeline");

  // Data state
  const [events, setEvents] = useState<RuntimeEvent[]>([]);
  const [executions, setExecutions] = useState<
    (Execution & { parent_id?: string })[]
  >([]);
  const [artifacts, setArtifacts] = useState<ArtifactRow[]>([]);

  const [loadingTimeline, setLoadingTimeline] = useState(true);
  const [loadingTree, setLoadingTree] = useState(true);
  const [loadingProvenance, setLoadingProvenance] = useState(true);
  const [err, setErr] = useState<string | null>(null);

  // Load recent trace IDs for dropdown
  useEffect(() => {
    fetchRecentTraceIds(20).then(setRecentIds, () => {});
  }, []);

  // Reload timeline + tree + provenance when traceId changes
  useEffect(() => {
    const tid = traceId.trim() || undefined;

    setLoadingTimeline(true);
    setLoadingTree(true);
    setLoadingProvenance(true);
    setErr(null);

    // Timeline
    fetchTraceTimeline(tid, 100)
      .then(setEvents)
      .catch((e: unknown) => {
        const ae = e as { body?: string; status?: number };
        setErr(`Timeline: HTTP ${ae?.status} ${ae?.body ?? ""}`);
      })
      .finally(() => setLoadingTimeline(false));

    // Execution tree
    fetchExecutionTree(tid)
      .then((execs) =>
        setExecutions(execs as (Execution & { parent_id?: string })[]),
      )
      .catch(() => setExecutions([]))
      .finally(() => setLoadingTree(false));

    // Provenance: pull audit logs, filter kind=artifact
    fetchAuditLogs({ limit: 200 })
      .then((resp) => {
        const rows: ArtifactRow[] = (resp.entries ?? [])
          .filter((e) => e.kind === "artifact")
          .map((e, i) => ({
            id: `prov-${i}`,
            name:
              (e.target ?? e.action ?? "unknown"),
            kind: e.kind,
            size: undefined,
            created_at: e.ts,
            consumed_by: undefined,
            trace_id: (e.detail?.trace_id as string | undefined) ?? "",
          }));
        // If a trace_id filter is active, filter artifacts too
        setArtifacts(
          tid ? rows.filter((r) => r.trace_id === tid) : rows,
        );
      })
      .catch(() => setArtifacts([]))
      .finally(() => setLoadingProvenance(false));
  }, [traceId]);

  const TABS: { key: Tab; label: string }[] = [
    { key: "timeline", label: L.tabTimeline },
    { key: "tree", label: L.tabTree },
    { key: "provenance", label: L.tabProvenance },
  ];

  return (
    <section className="space-y-4">
      <PageHeader title={L.title} subtitle={L.subtitle} />

      {/* ── Architecture accent box ────────────────────────────────────────── */}
      <div className="rounded-lg border border-accent/40 bg-accent/5 px-4 py-3 space-y-1">
        <p className="text-[11px] font-semibold text-accent">{L.accentTitle}</p>
        <p className="text-[11px] text-fg-3">{L.accentBody}</p>
      </div>

      {/* ── Trace selector ────────────────────────────────────────────────── */}
      <div className="flex flex-wrap gap-3 items-end">
        <label className="flex flex-col gap-0.5 text-xs flex-1 min-w-[200px]">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">
            {L.traceIdLabel}
          </span>
          <input
            value={traceId}
            onChange={(e) => setTraceId(e.target.value)}
            placeholder={L.traceIdPlaceholder}
            className="h-8 px-3 rounded-md bg-bg-3 border border-border outline-none focus-ring mono text-[11px]"
          />
        </label>

        {recentIds.length > 0 && (
          <label className="flex flex-col gap-0.5 text-xs">
            <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">
              {L.recentLabel}
            </span>
            <select
              value={traceId}
              onChange={(e) => setTraceId(e.target.value)}
              className="h-8 px-2 rounded-md bg-bg-3 border border-border outline-none focus-ring text-[11px] mono max-w-[220px]"
            >
              <option value="">—</option>
              {recentIds.map((id) => (
                <option key={id} value={id}>
                  {id}
                </option>
              ))}
            </select>
          </label>
        )}
      </div>

      {err && (
        <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">
          {err}
        </p>
      )}

      {/* ── Tabs ──────────────────────────────────────────────────────────── */}
      <div className="flex gap-0 border-b border-border">
        {TABS.map((t) => (
          <button
            key={t.key}
            onClick={() => setActiveTab(t.key)}
            className={[
              "px-4 py-2 text-xs font-medium border-b-2 transition-colors",
              activeTab === t.key
                ? "border-accent text-accent"
                : "border-transparent text-fg-3 hover:text-fg",
            ].join(" ")}
          >
            {t.label}
          </button>
        ))}
      </div>

      {/* ── Tab panels ────────────────────────────────────────────────────── */}
      {activeTab === "timeline" && (
        <TimelineTab events={events} loading={loadingTimeline} lang={lang} />
      )}
      {activeTab === "tree" && (
        <ExecutionTreeTab
          executions={executions}
          loading={loadingTree}
          lang={lang}
        />
      )}
      {activeTab === "provenance" && (
        <ProvenanceTab
          artifacts={artifacts}
          loading={loadingProvenance}
          lang={lang}
        />
      )}

      <p className="text-[10px] text-fg-4">{L.sourceNote}</p>
    </section>
  );
}
