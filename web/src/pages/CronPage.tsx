// CronPage — full CRUD for scheduled cron jobs.
// Backend: GET/POST/PATCH/DELETE/run-now /api/v1/cron

import { useEffect, useRef, useState } from "react";
import { useModalA11y } from "@/components/useModalA11y";
import Modal from "@/components/Modal";
import {
  type CronJob,
  createCronJob,
  deleteCronJob,
  fetchCronJobs,
  runCronJobNow,
  updateCronJob,
} from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import { Clock } from "@/components/icons";
import PageHeader from "@/components/PageHeader";

const STATUS_TONE: Record<string, string> = {
  ok: "bg-emerald-500/15 text-emerald-300",
  failed: "bg-rose-500/15 text-rose-300",
  running: "bg-amber-500/15 text-amber-300",
};

function relTime(iso?: string, lang: Lang = "en"): string {
  if (!iso) return "—";
  const diff = Date.now() - new Date(iso).getTime();
  if (isNaN(diff)) return iso;
  const abs = Math.abs(diff);
  const future = diff < 0;
  const zh = lang === "zh-CN";
  const fmt = (n: number, unit: "m" | "h" | "d"): string => {
    if (zh) {
      const u = unit === "m" ? "分钟" : unit === "h" ? "小时" : "天";
      return future ? `${n} ${u}后` : `${n} ${u}前`;
    }
    return future ? `in ${n}${unit}` : `${n}${unit} ago`;
  };
  if (abs < 60_000) return zh ? (future ? "<1 分钟后" : "<1 分钟前") : (future ? "in <1m" : "<1m ago");
  if (abs < 3_600_000) return fmt(Math.round(abs / 60_000), "m");
  if (abs < 86_400_000) return fmt(Math.round(abs / 3_600_000), "h");
  return fmt(Math.round(abs / 86_400_000), "d");
}

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "定时任务",
        totalFmt: (n: number) => `共 ${n} 个`,
        newJob: "+ 新建任务",
        search: "搜索",
        searchPlaceholder: "名称 / cron 表达式",
        noJobs: "暂无定时任务",
        noJobsBody: '点击 "+ 新建任务" 创建。',
        noJobsFilter: "无匹配的任务",
        colName: "名称",
        colSchedule: "cron 表达式",
        colEnabled: "已启用",
        colLastRun: "最后运行",
        colNextRun: "下次运行",
        colStatus: "状态",
        colActions: "操作",
        pause: "暂停",
        resume: "恢复",
        run: "立即运行",
        del: "删除",
        on: "开",
        off: "关",
        // Dialog
        dialogTitle: "新建定时任务",
        dialogEditTitle: "编辑定时任务",
        edit: "编辑",
        update: "保存",
        updating: "保存中…",
        name: "名称",
        schedule: "cron 表达式",
        scheduleHint: "(6 字段: 秒 分 时 日 月 周)",
        actionType: "动作类型",
        payload: "参数",
        payloadPlaceholder: "消息 / URL / …",
        enabled: "已启用",
        cancel: "取消",
        create: "创建",
        creating: "创建中…",
        nameRequired: "名称不能为空",
        scheduleRequired: "cron 表达式不能为空",
        deleteConfirmFmt: (name: string) => `确定删除定时任务 "${name}"？`,
        source: "来源",
        schedulerNote: "调度器每 30 秒 tick 一次。聊天/webhook 动作在 v1 仅记日志。",
      }
    : {
        title: "Cron Jobs",
        totalFmt: (n: number) => `${n} total`,
        newJob: "+ New job",
        search: "search",
        searchPlaceholder: "name / schedule",
        noJobs: "No scheduled jobs yet",
        noJobsBody: 'Click "+ New job" to create one.',
        noJobsFilter: "No jobs match the current filter",
        colName: "name",
        colSchedule: "schedule",
        colEnabled: "enabled",
        colLastRun: "last_run",
        colNextRun: "next_run",
        colStatus: "status",
        colActions: "actions",
        pause: "pause",
        resume: "resume",
        run: "run",
        del: "del",
        on: "on",
        off: "off",
        // Dialog
        dialogTitle: "New cron job",
        dialogEditTitle: "Edit cron job",
        edit: "edit",
        update: "Save",
        updating: "Saving…",
        name: "name",
        schedule: "schedule",
        scheduleHint: "(6-field cron: sec min hr dom mon dow)",
        actionType: "action type",
        payload: "payload",
        payloadPlaceholder: "message / URL / …",
        enabled: "enabled",
        cancel: "Cancel",
        create: "Create",
        creating: "Creating…",
        nameRequired: "Name is required",
        scheduleRequired: "Schedule is required",
        deleteConfirmFmt: (name: string) => `Delete cron job "${name}"?`,
        source: "Source",
        schedulerNote: "Scheduler ticks every 30 s. chat/webhook actions log only in v1.",
      };
}

// ---------------------------------------------------------------------------
// New-job dialog
// ---------------------------------------------------------------------------

type NewJobForm = {
  name: string;
  schedule: string;
  action_type: string;
  action_payload: string;
  enabled: boolean;
};

function JobDialog({
  lang,
  editing,
  onClose,
  onSaved,
}: {
  lang: Lang;
  editing: CronJob | null;
  onClose: () => void;
  onSaved: (job: CronJob) => void;
}) {
  const L = dict(lang);
  const isEdit = !!editing;
  const dialogRef = useRef<HTMLDivElement>(null);
  useModalA11y(dialogRef, true, onClose);
  const [form, setForm] = useState<NewJobForm>(
    editing
      ? {
          name: editing.name,
          schedule: editing.schedule,
          action_type: editing.action_type ?? "echo",
          action_payload: editing.action_payload ?? "",
          enabled: editing.enabled,
        }
      : {
          name: "",
          schedule: "0 */5 * * * *",
          action_type: "echo",
          action_payload: "",
          enabled: true,
        },
  );
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const set = (k: keyof NewJobForm, v: string | boolean) =>
    setForm((f) => ({ ...f, [k]: v }));

  async function submit() {
    if (!form.name.trim()) { setErr(L.nameRequired); return; }
    if (!form.schedule.trim()) { setErr(L.scheduleRequired); return; }
    setBusy(true);
    setErr(null);
    try {
      const payload = {
        name: form.name.trim(),
        schedule: form.schedule.trim(),
        action_type: form.action_type,
        action_payload: form.action_payload.trim(),
        enabled: form.enabled,
      };
      const job = isEdit
        ? await updateCronJob(editing!.id, payload)
        : await createCronJob(payload);
      onSaved(job);
    } catch (e: unknown) {
      const apiErr = e as { body?: string; status?: number };
      setErr(apiErr?.body ?? (isEdit ? "Failed to update job" : "Failed to create job"));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60" onClick={onClose}>
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label={isEdit ? L.dialogEditTitle : L.dialogTitle}
        tabIndex={-1}
        className="bg-bg-2 border border-border rounded-xl p-5 w-full max-w-md space-y-3 shadow-xl outline-none"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold">{isEdit ? L.dialogEditTitle : L.dialogTitle}</h2>

        {err && <p className="text-xs text-rose-400 px-2 py-1 bg-rose-500/10 rounded">{err}</p>}

        <label className="flex flex-col gap-0.5 text-xs">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.name}</span>
          <input
            className="px-2 py-1.5 rounded bg-bg-3 border border-border outline-none focus-ring"
            value={form.name}
            onChange={(e) => set("name", e.target.value)}
            placeholder="daily-digest"
          />
        </label>

        <label className="flex flex-col gap-0.5 text-xs">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">
            {L.schedule}{" "}
            <span className="text-fg-5 normal-case font-normal">{L.scheduleHint}</span>
          </span>
          <input
            className="px-2 py-1.5 rounded bg-bg-3 border border-border outline-none focus-ring mono"
            value={form.schedule}
            onChange={(e) => set("schedule", e.target.value)}
            placeholder="0 */5 * * * *"
          />
        </label>

        <label className="flex flex-col gap-0.5 text-xs">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.actionType}</span>
          <select
            className="px-2 py-1.5 rounded bg-bg-3 border border-border outline-none focus-ring"
            value={form.action_type}
            onChange={(e) => set("action_type", e.target.value)}
          >
            <option value="echo">echo (log only)</option>
            <option value="chat">chat (log only in v1)</option>
            <option value="webhook">webhook (log only in v1)</option>
          </select>
        </label>

        <label className="flex flex-col gap-0.5 text-xs">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.payload}</span>
          <input
            className="px-2 py-1.5 rounded bg-bg-3 border border-border outline-none focus-ring"
            value={form.action_payload}
            onChange={(e) => set("action_payload", e.target.value)}
            placeholder={L.payloadPlaceholder}
          />
        </label>

        <label className="flex items-center gap-2 text-xs cursor-pointer">
          <input
            type="checkbox"
            checked={form.enabled}
            onChange={(e) => set("enabled", e.target.checked)}
          />
          <span>{L.enabled}</span>
        </label>

        <div className="flex justify-end gap-2 pt-1">
          <button
            className="px-3 py-1.5 text-xs rounded border border-border hover:bg-hover"
            onClick={onClose}
            disabled={busy}
          >
            {L.cancel}
          </button>
          <button
            className="px-3 py-1.5 text-xs rounded bg-accent text-accent-fg hover:opacity-90 disabled:opacity-50"
            onClick={submit}
            disabled={busy}
          >
            {busy ? (isEdit ? L.updating : L.creating) : (isEdit ? L.update : L.create)}
          </button>
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

export default function CronPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [jobs, setJobs] = useState<CronJob[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [q, setQ] = useState("");
  const [showNew, setShowNew] = useState(false);
  const [editing, setEditing] = useState<CronJob | null>(null);
  const [busy, setBusy] = useState<string | null>(null); // job id being actioned

  async function refresh() {
    try {
      const j = await fetchCronJobs();
      setJobs(j);
      setErr(null);
    } catch (e: unknown) {
      const apiErr = e as { status?: number; body?: string };
      setErr(`HTTP ${apiErr?.status} ${apiErr?.body ?? ""}`);
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
  }, []);

  const filtered = jobs.filter(
    (j) =>
      !q ||
      j.name.toLowerCase().includes(q.toLowerCase()) ||
      j.schedule.includes(q),
  );

  // P1 fix (silent-failure hunt + designer):
  // - replace bare `confirm()` with state-driven Modal (themed,
  //   keyboard-accessible, doesn't get silenced in PWA/Electron).
  // - replace 3 `catch { /* ignore */ }` blocks with `setErr` so
  //   delete / run-now / pause-resume API failures are visible.
  const [pendingDelete, setPendingDelete] = useState<CronJob | null>(null);
  const surfaceErr = (label: string, e: unknown) => {
    const msg = e instanceof Error ? e.message : String(e);
    setErr(`${label}: ${msg}`);
  };

  async function toggleEnabled(job: CronJob) {
    setBusy(job.id);
    try {
      const updated = await updateCronJob(job.id, { enabled: !job.enabled });
      setJobs((prev) => prev.map((j) => (j.id === updated.id ? updated : j)));
    } catch (e) {
      surfaceErr(`Failed to ${job.enabled ? "pause" : "resume"} job`, e);
    } finally {
      setBusy(null);
    }
  }

  async function runNow(job: CronJob) {
    setBusy(job.id);
    try {
      const updated = await runCronJobNow(job.id);
      setJobs((prev) => prev.map((j) => (j.id === updated.id ? updated : j)));
    } catch (e) {
      surfaceErr("Failed to trigger job run", e);
    } finally {
      setBusy(null);
    }
  }

  function remove(job: CronJob) {
    setPendingDelete(job);
  }

  async function confirmRemove() {
    const job = pendingDelete;
    if (!job) return;
    setPendingDelete(null);
    setBusy(job.id);
    try {
      await deleteCronJob(job.id);
      setJobs((prev) => prev.filter((j) => j.id !== job.id));
    } catch (e) {
      surfaceErr("Failed to delete job", e);
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="space-y-4">
      {(showNew || editing) && (
        <JobDialog
          lang={lang}
          editing={editing}
          onClose={() => { setShowNew(false); setEditing(null); }}
          onSaved={(job) => {
            setJobs((prev) => {
              const i = prev.findIndex((p) => p.id === job.id);
              return i >= 0
                ? prev.map((p) => (p.id === job.id ? job : p))
                : [...prev, job];
            });
            setShowNew(false);
            setEditing(null);
          }}
        />
      )}

      <PageHeader
        title={L.title}
        subtitle={L.totalFmt(jobs.length)}
        actions={
          <button
            className="px-3 py-1.5 rounded-md bg-accent text-accent-fg text-[11px] hover:opacity-90"
            onClick={() => setShowNew(true)}
          >
            {L.newJob}
          </button>
        }
      />

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
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}

      {loading && <TableSkeleton cols={7} />}

      {!loading && filtered.length === 0 && (
        <EmptyState
          icon={Clock}
          title={jobs.length === 0 ? L.noJobs : L.noJobsFilter}
          body={jobs.length === 0 ? L.noJobsBody : undefined}
        />
      )}

      {!loading && filtered.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs table-fixed">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3 w-[20%]">{L.colName}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[16%] mono">{L.colSchedule}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[9%]">{L.colEnabled}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[15%]">{L.colLastRun}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[15%]">{L.colNextRun}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[11%]">{L.colStatus}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[14%]">{L.colActions}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((j) => (
                <tr key={j.id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 font-medium truncate max-w-0 overflow-hidden">
                    {j.name}
                  </td>
                  <td className="px-3 py-2 mono text-fg-3 truncate max-w-0 overflow-hidden">
                    {j.schedule}
                  </td>
                  <td className="px-3 py-2">
                    <span
                      className={`px-1.5 py-0.5 rounded text-[10px] mono ${j.enabled ? "bg-emerald-500/15 text-emerald-300" : "bg-white/10 text-fg-4"}`}
                    >
                      {j.enabled ? L.on : L.off}
                    </span>
                  </td>
                  <td className="px-3 py-2 text-fg-3">{relTime(j.last_run_at, lang)}</td>
                  <td className="px-3 py-2 text-fg-3">{relTime(j.next_run_at, lang)}</td>
                  <td className="px-3 py-2">
                    {j.last_status && (
                      <span
                        className={`px-1.5 py-0.5 rounded text-[10px] mono ${STATUS_TONE[j.last_status] ?? "bg-white/10 text-fg-3"}`}
                      >
                        {j.last_status}
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2">
                    <div className="flex items-center gap-1.5">
                      <button
                        title={j.enabled ? L.pause : L.resume}
                        disabled={busy === j.id}
                        onClick={() => toggleEnabled(j)}
                        className="px-1.5 py-0.5 text-[10px] rounded border border-border hover:bg-hover disabled:opacity-40"
                      >
                        {j.enabled ? L.pause : L.resume}
                      </button>
                      <button
                        title={L.edit}
                        onClick={() => setEditing(j)}
                        className="px-1.5 py-0.5 text-[10px] rounded border border-border hover:bg-hover"
                      >
                        {L.edit}
                      </button>
                      <button
                        title={L.run}
                        disabled={busy === j.id}
                        onClick={() => runNow(j)}
                        className="px-1.5 py-0.5 text-[10px] rounded border border-border hover:bg-hover disabled:opacity-40"
                      >
                        {L.run}
                      </button>
                      <button
                        title={L.del}
                        disabled={busy === j.id}
                        onClick={() => remove(j)}
                        className="px-1.5 py-0.5 text-[10px] rounded border border-rose-500/30 text-rose-400 hover:bg-rose-500/10 disabled:opacity-40"
                      >
                        {L.del}
                      </button>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/cron</code>. {L.schedulerNote}
      </p>

      <Modal
        open={!!pendingDelete}
        onClose={() => setPendingDelete(null)}
        title={L.del}
        width={420}
        footer={
          <>
            <button
              onClick={() => setPendingDelete(null)}
              className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg hover:bg-hover"
            >
              {L.cancel}
            </button>
            <button
              onClick={confirmRemove}
              className="px-3 h-8 rounded-md bg-rose-600 text-white text-xs font-medium hover:opacity-90"
            >
              {L.del}
            </button>
          </>
        }
      >
        <p className="text-sm text-fg-2">
          {pendingDelete ? L.deleteConfirmFmt(pendingDelete.name) : ""}
        </p>
      </Modal>
    </section>
  );
}
