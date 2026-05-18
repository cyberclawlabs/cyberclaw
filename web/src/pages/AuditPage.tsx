// AuditPage — security-scoped mutation view of audit logs.
// Same endpoint as LogsPage (/api/v1/audit/logs) but pre-filtered to kind=mutation.
// No modals. Full audit console on /admin → audit.

import { useEffect, useState } from "react";
import { type AuditEntry, fetchAuditLogs } from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import { Activity } from "@/components/icons";
import PageHeader from "@/components/PageHeader";

const KIND_TONE: Record<string, string> = {
  mutation: "bg-rose-500/15 text-rose-300",
  auth: "bg-amber-500/15 text-amber-300",
  read: "bg-cyan-500/15 text-cyan-300",
  governance: "bg-violet-500/15 text-violet-300",
};

function fmtDate(v: string): string {
  const d = new Date(v);
  return Number.isNaN(d.getTime()) ? v : d.toLocaleString();
}

function resultLabel(r: AuditEntry["result"]): { text: string; cls: string } {
  if (r === "success") return { text: "success", cls: "text-emerald-300" };
  const reason = (r as { failure: { reason: string } }).failure?.reason ?? "failed";
  return { text: reason, cls: "text-rose-300" };
}

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "审计",
        subtitle: (n: number) => `共 ${n} 条 · 安全视图 — 变更 + 治理事件`,
        subtitleLoading: "加载中… · 安全视图 — 变更 + 治理事件",
        kind: "类型",
        all: "全部",
        noEntries: "当前筛选条件下无审计记录",
        noEntriesBody: "当 Agent 执行动作时，变更事件会出现在这里。",
        colTs: "时间戳",
        colActor: "执行者",
        colKind: "类型",
        colAction: "动作",
        colTarget: "目标",
        colResult: "结果",
        source: "来源",
      }
    : {
        title: "Audit",
        subtitle: (n: number) => `${n} total · security-scoped view — mutations + governance events`,
        subtitleLoading: "loading… · security-scoped view — mutations + governance events",
        kind: "kind",
        all: "all",
        noEntries: "No audit entries for this filter",
        noEntriesBody: "Mutation events appear here as agents act.",
        colTs: "timestamp",
        colActor: "actor",
        colKind: "kind",
        colAction: "action",
        colTarget: "target",
        colResult: "result",
        source: "Source",
      };
}

const KIND_OPTIONS = ["all", "mutation", "auth", "governance", "read"] as const;

export default function AuditPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [entries, setEntries] = useState<AuditEntry[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [kindFilter, setKindFilter] = useState("mutation");

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    const q = kindFilter === "all" ? { limit: 50 } : { kind: kindFilter, limit: 50 };
    fetchAuditLogs(q).then(
      (r) => !cancelled && setEntries(r.entries ?? []),
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, [kindFilter]);

  return (
    <section className="space-y-4">
      <PageHeader
        title={L.title}
        /* P2-2 fix: avoid the "0 total" flash before the first load resolves */
        subtitle={loading ? L.subtitleLoading : L.subtitle(entries.length)}
      />

      <div className="flex flex-wrap items-end gap-2 text-xs">
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.kind}</span>
          <select
            value={kindFilter}
            onChange={(e) => setKindFilter(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-36"
          >
            {KIND_OPTIONS.map((k) => (
              <option key={k} value={k}>{k === "all" ? L.all : k}</option>
            ))}
          </select>
        </label>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}

      {loading && <TableSkeleton cols={6} />}

      {!loading && entries.length === 0 && (
        <EmptyState
          icon={Activity}
          title={L.noEntries}
          body={L.noEntriesBody}
        />
      )}

      {!loading && entries.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colTs}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colActor}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colKind}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colAction}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colTarget}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colResult}</th>
              </tr>
            </thead>
            <tbody>
              {entries.map((e, i) => {
                const res = resultLabel(e.result);
                return (
                  <tr key={i} className="border-t border-border hover:bg-hover">
                    <td className="px-3 py-2 mono text-fg-4 whitespace-nowrap">{fmtDate(e.ts)}</td>
                    <td className="px-3 py-2 mono text-fg-3">{e.actor}</td>
                    <td className="px-3 py-2">
                      <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${KIND_TONE[e.kind] ?? "bg-white/10 text-fg-3"}`}>
                        {e.kind}
                      </span>
                    </td>
                    <td className="px-3 py-2 mono text-fg-3">{e.action}</td>
                    <td className="px-3 py-2 text-fg-4 truncate max-w-[160px]">{e.target ?? "—"}</td>
                    <td className={`px-3 py-2 mono text-[10px] ${res.cls}`}>{res.text}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/audit/logs?kind={kindFilter}</code>. v2 minimal port — full audit console on /admin.
      </p>
    </section>
  );
}
