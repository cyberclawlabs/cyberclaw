// SessionsPage — aggregate view of all chat sessions (conversations)

import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { type SessionSummary, fetchSessions } from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import PageHeader from "@/components/PageHeader";
import { List } from "@/components/icons";

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "会话列表",
        filterPlaceholder: "按标题筛选…",
        countSuffix: (n: number) => `${n} 个会话`,
        emptyTitle: "暂无会话",
        emptyBody: "在对话页发起第一个会话后会出现在这里。",
        emptyFiltered: "没有匹配的会话",
        colTitle: "标题",
        colModel: "模型",
        colMsgs: "消息数",
        colTokens: "约 token",
        colLastActivity: "最近活动",
        colOwner: "所有者",
      }
    : {
        title: "Sessions",
        filterPlaceholder: "Filter by title…",
        countSuffix: (n: number) => `${n} session${n !== 1 ? "s" : ""}`,
        emptyTitle: "No sessions yet",
        emptyBody: "Start a conversation in Chat to create the first session.",
        emptyFiltered: "No sessions match your search",
        colTitle: "title",
        colModel: "model",
        colMsgs: "msgs",
        colTokens: "~tokens",
        colLastActivity: "last activity",
        colOwner: "owner",
      };
}

export default function SessionsPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const navigate = useNavigate();
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState<string | null>(null);
  const [search, setSearch] = useState("");

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    fetchSessions(100)
      .then((data) => {
        if (!cancelled) {
          setSessions(data);
          setLoading(false);
        }
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          setErr(e instanceof Error ? e.message : String(e));
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const query = search.trim().toLowerCase();
  const filtered = query
    ? sessions.filter((s) => s.title.toLowerCase().includes(query))
    : sessions;

  return (
    <section className="flex flex-col gap-4 p-4 max-w-full">
      <PageHeader title={L.title} />

      {/* Search bar */}
      <div className="flex items-center gap-3">
        <input
          type="text"
          placeholder={L.filterPlaceholder}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          className="h-8 px-3 rounded-md bg-bg-3 border border-border text-xs w-60 placeholder:text-fg-4 focus:outline-none focus:border-accent"
        />
        <span className="text-[11px] text-fg-4">
          {L.countSuffix(filtered.length)}
        </span>
      </div>

      {err && (
        <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>
      )}

      {loading && <TableSkeleton cols={6} />}

      {!loading && filtered.length === 0 && (
        <EmptyState
          icon={List}
          title={sessions.length === 0 ? L.emptyTitle : L.emptyFiltered}
          body={sessions.length === 0 ? L.emptyBody : undefined}
        />
      )}

      {!loading && filtered.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs table-fixed">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3 w-[30%]">{L.colTitle}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[12%]">{L.colModel}</th>
                <th className="px-3 py-2 font-medium text-fg-3 mono text-right w-[8%]">{L.colMsgs}</th>
                <th className="px-3 py-2 font-medium text-fg-3 mono text-right w-[10%]">{L.colTokens}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[12%]">{L.colLastActivity}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-[18%] truncate">{L.colOwner}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((s) => (
                <tr
                  key={s.id}
                  className="border-t border-border hover:bg-hover cursor-pointer"
                  onClick={() => navigate(`/chat?conv=${encodeURIComponent(s.id)}`)}
                >
                  <td className="px-3 py-2 font-medium truncate max-w-0">
                    <span className="block truncate" title={s.title}>
                      {s.title}
                    </span>
                  </td>
                  <td className="px-3 py-2 text-fg-3 truncate">{s.model ?? "—"}</td>
                  <td className="px-3 py-2 mono text-fg-3 text-right">{s.message_count}</td>
                  <td className="px-3 py-2 mono text-fg-3 text-right">
                    {s.estimated_tokens > 0 ? s.estimated_tokens.toLocaleString() : "—"}
                  </td>
                  <td className="px-3 py-2 text-fg-4">{s.last_activity}</td>
                  <td className="px-3 py-2 text-fg-4 truncate max-w-0">
                    <span className="block truncate mono" title={s.owner_user_id}>
                      {s.owner_user_id}
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
