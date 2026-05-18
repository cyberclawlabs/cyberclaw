// LearningPage — 顶部 Curator KPI 行 + 3 Tab: 每日摘要 / 组织记忆 / 进化时间线

import { useCallback, useEffect, useState } from "react";
import {
  type CuratorStatusResponse,
  type DailyDigestResponse,
  type EvolutionCycle,
  type OrgMemoryEntry,
  fetchCuratorStatus,
  fetchDailyDigest,
  fetchEvolutionTimeline,
  fetchOrgMemory,
  runCuratorNow,
} from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import Modal from "@/components/Modal";
import PageHeader from "@/components/PageHeader";
import { useToast } from "@/components/ToastBar";
import { Activity, Brain, Clock, FileText } from "@/components/icons";

// ─── Helpers ─────────────────────────────────────────────────────────────────

function fmtUnix(ts: number | null | undefined): string {
  if (!ts) return "—";
  try {
    return new Date(ts * 1000).toLocaleString();
  } catch {
    return "—";
  }
}

function fmtRelativeSecs(ts: number | null | undefined): string {
  if (!ts) return "—";
  try {
    const ms = Date.now() - ts * 1000;
    const sec = Math.floor(Math.abs(ms) / 1000);
    if (sec < 60) return `${sec}s`;
    const min = Math.floor(sec / 60);
    if (min < 60) return `${min}m`;
    const h = Math.floor(min / 60);
    if (h < 48) return `${h}h`;
    return `${Math.floor(h / 24)}d`;
  } catch {
    return "—";
  }
}

// ─── i18n ────────────────────────────────────────────────────────────────────

function dict(lang: Lang) {
  const zh = lang === "zh-CN";
  return {
    title: zh ? "学习与 Curator" : "Learning & Curator",
    // KPI
    lastRun: zh ? "上次运行" : "Last run",
    nextRun: zh ? "下次运行" : "Next run",
    totalRuns: zh ? "累计运行" : "Total runs",
    runNow: zh ? "手动触发" : "Run now",
    confirmTitle: zh ? "确认手动触发 Curator？" : "Trigger Curator run?",
    confirmBody: zh
      ? "将立即执行一次 Curator 扫描，结果写入审计日志。"
      : "A Curator pass will run immediately and results will be written to the audit log.",
    confirmCancel: zh ? "取消" : "Cancel",
    confirmGo: zh ? "立即运行" : "Run",
    runOk: zh ? "Curator 已触发" : "Curator triggered",
    runFailed: zh ? "触发失败" : "Trigger failed",
    ago: zh ? "前" : "ago",
    in: zh ? "后" : "in",
    // Tabs
    tabDigest: zh ? "每日摘要" : "Daily Digest",
    tabMemory: zh ? "组织记忆" : "Org Memory",
    tabEvolution: zh ? "进化时间线" : "Evolution Timeline",
    // Arch banner
    archTitle: zh
      ? "cyberclaw 的 Learning 与 Curator 是平台自我进化能力"
      : "cyberclaw Learning & Curator — platform self-evolution",
    archBody: zh
      ? "Curator 周期性扫描 Skill 使用、用户反馈、失败案例，自动归纳 Daily Digest（运营摘要），沉淀 Org Memory（团队知识），跟踪 Evolution Timeline（平台能力进化）。这是 cyberclaw 把「持续学习」做成平台一等公民的体现。"
      : "Curator periodically scans Skill usage, user feedback and failure cases, compiles the Daily Digest, consolidates Org Memory (team knowledge), and tracks the Evolution Timeline. This is how cyberclaw makes continuous learning a first-class platform capability.",
    // Digest tab
    digestDate: zh ? "日期" : "Date",
    digestToday: zh ? "今天" : "Today",
    digestExec: zh ? "执行" : "Executions",
    digestApprovals: zh ? "审批" : "Approvals",
    digestLearning: zh ? "学习条目" : "Learning entries",
    digestIncidents: zh ? "安全事件" : "Incidents",
    digestHighlights: zh ? "亮点" : "Highlights",
    digestEmpty: zh ? "今日暂无摘要数据" : "No digest data for this date",
    // Org memory tab
    orgSearch: zh ? "搜索记忆…" : "Search memory…",
    orgEmpty: zh ? "暂无组织记忆条目" : "No org memory entries",
    orgDetail: zh ? "记忆详情" : "Memory detail",
    orgKind: zh ? "类型" : "Kind",
    orgCreated: zh ? "创建时间" : "Created",
    orgContent: zh ? "内容" : "Content",
    // Evolution tab
    evoEmpty: zh ? "暂无进化记录（最近 30 天）" : "No evolution cycles in the last 30 days",
    evoTried: zh ? "运行次数" : "Tried",
    evoAccepted: zh ? "收敛" : "Converged",
    evoRolledBack: zh ? "回滚" : "Rolled back",
    evoPending: zh ? "待观察" : "Pending",
    source: zh ? "来源" : "Source",
    shown: (n: number) => zh ? `已展示 ${n} 条` : `${n} shown`,
  };
}

// ─── Shared tab button ────────────────────────────────────────────────────────

type Tab = "digest" | "memory" | "evolution";

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

// ─── KPI Card ─────────────────────────────────────────────────────────────────

function KpiCard({
  label,
  value,
  sub,
  icon: IconComp,
}: {
  label: string;
  value: React.ReactNode;
  sub?: string;
  icon?: React.ComponentType<{ size?: number; className?: string }>;
}) {
  return (
    <div className="bg-bg-2 border border-border rounded-lg px-4 py-3 flex items-start gap-3">
      {IconComp && (
        <div className="mt-0.5 text-fg-4 shrink-0">
          <IconComp size={16} />
        </div>
      )}
      <div className="min-w-0">
        <div className="text-[10px] text-fg-3 uppercase tracking-wide font-medium">{label}</div>
        <div className="text-[15px] font-semibold text-fg mt-0.5 mono">{value}</div>
        {sub && <div className="text-[10px] text-fg-4 mt-0.5 mono">{sub}</div>}
      </div>
    </div>
  );
}

// ─── Curator KPI Row ──────────────────────────────────────────────────────────

function CuratorKpiRow({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const toast = useToast();
  const [status, setStatus] = useState<CuratorStatusResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [confirm, setConfirm] = useState(false);
  const [running, setRunning] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    fetchCuratorStatus().then(
      (s) => { if (!cancelled) { setStatus(s); setLoading(false); } },
      () => { if (!cancelled) setLoading(false); },
    );
    return () => { cancelled = true; };
  }, []);

  const handleRunNow = useCallback(async () => {
    setRunning(true);
    try {
      const resp = await runCuratorNow();
      toast({
        msg: `${L.runOk} — run_id: ${resp.run_id}`,
        tone: "success",
      });
      // Refresh status
      const s = await fetchCuratorStatus();
      setStatus(s);
    } catch (e: unknown) {
      const errMsg = e && typeof e === "object" && "body" in e ? String((e as { body: unknown }).body) : String(e);
      toast({ msg: `${L.runFailed}: ${errMsg}`, tone: "error" });
    } finally {
      setRunning(false);
      setConfirm(false);
    }
  }, [L.runOk, L.runFailed, toast]);

  const lastRunSub = status?.last_run_at
    ? `${fmtRelativeSecs(status.last_run_at)} ${L.ago}`
    : undefined;
  const nextRunSub = status?.next_run_at
    ? `${L.in} ${fmtRelativeSecs(status.next_run_at)}`
    : undefined;

  return (
    <>
      <div className="flex flex-wrap items-center gap-3">
        <div className="grid grid-cols-3 gap-3 flex-1 min-w-0">
          <KpiCard
            label={L.lastRun}
            value={loading ? <span className="text-fg-4 text-[12px]">…</span> : fmtUnix(status?.last_run_at)}
            sub={lastRunSub}
            icon={Clock}
          />
          <KpiCard
            label={L.nextRun}
            value={loading ? <span className="text-fg-4 text-[12px]">…</span> : fmtUnix(status?.next_run_at)}
            sub={nextRunSub}
            icon={Activity}
          />
          <KpiCard
            label={L.totalRuns}
            value={loading ? <span className="text-fg-4 text-[12px]">…</span> : String(status?.total_runs ?? 0)}
            icon={Brain}
          />
        </div>
        <button
          onClick={() => setConfirm(true)}
          disabled={running}
          className="h-9 px-4 rounded-md bg-accent text-white text-[12px] font-medium disabled:opacity-50 hover:opacity-90 transition-opacity shrink-0 flex items-center gap-1.5"
        >
          {running ? "…" : L.runNow}
        </button>
      </div>

      <Modal
        open={confirm}
        onClose={() => setConfirm(false)}
        title={L.confirmTitle}
        footer={
          <>
            <button
              onClick={() => setConfirm(false)}
              className="px-3 py-1.5 rounded-md border border-border text-[12px] text-fg-3 hover:text-fg"
            >
              {L.confirmCancel}
            </button>
            <button
              onClick={handleRunNow}
              disabled={running}
              className="px-3 py-1.5 rounded-md bg-accent text-white text-[12px] font-medium disabled:opacity-50 hover:opacity-90"
            >
              {L.confirmGo}
            </button>
          </>
        }
      >
        <p className="text-[13px] text-fg-2">{L.confirmBody}</p>
      </Modal>
    </>
  );
}

// ─── Architecture Banner ──────────────────────────────────────────────────────

function ArchBanner({ lang }: { lang: Lang }) {
  const L = dict(lang);
  return (
    <div className="rounded-lg border border-accent/30 bg-accent/5 px-4 py-3 text-[12px] text-fg-2 leading-relaxed">
      <span className="font-medium text-accent">{L.archTitle} — </span>
      {L.archBody}
    </div>
  );
}

// ─── Tab A: Daily Digest ──────────────────────────────────────────────────────

const TAG_TONE: Record<string, string> = {
  learned: "bg-violet-500/15 text-violet-300",
  incident: "bg-rose-500/15 text-rose-300",
  mutation: "bg-accent/15 text-accent",
  blocked: "bg-amber-500/15 text-amber-300",
  "auto-approved": "bg-emerald-500/15 text-emerald-300",
};

function DigestKpiCard({ label, value, delta, tone }: { label: string; value: number; delta: string; tone?: string }) {
  const color = tone === "rose" ? "text-rose-300" : "text-fg";
  return (
    <div className="bg-bg-2 border border-border rounded-lg px-4 py-3">
      <div className="text-[10px] text-fg-3 uppercase tracking-wide font-medium">{label}</div>
      <div className={`text-[20px] font-semibold mono mt-0.5 ${color}`}>{value}</div>
      {delta && <div className="text-[10px] text-fg-4 mono mt-0.5">{delta}</div>}
    </div>
  );
}

function DailyDigestTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [date, setDate] = useState(() => new Date().toISOString().slice(0, 10));
  const [data, setData] = useState<DailyDigestResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const today = new Date().toISOString().slice(0, 10);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    fetchDailyDigest(date).then(
      (d) => { if (!cancelled) { setData(d); setLoading(false); } },
      (e: { status?: number; body?: string }) => {
        if (cancelled) return;
        setErr(`HTTP ${e?.status ?? "?"} ${e?.body ?? ""}`);
        setLoading(false);
      },
    );
    return () => { cancelled = true; };
  }, [date]);

  const adjustDate = (delta: number) => {
    const d = new Date(date);
    d.setDate(d.getDate() + delta);
    setDate(d.toISOString().slice(0, 10));
  };

  return (
    <div className="space-y-4">
      {/* Date navigation */}
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-[11px] text-fg-3">{L.digestDate}</span>
        <button
          onClick={() => adjustDate(-1)}
          className="h-7 w-7 flex items-center justify-center rounded border border-border text-fg-3 hover:text-fg hover:bg-hover"
          aria-label="previous day"
        >
          ‹
        </button>
        <input
          type="date"
          value={date}
          max={today}
          onChange={(e) => setDate(e.target.value)}
          className="h-7 px-2 bg-bg-3 border border-border rounded text-[12px] mono text-fg outline-none focus-ring"
        />
        <button
          onClick={() => adjustDate(1)}
          disabled={date >= today}
          className="h-7 w-7 flex items-center justify-center rounded border border-border text-fg-3 hover:text-fg hover:bg-hover disabled:opacity-30"
          aria-label="next day"
        >
          ›
        </button>
        <button
          onClick={() => setDate(today)}
          className="h-7 px-2.5 text-[11px] mono rounded border border-border text-fg-3 hover:text-fg hover:bg-hover"
        >
          {L.digestToday}
        </button>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={4} />}

      {!loading && !data && !err && (
        <EmptyState icon={FileText} title={L.digestEmpty} />
      )}

      {!loading && data && (
        <div className="space-y-4">
          {/* KPI stats */}
          <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
            <DigestKpiCard label={L.digestExec} value={data.stats.executions.value} delta={data.stats.executions.delta} />
            <DigestKpiCard label={L.digestApprovals} value={data.stats.approvals.value} delta={data.stats.approvals.delta} />
            <DigestKpiCard label={L.digestLearning} value={data.stats.learning_entries.value} delta={data.stats.learning_entries.delta} />
            <DigestKpiCard label={L.digestIncidents} value={data.stats.incidents.value} delta={data.stats.incidents.delta} tone="rose" />
          </div>

          {/* Highlights */}
          {data.highlights.length > 0 && (
            <div className="bg-bg-2 border border-border rounded-lg px-4 py-3 space-y-1.5">
              <div className="text-[10px] text-fg-3 uppercase tracking-wide font-medium mb-2">{L.digestHighlights}</div>
              {data.highlights.map((h, i) => (
                <div key={i} className="flex items-start gap-2 text-[12px]">
                  <span className="text-[14px] leading-none mt-0.5">{h.icon}</span>
                  <span className="text-fg-2 leading-snug">{h.label}</span>
                </div>
              ))}
            </div>
          )}

          {/* Feed */}
          {data.feed.length === 0 && (
            <EmptyState icon={FileText} title={L.digestEmpty} />
          )}
          {data.feed.map((bucket, bi) => (
            <div key={bi}>
              <div className="text-[10px] mono text-fg-4 uppercase tracking-wider mb-2 flex items-center gap-2">
                <span>{bucket.bucket}</span>
                <span className="flex-1 h-px bg-border" />
              </div>
              <div className="space-y-2">
                {bucket.entries.map((entry, ei) => (
                  <div key={ei} className="bg-bg-2 border border-border rounded-lg px-3 py-2.5">
                    <div className="flex items-center gap-2 mb-1">
                      <span className="text-[15px] leading-none">{entry.avatar}</span>
                      <span className="text-[12px] font-medium">{entry.name}</span>
                      <span className="text-[10px] mono text-fg-4">{entry.ts}</span>
                      <div className="flex gap-1 ml-auto flex-wrap">
                        {entry.tags.map((tag) => (
                          <span key={tag} className={`px-1.5 py-0.5 rounded text-[10px] mono ${TAG_TONE[tag] ?? "bg-white/10 text-fg-3"}`}>
                            {tag}
                          </span>
                        ))}
                      </div>
                    </div>
                    <div className="text-[12px] text-fg-2 leading-relaxed">{entry.body}</div>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ─── Tab B: Org Memory ────────────────────────────────────────────────────────

const KIND_AVATAR: Record<string, string> = {
  daily_digest: "📋",
  reflection: "◆",
  rule: "⚖",
  free_text: "◦",
};

const KIND_LABEL: Record<string, string> = {
  daily_digest: "Daily Digest",
  reflection: "Reflection",
  rule: "Rule",
  free_text: "Operator",
};

function OrgMemoryTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [entries, setEntries] = useState<OrgMemoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState<string | null>(null);
  const [q, setQ] = useState("");
  const [selected, setSelected] = useState<OrgMemoryEntry | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    fetchOrgMemory(100).then(
      (e) => { if (!cancelled) { setEntries(e); setLoading(false); } },
      (e: { status?: number; body?: string }) => {
        if (cancelled) return;
        setErr(`HTTP ${e?.status ?? "?"} ${e?.body ?? ""}`);
        setLoading(false);
      },
    );
    return () => { cancelled = true; };
  }, []);

  const filtered = entries.filter((e) => {
    if (!q) return true;
    const ql = q.toLowerCase();
    return (
      e.content.toLowerCase().includes(ql) ||
      e.kind.toLowerCase().includes(ql) ||
      e.id.toLowerCase().includes(ql)
    );
  });

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-2">
        <input
          value={q}
          onChange={(ev) => setQ(ev.target.value)}
          placeholder={L.orgSearch}
          className="h-8 px-3 rounded-md bg-bg-3 border border-border outline-none focus-ring text-[12px] w-72"
        />
        <span className="text-[10px] text-fg-4 ml-auto">{L.shown(filtered.length)}</span>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={3} />}

      {!loading && filtered.length === 0 && (
        <EmptyState icon={Brain} title={L.orgEmpty} />
      )}

      {!loading && filtered.length > 0 && (
        <div className="space-y-1.5">
          {filtered.map((entry) => (
            <button
              key={entry.id}
              onClick={() => setSelected(entry)}
              className={[
                "w-full text-left rounded-lg border px-3 py-2.5 transition-colors",
                selected?.id === entry.id
                  ? "bg-bg-3 border-accent/40"
                  : "bg-bg-2 border-border hover:bg-hover",
              ].join(" ")}
            >
              <div className="flex items-center gap-2 mb-1">
                <span className="text-[14px]">{KIND_AVATAR[entry.kind] ?? "◦"}</span>
                <span className="text-[11px] text-fg-3">{KIND_LABEL[entry.kind] ?? entry.kind}</span>
                <span className="ml-auto text-[10px] mono text-fg-4">
                  {entry.created_at.slice(0, 16).replace("T", " ")}
                </span>
              </div>
              <div className="text-[12px] text-fg-2 leading-snug line-clamp-2">{entry.content}</div>
            </button>
          ))}
        </div>
      )}

      {/* Detail drawer (modal) */}
      <Modal
        open={!!selected}
        onClose={() => setSelected(null)}
        title={L.orgDetail}
        width={520}
      >
        {selected && (
          <div className="space-y-3 text-[13px]">
            <div>
              <div className="text-[10px] text-fg-3 uppercase tracking-wide font-medium mb-1">{L.orgContent}</div>
              <p className="text-fg-2 leading-relaxed whitespace-pre-wrap">{selected.content}</p>
            </div>
            <div className="grid grid-cols-2 gap-2 text-[12px]">
              <div>
                <span className="text-fg-4">{L.orgKind}: </span>
                <span className="text-fg-2">{KIND_LABEL[selected.kind] ?? selected.kind}</span>
              </div>
              <div>
                <span className="text-fg-4">{L.orgCreated}: </span>
                <span className="mono text-fg-3">{selected.created_at.slice(0, 16).replace("T", " ")}</span>
              </div>
            </div>
            <div className="bg-bg-3 rounded px-2.5 py-2 text-[10px] mono text-fg-4 break-all">
              id: {selected.id}
            </div>
          </div>
        )}
      </Modal>
    </div>
  );
}

// ─── Tab C: Evolution Timeline ────────────────────────────────────────────────

const OUTCOME_DOT: Record<string, string> = {
  converged: "bg-emerald-500",
  max_iterations: "bg-amber-400",
  failed: "bg-rose-500",
};

const OUTCOME_BADGE: Record<string, string> = {
  converged: "bg-emerald-500/15 text-emerald-300",
  max_iterations: "bg-amber-500/15 text-amber-300",
  failed: "bg-rose-500/15 text-rose-300",
};

function EvolutionTimelineTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [cycles, setCycles] = useState<EvolutionCycle[]>([]);
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    fetchEvolutionTimeline(30).then(
      (c) => { if (!cancelled) { setCycles(c); setLoading(false); } },
      (e: { status?: number; body?: string }) => {
        if (cancelled) return;
        setErr(`HTTP ${e?.status ?? "?"} ${e?.body ?? ""}`);
        setLoading(false);
      },
    );
    return () => { cancelled = true; };
  }, []);

  const stats = {
    tried: cycles.length,
    converged: cycles.filter((c) => c.outcome === "converged").length,
    failed: cycles.filter((c) => c.outcome === "failed").length,
    pending: cycles.filter((c) => c.outcome === "max_iterations").length,
  };

  return (
    <div className="space-y-4">
      {/* Stats row */}
      <div className="grid grid-cols-4 gap-3">
        <KpiCard label={L.evoTried} value={String(stats.tried)} />
        <KpiCard label={L.evoAccepted} value={String(stats.converged)} />
        <KpiCard label={L.evoRolledBack} value={String(stats.failed)} />
        <KpiCard label={L.evoPending} value={String(stats.pending)} />
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={3} />}

      {!loading && cycles.length === 0 && !err && (
        <EmptyState icon={Activity} title={L.evoEmpty} />
      )}

      {!loading && cycles.length > 0 && (
        <div className="relative space-y-2">
          {/* Vertical line */}
          <div className="absolute left-[19px] top-0 bottom-0 w-px bg-border" />

          {cycles.map((cycle) => {
            const dotClass = OUTCOME_DOT[cycle.outcome] ?? "bg-slate-400";
            const badgeClass = OUTCOME_BADGE[cycle.outcome] ?? "bg-white/10 text-fg-3";
            const skillId = typeof cycle.meta?.skill_id === "string" ? cycle.meta.skill_id : null;
            const passRate = typeof cycle.meta?.pass_rate === "number"
              ? (cycle.meta.pass_rate as number).toFixed(2)
              : null;
            const isExpanded = expanded === cycle.id;

            return (
              <div key={cycle.id} className="relative pl-10">
                {/* Timeline dot */}
                <div className={`absolute left-[14px] top-3.5 w-2.5 h-2.5 rounded-full border-2 border-bg-1 ${dotClass}`} />

                <div className="bg-bg-2 border border-border rounded-lg overflow-hidden">
                  <button
                    className="w-full text-left px-3 py-2.5"
                    onClick={() => setExpanded(isExpanded ? null : cycle.id)}
                  >
                    <div className="flex items-center gap-2 flex-wrap">
                      <span className="text-[10px] mono text-fg-4">
                        {cycle.started_at.slice(0, 16).replace("T", " ")}
                      </span>
                      <span className="text-[12px] font-medium text-fg flex-1 min-w-0 truncate">
                        {skillId ?? cycle.id}
                      </span>
                      <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${badgeClass}`}>
                        {cycle.outcome}
                      </span>
                      {passRate && (
                        <span className={`text-[11px] mono font-semibold ${cycle.outcome === "converged" ? "text-emerald-400" : "text-fg-3"}`}>
                          {passRate}
                        </span>
                      )}
                      <span className="text-fg-4 text-[10px]">{isExpanded ? "▲" : "▼"}</span>
                    </div>
                  </button>

                  {isExpanded && (
                    <div className="border-t border-border px-3 py-2.5 space-y-2">
                      {cycle.gene_changed && (
                        <div>
                          <div className="text-[10px] mono text-fg-4 uppercase tracking-wider mb-1">variant</div>
                          <div className="text-[11px] mono text-fg-3 bg-bg-3 rounded px-2 py-1.5 break-all">{cycle.gene_changed}</div>
                        </div>
                      )}
                      <div>
                        <div className="text-[10px] mono text-fg-4 uppercase tracking-wider mb-1">meta</div>
                        <pre className="text-[11px] mono text-fg-3 bg-bg-3 rounded px-2 py-1.5 overflow-auto whitespace-pre-wrap">
                          {JSON.stringify(cycle.meta, null, 2)}
                        </pre>
                      </div>
                      {cycle.ended_at && (
                        <div className="text-[10px] mono text-fg-4">
                          ended: {cycle.ended_at.slice(0, 16).replace("T", " ")}
                        </div>
                      )}
                    </div>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/learning/evolution/timeline?days=30</code>. {L.shown(cycles.length)}
      </p>
    </div>
  );
}

// ─── Page ─────────────────────────────────────────────────────────────────────

export default function LearningPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [tab, setTab] = useState<Tab>("digest");

  const tabs = (
    <>
      <TabBtn active={tab === "digest"} onClick={() => setTab("digest")}>{L.tabDigest}</TabBtn>
      <TabBtn active={tab === "memory"} onClick={() => setTab("memory")}>{L.tabMemory}</TabBtn>
      <TabBtn active={tab === "evolution"} onClick={() => setTab("evolution")}>{L.tabEvolution}</TabBtn>
    </>
  );

  return (
    <section className="space-y-4">
      <PageHeader title={L.title} tabs={tabs} />
      <CuratorKpiRow lang={lang} />
      <ArchBanner lang={lang} />
      {tab === "digest" && <DailyDigestTab lang={lang} />}
      {tab === "memory" && <OrgMemoryTab lang={lang} />}
      {tab === "evolution" && <EvolutionTimelineTab lang={lang} />}
    </section>
  );
}
