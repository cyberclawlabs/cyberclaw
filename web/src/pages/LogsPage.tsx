// LogsPage — consumes /api/v1/audit/logs.
// MVP: list + three filters (kind / actor / action_prefix) + auto-refresh toggle.
// Core interactions: row filtering + level coloring + auto-refresh switch.

import { useCallback, useEffect, useRef, useState } from "react";
import { type AuditEntry, fetchAuditLogs, streamAuditLogs } from "@/lib/api";
import { type Lang, t } from "@/lib/i18n";
import PageHeader from "@/components/PageHeader";

/** Runtime guard for SSE-delivered AuditEntry frames (B-12).
 *  Only the 5 fields the UI actually renders are required; optional
 *  fields (target / ip / user_agent / detail / result) are allowed to
 *  be undefined to stay forward-compat with backend additions. */
function isAuditEntry(x: unknown): x is AuditEntry {
  if (!x || typeof x !== "object") return false;
  const o = x as Record<string, unknown>;
  return (
    typeof o.ts === "string" &&
    typeof o.actor === "string" &&
    typeof o.kind === "string" &&
    typeof o.action === "string"
  );
}

const KIND_TONE: Record<string, string> = {
  auth: "text-blue-400",
  approval: "text-amber-400",
  mutation: "text-rose-400",
  read: "text-emerald-400",
  policy: "text-purple-400",
};

function resultToBadge(r: AuditEntry["result"]): {
  label: string;
  cls: string;
} {
  if (r === "success") return { label: "ok", cls: "bg-emerald-500/15 text-emerald-400" };
  return {
    label: "fail",
    cls: "bg-rose-500/15 text-rose-400",
  };
}

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "日志",
        subtitleFmt: (n: number) => `${n} 条`,
        kind: "类型",
        actor: "执行者",
        actionPrefix: "动作前缀",
        limit: "条数",
        refresh: "刷新",
        liveTail: "实时流 (SSE)",
        streamLost: "流已断开 — 正在重连…",
        noEntries: "当前筛选条件下无日志记录",
        colTs: "时间戳",
        colKind: "类型",
        colActor: "执行者",
        colAction: "动作",
        colTarget: "目标",
        colResult: "结果",
        source: "来源",
      }
    : {
        title: "Logs",
        subtitleFmt: (n: number) => `${n} entries`,
        kind: "kind",
        actor: "actor",
        actionPrefix: "action prefix",
        limit: "limit",
        refresh: "Refresh",
        liveTail: "live tail (SSE)",
        streamLost: "Stream lost — reconnecting…",
        noEntries: "No log entries match the current filters.",
        colTs: "ts",
        colKind: "kind",
        colActor: "actor",
        colAction: "action",
        colTarget: "target",
        colResult: "result",
        source: "Source",
      };
}

export default function LogsPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [entries, setEntries] = useState<AuditEntry[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [autoRefresh, setAutoRefresh] = useState(false);
  const tableContainerRef = useRef<HTMLDivElement>(null);
  const [limit, setLimit] = useState(50);
  const [kind, setKind] = useState("");
  const [actor, setActor] = useState("");
  const [actionPrefix, setActionPrefix] = useState("");

  const refresh = useCallback(async () => {
    setLoading(true);
    setErr(null);
    try {
      const data = await fetchAuditLogs({
        limit,
        kind: kind || undefined,
        actor: actor || undefined,
        action_prefix: actionPrefix || undefined,
      });
      setEntries(data.entries);
    } catch (e: unknown) {
      const eobj = e as { status?: number; body?: string };
      setErr(`HTTP ${eobj.status ?? "?"} ${eobj.body ?? ""}`);
    } finally {
      setLoading(false);
    }
  }, [limit, kind, actor, actionPrefix]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // auto-refresh: SSE live-tail when on, closed when off.
  const sourceRef = useRef<EventSource | null>(null);
  useEffect(() => {
    if (!autoRefresh) {
      sourceRef.current?.close();
      sourceRef.current = null;
      return;
    }
    const source = streamAuditLogs({ kind: kind || undefined, actor: actor || undefined });
    sourceRef.current = source;
    source.onmessage = (ev: MessageEvent) => {
      try {
        const raw: unknown = JSON.parse(ev.data as string);
        // B-12 fix: SSE frames are not type-validated upstream — a
        // misbehaving server (or accidental message interleave from
        // another channel) could deliver a wrong-shape payload that
        // `as AuditEntry` cast would silently accept. Validate the
        // 5 required fields before committing into state.
        if (!isAuditEntry(raw)) {
          // Malformed shape — skip.
          return;
        }
        const entry = raw;
        setEntries((prev) => {
          const next = [...prev, entry];
          return next.slice(-limit);
        });
        setTimeout(() => {
          const container = tableContainerRef.current;
          if (!container) return;
          const nearBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 100;
          if (nearBottom) {
            container.lastElementChild?.lastElementChild?.scrollIntoView({ block: "end", behavior: "smooth" });
          }
        }, 0);
      } catch {
        // Malformed frame — skip.
      }
    };
    source.onerror = () => {
      setErr(L.streamLost);
    };
    return () => {
      source.close();
      sourceRef.current = null;
    };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoRefresh, kind, actor, limit]);

  return (
    <section className="space-y-4">
      <PageHeader title={L.title} subtitle={L.subtitleFmt(entries.length)} />
      <div className="flex flex-wrap items-end gap-2 text-xs">
        <FilterField label={L.kind} value={kind} onChange={setKind} placeholder="auth / approval / …" />
        <FilterField label={L.actor} value={actor} onChange={setActor} placeholder="cyberclaw / system / …" />
        <FilterField label={L.actionPrefix} value={actionPrefix} onChange={setActionPrefix} placeholder="login. / approval. / …" />
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.limit}</span>
          <select
            value={limit}
            onChange={(e) => setLimit(Number(e.target.value))}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border"
          >
            {[20, 50, 100, 200].map((n) => (
              <option key={n} value={n}>{n}</option>
            ))}
          </select>
        </label>
        <button
          onClick={() => void refresh()}
          disabled={loading}
          className="px-3 py-1.5 rounded-md bg-bg-3 border border-border hover:bg-hover disabled:opacity-50 text-xs"
        >
          {loading ? t("common.loading", lang) : L.refresh}
        </button>
        <label className="flex items-center gap-1 ml-auto">
          <input
            type="checkbox"
            checked={autoRefresh}
            onChange={(e) => setAutoRefresh(e.target.checked)}
          />
          <span className="text-fg-3">{L.liveTail}</span>
        </label>
      </div>

      {err && (
        <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>
      )}

      <div ref={tableContainerRef} className="rounded-lg border border-border overflow-hidden">
        <table className="w-full text-xs mono">
          <thead className="bg-bg-3">
            <tr className="text-left">
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colTs}</th>
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colKind}</th>
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colActor}</th>
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colAction}</th>
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colTarget}</th>
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colResult}</th>
            </tr>
          </thead>
          <tbody>
            {entries.length === 0 && !loading && (
              <tr>
                <td colSpan={6} className="px-3 py-8 text-center text-fg-4">
                  {L.noEntries}
                </td>
              </tr>
            )}
            {entries.map((e, i) => {
              const result = resultToBadge(e.result);
              return (
                <tr key={`${e.ts}-${i}`} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-1.5 text-fg-3 whitespace-nowrap">
                    {new Date(e.ts).toLocaleString()}
                  </td>
                  <td className={`px-3 py-1.5 ${KIND_TONE[e.kind] ?? "text-fg-3"}`}>
                    {e.kind}
                  </td>
                  <td className="px-3 py-1.5 text-fg-3">{e.actor}</td>
                  <td className="px-3 py-1.5">{e.action}</td>
                  <td className="px-3 py-1.5 text-fg-4">{e.target ?? "—"}</td>
                  <td className="px-3 py-1.5">
                    <span className={`px-1.5 py-0.5 rounded ${result.cls}`}>
                      {result.label}
                    </span>
                    {typeof e.result !== "string" && (
                      <span className="ml-2 text-fg-3 text-[10px]">{e.result.failure.reason}</span>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/audit/logs</code>
      </p>
    </section>
  );
}

function FilterField({
  label,
  value,
  onChange,
  placeholder,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  return (
    <label className="flex flex-col gap-0.5">
      <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{label}</span>
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="h-8 px-3 rounded-md bg-bg-3 border border-border outline-none focus:border-border-strong w-44"
      />
    </label>
  );
}
