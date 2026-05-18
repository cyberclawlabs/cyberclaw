// ReviewsPage — 3 tabs:
//   1. 审批队列 (existing)
//   2. Audit Agent — KPI + accuracy curve + learned patterns
//   3. Intent Parsing — dev tool (POST /api/v1/reviews/parse-intent; graceful degrade if 404)

import { useEffect, useRef, useState } from "react";
import {
  type AccuracyCurvePoint,
  type AuditAgentProfile,
  type IntentParseResult,
  type Review,
  fetchAuditAgentAccuracyCurve,
  fetchAuditAgentProfile,
  fetchReviews,
  parseIntent,
} from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import TableSkeleton from "@/components/TableSkeleton";
import EmptyState from "@/components/EmptyState";
import { Shield } from "@/components/icons";
import PageHeader from "@/components/PageHeader";

// ---------------------------------------------------------------------------
// Colour helpers
// ---------------------------------------------------------------------------

const STATUS_TONE: Record<string, string> = {
  pending: "bg-amber-500/15 text-amber-300",
  approved: "bg-emerald-500/15 text-emerald-300",
  rejected: "bg-rose-500/15 text-rose-300",
};

const RISK_TONE: Record<string, string> = {
  critical: "bg-rose-500/20 text-rose-300",
  high: "bg-orange-500/15 text-orange-300",
  medium: "bg-amber-500/15 text-amber-300",
  low: "bg-emerald-500/15 text-emerald-300",
};

function fmtDate(v: string | undefined): string {
  if (!v) return "—";
  const d = new Date(v);
  return Number.isNaN(d.getTime()) ? v : d.toLocaleString();
}

function fmtPct(v: number | undefined): string {
  if (v === undefined || v === null) return "—";
  return `${Math.round(v * 100)}%`;
}

// ---------------------------------------------------------------------------
// i18n
// ---------------------------------------------------------------------------

function dict(lang: Lang) {
  const zh = lang === "zh-CN";
  return {
    // tabs
    tabQueue:   zh ? "审批队列"   : "Approval Queue",
    tabAudit:   zh ? "审计 Agent"  : "Audit Agent",
    tabIntent:  zh ? "意图解析"   : "Intent Parsing",
    // queue
    title:       zh ? "审批"        : "Reviews",
    total:       (n: number) => zh ? `共 ${n} 条` : `${n} total`,
    statusLabel: zh ? "状态"        : "status",
    all:         zh ? "全部"        : "all",
    pending:     zh ? "待审批"      : "pending",
    approved:    zh ? "已批准"      : "approved",
    rejected:    zh ? "已拒绝"      : "rejected",
    noReviews:   zh ? "暂无审批请求" : "No reviews found",
    noReviewsBody: zh
      ? "当 Agent 动作需要人工审批时，审批请求会出现在这里。"
      : "Reviews are created when agent actions require human approval.",
    colId:       zh ? "ID"         : "id",
    colActor:    zh ? "执行者"      : "actor",
    colRisk:     zh ? "风险"        : "risk",
    colStatus:   zh ? "状态"        : "status",
    colDecision: zh ? "决策"        : "decision",
    colSummary:  zh ? "摘要"        : "summary",
    colCreated:  zh ? "创建时间"    : "created",
    source:      zh ? "来源"        : "Source",
    // audit agent
    auditTitle:     zh ? "审计 Agent 能力看板"              : "Audit Agent Dashboard",
    auditSubtitle:  zh ? "cyberclaw 内置自动复核能力"        : "cyberclaw built-in automated review capability",
    kpiModel:       zh ? "模型"                            : "Model",
    kpiTotal:       zh ? "累计审核"                        : "Total Reviewed",
    kpiAccuracy:    zh ? "精度（近 7 天）"                  : "Accuracy (last 7d)",
    kpiPending:     zh ? "待标注"                          : "Pending Labels",
    curveTitle:     zh ? "精度曲线"                        : "Accuracy Curve",
    curveEmpty:     zh ? "暂无精度曲线数据"                 : "No accuracy curve data yet",
    legendHuman:    zh ? "人工一致率"                      : "Human Agreement",
    legendFP:       zh ? "假阳率"                          : "False Positive",
    legendFN:       zh ? "假阴率"                          : "False Negative",
    archNote: zh
      ? "cyberclaw 用 Audit Agent 自动复核高风险操作，与人审协同 — 减少操作员负担，同时保持审批链可解释（每次决策都有 prompt + 模型版本 + accuracy 历史）。"
      : "cyberclaw uses an Audit Agent to automatically review high-risk operations alongside human reviewers — reducing operator burden while keeping the approval chain explainable (every decision has a prompt, model version, and accuracy history).",
    patternsTitle:  zh ? "已学模式"                        : "Learned Patterns",
    patternsEmpty:  zh ? "暂无已学模式"                    : "No learned patterns yet",
    colPattern:     zh ? "模式"                            : "Pattern",
    colCount:       zh ? "次数"                            : "Count",
    colLastSeen:    zh ? "最后出现"                        : "Last Seen",
    // intent parsing
    intentTitle:    zh ? "意图解析调试工具"                 : "Intent Parsing Debug Tool",
    intentSubtitle: zh ? "实验性 dev 工具 — 调用不写入 audit"  : "Experimental dev tool — calls do not write to audit",
    intentPlaceholder: zh ? "输入用户消息，例如：创建一个部署 Skill"
                          : "Enter a user message, e.g. create a deploy skill",
    parseBtn:       zh ? "解析意图"                        : "Parse Intent",
    parsing:        zh ? "解析中…"                         : "Parsing…",
    intentClass:    zh ? "意图分类"                        : "Intent Class",
    confidence:     zh ? "置信度"                          : "Confidence",
    entities:       zh ? "解析实体"                        : "Entities",
    route:          zh ? "推荐路由"                        : "Recommended Route",
    historyTitle:   zh ? "历史记录（最近 10 条）"           : "History (last 10)",
    historyEmpty:   zh ? "尚无历史"                        : "No history yet",
    errNotFound:    zh ? "端点暂未上线，返回模拟数据"         : "Endpoint not yet deployed — returning mock data",
    errGeneric:     zh ? "解析失败"                        : "Parse failed",
  };
}

// ---------------------------------------------------------------------------
// Tiny tab bar component
// ---------------------------------------------------------------------------

type TabItem = { id: string; label: string };

function TabBar({
  tabs,
  active,
  onChange,
}: {
  tabs: TabItem[];
  active: string;
  onChange: (id: string) => void;
}) {
  return (
    <div className="flex gap-1 border-b border-border mb-4">
      {tabs.map((t) => (
        <button
          key={t.id}
          onClick={() => onChange(t.id)}
          className={[
            "px-3 py-2 text-[12px] font-medium transition-colors border-b-2 -mb-px",
            active === t.id
              ? "border-accent text-fg"
              : "border-transparent text-fg-3 hover:text-fg-2",
          ].join(" ")}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// SVG Line Chart — pure SVG, no library dependencies
// ---------------------------------------------------------------------------

type Series = {
  values: (number | undefined)[];
  color: string;
  label: string;
};

function SvgLineChart({
  series,
  labels,
  height = 100,
}: {
  series: Series[];
  labels: string[];
  height?: number;
}) {
  const W = 600;
  const H = height;
  const PAD_L = 36;
  const PAD_R = 8;
  const PAD_T = 8;
  const PAD_B = 20;
  const innerW = W - PAD_L - PAD_R;
  const innerH = H - PAD_T - PAD_B;

  // Collect all defined values to find min/max
  const allVals = series.flatMap((s) => s.values.filter((v): v is number => v !== undefined));
  const minV = allVals.length ? Math.min(...allVals) : 0;
  const maxV = allVals.length ? Math.max(...allVals) : 1;
  const range = maxV - minV || 1;

  const n = labels.length;
  if (n < 2) return <div className="text-[11px] text-fg-4 mono py-4 text-center">—</div>;

  function xOf(i: number) {
    return PAD_L + (i / (n - 1)) * innerW;
  }
  function yOf(v: number) {
    return PAD_T + (1 - (v - minV) / range) * innerH;
  }

  const yTicks = [0, 0.25, 0.5, 0.75, 1].map((f) => minV + f * range);

  return (
    <svg
      viewBox={`0 0 ${W} ${H}`}
      className="w-full"
      style={{ height }}
      aria-label="accuracy curve chart"
    >
      {/* Y grid lines */}
      {yTicks.map((v, i) => (
        <g key={i}>
          <line
            x1={PAD_L}
            x2={W - PAD_R}
            y1={yOf(v)}
            y2={yOf(v)}
            stroke="currentColor"
            strokeOpacity={0.08}
            strokeWidth={1}
          />
          <text
            x={PAD_L - 4}
            y={yOf(v) + 3.5}
            fontSize={9}
            fill="currentColor"
            opacity={0.4}
            textAnchor="end"
          >
            {Math.round(v * 100)}
          </text>
        </g>
      ))}

      {/* X axis labels — every ~5 or fewer */}
      {labels.map((lbl, i) => {
        const step = Math.ceil(n / 6);
        if (i % step !== 0 && i !== n - 1) return null;
        return (
          <text
            key={i}
            x={xOf(i)}
            y={H - 4}
            fontSize={8}
            fill="currentColor"
            opacity={0.4}
            textAnchor="middle"
          >
            {lbl.slice(5)} {/* MM-DD */}
          </text>
        );
      })}

      {/* Series lines */}
      {series.map((s) => {
        // Build polyline points skipping undefined
        const pts: string[] = [];
        s.values.forEach((v, i) => {
          if (v !== undefined) pts.push(`${xOf(i)},${yOf(v)}`);
        });
        if (pts.length < 2) return null;
        return (
          <polyline
            key={s.label}
            points={pts.join(" ")}
            fill="none"
            stroke={s.color}
            strokeWidth={1.5}
            strokeLinejoin="round"
            strokeLinecap="round"
          />
        );
      })}

      {/* Dots at each defined point */}
      {series.map((s) =>
        s.values.map((v, i) =>
          v !== undefined ? (
            <circle key={`${s.label}-${i}`} cx={xOf(i)} cy={yOf(v)} r={2.5} fill={s.color} opacity={0.85} />
          ) : null,
        ),
      )}
    </svg>
  );
}

// ---------------------------------------------------------------------------
// Tab A: Audit Agent
// ---------------------------------------------------------------------------

function AuditAgentTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [profile, setProfile] = useState<AuditAgentProfile | null>(null);
  const [curve, setCurve] = useState<AccuracyCurvePoint[]>([]);
  const [loading, setLoading] = useState(true);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setErr(null);
    Promise.allSettled([fetchAuditAgentProfile(), fetchAuditAgentAccuracyCurve(7)]).then(
      ([pRes, cRes]) => {
        if (cancelled) return;
        if (pRes.status === "fulfilled") setProfile(pRes.value);
        else setErr(`profile: HTTP ${(pRes.reason as { status?: number })?.status ?? "?"}`);
        if (cRes.status === "fulfilled") setCurve(cRes.value.points ?? []);
        setLoading(false);
      },
    );
    return () => { cancelled = true; };
  }, []);

  const kpiCards = [
    {
      label: L.kpiModel,
      value: profile?.model_id ?? profile?.agent_id ?? "internal.audit.governor",
    },
    {
      label: L.kpiTotal,
      value: profile?.total_decisions ?? "—",
    },
    {
      label: L.kpiAccuracy,
      value: profile ? fmtPct(profile.accuracy) : "—",
    },
    {
      label: L.kpiPending,
      value: profile?.pending_labels ?? "—",
    },
  ];

  const chartLabels = curve.map((p) => p.date);
  const seriesHuman: Series = {
    label: L.legendHuman,
    color: "#34d399", // emerald-400
    values: curve.map((p) => p.human_agreement ?? p.accuracy),
  };
  const seriesFP: Series = {
    label: L.legendFP,
    color: "#fbbf24", // amber-400
    values: curve.map((p) => p.false_positive_rate),
  };
  const seriesFN: Series = {
    label: L.legendFN,
    color: "#fb7185", // rose-400
    values: curve.map((p) => p.false_negative_rate),
  };
  const activeSeries = [seriesHuman, seriesFP, seriesFN].filter(
    (s) => s.values.some((v) => v !== undefined),
  );

  return (
    <div className="space-y-4 anim-fade-in">
      {err && <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>}

      {/* KPI row */}
      {loading ? (
        <TableSkeleton cols={4} />
      ) : (
        <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
          {kpiCards.map((c) => (
            <div
              key={c.label}
              className="rounded-xl border border-border bg-bg-2 p-4 space-y-1"
            >
              <div className="text-[10px] uppercase tracking-widest text-fg-4 mono">{c.label}</div>
              <div className="text-[18px] font-semibold mono tabular-nums text-fg truncate">
                {String(c.value)}
              </div>
            </div>
          ))}
        </div>
      )}

      {/* Accuracy curve */}
      <div className="rounded-xl border border-border bg-bg-2 p-4 space-y-3">
        <div className="flex items-center justify-between">
          <span className="text-[11px] uppercase tracking-widest text-fg-4 mono">{L.curveTitle}</span>
          {/* Legend */}
          <div className="flex items-center gap-3 text-[10px] text-fg-4">
            {[seriesHuman, seriesFP, seriesFN].map((s) => (
              <span key={s.label} className="flex items-center gap-1">
                <span
                  className="inline-block w-3 h-0.5 rounded"
                  style={{ backgroundColor: s.color }}
                />
                {s.label}
              </span>
            ))}
          </div>
        </div>
        {loading ? (
          <div className="h-24 bg-bg-3 animate-pulse rounded" />
        ) : curve.length === 0 ? (
          <p className="text-[12px] text-fg-4 py-6 text-center">{L.curveEmpty}</p>
        ) : (
          <SvgLineChart series={activeSeries} labels={chartLabels} height={100} />
        )}
      </div>

      {/* Architecture note */}
      <div className="rounded-xl border border-amber-500/20 bg-amber-500/5 p-4">
        <p className="text-[12px] text-amber-200/80 leading-relaxed">{L.archNote}</p>
      </div>

      {/* Learned patterns table */}
      <div className="rounded-xl border border-border bg-bg-2 overflow-hidden">
        <div className="px-4 py-3 border-b border-border flex items-center justify-between">
          <span className="text-[12px] font-semibold text-fg">{L.patternsTitle}</span>
          <span className="text-[11px] text-fg-4 mono">
            {profile?.learned_patterns?.length ?? 0} entries
          </span>
        </div>
        {!profile || profile.learned_patterns.length === 0 ? (
          <p className="px-4 py-6 text-center text-[12px] text-fg-4">{L.patternsEmpty}</p>
        ) : (
          <table className="w-full text-xs">
            <thead className="bg-bg-3">
              <tr className="text-left">
                <th className="px-3 py-2 font-medium text-fg-3">{L.colPattern}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colCount}</th>
                <th className="px-3 py-2 font-medium text-fg-3">{L.colLastSeen}</th>
              </tr>
            </thead>
            <tbody>
              {profile.learned_patterns.map((p, i) => (
                <tr key={`${p.pattern}-${i}`} className="border-t border-border hover:bg-hover">
                  <td className="px-3 py-2 mono text-fg truncate max-w-xs">{p.pattern}</td>
                  <td className="px-3 py-2 mono text-fg-3 tabular-nums">{p.count}</td>
                  <td className="px-3 py-2 mono text-fg-4">{fmtDate(p.last_seen)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Tab B: Intent Parsing
// ---------------------------------------------------------------------------

type IntentHistoryEntry = {
  message: string;
  result: IntentParseResult;
  ts: number;
};

const MOCK_INTENT_RESULT: IntentParseResult = {
  intent_class: "CreateSkill",
  confidence: 0.87,
  entities: { skill_name: "deploy", category: "devops" },
  recommended_route: "/api/v1/skills/create",
};

function IntentParsingTab({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [message, setMessage] = useState("");
  const [result, setResult] = useState<IntentParseResult | null>(null);
  const [history, setHistory] = useState<IntentHistoryEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);

  const handleParse = async () => {
    const msg = message.trim();
    if (!msg || loading) return;
    setLoading(true);
    setNotice(null);
    setResult(null);
    try {
      const res = await parseIntent(msg);
      setResult(res);
      setHistory((h) => [{ message: msg, result: res, ts: Date.now() }, ...h].slice(0, 10));
    } catch (e: unknown) {
      const err = e as { status?: number };
      if (err.status === 404 || err.status === 405) {
        // Endpoint not deployed — graceful degrade to mock
        setNotice(L.errNotFound);
        setResult(MOCK_INTENT_RESULT);
        setHistory((h) =>
          [{ message: msg, result: MOCK_INTENT_RESULT, ts: Date.now() }, ...h].slice(0, 10),
        );
      } else {
        setNotice(`${L.errGeneric}: HTTP ${err.status ?? "?"}`);
      }
    } finally {
      setLoading(false);
    }
  };

  const handleKey = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void handleParse();
    }
  };

  return (
    <div className="space-y-4 anim-fade-in">
      {/* Input */}
      <div className="rounded-xl border border-border bg-bg-2 p-4 space-y-3">
        <textarea
          ref={inputRef}
          value={message}
          onChange={(e) => setMessage(e.target.value)}
          onKeyDown={handleKey}
          rows={3}
          placeholder={L.intentPlaceholder}
          className="w-full bg-bg-3 border border-border rounded-lg px-3 py-2 text-[13px] text-fg placeholder:text-fg-4 resize-none outline-none focus:border-accent/60 transition-colors"
        />
        <div className="flex items-center gap-2">
          <button
            onClick={handleParse}
            disabled={loading || !message.trim()}
            className="px-4 py-1.5 rounded-md bg-accent text-accent-fg text-[12px] font-medium hover:opacity-90 disabled:opacity-40 disabled:cursor-not-allowed transition-opacity"
          >
            {loading ? L.parsing : L.parseBtn}
          </button>
          {notice && (
            <span className="text-[11px] text-amber-300 bg-amber-500/10 px-2 py-1 rounded">
              {notice}
            </span>
          )}
        </div>
      </div>

      {/* Result */}
      {result && (
        <div className="rounded-xl border border-border bg-bg-2 p-4 space-y-3 anim-fade-in">
          <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
            <div className="rounded-lg bg-bg-3 border border-border p-3 space-y-1">
              <div className="text-[10px] uppercase tracking-widest text-fg-4 mono">{L.intentClass}</div>
              <div className="text-[14px] font-semibold text-fg mono">{result.intent_class}</div>
            </div>
            <div className="rounded-lg bg-bg-3 border border-border p-3 space-y-1">
              <div className="text-[10px] uppercase tracking-widest text-fg-4 mono">{L.confidence}</div>
              <div className="text-[14px] font-semibold text-fg mono tabular-nums">
                {fmtPct(result.confidence)}
                <span
                  className="ml-2 inline-block h-2 rounded-full bg-accent/30 align-middle"
                  style={{ width: `${Math.round(result.confidence * 64)}px` }}
                />
              </div>
            </div>
            <div className="col-span-2 rounded-lg bg-bg-3 border border-border p-3 space-y-1">
              <div className="text-[10px] uppercase tracking-widest text-fg-4 mono">{L.route}</div>
              <div className="text-[13px] text-fg mono">{result.recommended_route ?? "—"}</div>
            </div>
          </div>

          {/* Entities */}
          {Object.keys(result.entities ?? {}).length > 0 && (
            <div className="rounded-lg bg-bg-3 border border-border p-3 space-y-2">
              <div className="text-[10px] uppercase tracking-widest text-fg-4 mono">{L.entities}</div>
              <div className="flex flex-wrap gap-2">
                {Object.entries(result.entities).map(([k, v]) => (
                  <span
                    key={k}
                    className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md bg-bg-2 border border-border text-[11px] mono"
                  >
                    <span className="text-fg-4">{k}</span>
                    <span className="text-fg">{String(v)}</span>
                  </span>
                ))}
              </div>
            </div>
          )}
        </div>
      )}

      {/* History */}
      <div className="rounded-xl border border-border bg-bg-2 overflow-hidden">
        <div className="px-4 py-3 border-b border-border">
          <span className="text-[12px] font-semibold text-fg">{L.historyTitle}</span>
        </div>
        {history.length === 0 ? (
          <p className="px-4 py-6 text-center text-[12px] text-fg-4">{L.historyEmpty}</p>
        ) : (
          <div>
            {history.map((h) => (
              <div
                key={h.ts}
                className="px-4 py-3 border-t border-border/60 first:border-t-0 hover:bg-hover cursor-pointer"
                onClick={() => {
                  setMessage(h.message);
                  setResult(h.result);
                  inputRef.current?.focus();
                }}
              >
                <div className="flex items-start gap-3">
                  <div className="flex-1 min-w-0">
                    <div className="text-[12px] text-fg truncate">{h.message}</div>
                    <div className="text-[11px] text-fg-4 mono mt-0.5">
                      {h.result.intent_class} · {fmtPct(h.result.confidence)}
                    </div>
                  </div>
                  <div className="text-[10px] text-fg-4 mono shrink-0">
                    {new Date(h.ts).toLocaleTimeString()}
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main: ReviewsPage (3 tabs)
// ---------------------------------------------------------------------------

const STATUS_OPTIONS = ["all", "pending", "approved", "rejected"] as const;

export default function ReviewsPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [activeTab, setActiveTab] = useState("queue");

  // Queue tab state
  const [reviews, setReviews] = useState<Review[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [status, setStatus] = useState("all");

  useEffect(() => {
    if (activeTab !== "queue") return;
    let cancelled = false;
    setLoading(true);
    setErr(null);
    const s = status === "all" ? undefined : status;
    fetchReviews(s, 50)
      .then(
        (r) => !cancelled && setReviews(r),
        (e: { status?: number; body?: string }) =>
          !cancelled && setErr(`HTTP ${e?.status} ${e?.body ?? ""}`),
      )
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [status, activeTab]);

  const statusLabels: Record<string, string> = {
    all: L.all,
    pending: L.pending,
    approved: L.approved,
    rejected: L.rejected,
  };

  const tabs: TabItem[] = [
    { id: "queue",  label: L.tabQueue },
    { id: "audit",  label: L.tabAudit },
    { id: "intent", label: L.tabIntent },
  ];

  return (
    <section className="space-y-4">
      <PageHeader title={L.title} subtitle={L.total(reviews.length)} />

      <TabBar tabs={tabs} active={activeTab} onChange={setActiveTab} />

      {/* ---- Tab: Approval Queue ---- */}
      {activeTab === "queue" && (
        <div className="space-y-4">
          <div className="flex flex-wrap items-end gap-2 text-xs">
            <label className="flex flex-col gap-0.5">
              <span className="text-fg-3 font-medium uppercase tracking-wide text-[11px]">
                {L.statusLabel}
              </span>
              <select
                value={status}
                onChange={(e) => setStatus(e.target.value)}
                className="h-8 px-2 rounded-md bg-bg-3 border border-border w-32"
              >
                {STATUS_OPTIONS.map((s) => (
                  <option key={s} value={s}>
                    {statusLabels[s] ?? s}
                  </option>
                ))}
              </select>
            </label>
          </div>

          {err && (
            <p className="text-xs text-rose-400 px-2 py-1.5 bg-rose-500/10 rounded">{err}</p>
          )}
          {loading && <TableSkeleton cols={7} />}
          {!loading && reviews.length === 0 && (
            <EmptyState icon={Shield} title={L.noReviews} body={L.noReviewsBody} />
          )}
          {!loading && reviews.length > 0 && (
            <div className="rounded-lg border border-border overflow-hidden bg-bg-2">
              <table className="w-full text-xs">
                <thead className="bg-bg-3">
                  <tr className="text-left">
                    <th className="px-3 py-2 font-medium text-fg-3">{L.colId}</th>
                    <th className="px-3 py-2 font-medium text-fg-3">{L.colActor}</th>
                    <th className="px-3 py-2 font-medium text-fg-3">{L.colRisk}</th>
                    <th className="px-3 py-2 font-medium text-fg-3">{L.colStatus}</th>
                    <th className="px-3 py-2 font-medium text-fg-3">{L.colDecision}</th>
                    <th className="px-3 py-2 font-medium text-fg-3">{L.colSummary}</th>
                    <th className="px-3 py-2 font-medium text-fg-3">{L.colCreated}</th>
                  </tr>
                </thead>
                <tbody>
                  {reviews.map((r) => (
                    <tr key={r.id} className="border-t border-border hover:bg-hover">
                      <td className="px-3 py-2 mono text-fg-4 truncate max-w-[120px]">{r.id}</td>
                      <td className="px-3 py-2 mono text-fg-4">{r.actor ?? r.agent_id ?? "—"}</td>
                      <td className="px-3 py-2">
                        {r.risk ? (
                          <span
                            className={`px-1.5 py-0.5 rounded text-[10px] mono ${RISK_TONE[r.risk.toLowerCase()] ?? "bg-white/10 text-fg-3"}`}
                          >
                            {r.risk}
                          </span>
                        ) : (
                          "—"
                        )}
                      </td>
                      <td className="px-3 py-2">
                        <span
                          className={`px-1.5 py-0.5 rounded text-[10px] mono ${STATUS_TONE[r.status] ?? "bg-white/10 text-fg-3"}`}
                        >
                          {r.status}
                        </span>
                      </td>
                      <td className="px-3 py-2 text-fg-3">{r.decision ?? "—"}</td>
                      <td className="px-3 py-2 text-fg-3 max-w-xs truncate">{r.summary ?? "—"}</td>
                      <td className="px-3 py-2 mono text-fg-4">{fmtDate(r.created_at)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
          <p className="text-[10px] text-fg-4">
            {L.source}: <code>/api/v1/reviews</code>
          </p>
        </div>
      )}

      {/* ---- Tab: Audit Agent ---- */}
      {activeTab === "audit" && <AuditAgentTab lang={lang} />}

      {/* ---- Tab: Intent Parsing ---- */}
      {activeTab === "intent" && <IntentParsingTab lang={lang} />}
    </section>
  );
}
