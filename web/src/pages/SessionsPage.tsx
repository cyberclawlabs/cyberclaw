// SessionsPage — aggregate view of all chat sessions (conversations)
//
// v1.4 W1 SPIKE (2026-05-27): inline detail panel. Row click → side panel
// shows messages without leaving SessionsPage. "Open in Chat" button still
// available for full chat UI. Closes the 5.5× gap with hermes Sessions
// single-page-full-feature pattern (hm 852 vs prior cb 154 lines).

import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  type Conversation,
  type SessionSummary,
  fetchConversation,
  fetchSessions,
} from "@/lib/api";
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
        detailLoading: "加载中…",
        detailEmpty: "暂无消息",
        openInChat: "在对话页打开",
        close: "关闭",
        userRole: "用户",
        assistantRole: "助手",
        systemRole: "系统",
        toolRole: "工具",
        searchInDetail: "在消息中搜索…",
        expand: "展开",
        collapse: "折叠",
        messageCount: (n: number) => `${n} 条消息`,
        noMatchInDetail: "没有匹配的消息",
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
        detailLoading: "loading…",
        detailEmpty: "no messages",
        openInChat: "Open in Chat",
        close: "Close",
        userRole: "user",
        assistantRole: "assistant",
        systemRole: "system",
        toolRole: "tool",
        searchInDetail: "Search messages…",
        expand: "Expand",
        collapse: "Collapse",
        messageCount: (n: number) => `${n} message${n !== 1 ? "s" : ""}`,
        noMatchInDetail: "No messages match",
      };
}

export default function SessionsPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const navigate = useNavigate();
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  // v1.4 W1: inline detail
  const [detailId, setDetailId] = useState<string | null>(null);
  const [detail, setDetail] = useState<Conversation | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailSearch, setDetailSearch] = useState("");
  const [expanded, setExpanded] = useState<Set<number>>(new Set());

  const toggleExpand = (i: number) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      return next;
    });
  };

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

  // v1.4 W1 SPIKE: lazy-fetch session detail when row clicked
  useEffect(() => {
    if (!detailId) {
      setDetail(null);
      return;
    }
    let cancelled = false;
    setDetailLoading(true);
    setDetail(null);
    setDetailSearch("");
    setExpanded(new Set());
    fetchConversation(detailId)
      .then((data) => {
        if (!cancelled) {
          setDetail(data);
          setDetailLoading(false);
        }
      })
      .catch(() => {
        if (!cancelled) setDetailLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [detailId]);

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
        <div className="flex gap-4 min-h-0 flex-1">
          {/* List */}
          <div className={`rounded-lg border border-border overflow-hidden bg-bg-2 ${detailId ? "flex-[3]" : "flex-1"}`}>
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
                    className={`border-t border-border hover:bg-hover cursor-pointer ${detailId === s.id ? "bg-hover" : ""}`}
                    onClick={() => setDetailId(detailId === s.id ? null : s.id)}
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

          {/* v1.4 W1 SPIKE: Inline detail panel */}
          {detailId && (
            <aside className="flex-[2] rounded-lg border border-border bg-bg-2 flex flex-col min-w-0">
              <div className="flex items-center justify-between px-3 py-2 bg-bg-3 border-b border-border gap-2">
                <span className="text-xs font-medium truncate" title={detail?.title}>
                  {detail?.title ?? "—"}
                </span>
                <div className="flex items-center gap-2 shrink-0">
                  <button
                    className="text-[11px] px-2 py-1 rounded bg-accent/10 text-accent hover:bg-accent/20"
                    onClick={() => navigate(`/chat?conv=${encodeURIComponent(detailId)}`)}
                  >
                    {L.openInChat}
                  </button>
                  <button
                    className="text-[11px] px-2 py-1 rounded hover:bg-hover text-fg-3"
                    onClick={() => setDetailId(null)}
                  >
                    {L.close}
                  </button>
                </div>
              </div>
              {/* Search + count strip */}
              {!detailLoading && detail?.messages && detail.messages.length > 0 && (
                <div className="flex items-center gap-2 px-3 py-2 border-b border-border bg-bg-2">
                  <input
                    type="text"
                    placeholder={L.searchInDetail}
                    value={detailSearch}
                    onChange={(e) => setDetailSearch(e.target.value)}
                    className="flex-1 h-7 px-2 rounded bg-bg-3 border border-border text-[11px] placeholder:text-fg-4 focus:outline-none focus:border-accent"
                  />
                  <span className="text-[10px] text-fg-4 shrink-0 mono">
                    {L.messageCount(detail.messages.length)}
                  </span>
                </div>
              )}
              <div className="flex-1 overflow-y-auto p-3 space-y-2 min-h-[400px] max-h-[70vh]">
                {detailLoading && <p className="text-xs text-fg-4">{L.detailLoading}</p>}
                {!detailLoading && (!detail?.messages || detail.messages.length === 0) && (
                  <p className="text-xs text-fg-4">{L.detailEmpty}</p>
                )}
                {!detailLoading && (() => {
                  const dq = detailSearch.trim().toLowerCase();
                  const all = detail?.messages ?? [];
                  const matched = dq
                    ? all
                        .map((m, i) => ({ m, i }))
                        .filter(({ m }) => {
                          const c = typeof m.content === "string" ? m.content : JSON.stringify(m.content);
                          return c.toLowerCase().includes(dq);
                        })
                    : all.map((m, i) => ({ m, i }));

                  if (dq && matched.length === 0) {
                    return <p className="text-xs text-fg-4">{L.noMatchInDetail}</p>;
                  }

                  return matched.map(({ m, i }) => {
                    const content = typeof m.content === "string" ? m.content : JSON.stringify(m.content);
                    const isLong = content.length > 500;
                    const isOpen = expanded.has(i) || !isLong;
                    const shown = isOpen ? content : content.slice(0, 500) + "…";
                    return (
                      <div key={i} className="text-xs">
                        <div className="flex items-baseline justify-between gap-2 mb-0.5">
                          <span className="font-medium text-fg-3">
                            {m.role === "user"
                              ? L.userRole
                              : m.role === "assistant"
                                ? L.assistantRole
                                : m.role === "system"
                                  ? L.systemRole
                                  : L.toolRole}
                          </span>
                          {isLong && (
                            <button
                              className="text-[10px] text-accent hover:underline shrink-0"
                              onClick={() => toggleExpand(i)}
                            >
                              {isOpen ? L.collapse : L.expand}
                            </button>
                          )}
                        </div>
                        <div className="text-fg-2 whitespace-pre-wrap break-words pl-3 border-l border-border">
                          {shown}
                        </div>
                      </div>
                    );
                  });
                })()}
              </div>
            </aside>
          )}
        </div>
      )}
    </section>
  );
}
