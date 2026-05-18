// TasksPage — 5 tabs: List / Executions / Persistent / Capability Monitor / Kanban
// Merges: ExecutionsPage + KanbanPage + PersistentExecution + CapabilityMonitor.

import { useEffect, useMemo, useState } from "react";
import {
  type CapabilityRequestRow,
  type Execution,
  type SkillUsageRow,
  type Task,
  cancelExecution,
  fetchCapabilityRequests,
  fetchExecutions,
  fetchSkillUsage,
  fetchTasks,
  resumeExecution,
} from "@/lib/api";
import { useToast } from "@/components/ToastBar";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import { Activity, List } from "@/components/icons";
import PageHeader from "@/components/PageHeader";

// ─── i18n ────────────────────────────────────────────────────────────────────

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        source: "来源",
        pageTitle: "任务与执行",
        tabList: "列表",
        tabExecutions: "执行",
        tabPersistent: "持久化执行",
        tabCapMon: "能力监控",
        tabKanban: "看板",
        // List tab
        status: "状态",
        all: "全部",
        noTasks: "暂无任务",
        noTasksBody: "Agent 执行任务时会在这里生成记录。",
        colId: "id",
        colAgent: "agent_id",
        colTitle: "标题",
        colStatus: "状态",
        colCreated: "创建时间",
        colUpdated: "更新时间",
        taskCount: (n: number) => `${n} 条`,
        // Executions tab
        risk: "风险",
        noExecs: "暂无执行记录",
        noExecsBody: "Agent 运行 Capability 时会在这里生成记录。",
        colExecId: "exec_id",
        colCapability: "capability",
        colRisk: "风险",
        colStarted: "开始时间",
        colDur: "耗时",
        colAction: "操作",
        cancel: "取消",
        resume: "恢复",
        cancelledMsg: "已取消执行",
        cancelFailed: "取消失败",
        resumedMsg: "已恢复执行",
        resumeFailed: "恢复失败",
        execCount: (n: number) => `${n} 条`,
        // Persistent tab
        persistentTitle: "持久化执行",
        persistentArchHint: "cyberclaw 内生持久化执行循环（Story → Iteration → Verdict 骨架），endpoint 待暴露 — 当前 view 走 executions 兜底显示",
        persistentKpiActive: "活跃持久化循环",
        persistentKpiPassed: "今日通过裁决",
        persistentKpiStuck: "卡住的循环",
        persistentNoData: "暂无持久化执行记录",
        persistentNoDataBody: "持久化执行循环运行时记录会出现在这里。",
        colStorySummary: "故事 / Story",
        colAcceptance: "验收标准",
        colIteration: "迭代次数",
        colVerdict: "裁决",
        verdictPassed: "通过",
        verdictFailed: "失败",
        verdictPending: "待定",
        drawerStory: "故事详情",
        drawerCriteria: "验收标准",
        drawerVerdictHistory: "裁决历史",
        drawerClose: "关闭",
        persistentCount: (n: number) => `${n} 条运行中`,
        // Capability Monitor tab
        capMonTitle: "能力监控",
        capMonKpiPending: "待处理请求",
        capMonKpiFailedClusters: "失败聚类",
        capMonKpiTopSkill: "最常用 Skill",
        capMonKpiOpenFeedback: "待处理反馈",
        capMonQueueTitle: "能力请求队列",
        capMonUsageTitle: "Skill 使用 Telemetry",
        capMonNoRequests: "暂无能力请求",
        capMonNoRequestsBody: "Agent 尝试调用未授权或不存在的 Capability 时，记录会在这里出现。",
        capMonNoUsage: "暂无 Skill 使用记录",
        capMonNoUsageBody: "Skill 被调用后，使用统计会在这里展示。",
        colToolName: "工具名",
        colFailureReason: "失败原因",
        colAttemptedCap: "尝试能力 ID",
        colCount: "次数",
        colLastSeen: "最后出现",
        colReqStatus: "状态",
        colRequester: "请求者",
        colCreatedAt: "创建时间",
        colSkillName: "Skill 名",
        colOrigin: "来源",
        colInvocations: "调用次数",
        colDaysIdle: "闲置天数",
        colLastUsed: "最后使用",
        filterPending: "待处理",
        filterAll: "全部",
        filterImplemented: "已实现",
        filterRejected: "已拒绝",
        reqCount: (n: number) => `${n} 条`,
        skillCount: (n: number) => `${n} 条`,
        drawerCapDetail: "能力请求详情",
        drawerRaw: "原始数据",
        // Kanban tab
        readOnly: "只读视图",
        kanbanLink: "/admin → 看板",
        refresh: "↺ 刷新",
        loading: "加载中…",
        noKanbanTasks: "暂无任务",
        kanbanTotal: (n: number) => `共 ${n} 条`,
        // Column labels
        colPending: "待处理",
        colRunning: "运行中",
        colDone: "已完成",
        colFailed: "已失败",
      }
    : {
        source: "Source",
        pageTitle: "Tasks & Executions",
        tabList: "List",
        tabExecutions: "Executions",
        tabPersistent: "Persistent",
        tabCapMon: "Capability Monitor",
        tabKanban: "Kanban",
        // List tab
        status: "status",
        all: "all",
        noTasks: "No tasks found",
        noTasksBody: "Tasks are created when agents execute work.",
        colId: "id",
        colAgent: "agent_id",
        colTitle: "title",
        colStatus: "status",
        colCreated: "created",
        colUpdated: "updated",
        taskCount: (n: number) => `${n} shown`,
        // Executions tab
        risk: "risk",
        noExecs: "No executions found",
        noExecsBody: "Executions appear when agents run capabilities.",
        colExecId: "exec_id",
        colCapability: "capability",
        colRisk: "risk",
        colStarted: "started",
        colDur: "dur",
        colAction: "action",
        cancel: "Cancel",
        resume: "Resume",
        cancelledMsg: "Execution cancelled",
        cancelFailed: "Cancel failed",
        resumedMsg: "Execution resumed",
        resumeFailed: "Resume failed",
        execCount: (n: number) => `${n} shown`,
        // Persistent tab
        persistentTitle: "Persistent Execution",
        persistentArchHint:
          "CyberClaw native persistent execution loop (Story → Iteration → Verdict skeleton). Dedicated endpoint not yet exposed — this view falls back to the executions endpoint.",
        persistentKpiActive: "Active Loops",
        persistentKpiPassed: "Passed Today",
        persistentKpiStuck: "Stuck Loops",
        persistentNoData: "No persistent executions",
        persistentNoDataBody: "Persistent execution loops will appear here when running.",
        colStorySummary: "Story",
        colAcceptance: "Acceptance",
        colIteration: "Iterations",
        colVerdict: "Verdict",
        verdictPassed: "passed",
        verdictFailed: "failed",
        verdictPending: "pending",
        drawerStory: "Story Details",
        drawerCriteria: "Acceptance Criteria",
        drawerVerdictHistory: "Verdict History",
        drawerClose: "Close",
        persistentCount: (n: number) => `${n} running`,
        // Capability Monitor tab
        capMonTitle: "Capability Monitor",
        capMonKpiPending: "Pending Requests",
        capMonKpiFailedClusters: "Failure Clusters",
        capMonKpiTopSkill: "Top Skill",
        capMonKpiOpenFeedback: "Open Feedbacks",
        capMonQueueTitle: "Capability Requests Queue",
        capMonUsageTitle: "Skill Usage Telemetry",
        capMonNoRequests: "No capability requests",
        capMonNoRequestsBody:
          "When agents attempt unauthorized or missing capabilities, records appear here.",
        capMonNoUsage: "No skill usage records",
        capMonNoUsageBody: "Skill invocation statistics appear here after use.",
        colToolName: "tool_name",
        colFailureReason: "failure_reason",
        colAttemptedCap: "attempted_cap",
        colCount: "count",
        colLastSeen: "last_seen",
        colReqStatus: "status",
        colRequester: "requester",
        colCreatedAt: "created_at",
        colSkillName: "skill_name",
        colOrigin: "origin",
        colInvocations: "invocations",
        colDaysIdle: "days_idle",
        colLastUsed: "last_used",
        filterPending: "pending",
        filterAll: "all",
        filterImplemented: "implemented",
        filterRejected: "rejected",
        reqCount: (n: number) => `${n} shown`,
        skillCount: (n: number) => `${n} shown`,
        drawerCapDetail: "Capability Request Detail",
        drawerRaw: "Raw JSON",
        // Kanban tab
        readOnly: "Read-only view — drag-drop on",
        kanbanLink: "/admin → Kanban",
        refresh: "↺ Refresh",
        loading: "Loading…",
        noKanbanTasks: "no tasks",
        kanbanTotal: (n: number) => `${n} total`,
        // Column labels
        colPending: "Pending",
        colRunning: "Running",
        colDone: "Done",
        colFailed: "Failed",
      };
}

// ─── Shared ──────────────────────────────────────────────────────────────────

type Tab = "list" | "executions" | "persistent" | "capmon" | "kanban";

function TabBtn({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
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

function fmtDate(v: string | undefined): string {
  if (!v) return "—";
  const d = new Date(v);
  return Number.isNaN(d.getTime()) ? v : d.toLocaleString();
}

function fmtDateShort(v: string | undefined): string {
  if (!v) return "—";
  const d = new Date(v);
  return Number.isNaN(d.getTime()) ? v : d.toLocaleDateString();
}

function fmtUnix(ts: number | undefined): string {
  if (!ts) return "—";
  return new Date(ts * 1000).toLocaleString();
}

function fmtRel(ts: number | string | undefined): string {
  if (!ts) return "—";
  const ms = typeof ts === "number" ? Date.now() - ts * 1000 : Date.now() - new Date(ts).getTime();
  if (ms < 0) return "soon";
  const sec = Math.floor(ms / 1000);
  if (sec < 60) return `${sec}s ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const h = Math.floor(min / 60);
  if (h < 48) return `${h}h ago`;
  return `${Math.floor(h / 24)}d ago`;
}

const TASK_STATUS_TONE: Record<string, string> = {
  pending: "bg-amber-500/15 text-amber-300",
  running: "bg-cyan-500/15 text-cyan-300",
  done: "bg-emerald-500/15 text-emerald-300",
  failed: "bg-rose-500/15 text-rose-300",
};

const EXEC_STATUS_TONE: Record<string, string> = {
  running: "bg-cyan-500/15 text-cyan-300",
  done: "bg-emerald-500/15 text-emerald-300",
  failed: "bg-rose-500/15 text-rose-300",
  pending_review: "bg-amber-500/15 text-amber-300",
  cancelled: "bg-zinc-500/15 text-zinc-400",
};

const RISK_TONE: Record<string, string> = {
  Low: "bg-emerald-500/15 text-emerald-300",
  Medium: "bg-amber-500/15 text-amber-300",
  High: "bg-rose-500/15 text-rose-300",
  Critical: "bg-rose-700/20 text-rose-300",
};

const REQ_STATUS_TONE: Record<string, string> = {
  pending: "bg-amber-500/15 text-amber-300",
  implemented: "bg-emerald-500/15 text-emerald-300",
  rejected: "bg-zinc-500/15 text-zinc-400",
};

const FAIL_REASON_TONE: Record<string, string> = {
  not_found: "bg-amber-500/15 text-amber-300",
  governance_denied: "bg-rose-500/15 text-rose-300",
  execution_error: "bg-orange-500/15 text-orange-300",
};

// ─── KPI Card ─────────────────────────────────────────────────────────────────

function KpiCard({
  label,
  value,
  tone = "default",
}: {
  label: string;
  value: string | number;
  tone?: "default" | "green" | "amber" | "red";
}) {
  const toneClass =
    tone === "green"
      ? "text-emerald-300"
      : tone === "amber"
        ? "text-amber-300"
        : tone === "red"
          ? "text-rose-300"
          : "text-fg";
  return (
    <div className="rounded-lg border border-border bg-bg-2 px-4 py-3">
      <div className="text-[10px] uppercase tracking-wide text-fg-3 mb-1">{label}</div>
      <div className={`text-xl font-semibold tabular-nums ${toneClass}`}>{value}</div>
    </div>
  );
}

// ─── Tab: List ───────────────────────────────────────────────────────────────

const TASK_STATUS_OPTIONS = ["all", "pending", "running", "done", "failed"] as const;

function ListTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [status, setStatus] = useState("all");

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    fetchTasks(status, 50).then(
      (t) => !cancelled && setTasks(t),
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, [status]);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-end gap-2 text-xs">
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.status}</span>
          <select
            value={status}
            onChange={(e) => setStatus(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-32"
          >
            {TASK_STATUS_OPTIONS.map((s) => (
              <option key={s} value={s}>{s === "all" ? L.all : s}</option>
            ))}
          </select>
        </label>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={6} />}

      {!loading && tasks.length === 0 && (
        <EmptyState icon={List} title={L.noTasks} body={L.noTasksBody} />
      )}

      {!loading && tasks.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colId}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colAgent}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colTitle}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colStatus}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colCreated}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colUpdated}</th>
              </tr>
            </thead>
            <tbody>
              {tasks.map((t) => (
                <tr key={t.id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 mono text-fg-4 truncate max-w-[120px]">{t.id}</td>
                  <td className="px-3 py-2 mono text-fg-4">{t.agent_id ?? "—"}</td>
                  <td className="px-3 py-2 text-fg-2 truncate max-w-xs">{t.title ?? t.description ?? "—"}</td>
                  <td className="px-3 py-2">
                    <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${TASK_STATUS_TONE[t.status] ?? "bg-white/10 text-fg-3"}`}>
                      {t.status}
                    </span>
                  </td>
                  <td className="px-3 py-2 mono text-fg-4">{fmtDate(t.created_at)}</td>
                  <td className="px-3 py-2 mono text-fg-4">{fmtDate(t.updated_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/tasks?status={status}</code>. {L.taskCount(tasks.length)}
      </p>
    </div>
  );
}

// ─── Tab: Executions ─────────────────────────────────────────────────────────

const EXEC_STATUS_OPTIONS = ["all", "running", "done", "failed", "pending_review", "cancelled"] as const;
const RISK_OPTIONS = ["all", "Low", "Medium", "High", "Critical"] as const;

function fmtDur(secs: number | undefined): string {
  if (secs === undefined || secs === null) return "—";
  if (secs < 60) return `${secs}s`;
  return `${Math.floor(secs / 60)}m${secs % 60}s`;
}

function ExecutionsTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [executions, setExecutions] = useState<Execution[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [status, setStatus] = useState("all");
  const [risk, setRisk] = useState("all");
  const [actionBusy, setActionBusy] = useState<string | null>(null);
  const toast = useToast();

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    fetchExecutions(status, risk, 50).then(
      (e) => !cancelled && setExecutions(e),
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, [status, risk]);

  const handleCancel = async (id: string) => {
    setActionBusy(id);
    try {
      await cancelExecution(id);
      setExecutions((prev) => prev.map((ex) => ex.exec_id === id ? { ...ex, status: "cancelled" } : ex));
      toast({ tone: "success", msg: L.cancelledMsg });
    } catch (e: unknown) {
      const ae = e as { body?: string };
      toast({ tone: "error", msg: `${L.cancelFailed}: ${ae?.body ?? "unknown"}` });
    } finally {
      setActionBusy(null);
    }
  };

  const handleResume = async (id: string) => {
    setActionBusy(id);
    try {
      const updated = await resumeExecution(id);
      setExecutions((prev) => prev.map((ex) => ex.exec_id === id ? { ...ex, status: updated.status } : ex));
      toast({ tone: "success", msg: L.resumedMsg });
    } catch (e: unknown) {
      const ae = e as { body?: string };
      toast({ tone: "error", msg: `${L.resumeFailed}: ${ae?.body ?? "unknown"}` });
    } finally {
      setActionBusy(null);
    }
  };

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-end gap-2 text-xs">
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.status}</span>
          <select
            value={status}
            onChange={(e) => setStatus(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-36"
          >
            {EXEC_STATUS_OPTIONS.map((s) => (
              <option key={s} value={s}>{s === "all" ? L.all : s}</option>
            ))}
          </select>
        </label>
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.risk}</span>
          <select
            value={risk}
            onChange={(e) => setRisk(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-28"
          >
            {RISK_OPTIONS.map((r) => (
              <option key={r} value={r}>{r === "all" ? L.all : r}</option>
            ))}
          </select>
        </label>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={7} />}

      {!loading && executions.length === 0 && (
        <EmptyState icon={Activity} title={L.noExecs} body={L.noExecsBody} />
      )}

      {!loading && executions.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colExecId}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colAgent}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colCapability}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colStatus}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colRisk}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colStarted}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colDur}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colAction}</th>
              </tr>
            </thead>
            <tbody>
              {executions.map((ex) => (
                <tr key={ex.exec_id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 mono text-fg-4 truncate max-w-[120px]">{ex.exec_id}</td>
                  <td className="px-3 py-2 mono text-fg-4">{ex.agent_id ?? "—"}</td>
                  <td className="px-3 py-2 text-fg-2 truncate max-w-[140px]">{ex.capability ?? "—"}</td>
                  <td className="px-3 py-2">
                    <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${EXEC_STATUS_TONE[ex.status] ?? "bg-white/10 text-fg-3"}`}>
                      {ex.status}
                    </span>
                  </td>
                  <td className="px-3 py-2">
                    {ex.risk ? (
                      <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${RISK_TONE[ex.risk] ?? "bg-white/10 text-fg-3"}`}>
                        {ex.risk}
                      </span>
                    ) : "—"}
                  </td>
                  <td className="px-3 py-2 mono text-fg-4">{fmtDate(ex.started_at)}</td>
                  <td className="px-3 py-2 mono text-fg-4">{fmtDur(ex.duration_secs)}</td>
                  <td className="px-3 py-2">
                    {ex.status === "running" && (
                      <button
                        onClick={() => handleCancel(ex.exec_id)}
                        disabled={actionBusy === ex.exec_id}
                        className="px-2 py-0.5 rounded text-[10px] bg-rose-500/10 border border-rose-500/20 hover:bg-rose-500/20 text-rose-400 disabled:opacity-50"
                      >
                        {L.cancel}
                      </button>
                    )}
                    {(ex.status === "paused" || ex.status === "cancelled") && (
                      <button
                        onClick={() => handleResume(ex.exec_id)}
                        disabled={actionBusy === ex.exec_id}
                        className="px-2 py-0.5 rounded text-[10px] bg-emerald-500/10 border border-emerald-500/20 hover:bg-emerald-500/20 text-emerald-400 disabled:opacity-50"
                      >
                        {L.resume}
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/executions?status={status}&risk={risk}</code>. {L.execCount(executions.length)}
      </p>
    </div>
  );
}

// ─── Tab: Persistent Execution ───────────────────────────────────────────────

// PersistentExecution dedicated endpoint is not yet exposed by the server.
// This tab falls back to /api/v1/executions?status=running and renders
// an architecture hint card explaining the pending endpoint.

type PersistentRow = Execution & {
  // Fields that would come from a dedicated persistent endpoint when available.
  story_summary?: string;
  acceptance_status?: string;
  iteration_count?: number;
  verdict?: string;
};

function PersistentDrawer({
  row,
  onClose,
  lang,
}: {
  row: PersistentRow;
  onClose: () => void;
  lang: Lang;
}) {
  const L = dict(lang);
  // P1-G fix: Escape closes drawer (consistent with Modal + HandoffDrawer).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div
      className="fixed inset-0 z-50 flex justify-end"
      onClick={onClose}
    >
      <div
        className="w-full max-w-lg h-full bg-bg border-l border-border overflow-y-auto shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-4 border-b border-border">
          <div>
            <div className="text-sm font-medium text-fg">{L.drawerStory}</div>
            <div className="mono text-[10px] text-fg-4 truncate">{row.exec_id}</div>
          </div>
          <button
            onClick={onClose}
            className="text-fg-3 hover:text-fg text-[11px] px-2 py-1 rounded border border-border"
          >
            {L.drawerClose}
          </button>
        </div>
        <div className="p-5 space-y-5 text-xs">
          <section className="space-y-2">
            <div className="text-[10px] uppercase tracking-wide text-fg-3 font-medium">{L.drawerStory}</div>
            <div className="rounded-md border border-border bg-bg-3 px-3 py-2 text-fg-2">
              {row.story_summary ?? row.capability ?? row.exec_id}
            </div>
          </section>

          <section className="space-y-2">
            <div className="text-[10px] uppercase tracking-wide text-fg-3 font-medium">{L.drawerCriteria}</div>
            <div className="rounded-md border border-border bg-bg-3 px-3 py-2 text-fg-4 italic">
              {row.acceptance_status ?? "—"}
            </div>
          </section>

          <section className="space-y-2">
            <div className="text-[10px] uppercase tracking-wide text-fg-3 font-medium">{L.drawerVerdictHistory}</div>
            <div className="rounded-md border border-border bg-bg-3 px-3 py-2 text-fg-4 italic">
              {row.verdict
                ? <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${row.verdict === "passed" ? "bg-emerald-500/15 text-emerald-300" : "bg-rose-500/15 text-rose-300"}`}>{row.verdict}</span>
                : "—"}
            </div>
          </section>

          <section className="space-y-1.5">
            <div className="text-[10px] uppercase tracking-wide text-fg-3 font-medium">meta</div>
            <div className="grid grid-cols-2 gap-x-4 gap-y-1.5">
              <div>
                <div className="text-[10px] text-fg-4">agent_id</div>
                <div className="mono text-[11px] text-fg-2 truncate">{row.agent_id ?? "—"}</div>
              </div>
              <div>
                <div className="text-[10px] text-fg-4">iteration_count</div>
                <div className="mono text-[11px] text-fg-2">{row.iteration_count ?? "—"}</div>
              </div>
              <div>
                <div className="text-[10px] text-fg-4">status</div>
                <div className="mono text-[11px] text-fg-2">{row.status}</div>
              </div>
              <div>
                <div className="text-[10px] text-fg-4">started_at</div>
                <div className="mono text-[11px] text-fg-2">{fmtDate(row.started_at)}</div>
              </div>
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}

function PersistentTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [rows, setRows] = useState<PersistentRow[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<PersistentRow | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    // Fallback: use running executions since dedicated endpoint not yet exposed.
    fetchExecutions("running", "all", 50).then(
      (execs) => {
        if (!cancelled) setRows(execs as PersistentRow[]);
      },
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, []);

  const kpiActive = rows.length;
  const kpiPassed = rows.filter((r) => r.verdict === "passed").length;
  const kpiStuck = rows.filter((r) => (r.iteration_count ?? 0) >= 5).length;

  return (
    <div className="space-y-4">
      {/* Architecture hint card */}
      <div className="rounded-lg border border-amber-500/30 bg-amber-500/8 px-4 py-3 text-[11px] text-amber-300 leading-relaxed">
        <span className="font-medium uppercase tracking-wide mr-2 text-[10px]">arch note</span>
        {L.persistentArchHint}
      </div>

      {/* KPI row */}
      <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
        <KpiCard label={L.persistentKpiActive} value={loading ? "…" : kpiActive} tone="default" />
        <KpiCard label={L.persistentKpiPassed} value={loading ? "…" : kpiPassed} tone="green" />
        <KpiCard label={L.persistentKpiStuck} value={loading ? "…" : kpiStuck} tone={kpiStuck > 0 ? "red" : "default"} />
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={5} />}

      {!loading && rows.length === 0 && (
        <EmptyState icon={Activity} title={L.persistentNoData} body={L.persistentNoDataBody} />
      )}

      {!loading && rows.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colAgent}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colStorySummary}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colAcceptance}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colIteration}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colVerdict}</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => {
                const verdict = r.verdict;
                const verdictTone =
                  verdict === "passed"
                    ? "bg-emerald-500/15 text-emerald-300"
                    : verdict === "failed"
                      ? "bg-rose-500/15 text-rose-300"
                      : "bg-zinc-500/15 text-zinc-400";
                return (
                  <tr
                    key={r.exec_id}
                    className="border-t border-border hover:bg-hover cursor-pointer"
                    onClick={() => setSelected(r)}
                  >
                    <td className="px-3 py-2 mono text-fg-4">{r.agent_id ?? "—"}</td>
                    <td className="px-3 py-2 text-fg-2 truncate max-w-[200px]">
                      {r.story_summary ?? r.capability ?? r.exec_id}
                    </td>
                    <td className="px-3 py-2 text-fg-3 truncate max-w-[160px]">
                      {r.acceptance_status ?? "—"}
                    </td>
                    <td className="px-3 py-2 mono text-fg-4 tabular-nums">
                      {r.iteration_count ?? "—"}
                    </td>
                    <td className="px-3 py-2">
                      {verdict ? (
                        <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${verdictTone}`}>
                          {verdict}
                        </span>
                      ) : (
                        <span className="px-1.5 py-0.5 rounded text-[10px] mono bg-zinc-500/15 text-zinc-400">
                          {L.verdictPending}
                        </span>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/executions?status=running</code>. {L.persistentCount(rows.length)}
      </p>

      {selected && (
        <PersistentDrawer row={selected} onClose={() => setSelected(null)} lang={lang} />
      )}
    </div>
  );
}

// ─── Tab: Capability Monitor ──────────────────────────────────────────────────

function CapReqDrawer({
  row,
  onClose,
  lang,
}: {
  row: CapabilityRequestRow;
  onClose: () => void;
  lang: Lang;
}) {
  const L = dict(lang);
  // P1-G fix: Escape closes drawer.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="fixed inset-0 z-50 flex justify-end" onClick={onClose}>
      <div
        className="w-full max-w-lg h-full bg-bg border-l border-border overflow-y-auto shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-5 py-4 border-b border-border">
          <div>
            <div className="text-sm font-medium text-fg">{L.drawerCapDetail}</div>
            <div className="mono text-[10px] text-fg-4 truncate">{row.id}</div>
          </div>
          <button
            onClick={onClose}
            className="text-fg-3 hover:text-fg text-[11px] px-2 py-1 rounded border border-border"
          >
            {L.drawerClose}
          </button>
        </div>
        <div className="p-5 space-y-5 text-xs">
          <div className="grid grid-cols-2 gap-x-4 gap-y-3">
            <div>
              <div className="text-[10px] text-fg-4 uppercase tracking-wide mb-1">{L.colToolName}</div>
              <div className="mono text-fg font-medium">{row.tool_name}</div>
            </div>
            <div>
              <div className="text-[10px] text-fg-4 uppercase tracking-wide mb-1">{L.colCount}</div>
              <div className="mono text-[18px] font-semibold text-fg tabular-nums">{row.count ?? "—"}</div>
            </div>
            <div>
              <div className="text-[10px] text-fg-4 uppercase tracking-wide mb-1">{L.colReqStatus}</div>
              <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${REQ_STATUS_TONE[row.status] ?? "bg-white/10 text-fg-3"}`}>
                {row.status}
              </span>
            </div>
            <div>
              <div className="text-[10px] text-fg-4 uppercase tracking-wide mb-1">{L.colFailureReason}</div>
              <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${FAIL_REASON_TONE[row.failure_reason ?? ""] ?? "bg-white/10 text-fg-3"}`}>
                {row.failure_reason ?? "—"}
              </span>
            </div>
            <div>
              <div className="text-[10px] text-fg-4 uppercase tracking-wide mb-1">{L.colLastSeen}</div>
              <div className="mono text-fg-2">{fmtDate(row.last_seen_at ?? row.requested_at)}</div>
            </div>
            <div>
              <div className="text-[10px] text-fg-4 uppercase tracking-wide mb-1">{L.colCreatedAt}</div>
              <div className="mono text-fg-2">{fmtDate(row.requested_at)}</div>
            </div>
            <div className="col-span-2">
              <div className="text-[10px] text-fg-4 uppercase tracking-wide mb-1">{L.colAttemptedCap}</div>
              <div className="mono text-fg-3 break-all">{row.attempted_capability_id ?? row.attempted_connector_id ?? "—"}</div>
            </div>
          </div>
          {row.notes && (
            <div>
              <div className="text-[10px] text-fg-4 uppercase tracking-wide mb-1.5">notes</div>
              <div className="bg-bg-3 border border-border rounded px-3 py-2 text-fg-2 whitespace-pre-wrap">{row.notes}</div>
            </div>
          )}
          <div>
            <div className="text-[10px] text-fg-4 uppercase tracking-wide mb-1.5">{L.drawerRaw}</div>
            <pre className="bg-bg-3 border border-border rounded px-3 py-2 mono text-[10px] text-fg-3 overflow-x-auto whitespace-pre-wrap break-words">
              {JSON.stringify(row, null, 2)}
            </pre>
          </div>
        </div>
      </div>
    </div>
  );
}

const CAP_REQ_FILTER_OPTIONS = ["pending", "all", "implemented", "rejected"] as const;

function CapMonTab({ lang }: { lang: Lang }) {
  const L = dict(lang);

  // Capability requests state
  const [reqFilter, setReqFilter] = useState<string>("pending");
  const [requests, setRequests] = useState<CapabilityRequestRow[]>([]);
  const [reqLoading, setReqLoading] = useState(true);
  const [reqErr, setReqErr] = useState<string | null>(null);
  const [selectedReq, setSelectedReq] = useState<CapabilityRequestRow | null>(null);

  // Skill usage state
  const [usageRows, setUsageRows] = useState<SkillUsageRow[]>([]);
  const [usageLoading, setUsageLoading] = useState(true);
  const [usageErr, setUsageErr] = useState<string | null>(null);
  const [usageSort, setUsageSort] = useState<"use_count" | "days_idle" | "skill">("use_count");

  // Load capability requests
  useEffect(() => {
    let cancelled = false;
    setReqLoading(true);
    setReqErr(null);
    fetchCapabilityRequests(reqFilter, 200).then(
      (r) => !cancelled && setRequests(r.entries),
      (e) => !cancelled && setReqErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setReqLoading(false));
    return () => { cancelled = true; };
  }, [reqFilter]);

  // Load skill usage (once on mount)
  useEffect(() => {
    let cancelled = false;
    setUsageLoading(true);
    setUsageErr(null);
    fetchSkillUsage().then(
      (r) => !cancelled && setUsageRows(r.records),
      (e) => !cancelled && setUsageErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setUsageLoading(false));
    return () => { cancelled = true; };
  }, []);

  // KPIs
  const kpiPending = requests.filter((r) => r.status === "pending").length;
  const kpiClusters = requests.length;
  const topSkill = useMemo(() => {
    if (usageRows.length === 0) return "—";
    const top = [...usageRows].sort((a, b) => b.use_count - a.use_count)[0];
    return top?.skill ?? "—";
  }, [usageRows]);
  const kpiOpenFeedback = kpiPending;

  // Sorted skill usage
  const sortedUsage = useMemo(() => {
    return [...usageRows].sort((a, b) => {
      if (usageSort === "use_count") return b.use_count - a.use_count;
      if (usageSort === "days_idle") return b.days_idle - a.days_idle;
      return a.skill.localeCompare(b.skill);
    });
  }, [usageRows, usageSort]);

  // Sort indicator helper
  const SortBtn = ({ col, label }: { col: typeof usageSort; label: string }) => (
    <button
      onClick={() => setUsageSort(col)}
      className={`hover:text-fg transition-colors ${usageSort === col ? "text-fg font-medium" : "text-fg-3"}`}
    >
      {label}{usageSort === col ? " ↓" : ""}
    </button>
  );

  return (
    <div className="space-y-4">
      {/* KPI row */}
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
        <KpiCard label={L.capMonKpiPending} value={reqLoading ? "…" : kpiPending} tone={kpiPending > 0 ? "amber" : "default"} />
        <KpiCard label={L.capMonKpiFailedClusters} value={reqLoading ? "…" : kpiClusters} tone="default" />
        <KpiCard label={L.capMonKpiTopSkill} value={usageLoading ? "…" : topSkill} tone="default" />
        <KpiCard label={L.capMonKpiOpenFeedback} value={reqLoading ? "…" : kpiOpenFeedback} tone="default" />
      </div>

      {/* Table 1 — Capability Requests Queue */}
      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <div className="text-[11px] font-medium text-fg">{L.capMonQueueTitle}</div>
          <div className="inline-flex items-center rounded-md border border-border bg-bg-3 p-0.5">
            {CAP_REQ_FILTER_OPTIONS.map((f) => (
              <button
                key={f}
                onClick={() => setReqFilter(f)}
                className={[
                  "px-2 h-6 text-[10px] rounded-[4px] mono transition-colors",
                  reqFilter === f
                    ? "bg-bg-2 text-fg border border-border"
                    : "text-fg-3 hover:text-fg",
                ].join(" ")}
              >
                {f === "pending" ? L.filterPending : f === "all" ? L.filterAll : f === "implemented" ? L.filterImplemented : L.filterRejected}
              </button>
            ))}
          </div>
        </div>

        {reqErr && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{reqErr}</p>}
        {reqLoading && <TableSkeleton cols={6} />}

        {!reqLoading && requests.length === 0 && (
          <EmptyState icon={Activity} title={L.capMonNoRequests} body={L.capMonNoRequestsBody} />
        )}

        {!reqLoading && requests.length > 0 && (
          <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
            <table className="w-full text-xs">
              <thead className="bg-bg-3">
                <tr className="text-left">
                  <th className="px-3 py-2 font-medium text-fg-3">{L.colToolName}</th>
                  <th className="px-3 py-2 font-medium text-fg-3">{L.colFailureReason}</th>
                  <th className="px-3 py-2 font-medium text-fg-3">{L.colAttemptedCap}</th>
                  <th className="px-3 py-2 font-medium text-fg-3 text-right">{L.colCount}</th>
                  <th className="px-3 py-2 font-medium text-fg-3">{L.colLastSeen}</th>
                  <th className="px-3 py-2 font-medium text-fg-3">{L.colReqStatus}</th>
                </tr>
              </thead>
              <tbody>
                {requests.map((r) => {
                  const c = r.count ?? 0;
                  const cTone = c >= 5 ? "text-rose-400" : c >= 2 ? "text-amber-400" : "text-fg-2";
                  return (
                    <tr
                      key={r.id}
                      onClick={() => setSelectedReq(r)}
                      className="border-t border-border hover:bg-hover cursor-pointer"
                    >
                      <td className="px-3 py-2 mono text-fg">{r.tool_name}</td>
                      <td className="px-3 py-2">
                        <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${FAIL_REASON_TONE[r.failure_reason ?? ""] ?? "bg-white/10 text-fg-3"}`}>
                          {r.failure_reason ?? "unknown"}
                        </span>
                      </td>
                      <td className="px-3 py-2 mono text-fg-3 truncate max-w-[200px]">
                        {r.attempted_capability_id ?? r.attempted_connector_id ?? "—"}
                      </td>
                      <td className={`px-3 py-2 text-right mono tabular-nums font-semibold ${cTone}`}>
                        {r.count ?? "—"}
                      </td>
                      <td className="px-3 py-2 mono text-fg-4 text-[11px]" title={fmtDate(r.last_seen_at ?? r.requested_at)}>
                        {fmtRel(r.last_seen_at ?? r.requested_at)}
                      </td>
                      <td className="px-3 py-2">
                        <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${REQ_STATUS_TONE[r.status] ?? "bg-white/10 text-fg-3"}`}>
                          {r.status}
                        </span>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}

        <p className="text-[10px] text-fg-4">
          {L.source}: <code>/api/v1/admin/capability-requests</code>. {L.reqCount(requests.length)}
        </p>
      </div>

      {/* Table 2 — Skill Usage Telemetry */}
      <div className="space-y-2">
        <div className="text-[11px] font-medium text-fg">{L.capMonUsageTitle}</div>

        {usageErr && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{usageErr}</p>}
        {usageLoading && <TableSkeleton cols={5} />}

        {!usageLoading && usageRows.length === 0 && (
          <EmptyState icon={List} title={L.capMonNoUsage} body={L.capMonNoUsageBody} />
        )}

        {!usageLoading && usageRows.length > 0 && (
          <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
            <table className="w-full text-xs">
              <thead className="bg-bg-3">
                <tr className="text-left">
                  <th className="px-3 py-2 font-medium text-fg-3">
                    <SortBtn col="skill" label={L.colSkillName} />
                  </th>
                  <th className="px-3 py-2 font-medium text-fg-3">{L.colOrigin}</th>
                  <th className="px-3 py-2 font-medium text-fg-3 text-right">
                    <SortBtn col="use_count" label={L.colInvocations} />
                  </th>
                  <th className="px-3 py-2 font-medium text-fg-3">{L.colLastUsed}</th>
                  <th className="px-3 py-2 font-medium text-fg-3 text-right">
                    <SortBtn col="days_idle" label={L.colDaysIdle} />
                  </th>
                </tr>
              </thead>
              <tbody>
                {sortedUsage.map((r, i) => {
                  const idle = r.days_idle ?? 0;
                  const idleTone =
                    idle >= 30
                      ? "bg-rose-500/15 text-rose-300"
                      : idle >= 14
                        ? "bg-amber-500/15 text-amber-300"
                        : "bg-zinc-500/15 text-zinc-400";
                  return (
                    <tr key={r.skill || i} className="border-t border-border hover:bg-hover">
                      <td className="px-3 py-2 mono text-fg">{r.skill}</td>
                      <td className="px-3 py-2">
                        <span className="px-1.5 py-0.5 rounded text-[10px] mono bg-white/10 text-fg-3">
                          {r.origin}
                        </span>
                      </td>
                      <td className="px-3 py-2 text-right mono tabular-nums text-fg-2">
                        {r.use_count}
                      </td>
                      <td className="px-3 py-2 mono text-fg-4 text-[11px]" title={fmtUnix(r.last_used_at)}>
                        {fmtRel(r.last_used_at)}
                      </td>
                      <td className="px-3 py-2 text-right">
                        <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${idleTone}`}>
                          {idle}d
                        </span>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}

        <p className="text-[10px] text-fg-4">
          {L.source}: <code>/api/v1/admin/curator/skill-usage</code>. {L.skillCount(usageRows.length)}
        </p>
      </div>

      {selectedReq && (
        <CapReqDrawer row={selectedReq} onClose={() => setSelectedReq(null)} lang={lang} />
      )}
    </div>
  );
}

// ─── Tab: Kanban ─────────────────────────────────────────────────────────────

function TaskCard({ task }: { task: Task }) {
  return (
    <div className="rounded-md border border-border bg-bg-2 p-3 space-y-1.5 text-xs">
      <div className="text-fg font-medium leading-snug truncate">
        {task.title ?? task.description ?? task.id}
      </div>
      {task.agent_id && (
        <div className="mono text-[10px] text-fg-4 truncate">{task.agent_id}</div>
      )}
      <div className="flex items-center justify-between text-[10px] mono text-fg-4">
        <span className="truncate">{(task.id ?? "").slice(0, 8)}</span>
        <span>{fmtDateShort(task.created_at)}</span>
      </div>
    </div>
  );
}

type KanbanColDef = { key: string; labelKey: keyof ReturnType<typeof dict>; dot: string; bg: string };

const COLUMN_DEFS: KanbanColDef[] = [
  { key: "pending", labelKey: "colPending", dot: "bg-amber-400",   bg: "bg-amber-500/10 border-amber-500/20"   },
  { key: "running", labelKey: "colRunning", dot: "bg-cyan-400",    bg: "bg-cyan-500/10 border-cyan-500/20"     },
  { key: "done",    labelKey: "colDone",    dot: "bg-emerald-400", bg: "bg-emerald-500/10 border-emerald-500/20" },
  { key: "failed",  labelKey: "colFailed",  dot: "bg-rose-400",    bg: "bg-rose-500/10 border-rose-500/20"     },
];

function KanbanColumn({
  label,
  dot,
  bg,
  tasks,
  noTasksLabel,
}: {
  label: string;
  dot: string;
  bg: string;
  tasks: Task[];
  noTasksLabel: string;
}) {
  return (
    <div className={`flex flex-col rounded-lg border ${bg} min-h-[320px]`}>
      <div className="flex items-center gap-2 px-3 pt-3 pb-2 shrink-0">
        <span className={`w-2 h-2 rounded-full shrink-0 ${dot}`} />
        <span className="text-[11px] font-medium uppercase tracking-wide text-fg">{label}</span>
        <span className="text-[10px] mono text-fg-4 tabular-nums">{tasks.length}</span>
      </div>
      <div className="flex-1 px-2 pb-2 space-y-2 overflow-y-auto">
        {tasks.length === 0 ? (
          <div className="rounded-md border border-dashed border-border/50 py-6 text-center text-[11px] text-fg-4">
            {noTasksLabel}
          </div>
        ) : (
          tasks.map((t) => <TaskCard key={t.id} task={t} />)
        )}
      </div>
    </div>
  );
}

function KanbanTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState<string | null>(null);

  const load = () => {
    setLoading(true);
    setErr(null);
    fetchTasks("all", 200).then(
      (t) => { setTasks(t); setLoading(false); },
      (e) => { setErr(`HTTP ${e?.status} ${e?.body ?? ""}`); setLoading(false); },
    );
  };

  useEffect(() => { load(); }, []);

  const grouped = useMemo(() => {
    const pending: Task[] = [];
    const running: Task[] = [];
    const done: Task[] = [];
    const failed: Task[] = [];
    const map: Record<string, Task[]> = { pending, running, done, failed };
    for (const task of tasks) {
      const s = (task.status ?? "pending").toLowerCase();
      if (map[s]) map[s].push(task);
      else pending.push(task);
    }
    return map;
  }, [tasks]);

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between text-xs">
        <p className="text-[11px] text-fg-4">
          {L.readOnly}{" "}
          <a
            href="/admin"
            className="text-accent hover:underline"
            onClick={() => sessionStorage.setItem("cyberclaw.admin.page", "kanban")}
          >
            {L.kanbanLink}
          </a>
        </p>
        <button
          onClick={load}
          className="text-[11px] px-2 py-1 rounded-md bg-bg-3 border border-border hover:bg-hover text-fg-3"
        >
          {L.refresh}
        </button>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}

      {loading ? (
        <div className="text-xs text-fg-4 py-8 text-center">{L.loading}</div>
      ) : (
        <div className="grid grid-cols-1 sm:grid-cols-2 xl:grid-cols-4 gap-3">
          {COLUMN_DEFS.map((col) => (
            <KanbanColumn
              key={col.key}
              label={L[col.labelKey] as string}
              dot={col.dot}
              bg={col.bg}
              tasks={grouped[col.key] ?? []}
              noTasksLabel={L.noKanbanTasks}
            />
          ))}
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/tasks?status=all&limit=200</code>. {L.kanbanTotal(tasks.length)}
      </p>
    </div>
  );
}

// ─── Page ────────────────────────────────────────────────────────────────────

export default function TasksPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [tab, setTab] = useState<Tab>("list");

  const tabs = (
    <>
      <TabBtn active={tab === "list"} onClick={() => setTab("list")}>{L.tabList}</TabBtn>
      <TabBtn active={tab === "executions"} onClick={() => setTab("executions")}>{L.tabExecutions}</TabBtn>
      <TabBtn active={tab === "persistent"} onClick={() => setTab("persistent")}>{L.tabPersistent}</TabBtn>
      <TabBtn active={tab === "capmon"} onClick={() => setTab("capmon")}>{L.tabCapMon}</TabBtn>
      <TabBtn active={tab === "kanban"} onClick={() => setTab("kanban")}>{L.tabKanban}</TabBtn>
    </>
  );

  return (
    <section className="space-y-4">
      <PageHeader title={L.pageTitle} tabs={tabs} />
      {tab === "list" && <ListTab lang={lang} />}
      {tab === "executions" && <ExecutionsTab lang={lang} />}
      {tab === "persistent" && <PersistentTab lang={lang} />}
      {tab === "capmon" && <CapMonTab lang={lang} />}
      {tab === "kanban" && <KanbanTab lang={lang} />}
    </section>
  );
}
