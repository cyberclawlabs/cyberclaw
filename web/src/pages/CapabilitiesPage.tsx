// CapabilitiesPage — 3 tabs: Capabilities / Monitor / Tools
// Merges: CapabilityMonitorPage + ToolsPage into this host.

import { useEffect, useState } from "react";
import { type Capability, type Tool, fetchCapabilities, fetchTools } from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import { Cpu, Shield, Wrench } from "@/components/icons";
import PageHeader from "@/components/PageHeader";

// ─── Shared ──────────────────────────────────────────────────────────────────

const RISK_TONE: Record<string, string> = {
  Low: "bg-emerald-500/15 text-emerald-300",
  Medium: "bg-amber-500/15 text-amber-300",
  High: "bg-rose-500/15 text-rose-300",
  Critical: "bg-rose-700/20 text-rose-300",
  low: "bg-emerald-500/15 text-emerald-300",
  medium: "bg-amber-500/15 text-amber-300",
  high: "bg-rose-500/15 text-rose-300",
  critical: "bg-rose-700/20 text-rose-300",
};

type Tab = "capabilities" | "monitor" | "tools";

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

/// Convert backend raw risk level string ("Low"/"Medium"/...) to i18n label.
function riskLabel(L: ReturnType<typeof dict>, raw: string): string {
  switch (raw) {
    case "Low":      return L.riskLow;
    case "Medium":   return L.riskMedium;
    case "High":     return L.riskHigh;
    case "Critical": return L.riskCritical;
    default:         return raw;
  }
}

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "能力与工具",
        tabCapabilities: "能力",
        tabMonitor: "监控",
        tabTools: "工具",
        riskLow: "低",
        riskMedium: "中",
        riskHigh: "高",
        riskCritical: "关键",
        source: "来源",
        all: "全部",
        // Capabilities tab
        colCapabilityId: "capability_id",
        colName: "名称",
        colRisk: "风险",
        colSource: "来源",
        colDescription: "描述",
        noCapabilities: "该来源下无能力",
        shownFmt: (n: number) => `已展示 ${n} 条`,
        // Monitor tab
        search: "搜索",
        verdict: "决策",
        risk: "风险",
        searchPlaceholder: "id / capability / connector",
        colId: "id",
        colConnector: "connector",
        colVerdict: "决策",
        colReason: "原因",
        colEvaluatedAt: "评估时间",
        noVerdicts: "未找到决策记录",
        noVerdictsFilter: "无匹配的决策记录",
        // Tools tab
        searchToolPlaceholder: "名称 / 描述",
        connector: "connector",
        colEffects: "效果",
        noTools: "未注册工具",
        noToolsFilter: "无匹配的工具",
      }
    : {
        title: "Capabilities & Tools",
        tabCapabilities: "Capabilities",
        tabMonitor: "Monitor",
        tabTools: "Tools",
        riskLow: "Low",
        riskMedium: "Medium",
        riskHigh: "High",
        riskCritical: "Critical",
        source: "Source",
        all: "all",
        // Capabilities tab
        colCapabilityId: "capability_id",
        colName: "name",
        colRisk: "risk",
        colSource: "source",
        colDescription: "description",
        noCapabilities: "No capabilities found for this source",
        shownFmt: (n: number) => `${n} shown`,
        // Monitor tab
        search: "search",
        verdict: "verdict",
        risk: "risk",
        searchPlaceholder: "id / capability / connector",
        colId: "id",
        colConnector: "connector",
        colVerdict: "verdict",
        colReason: "reason",
        colEvaluatedAt: "evaluated_at",
        noVerdicts: "No capability verdicts found",
        noVerdictsFilter: "No verdicts match the current filters",
        // Tools tab
        searchToolPlaceholder: "name / description",
        connector: "connector",
        colEffects: "effects",
        noTools: "No tools registered",
        noToolsFilter: "No tools match the current filters",
      };
}

// ─── Tab: Capabilities ───────────────────────────────────────────────────────

function CapabilitiesTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [capabilities, setCapabilities] = useState<Capability[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [source, setSource] = useState("all");
  const [sources, setSources] = useState<string[]>([]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    fetchCapabilities(50).then(
      (caps) => {
        if (cancelled) return;
        setCapabilities(caps);
        const uniq = Array.from(new Set(caps.map((c) => c.connector_id).filter(Boolean))) as string[];
        setSources(["all", ...uniq]);
      },
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, []);

  const visible = source === "all"
    ? capabilities
    : capabilities.filter((c) => c.connector_id === source);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-end gap-2 text-xs">
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.colSource}</span>
          <select
            value={source}
            onChange={(e) => setSource(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-32"
          >
            {sources.map((s) => (
              <option key={s} value={s}>{s === "all" ? L.all : s}</option>
            ))}
          </select>
        </label>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={5} />}

      {!loading && visible.length === 0 && (
        <EmptyState icon={Cpu} title={L.noCapabilities} />
      )}

      {!loading && visible.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs table-fixed">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3 w-36">{L.colCapabilityId}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-32">{L.colName}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-20">{L.colRisk}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-28">{L.colSource}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colDescription}</th>
              </tr>
            </thead>
            <tbody>
              {visible.map((cap) => (
                <tr key={cap.id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 mono text-fg-4 truncate">{cap.id}</td>
                  <td className="px-3 py-2 text-fg-2 truncate">{cap.title ?? "—"}</td>
                  <td className="px-3 py-2">
                    {cap.risk_level ? (
                      <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${RISK_TONE[cap.risk_level] ?? "bg-white/10 text-fg-3"}`}>
                        {riskLabel(L, cap.risk_level)}
                      </span>
                    ) : "—"}
                  </td>
                  <td className="px-3 py-2 mono text-fg-4 truncate">{cap.connector_id ?? "—"}</td>
                  <td className="px-3 py-2 text-fg-3 truncate">{cap.description ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/capabilities</code>. {L.shownFmt(visible.length)}
      </p>
    </div>
  );
}

// ─── Tab: Monitor ────────────────────────────────────────────────────────────

type CapabilityVerdict = {
  id: string;
  capability_id?: string;
  connector_id?: string;
  verdict?: "allow" | "deny" | "ask" | string;
  risk_level?: "Low" | "Medium" | "High" | "Critical" | string;
  reason?: string;
  evaluated_at?: string;
};

const VERDICT_TONE: Record<string, string> = {
  allow: "bg-emerald-500/15 text-emerald-300",
  deny: "bg-rose-500/15 text-rose-300",
  ask: "bg-amber-500/15 text-amber-300",
};

async function fetchCapabilityMonitor(): Promise<CapabilityVerdict[]> {
  const jwt = sessionStorage.getItem("cyberclaw.admin.jwt");
  const headers: HeadersInit = {};
  if (jwt) headers["Authorization"] = `Bearer ${jwt}`;
  const res = await fetch("/api/v1/capability-monitor", { headers });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw { status: res.status, body };
  }
  const data = (await res.json()) as CapabilityVerdict[] | { verdicts: CapabilityVerdict[] };
  return Array.isArray(data) ? data : (data as { verdicts?: CapabilityVerdict[] }).verdicts ?? [];
}

function MonitorTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [verdicts, setVerdicts] = useState<CapabilityVerdict[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [q, setQ] = useState("");
  const [verdict, setVerdict] = useState("");
  const [risk, setRisk] = useState("");

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    fetchCapabilityMonitor().then(
      (v) => { if (!cancelled) { setVerdicts(v); setLoading(false); } },
      (e: { status?: number; body?: string }) => {
        if (cancelled) return;
        setErr(`HTTP ${e?.status ?? "?"} ${e?.body ?? ""}`);
        setLoading(false);
      },
    );
    return () => { cancelled = true; };
  }, []);

  const filtered = verdicts.filter((v) => {
    if (q && !(v.id + " " + (v.capability_id ?? "") + " " + (v.connector_id ?? "")).toLowerCase().includes(q.toLowerCase())) return false;
    if (verdict && v.verdict !== verdict) return false;
    if (risk && v.risk_level !== risk) return false;
    return true;
  });

  return (
    <div className="space-y-4">
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
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.verdict}</span>
          <select
            value={verdict}
            onChange={(e) => setVerdict(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-24"
          >
            <option value="">{L.all}</option>
            <option value="allow">allow</option>
            <option value="deny">deny</option>
            <option value="ask">ask</option>
          </select>
        </label>
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.risk}</span>
          <select
            value={risk}
            onChange={(e) => setRisk(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-24"
          >
            <option value="">{L.all}</option>
            <option value="Low">{L.riskLow}</option>
            <option value="Medium">{L.riskMedium}</option>
            <option value="High">{L.riskHigh}</option>
            <option value="Critical">{L.riskCritical}</option>
          </select>
        </label>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={7} />}

      {!loading && filtered.length === 0 && (
        <EmptyState
          icon={Shield}
          title={verdicts.length === 0 ? L.noVerdicts : L.noVerdictsFilter}
        />
      )}

      {!loading && filtered.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs table-fixed">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3 w-24">{L.colId}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-36">{L.colCapabilityId}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-28">{L.colConnector}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-20">{L.colVerdict}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-20">{L.colRisk}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colReason}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-36">{L.colEvaluatedAt}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((v) => (
                <tr key={v.id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 mono text-fg-4 truncate">{v.id}</td>
                  <td className="px-3 py-2 text-fg-3 truncate">{v.capability_id ?? "—"}</td>
                  <td className="px-3 py-2 mono text-fg-4 truncate">{v.connector_id ?? "—"}</td>
                  <td className="px-3 py-2">
                    {v.verdict && (
                      <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${VERDICT_TONE[v.verdict] ?? "bg-white/10 text-fg-3"}`}>
                        {v.verdict}
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2">
                    {v.risk_level && (
                      <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${RISK_TONE[v.risk_level] ?? "bg-white/10 text-fg-3"}`}>
                        {riskLabel(L, v.risk_level)}
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2 text-fg-3 truncate">{v.reason ?? "—"}</td>
                  <td className="px-3 py-2 mono text-fg-4 truncate">{v.evaluated_at ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/capability-monitor</code>. {L.shownFmt(filtered.length)}
      </p>
    </div>
  );
}

// ─── Tab: Tools ───────────────────────────────────────────────────────────────

function ToolsTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [tools, setTools] = useState<Tool[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [q, setQ] = useState("");
  const [risk, setRisk] = useState("");
  const [connector, setConnector] = useState("");
  const [connectors, setConnectors] = useState<string[]>([]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    fetchTools().then(
      (ts) => {
        if (cancelled) return;
        setTools(ts);
        const uniq = Array.from(new Set(ts.map((t) => t.connector_id).filter(Boolean))) as string[];
        setConnectors(["all", ...uniq]);
      },
      (e) => !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
    ).finally(() => !cancelled && setLoading(false));
    return () => { cancelled = true; };
  }, []);

  const filtered = tools.filter((t) => {
    if (q && !((t.name + " " + (t.description ?? "")).toLowerCase().includes(q.toLowerCase()))) return false;
    if (risk && t.risk_level !== risk) return false;
    if (connector && connector !== "all" && t.connector_id !== connector) return false;
    return true;
  });

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-end gap-2 text-xs">
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.search}</span>
          <input
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder={L.searchToolPlaceholder}
            className="h-8 px-3 rounded-md bg-bg-3 border border-border outline-none focus-ring w-56"
          />
        </label>
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.connector}</span>
          <select
            value={connector}
            onChange={(e) => setConnector(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-36"
          >
            {connectors.map((c) => (
              <option key={c} value={c}>{c === "all" ? L.all : c}</option>
            ))}
          </select>
        </label>
        <label className="flex flex-col gap-0.5">
          <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">{L.risk}</span>
          <select
            value={risk}
            onChange={(e) => setRisk(e.target.value)}
            className="h-8 px-2 rounded-md bg-bg-3 border border-border w-28"
          >
            <option value="">{L.all}</option>
            <option value="low">low</option>
            <option value="medium">medium</option>
            <option value="high">high</option>
            <option value="critical">critical</option>
          </select>
        </label>
      </div>

      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}
      {loading && <TableSkeleton cols={5} />}

      {!loading && filtered.length === 0 && (
        <EmptyState
          icon={Wrench}
          title={tools.length === 0 ? L.noTools : L.noToolsFilter}
        />
      )}

      {!loading && filtered.length > 0 && (
        <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
          <table className="w-full text-xs table-fixed">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3 w-36">{L.colName}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-32">{L.connector}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-20">{L.colRisk}</th>
                <th className="px-3 py-2 font-medium text-fg-3 w-36">{L.colEffects}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colDescription}</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((t) => (
                <tr key={t.tool_id} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 font-medium truncate">{t.name}</td>
                  <td className="px-3 py-2 mono text-fg-4 truncate">{t.connector_id ?? "—"}</td>
                  <td className="px-3 py-2">
                    {t.risk_level ? (
                      <span className={`px-1.5 py-0.5 rounded text-[10px] mono ${RISK_TONE[t.risk_level] ?? "bg-white/10 text-fg-3"}`}>
                        {riskLabel(L, t.risk_level)}
                      </span>
                    ) : "—"}
                  </td>
                  <td className="px-3 py-2 mono text-fg-4 truncate">{t.effects?.join(", ") ?? "—"}</td>
                  <td className="px-3 py-2 text-fg-3 truncate">{t.description ?? "—"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <p className="text-[10px] text-fg-4">
        {L.source}: <code>/api/v1/tools</code>. {L.shownFmt(filtered.length)}
      </p>
    </div>
  );
}

// ─── Page ────────────────────────────────────────────────────────────────────

export default function CapabilitiesPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [tab, setTab] = useState<Tab>("capabilities");

  const tabs = (
    <>
      <TabBtn active={tab === "capabilities"} onClick={() => setTab("capabilities")}>{L.tabCapabilities}</TabBtn>
      <TabBtn active={tab === "monitor"} onClick={() => setTab("monitor")}>{L.tabMonitor}</TabBtn>
      <TabBtn active={tab === "tools"} onClick={() => setTab("tools")}>{L.tabTools}</TabBtn>
    </>
  );

  return (
    <section className="space-y-4">
      <PageHeader title={L.title} tabs={tabs} />
      {tab === "capabilities" && <CapabilitiesTab lang={lang} />}
      {tab === "monitor" && <MonitorTab lang={lang} />}
      {tab === "tools" && <ToolsTab lang={lang} />}
    </section>
  );
}
