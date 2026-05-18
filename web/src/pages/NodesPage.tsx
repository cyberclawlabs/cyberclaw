// NodesPage — minimal v2 port.
// Endpoint: GET /api/v1/cluster/nodes (live, returns 200).
// Full cluster management on /admin (page = 'nodes').

import { useEffect, useState } from "react";
import { type ClusterNode, fetchNodes } from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import { Network } from "@/components/icons";

const STATUS_TONE: Record<string, string> = {
  leader: "bg-emerald-500/15 text-emerald-300",
  follower: "bg-white/10 text-fg-3",
  candidate: "bg-amber-500/15 text-amber-300",
  offline: "bg-rose-500/15 text-rose-300",
};

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "节点",
        totalFmt: (n: number) => `共 ${n} 个`,
        search: "搜索",
        searchPlaceholder: "node_id / addr / region",
        role: "角色",
        all: "全部",
        noNodes: "未找到集群节点",
        noNodesFilter: "无匹配的节点",
        colNodeId: "node_id",
        colRole: "角色",
        colStatus: "状态",
        colLastHeartbeat: "最后心跳",
        colRegion: "区域",
        colActiveExecs: "活跃执行",
        colVersion: "版本",
        source: "来源",
      }
    : {
        title: "Nodes",
        totalFmt: (n: number) => `${n} total`,
        search: "search",
        searchPlaceholder: "node_id / addr / region",
        role: "role",
        all: "all",
        noNodes: "No cluster nodes found",
        noNodesFilter: "No nodes match the current filters",
        colNodeId: "node_id",
        colRole: "role",
        colStatus: "status",
        colLastHeartbeat: "last_heartbeat",
        colRegion: "region",
        colActiveExecs: "active_execs",
        colVersion: "version",
        source: "Source",
      };
}

export default function NodesPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [nodes, setNodes] = useState<ClusterNode[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [q, setQ] = useState("");
  const [role, setRole] = useState("");

  useEffect(() => {
    let cancelled = false;
    fetchNodes().then(
      (n) => !cancelled && setNodes(n),
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, []);

  const roles = Array.from(new Set(nodes.map((n) => n.role).filter(Boolean))) as string[];

  const filtered = nodes.filter((n) => {
    if (q && !(n.node_id + " " + (n.addr ?? "") + " " + (n.region ?? "")).toLowerCase().includes(q.toLowerCase())) return false;
    if (role && n.role !== role) return false;
    return true;
  });

  return (
    <section className="space-y-4">
      <header className="space-y-1">
        <h2 className="text-base font-medium text-fg">{L.title}</h2>
        <p className="text-xs text-fg-3">{L.totalFmt(nodes.length)}</p>
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
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.role}</span>
          <select
            value={role}
            onChange={(e) => setRole(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-32"
          >
            <option value="">{L.all}</option>
            {roles.map((r) => (
              <option key={r} value={r}>{r}</option>
            ))}
          </select>
        </label>
      </div>

      {err && (
        <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>
      )}

      {loading && <TableSkeleton cols={7} />}

      {!loading && filtered.length === 0 && (
        <EmptyState
          icon={Network}
          title={nodes.length === 0 ? L.noNodes : L.noNodesFilter}
        />
      )}

      {!loading && filtered.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colNodeId}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colRole}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colStatus}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colLastHeartbeat}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colRegion}</th>
                <th className="px-3 py-2 font-medium text-fg-3 mono text-right">{L.colActiveExecs}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colVersion}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((n) => (
                <tr key={n.node_id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 mono text-fg-4 max-w-[200px] truncate">{n.node_id}</td>
                  <td className="px-3 py-2 mono text-fg-3">{n.role ?? "—"}</td>
                  <td className="px-3 py-2">
                    {n.status && (
                      <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${STATUS_TONE[n.status] ?? "bg-white/10 text-fg-3"}`}>
                        {n.status}
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2 mono text-fg-4">{n.last_heartbeat ? n.last_heartbeat.replace("T", " ").slice(0, 19) : "—"}</td>
                  <td className="px-3 py-2 text-fg-3">{n.region ?? "—"}</td>
                  <td className="px-3 py-2 mono text-right text-fg-3">{n.active_execs ?? "—"}</td>
                  <td className="px-3 py-2 mono text-fg-4">{n.version ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/cluster/nodes</code>
      </p>
    </section>
  );
}
