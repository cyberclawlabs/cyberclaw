// MemoryPage — 3 top-level tabs: List (L0/L1/L2) | Search | Agent Notes
// Existing mutate path (edit / delete) is untouched.

import { useEffect, useState } from "react";
import {
  type MemoryRecord,
  type MemorySearchResult,
  type AgentNote,
  fetchMemory,
  editMemory,
  deleteMemory,
  searchMemory,
  fetchAgentNotes,
} from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import { Database } from "@/components/icons";
import PageHeader from "@/components/PageHeader";
import Modal from "@/components/Modal";
import { useToast } from "@/components/ToastBar";

// ─── Types ────────────────────────────────────────────────────────────────────

type Level = "L0" | "L1" | "L2";
type MainTab = "list" | "search" | "notes";

// ─── i18n ─────────────────────────────────────────────────────────────────────

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "记忆",
        totalFmt: (n: number) => `共 ${n} 条`,
        // Main tabs
        tabList: "列表",
        tabSearch: "搜索",
        tabNotes: "Agent 笔记",
        // List tab
        levelLabels: {
          L0: "工作记忆 (L0)",
          L1: "情节记忆 (L1)",
          L2: "程序记忆 (L2)",
        } as Record<Level, string>,
        agentFilter: "agent_id",
        agentFilterPlaceholder: "按 agent 过滤",
        noRecords: "暂无记忆记录",
        noRecordsBody: "Agent 执行时会自动写入。",
        noRecordsFilter: "无匹配的记录",
        colAgentId: "agent_id",
        colLevel: "级别",
        colPreview: "预览",
        colSize: "大小",
        colCreated: "创建时间",
        colUpdated: "更新时间",
        colActions: "操作",
        edit: "编辑",
        delete: "删除",
        editTitle: "编辑记忆记录",
        deleteTitle: "删除记忆记录",
        deleteConfirmFmt: (id: string) => (
          <>
            永久删除记忆记录 <strong className="text-fg mono">{id}</strong>？此操作不可撤销。
          </>
        ),
        cancel: "取消",
        save: "保存",
        updatedMsg: "记忆记录已更新",
        deletedMsg: "记忆记录已删除",
        editFailedFmt: (r: string) => `编辑失败：${r}`,
        deleteFailedFmt: (r: string) => `删除失败：${r}`,
        source: "来源",
        // Search tab
        searchPlaceholder: "输入关键词搜索记忆…",
        searchBtn: "搜索",
        searchEmpty: "无搜索结果",
        searchEmptyBody: "尝试更换关键词。",
        searchNoQuery: "请输入搜索词",
        searchNoQueryBody: "输入关键词后点击搜索。",
        searchEndpointWarn: "搜索端点未就绪，回退到列表",
        colScore: "相关度",
        // Notes tab
        notesEmpty: "暂无 Agent 笔记",
        notesEmptyBody: "Agent 执行时会自动积累私笔记。",
        notesEndpointWarn: "Agent Notes 端点未就绪，回退到列表",
        notesGroupLabel: (agent: string) => `Agent: ${agent}`,
        colContent: "内容",
        colTs: "时间",
      }
    : {
        title: "Memory",
        totalFmt: (n: number) => `${n} total`,
        // Main tabs
        tabList: "List",
        tabSearch: "Search",
        tabNotes: "Agent Notes",
        // List tab
        levelLabels: {
          L0: "Working (L0)",
          L1: "Episodic (L1)",
          L2: "Procedural (L2)",
        } as Record<Level, string>,
        agentFilter: "agent_id",
        agentFilterPlaceholder: "filter by agent",
        noRecords: "No memory records",
        noRecordsBody: "Agents auto-write here as they execute.",
        noRecordsFilter: "No records match the current filter",
        colAgentId: "agent_id",
        colLevel: "level",
        colPreview: "preview",
        colSize: "size",
        colCreated: "created",
        colUpdated: "updated",
        colActions: "actions",
        edit: "Edit",
        delete: "Delete",
        editTitle: "Edit memory record",
        deleteTitle: "Delete memory record",
        deleteConfirmFmt: (id: string) => (
          <>
            Permanently delete memory record <strong className="text-fg mono">{id}</strong>? This cannot be undone.
          </>
        ),
        cancel: "Cancel",
        save: "Save",
        updatedMsg: "Memory record updated",
        deletedMsg: "Memory record deleted",
        editFailedFmt: (r: string) => `Edit failed: ${r}`,
        deleteFailedFmt: (r: string) => `Delete failed: ${r}`,
        source: "Source",
        // Search tab
        searchPlaceholder: "Search memory by keyword…",
        searchBtn: "Search",
        searchEmpty: "No results",
        searchEmptyBody: "Try a different query.",
        searchNoQuery: "Enter a search term",
        searchNoQueryBody: "Type a keyword and press Search.",
        searchEndpointWarn: "Search endpoint not ready — falling back to list",
        colScore: "score",
        // Notes tab
        notesEmpty: "No agent notes yet",
        notesEmptyBody: "Agents accumulate notes across executions.",
        notesEndpointWarn: "Agent Notes endpoint not ready — falling back to list",
        notesGroupLabel: (agent: string) => `Agent: ${agent}`,
        colContent: "content",
        colTs: "time",
      };
}

// ─── Shared helpers ───────────────────────────────────────────────────────────

const LEVEL_TONE: Record<Level, string> = {
  L0: "bg-cyan-500/15 text-cyan-300",
  L1: "bg-violet-500/15 text-violet-300",
  L2: "bg-amber-500/15 text-amber-300",
};

function fmtDate(v: string | undefined): string {
  if (!v) return "—";
  const d = new Date(v);
  return Number.isNaN(d.getTime()) ? v : d.toLocaleString();
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  return `${(n / 1024).toFixed(1)} KB`;
}

// ─── List Tab ─────────────────────────────────────────────────────────────────

function ListTab({
  lang,
  L,
}: {
  lang: Lang;
  L: ReturnType<typeof dict>;
}) {
  const [tab, setTab] = useState<Level>("L0");
  const [records, setRecords] = useState<MemoryRecord[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [agentFilter, setAgentFilter] = useState("");
  const toast = useToast();

  const [editTarget, setEditTarget] = useState<MemoryRecord | null>(null);
  const [editContent, setEditContent] = useState("");
  const [editBusy, setEditBusy] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<MemoryRecord | null>(null);
  const [deleteBusy, setDeleteBusy] = useState(false);

  const refetch = (level: Level) => {
    setLoading(true);
    setErr(null);
    fetchMemory(level, 50).then(
      (r) => setRecords(r),
      (e) => setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => setLoading(false));
  };

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    fetchMemory(tab, 50).then(
      (r) => !cancelled && setRecords(r),
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, [tab]);

  const filtered = records.filter(
    (r) => !agentFilter || r.agent_id.toLowerCase().includes(agentFilter.toLowerCase()),
  );

  const handleEdit = async () => {
    if (!editTarget) return;
    setEditBusy(true);
    try {
      await editMemory(editTarget.id, editContent);
      toast({ tone: "success", msg: L.updatedMsg });
      setEditTarget(null);
      refetch(tab);
    } catch (e: unknown) {
      const ae = e as { status?: number; body?: string };
      toast({ tone: "error", msg: L.editFailedFmt(ae?.body ?? "unknown") });
    } finally {
      setEditBusy(false);
    }
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    setDeleteBusy(true);
    try {
      await deleteMemory(deleteTarget.id);
      toast({ tone: "success", msg: L.deletedMsg });
      setRecords((prev) => prev.filter((r) => r.id !== deleteTarget.id));
      setDeleteTarget(null);
    } catch (e: unknown) {
      const ae = e as { status?: number; body?: string };
      toast({ tone: "error", msg: L.deleteFailedFmt(ae?.body ?? "unknown") });
      setDeleteTarget(null);
    } finally {
      setDeleteBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-end gap-2 text-xs">
        <div className="flex gap-1">
          {(["L0", "L1", "L2"] as Level[]).map((l) => (
            <button
              key={l}
              onClick={() => setTab(l)}
              className={[
                "px-3 py-1 rounded-md border text-xs transition-colors",
                tab === l
                  ? "bg-bg-3 border-border-strong text-fg"
                  : "border-border text-fg-3 hover:bg-hover",
              ].join(" ")}
            >
              {L.levelLabels[l]}
            </button>
          ))}
        </div>
        <label className="flex flex-col gap-0.5 ml-2">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.agentFilter}</span>
          <input
            value={agentFilter}
            onChange={(e) => setAgentFilter(e.target.value)}
            placeholder={L.agentFilterPlaceholder}
            className="h-8 px-3 rounded-md bg-bg-3 border border-border outline-none focus-ring w-48"
          />
        </label>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={7} />}

      {!loading && filtered.length === 0 && (
        <EmptyState
          icon={Database}
          title={records.length === 0 ? L.noRecords : L.noRecordsFilter}
          body={records.length === 0 ? L.noRecordsBody : undefined}
        />
      )}

      {!loading && filtered.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colAgentId}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colLevel}</th>
                <th className="px-3 py-2 font-medium text-fg-3 max-w-sm">{L.colPreview}</th>
                <th className="px-3 py-2 font-medium text-fg-3 text-right mono">{L.colSize}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colCreated}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colUpdated}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colActions}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((r) => (
                <tr key={r.id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 mono text-fg-4">{r.agent_id}</td>
                  <td className="px-3 py-2">
                    <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${LEVEL_TONE[r.level as Level] ?? "bg-white/10"}`}>
                      {r.level}
                    </span>
                  </td>
                  <td className="px-3 py-2 text-fg-3 max-w-sm truncate">{r.preview || "—"}</td>
                  <td className="px-3 py-2 mono text-right text-fg-4">{fmtBytes(r.size_bytes)}</td>
                  <td className="px-3 py-2 text-fg-4 mono">{fmtDate(r.created_at)}</td>
                  <td className="px-3 py-2 text-fg-4 mono">{fmtDate(r.updated_at)}</td>
                  <td className="px-3 py-2">
                    <div className="flex items-center gap-1.5">
                      <button
                        onClick={() => { setEditTarget(r); setEditContent(r.preview); }}
                        className="px-2 py-0.5 rounded text-[10px] bg-bg-3 border border-border hover:bg-hover text-fg-3 hover:text-fg"
                      >
                        {L.edit}
                      </button>
                      <button
                        onClick={() => setDeleteTarget(r)}
                        className="px-2 py-0.5 rounded text-[10px] bg-rose-500/10 border border-rose-500/20 hover:bg-rose-500/20 text-rose-400"
                      >
                        {L.delete}
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
        {L.source}: <code>/api/v1/memory?level={tab}</code>
      </p>

      {/* Edit Modal */}
      <Modal
        open={!!editTarget}
        onClose={() => setEditTarget(null)}
        title={L.editTitle}
        width={540}
        footer={
          <>
            <button onClick={() => setEditTarget(null)} className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg hover:bg-hover">
              {L.cancel}
            </button>
            <button onClick={handleEdit} disabled={editBusy} className="px-3 h-8 rounded-md bg-accent text-white text-xs font-medium hover:opacity-90 disabled:opacity-50">
              {L.save}
            </button>
          </>
        }
      >
        <div className="space-y-2">
          <p className="text-[11px] text-fg-4 mono">{editTarget?.id}</p>
          <textarea
            value={editContent}
            onChange={(e) => setEditContent(e.target.value)}
            rows={8}
            className="w-full px-3 py-2 rounded-md bg-bg-3 border border-border outline-none focus-ring text-xs font-mono resize-y"
          />
        </div>
      </Modal>

      {/* Delete confirm Modal */}
      <Modal
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
        title={L.deleteTitle}
        width={400}
        footer={
          <>
            <button onClick={() => setDeleteTarget(null)} className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg hover:bg-hover">
              {L.cancel}
            </button>
            <button onClick={handleDelete} disabled={deleteBusy} className="px-3 h-8 rounded-md bg-rose-600 text-white text-xs font-medium hover:opacity-90 disabled:opacity-50">
              {L.delete}
            </button>
          </>
        }
      >
        <p className="text-sm text-fg-2">
          {deleteTarget ? L.deleteConfirmFmt(deleteTarget.id) : null}
        </p>
      </Modal>
    </div>
  );
}

// ─── Search Tab ───────────────────────────────────────────────────────────────

function SearchTab({ L }: { L: ReturnType<typeof dict> }) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<MemorySearchResult[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [warn, setWarn] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const handleSearch = async () => {
    if (!query.trim()) return;
    setLoading(true);
    setErr(null);
    setWarn(null);
    try {
      const r = await searchMemory(query.trim());
      setResults(r);
    } catch (e: unknown) {
      const ae = e as { status?: number; body?: string };
      if (ae?.status === 404) {
        setWarn(L.searchEndpointWarn);
        setResults([]);
      } else {
        setErr(`HTTP ${ae?.status} ${ae?.body ?? ""}`);
      }
    } finally {
      setLoading(false);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") handleSearch();
  };

  return (
    <div className="space-y-4">
      {/* Search bar */}
      <div className="flex gap-2 items-center">
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={L.searchPlaceholder}
          className="h-8 flex-1 px-3 rounded-md bg-bg-3 border border-border outline-none focus-ring text-xs"
        />
        <button
          onClick={handleSearch}
          disabled={loading || !query.trim()}
          className="h-8 px-4 rounded-md bg-accent text-white text-xs font-medium hover:opacity-90 disabled:opacity-50"
        >
          {L.searchBtn}
        </button>
      </div>

      {/* Warnings / errors */}
      {warn && (
        <p className="text-xs text-amber-400 px-2 py-1.5 bg-amber-500/10 rounded border border-amber-500/20">
          {warn}
        </p>
      )}
      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}

      {/* Loading */}
      {loading && <TableSkeleton cols={5} />}

      {/* No query yet */}
      {!loading && results === null && !err && !warn && (
        <EmptyState
          icon={Database}
          title={L.searchNoQuery}
          body={L.searchNoQueryBody}
        />
      )}

      {/* Empty results */}
      {!loading && results !== null && results.length === 0 && !warn && (
        <EmptyState
          icon={Database}
          title={L.searchEmpty}
          body={L.searchEmptyBody}
        />
      )}

      {/* Results table */}
      {!loading && results !== null && results.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colAgentId}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colLevel}</th>
                <th className="px-3 py-2 font-medium text-fg-3 max-w-sm">{L.colPreview}</th>
                <th className="px-3 py-2 font-medium text-fg-3 text-right mono">{L.colScore}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colUpdated}</th>
              </tr>
            </thead>
            <tbody>
              {results.map((r) => (
                <tr key={r.id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 mono text-fg-4">{r.agent_id}</td>
                  <td className="px-3 py-2">
                    <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${LEVEL_TONE[r.level as Level] ?? "bg-white/10"}`}>
                      {r.level}
                    </span>
                  </td>
                  <td className="px-3 py-2 text-fg-3 max-w-sm truncate">{r.preview || "—"}</td>
                  <td className="px-3 py-2 mono text-right text-fg-4">
                    {r.score !== undefined ? r.score.toFixed(3) : "—"}
                  </td>
                  <td className="px-3 py-2 text-fg-4 mono">{fmtDate(r.updated_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

// ─── Agent Notes Tab ──────────────────────────────────────────────────────────

function AgentNotesTab({ L }: { L: ReturnType<typeof dict> }) {
  const [notes, setNotes] = useState<AgentNote[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [warn, setWarn] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    setWarn(null);
    fetchAgentNotes().then(
      (r) => !cancelled && setNotes(r),
      (e: unknown) => {
        if (cancelled) return;
        const ae = e as { status?: number; body?: string };
        if (ae?.status === 404) {
          setWarn(L.notesEndpointWarn);
          setNotes([]);
        } else {
          setErr(`HTTP ${ae?.status} ${ae?.body ?? ""}`);
        }
      },
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, [L.notesEndpointWarn]);

  // Group notes by agent_id
  const grouped: Map<string, AgentNote[]> = new Map();
  if (notes) {
    for (const n of notes) {
      const key = n.agent_id || "unknown";
      const existing = grouped.get(key) ?? [];
      existing.push(n);
      grouped.set(key, existing);
    }
  }

  return (
    <div className="space-y-4">
      {warn && (
        <p className="text-xs text-amber-400 px-2 py-1.5 bg-amber-500/10 rounded border border-amber-500/20">
          {warn}
        </p>
      )}
      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}

      {loading && <TableSkeleton cols={3} />}

      {!loading && notes !== null && notes.length === 0 && !warn && (
        <EmptyState
          icon={Database}
          title={L.notesEmpty}
          body={L.notesEmptyBody}
        />
      )}

      {!loading && grouped.size > 0 && (
        <div className="space-y-6">
          {Array.from(grouped.entries()).map(([agentId, agentNotes]) => (
            <div key={agentId} className="space-y-2">
              {/* Group header */}
              <div className="flex items-center gap-2">
                <span className="text-[11px] font-medium text-fg-3 mono">{L.notesGroupLabel(agentId)}</span>
                <span className="text-[10px] text-fg-4">({agentNotes.length})</span>
              </div>
              {/* Notes table for this agent */}
              <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
                <table className="w-full text-xs">
                  <thead className="bg-bg-3">
                    <tr className="text-left">
                      <th className="px-3 py-2 font-medium text-fg-3 w-full">{L.colContent}</th>
                      <th className="px-3 py-2 font-medium text-fg-3 whitespace-nowrap">{L.colTs}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {agentNotes.map((n, i) => {
                      const ts = n.created_at ?? n.ts;
                      return (
                        <tr key={n.id ?? i} className="border-t border-border hover:bg-hover">
                          <td className="px-3 py-2 text-fg-2 whitespace-pre-wrap break-words max-w-lg">
                            {n.content}
                          </td>
                          <td className="px-3 py-2 text-fg-4 mono whitespace-nowrap">{fmtDate(ts)}</td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ─── Page root ────────────────────────────────────────────────────────────────

export default function MemoryPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [mainTab, setMainTab] = useState<MainTab>("list");

  const MAIN_TABS: { value: MainTab; label: string }[] = [
    { value: "list", label: L.tabList },
    { value: "search", label: L.tabSearch },
    { value: "notes", label: L.tabNotes },
  ];

  return (
    <section className="space-y-4">
      <PageHeader title={L.title} />

      {/* Top-level tab bar */}
      <div className="flex gap-1">
        {MAIN_TABS.map(({ value, label }) => (
          <button
            key={value}
            onClick={() => setMainTab(value)}
            className={[
              "px-3 py-1.5 rounded-md border text-xs transition-colors",
              mainTab === value
                ? "bg-bg-3 border-border-strong text-fg font-medium"
                : "border-border text-fg-3 hover:bg-hover",
            ].join(" ")}
          >
            {label}
          </button>
        ))}
      </div>

      {/* Tab bodies */}
      {mainTab === "list" && <ListTab lang={lang} L={L} />}
      {mainTab === "search" && <SearchTab L={L} />}
      {mainTab === "notes" && <AgentNotesTab L={L} />}
    </section>
  );
}
