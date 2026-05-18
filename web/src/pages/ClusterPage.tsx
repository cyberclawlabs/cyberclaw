// ClusterPage — minimal v2 port. Brain coordinator state + registered brains.
// Endpoint /api/v1/cluster returns 200 with cluster summary; empty cluster still renders empty-state.
// Note: /cluster/nodes is handled by NodesPage (different endpoint, live).
// Full management on /admin → cluster.

import { useEffect, useState } from "react";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import { Layers } from "@/components/icons";

type BrainEntry = {
  brain_id: string;
  role?: "coordinator" | "worker" | string;
  status?: "active" | "idle" | "offline" | string;
  last_heartbeat?: string;
  active_loops?: number;
  region?: string;
  addr?: string;
};

type ClusterState = {
  coordinator_id?: string;
  election_term?: number;
  brains?: BrainEntry[];
  total_active_loops?: number;
};

type ClusterResponse = ClusterState | BrainEntry[];

const STATUS_TONE: Record<string, string> = {
  active: "bg-emerald-500/15 text-emerald-300",
  idle: "bg-white/10 text-fg-3",
  offline: "bg-rose-500/15 text-rose-300",
};

async function fetchClusterState(): Promise<{ state: ClusterState | null; brains: BrainEntry[] }> {
  const jwt = sessionStorage.getItem("cyberclaw.admin.jwt");
  const headers: HeadersInit = {};
  if (jwt) headers["Authorization"] = `Bearer ${jwt}`;
  const res = await fetch("/api/v1/cluster", { headers });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw { status: res.status, body };
  }
  const data = (await res.json()) as ClusterResponse;
  if (Array.isArray(data)) {
    return { state: null, brains: data };
  }
  const s = data as ClusterState;
  return { state: s, brains: s.brains ?? [] };
}

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "集群",
        totalFmt: (n: number) => `${n} 个节点已注册`,
        coordinator: "协调器",
        electionTerm: "选举轮次",
        activeLoops: "活跃循环",
        status: "状态",
        all: "全部",
        noBrains: "集群中未注册节点",
        noBrainsFilter: "无匹配的节点",
        colBrainId: "brain_id",
        colRole: "角色",
        colStatus: "状态",
        colLastHeartbeat: "最后心跳",
        colLoops: "循环数",
        colRegion: "区域",
        colAddr: "地址",
        source: "来源",
      }
    : {
        title: "Cluster",
        totalFmt: (n: number) => `${n} brains registered`,
        coordinator: "coordinator",
        electionTerm: "election term",
        activeLoops: "active loops",
        status: "status",
        all: "all",
        noBrains: "No brains registered in the cluster",
        noBrainsFilter: "No brains match the current filters",
        colBrainId: "brain_id",
        colRole: "role",
        colStatus: "status",
        colLastHeartbeat: "last_heartbeat",
        colLoops: "loops",
        colRegion: "region",
        colAddr: "addr",
        source: "Source",
      };
}

export default function ClusterPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [state, setState] = useState<ClusterState | null>(null);
  const [brains, setBrains] = useState<BrainEntry[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [status, setStatus] = useState("");
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    fetchClusterState().then(
      ({ state: s, brains: b }) => {
        if (cancelled) return;
        setState(s);
        setBrains(b);
        setLoading(false);
      },
      (e: { status?: number; body?: string }) => {
        if (cancelled) return;
        setErr(`HTTP ${e?.status ?? "?"} ${e?.body ?? ""}`);
        setLoading(false);
      },
    );
    return () => { cancelled = true; };
  }, []);

  const filtered = brains.filter((b) => {
    if (status && b.status !== status) return false;
    return true;
  });

  return (
    <section className="space-y-4">
      <header className="space-y-1">
        <h2 className="text-base font-medium text-fg">{L.title}</h2>
        <p className="text-xs text-fg-3">{L.totalFmt(brains.length)}</p>
      </header>

      {state && (
        <div className="grid grid-cols-2 sm:grid-cols-3 gap-2 text-xs">
          <div className="rounded-lg border border-border bg-bg-2 px-3 py-2 space-y-0.5">
            <div className="text-[10px] mono text-fg-4 uppercase">{L.coordinator}</div>
            <div className="font-medium truncate">{state.coordinator_id ?? "—"}</div>
          </div>
          <div className="rounded-lg border border-border bg-bg-2 px-3 py-2 space-y-0.5">
            <div className="text-[10px] mono text-fg-4 uppercase">{L.electionTerm}</div>
            <div className="font-medium mono">{state.election_term ?? "—"}</div>
          </div>
          <div className="rounded-lg border border-border bg-bg-2 px-3 py-2 space-y-0.5">
            <div className="text-[10px] mono text-fg-4 uppercase">{L.activeLoops}</div>
            <div className="font-medium mono">{state.total_active_loops ?? "—"}</div>
          </div>
        </div>
      )}

      <div className="flex flex-wrap items-end gap-2 text-xs">
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.status}</span>
          <select
            value={status}
            onChange={(e) => setStatus(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-28"
          >
            <option value="">{L.all}</option>
            <option value="active">active</option>
            <option value="idle">idle</option>
            <option value="offline">offline</option>
          </select>
        </label>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}

      {loading && <TableSkeleton cols={7} />}

      {!loading && filtered.length === 0 && (
        <EmptyState
          icon={Layers}
          title={brains.length === 0 ? L.noBrains : L.noBrainsFilter}
        />
      )}

      {!loading && filtered.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colBrainId}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colRole}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colStatus}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colLastHeartbeat}</th>
                <th className="px-3 py-2 font-medium text-fg-3 mono text-right">{L.colLoops}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colRegion}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colAddr}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((b) => (
                <tr key={b.brain_id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 mono text-fg-4">{b.brain_id}</td>
                  <td className="px-3 py-2 text-fg-3 mono">{b.role ?? "—"}</td>
                  <td className="px-3 py-2">
                    {b.status && (
                      <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${STATUS_TONE[b.status] ?? "bg-white/10"}`}>
                        {b.status}
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2 mono text-fg-4">{b.last_heartbeat ?? "—"}</td>
                  <td className="px-3 py-2 mono text-right text-fg-3">{b.active_loops ?? "—"}</td>
                  <td className="px-3 py-2 text-fg-3">{b.region ?? "—"}</td>
                  <td className="px-3 py-2 mono text-fg-4">{b.addr ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/cluster</code>. v2 minimal port — full management on /admin.
      </p>
    </section>
  );
}
