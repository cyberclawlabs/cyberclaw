// SecurityPage — cyberclaw 安全内生能力总览。
//
// 后端端点：
//   GET  /api/v1/security/permission/rules   (DangerousCapabilityFilter + ToolPermissionMatcher)
//   PATCH /api/v1/security/permission/rules/:id  (进入 /reviews 审批流，不直接改 live 规则集)
//   GET  /api/v1/security/injection/hits     (Prompt injection 命中，audit sink 过滤)
//   GET  /api/v1/governance/policy-rules     (RuleBasedPolicyEngine 声明式规则)
//
// toggle 规则 → PATCH 端点 → 产生 proposal (status=pending) → /reviews 审批。
// 不存在绕过治理的直接 mutate 路径。

import { useEffect, useState } from "react";
import {
  type InjectionHit,
  type PermissionRule,
  type PolicyRule,
  type PolicyRulesResponse,
  fetchInjectionHits,
  fetchPermissionRules,
  fetchPolicyRules,
  togglePermissionRule,
} from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import { Shield } from "@/components/icons";
import PageHeader from "@/components/PageHeader";

// ---------------------------------------------------------------------------
// i18n
// ---------------------------------------------------------------------------

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "安全",
        subtitle: "cyberclaw 自带的多层防护规则与事件",
        kpiActiveRules: "启用规则",
        kpiHitsToday: "24h 命中",
        kpiCriticalHigh: "严重 / 高 规则",
        kpiPendingProposals: "待审批提案",
        kpiCriticalHighSub: (n: number) => `共 ${n} 条高优先`,
        tabRules: "规则",
        tabInjection: "注入命中",
        tabPolicy: "策略",
        colSource: "来源",
        colPattern: "Pattern",
        colAction: "Action",
        colSeverity: "Severity",
        colEnabled: "启用",
        colTs: "时间",
        colActor: "Actor",
        colPatternMatched: "命中 Pattern",
        colKind: "Kind",
        colAgent: "Agent",
        colCapability: "Capability",
        colPriority: "Priority",
        colReason: "原因",
        sourceNote:
          "规则来源：危险能力过滤器（dcf）+ 工具权限白名单（tpm）。Toggle 操作进入 /reviews 审批流，不直接改变 live 规则集。",
        noRules: "暂无规则",
        noHits: "暂无注入命中记录",
        noPolicy: "未配置声明式策略",
        noPolicyBody: "当前使用默认引擎（基于风险等级）。在配置中切换到规则引擎可启用声明式规则。",
        engineDefault: "引擎：默认（风险等级）",
        engineRuleBased: "引擎：声明式规则",
        proposalQueued: "已进入审批队列",
        proposalFailed: "提案失败",
        archTitle: "cyberclaw 安全内生架构",
        archDesc1: "cyberclaw 把安全做成平台内生能力，不是外挂模块",
        archDesc2: "检测 → 策略 → 审计 三层防护",
        archDesc3: "严重度 5 级贯穿 Capability / Skill / Connector",
        wildcard: "通配",
      }
    : {
        title: "Security",
        subtitle: "Built-in multi-layer protection rules and events",
        kpiActiveRules: "Active rules",
        kpiHitsToday: "Hits today",
        kpiCriticalHigh: "Critical/High rules",
        kpiPendingProposals: "Pending proposals",
        kpiCriticalHighSub: (n: number) => `${n} high-priority`,
        tabRules: "Rules",
        tabInjection: "Injection hits",
        tabPolicy: "Policy",
        colSource: "Source",
        colPattern: "Pattern",
        colAction: "Action",
        colSeverity: "Severity",
        colEnabled: "Enabled",
        colTs: "Time",
        colActor: "Actor",
        colPatternMatched: "Pattern matched",
        colKind: "Kind",
        colAgent: "Agent",
        colCapability: "Capability",
        colPriority: "Priority",
        colReason: "Reason",
        sourceNote:
          "Rule sources: dangerous-capability filter (dcf) + tool permission allowlist (tpm). Toggling enqueues a proposal into /reviews — it does NOT mutate the live rule set directly.",
        noRules: "No rules found",
        noHits: "No injection hits recorded",
        noPolicy: "No declarative policy configured",
        noPolicyBody:
          "Currently using the default engine (risk-level based). Switch to the rule-based engine in config to use declarative rules.",
        engineDefault: "engine: default (risk-level)",
        engineRuleBased: "engine: rule-based (declarative)",
        proposalQueued: "Proposal queued for review",
        proposalFailed: "Proposal failed",
        archTitle: "CyberClaw security architecture",
        archDesc1: "Security is a built-in platform capability, not a bolt-on module",
        archDesc2: "Detection → Policy → Audit three-layer defense",
        archDesc3: "5-level Severity across Capability / Skill / Connector",
        wildcard: "wildcard",
      };
}

// ---------------------------------------------------------------------------
// Severity color helpers
// ---------------------------------------------------------------------------

function severityClass(severity: string): string {
  switch (severity.toLowerCase()) {
    case "critical": return "text-rose-500";
    case "high":     return "text-orange-500";
    case "medium":   return "text-amber-500";
    case "low":      return "text-blue-500";
    default:         return "text-fg-3";  // Info / unknown
  }
}

function severityBadge(severity: string) {
  const base = "px-1.5 py-0.5 rounded text-[10px] font-medium uppercase tracking-wide";
  switch (severity.toLowerCase()) {
    case "critical": return `${base} bg-rose-500/15 text-rose-500`;
    case "high":     return `${base} bg-orange-500/15 text-orange-500`;
    case "medium":   return `${base} bg-amber-500/15 text-amber-500`;
    case "low":      return `${base} bg-blue-500/15 text-blue-500`;
    default:         return `${base} bg-fg-4/15 text-fg-3`;
  }
}

// ---------------------------------------------------------------------------
// KPI card (matches StatusPage pattern)
// ---------------------------------------------------------------------------

function KpiCard({
  label, value, sub,
}: { label: string; value: string | number; sub?: string }) {
  return (
    <div className="rounded-xl border border-border bg-bg-2 p-4 flex flex-col gap-1 min-w-0">
      <span className="text-[10px] mono text-fg-4 uppercase tracking-widest">{label}</span>
      <span className="text-2xl mono font-semibold text-fg truncate">{value}</span>
      {sub && <span className="text-[11px] text-fg-3 truncate">{sub}</span>}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Tab button
// ---------------------------------------------------------------------------

function TabBtn({
  label, active, onClick,
}: { label: string; active: boolean; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      className={[
        "px-4 py-1.5 text-xs rounded-md border transition-colors",
        active
          ? "bg-accent text-accent-fg border-accent"
          : "border-border text-fg-2 hover:bg-hover hover:text-fg",
      ].join(" ")}
    >
      {label}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Rules tab
// ---------------------------------------------------------------------------

function RulesTable({
  rules, lang, onToggle, busyId,
}: {
  rules: PermissionRule[];
  lang: Lang;
  onToggle: (rule: PermissionRule) => void;
  busyId: string | null;
}) {
  const L = dict(lang);
  if (rules.length === 0) {
    return <EmptyState icon={Shield} title={L.noRules} />;
  }
  return (
    <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
      <table className="w-full text-xs table-fixed">
        <thead className="bg-bg-3">
          <tr className="text-left">
            <th className="px-3 py-2 font-medium text-fg-3 w-[10%] mono">{L.colSource}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[35%]">{L.colPattern}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[12%]">{L.colAction}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[14%]">{L.colSeverity}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[10%] text-center">{L.colEnabled}</th>
          </tr>
        </thead>
        <tbody>
          {rules.map((r) => (
            <tr key={r.id} className="border-t border-border hover:bg-hover">
              <td className="px-3 py-2 mono text-fg-4 truncate">{r.source.toUpperCase()}</td>
              <td className="px-3 py-2 mono text-fg-2 truncate max-w-0 overflow-hidden">{r.pattern}</td>
              <td className="px-3 py-2">
                <span className={[
                  "px-1.5 py-0.5 rounded text-[10px] font-medium uppercase mono",
                  r.action === "deny" ? "bg-rose-500/10 text-rose-400" :
                  r.action === "ask"  ? "bg-amber-500/10 text-amber-400" :
                  r.action === "warn" ? "bg-orange-500/10 text-orange-400" :
                                        "bg-emerald-500/10 text-emerald-400",
                ].join(" ")}>
                  {r.action}
                </span>
              </td>
              <td className="px-3 py-2">
                <span className={severityBadge(r.severity)}>{r.severity}</span>
              </td>
              <td className="px-3 py-2 text-center">
                <button
                  disabled={busyId === r.id}
                  onClick={() => onToggle(r)}
                  className={[
                    "relative inline-flex h-5 w-9 items-center rounded-full transition-colors focus-ring",
                    "disabled:opacity-40",
                    r.enabled ? "bg-accent" : "bg-fg-4/30",
                  ].join(" ")}
                  title="Toggle (enters /reviews approval flow)"
                >
                  <span
                    className={[
                      "inline-block h-3.5 w-3.5 rounded-full bg-white shadow transition-transform",
                      r.enabled ? "translate-x-4" : "translate-x-1",
                    ].join(" ")}
                  />
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Injection hits tab
// ---------------------------------------------------------------------------

function fmtTs(ts: number): string {
  try {
    return new Date(ts).toLocaleString([], {
      month: "2-digit", day: "2-digit",
      hour: "2-digit", minute: "2-digit", second: "2-digit",
    });
  } catch {
    return String(ts);
  }
}

function InjectionTable({
  hits, lang,
}: { hits: InjectionHit[]; lang: Lang }) {
  const L = dict(lang);
  if (hits.length === 0) {
    return <EmptyState icon={Shield} title={L.noHits} />;
  }
  return (
    <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
      <table className="w-full text-xs table-fixed">
        <thead className="bg-bg-3">
          <tr className="text-left">
            <th className="px-3 py-2 font-medium text-fg-3 w-[18%]">{L.colTs}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[14%]">{L.colActor}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[35%]">{L.colPatternMatched}</th>
            <th className="px-3 py-2 font-medium text-fg-3 w-[13%]">{L.colSeverity}</th>
          </tr>
        </thead>
        <tbody>
          {hits.map((h) => (
            <tr key={h.id} className="border-t border-border hover:bg-hover">
              <td className="px-3 py-2 mono text-fg-3">{fmtTs(h.ts)}</td>
              <td className="px-3 py-2 mono text-fg-2 truncate">{h.actor}</td>
              <td className="px-3 py-2 mono text-fg-2 truncate max-w-0 overflow-hidden">{h.pattern}</td>
              <td className="px-3 py-2">
                <span className={severityBadge(h.severity)}>{h.severity}</span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Policy tab
// ---------------------------------------------------------------------------

function PolicyTable({
  resp, lang,
}: { resp: PolicyRulesResponse | null; lang: Lang }) {
  const L = dict(lang);
  if (!resp) {
    return <TableSkeleton cols={5} />;
  }
  const engineLabel = resp.engine === "rule_based" ? L.engineRuleBased : L.engineDefault;
  if (resp.rules.length === 0) {
    return (
      <div className="space-y-3">
        <p className="text-[10px] mono text-fg-4">{engineLabel}</p>
        <EmptyState icon={Shield} title={L.noPolicy} body={L.noPolicyBody} />
      </div>
    );
  }
  return (
    <div className="space-y-2">
      <p className="text-[10px] mono text-fg-4">{engineLabel}</p>
      <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
        <table className="w-full text-xs table-fixed">
          <thead className="bg-bg-3">
            <tr className="text-left">
              <th className="px-3 py-2 font-medium text-fg-3 w-[10%]">{L.colKind}</th>
              <th className="px-3 py-2 font-medium text-fg-3 w-[20%]">{L.colAgent}</th>
              <th className="px-3 py-2 font-medium text-fg-3 w-[25%]">{L.colCapability}</th>
              <th className="px-3 py-2 font-medium text-fg-3 w-[10%]">{L.colPriority}</th>
              <th className="px-3 py-2 font-medium text-fg-3 w-[35%]">{L.colReason}</th>
            </tr>
          </thead>
          <tbody>
            {resp.rules.map((r: PolicyRule, i: number) => (
              <tr key={i} className="border-t border-border hover:bg-hover">
                <td className="px-3 py-2">
                  <span className={[
                    "px-1.5 py-0.5 rounded text-[10px] font-medium uppercase mono",
                    r.kind === "deny" ? "bg-rose-500/10 text-rose-400" : "bg-emerald-500/10 text-emerald-400",
                  ].join(" ")}>
                    {r.kind}
                  </span>
                </td>
                <td className="px-3 py-2 mono text-fg-2 truncate">{r.agent_id ?? <span className="text-fg-4">{L.wildcard}</span>}</td>
                <td className="px-3 py-2 mono text-fg-2 truncate">{r.capability_id ?? <span className="text-fg-4">{L.wildcard}</span>}</td>
                <td className="px-3 py-2 mono text-fg-3">{r.priority}</td>
                <td className="px-3 py-2 text-fg-3 truncate max-w-0 overflow-hidden">{r.reason ?? "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Architecture callout
// ---------------------------------------------------------------------------

function ArchCallout({ lang }: { lang: Lang }) {
  const L = dict(lang);
  return (
    <div className="rounded-xl border border-accent/30 bg-accent/5 px-5 py-4 space-y-1.5">
      <p className="text-[11px] font-semibold text-accent">{L.archTitle}</p>
      <ul className="text-[11px] text-fg-2 space-y-0.5 list-disc list-inside">
        <li>{L.archDesc1}</li>
        <li>{L.archDesc2}</li>
        <li>{L.archDesc3}</li>
      </ul>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main page
// ---------------------------------------------------------------------------

type Tab = "rules" | "injection" | "policy";

export default function SecurityPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [tab, setTab] = useState<Tab>("rules");

  const [rules, setRules] = useState<PermissionRule[]>([]);
  const [rulesLoading, setRulesLoading] = useState(true);
  const [rulesErr, setRulesErr] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [statusMsg, setStatusMsg] = useState<string | null>(null);

  const [hits, setHits] = useState<InjectionHit[]>([]);
  const [hitsLoading, setHitsLoading] = useState(true);

  const [policyResp, setPolicyResp] = useState<PolicyRulesResponse | null>(null);

  useEffect(() => {
    fetchPermissionRules()
      .then(setRules)
      .catch((e: { status?: number; body?: string }) =>
        setRulesErr(`HTTP ${e?.status} ${e?.body ?? ""}`)
      )
      .finally(() => setRulesLoading(false));

    fetchInjectionHits()
      .then(setHits)
      .catch(() => setHits([]))
      .finally(() => setHitsLoading(false));

    fetchPolicyRules()
      .then(setPolicyResp)
      .catch(() => setPolicyResp({ engine: "default", rules: [] }));
  }, []);

  // KPI derivations
  const activeRules = rules.filter((r) => r.enabled).length;
  const criticalHighCount = rules.filter((r) =>
    r.severity.toLowerCase() === "critical" || r.severity.toLowerCase() === "high"
  ).length;
  // hits "today": ts is epoch ms, filter last 24h
  const now = Date.now();
  const hitsToday = hits.filter((h) => now - h.ts < 86_400_000).length;

  async function handleToggle(rule: PermissionRule) {
    setBusyId(rule.id);
    setStatusMsg(null);
    try {
      await togglePermissionRule(rule.id, !rule.enabled);
      setStatusMsg(L.proposalQueued);
      // optimistically update display to show intent; server keeps live rule unchanged until approved
      setRules((prev) =>
        prev.map((r) => (r.id === rule.id ? { ...r, enabled: !r.enabled } : r))
      );
    } catch {
      setStatusMsg(L.proposalFailed);
    } finally {
      setBusyId(null);
    }
  }

  return (
    <section className="space-y-5">
      <PageHeader
        title={L.title}
        subtitle={L.subtitle}
      />

      {/* KPI strip */}
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
        <KpiCard
          label={L.kpiActiveRules}
          value={rulesLoading ? "…" : activeRules}
          sub={rulesLoading ? undefined : `/ ${rules.length} total`}
        />
        <KpiCard
          label={L.kpiHitsToday}
          value={hitsLoading ? "…" : hitsToday}
          sub={hitsLoading ? undefined : `${hits.length} total`}
        />
        <KpiCard
          label={L.kpiCriticalHigh}
          value={rulesLoading ? "…" : criticalHighCount}
          sub={rulesLoading ? undefined : L.kpiCriticalHighSub(criticalHighCount)}
        />
        <KpiCard
          label={L.kpiPendingProposals}
          value="—"
          sub="/reviews"
        />
      </div>

      {/* Architecture callout */}
      <ArchCallout lang={lang} />

      {/* Tab bar */}
      <div className="flex gap-2 flex-wrap">
        <TabBtn label={L.tabRules} active={tab === "rules"} onClick={() => setTab("rules")} />
        <TabBtn label={L.tabInjection} active={tab === "injection"} onClick={() => setTab("injection")} />
        <TabBtn label={L.tabPolicy} active={tab === "policy"} onClick={() => setTab("policy")} />
      </div>

      {/* Status message */}
      {statusMsg && (
        <p className="text-xs px-2 py-1 rounded bg-accent/10 text-accent">{statusMsg}</p>
      )}

      {/* Error */}
      {rulesErr && tab === "rules" && (
        <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{rulesErr}</p>
      )}

      {/* Tab content */}
      {tab === "rules" && (
        <>
          {rulesLoading ? (
            <TableSkeleton cols={5} />
          ) : (
            <RulesTable
              rules={rules}
              lang={lang}
              onToggle={handleToggle}
              busyId={busyId}
            />
          )}
          <p className="text-[10px] text-fg-4">{L.sourceNote}</p>
        </>
      )}

      {tab === "injection" && (
        <>
          {hitsLoading ? (
            <TableSkeleton cols={4} />
          ) : (
            <InjectionTable hits={hits} lang={lang} />
          )}
        </>
      )}

      {tab === "policy" && (
        <PolicyTable resp={policyResp} lang={lang} />
      )}
    </section>
  );
}
