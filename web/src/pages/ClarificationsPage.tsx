// ClarificationsPage — minimal v2 port.
// Full management available on /admin (page = 'clarifications').

import { useEffect, useState } from "react";
import { type Clarification, fetchClarifications } from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import { HelpCircle } from "@/components/icons";

const STATUS_TONE: Record<string, string> = {
  pending: "bg-amber-500/15 text-amber-300",
  answered: "bg-emerald-500/15 text-emerald-300",
};

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "澄清",
        totalFmt: (n: number) => `共 ${n} 条`,
        search: "搜索",
        searchPlaceholder: "id / 问题",
        status: "状态",
        all: "全部",
        noClarifications: "未找到澄清请求",
        noClarificationsFilter: "无匹配的澄清请求",
        colId: "id",
        colConversationId: "conversation_id",
        colQuestion: "问题",
        colStatus: "状态",
        colCreatedAt: "创建时间",
        source: "来源",
      }
    : {
        title: "Clarifications",
        totalFmt: (n: number) => `${n} total`,
        search: "search",
        searchPlaceholder: "id / question",
        status: "status",
        all: "all",
        noClarifications: "No clarifications found",
        noClarificationsFilter: "No clarifications match the current filters",
        colId: "id",
        colConversationId: "conversation_id",
        colQuestion: "question",
        colStatus: "status",
        colCreatedAt: "created_at",
        source: "Source",
      };
}

export default function ClarificationsPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [items, setItems] = useState<Clarification[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [q, setQ] = useState("");
  const [status, setStatus] = useState("");

  useEffect(() => {
    let cancelled = false;
    fetchClarifications().then(
      (d) => !cancelled && setItems(d),
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, []);

  const filtered = items.filter((c) => {
    if (q && !(c.id + " " + c.question).toLowerCase().includes(q.toLowerCase())) return false;
    if (status && c.status !== status) return false;
    return true;
  });

  return (
    <section className="space-y-4">
      <header className="space-y-1">
        <h2 className="text-base font-medium text-fg">{L.title}</h2>
        <p className="text-xs text-fg-3">{L.totalFmt(items.length)}</p>
      </header>

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
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.status}</span>
          <select
            value={status}
            onChange={(e) => setStatus(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-28"
          >
            <option value="">{L.all}</option>
            <option value="pending">pending</option>
            <option value="answered">answered</option>
          </select>
        </label>
      </div>

      {err && (
        <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>
      )}

      {loading && <TableSkeleton cols={5} />}

      {!loading && filtered.length === 0 && (
        <EmptyState
          icon={HelpCircle}
          title={items.length === 0 ? L.noClarifications : L.noClarificationsFilter}
        />
      )}

      {!loading && filtered.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colId}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colConversationId}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colQuestion}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colStatus}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colCreatedAt}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((c) => (
                <tr key={c.id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 mono text-fg-4">{c.id}</td>
                  <td className="px-3 py-2 mono text-fg-3">{c.conversation_id ?? "—"}</td>
                  <td className="px-3 py-2 max-w-xs truncate">{c.question}</td>
                  <td className="px-3 py-2">
                    <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${STATUS_TONE[c.status] ?? "bg-white/10 text-fg-3"}`}>
                      {c.status}
                    </span>
                  </td>
                  <td className="px-3 py-2 mono text-fg-4">{c.created_at ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/clarifications</code>. Full management on /admin.
      </p>
    </section>
  );
}
