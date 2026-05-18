// ModelsPage — config-bound model catalog + usage statistics (v2 stack).
// Catalog source: GET /api/v1/admin/llm/models (backed by ~/.cyberclaw/models.json).
// User can: 设为默认 (PUT default) / 删除 (DELETE). Per-task-type localStorage
// defaults remain orthogonal (browser-only hint, not pushed to server).

import { useEffect, useRef, useState } from "react";
import { type Lang } from "@/lib/i18n";
import {
  fetchUsage,
  fetchModelsCatalog,
  deleteModelFromCatalog,
  setDefaultModel,
  type UsageInfo,
  type ModelUsageSnapshot,
  type ModelsCatalog,
  type ModelEntry,
} from "@/lib/api";
import { useToast } from "@/components/ToastBar";
import Modal from "@/components/Modal";

const TASK_TYPES = ["chat", "embedding", "code", "creative"] as const;
type TaskType = (typeof TASK_TYPES)[number];

const DEFAULTS_KEY = "cyberclaw.admin.models.defaults";

type ModelDefaults = Partial<Record<TaskType, string>>;

function loadDefaults(): ModelDefaults {
  try {
    const raw = localStorage.getItem(DEFAULTS_KEY);
    return raw ? (JSON.parse(raw) as ModelDefaults) : {};
  } catch {
    return {};
  }
}

function saveDefaults(d: ModelDefaults): void {
  try {
    localStorage.setItem(DEFAULTS_KEY, JSON.stringify(d));
  } catch {
    // ignore
  }
}

const PROVIDER_TONE: Record<string, string> = {
  anthropic: "bg-amber-500/15 text-amber-300",
  openai: "bg-emerald-500/15 text-emerald-300",
  minimax: "bg-blue-500/15 text-blue-300",
  ollama: "bg-purple-500/15 text-purple-300",
};

function fmtNum(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

function fmtCost(n: number): string {
  return `$${n.toFixed(4)}`;
}

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        registryTitle: "模型管理",
        registrySubtitle: "可调用的 LLM 目录 + 24 小时实际用量。",
        registrySourceBanner: "目录持久化到 ~/.cyberclaw/models.json：点「设默认」修改服务端默认（写文件），点「删除」移除条目，点「用作 ▾」记录本地任务类型偏好（仅当前浏览器）。下方为该 model 真实被调用统计。",
        colModelId: "model id",
        colLabel: "名称",
        colProvider: "提供商",
        colRole: "角色",
        colRequests: "请求数",
        colTokens: "token 数",
        colCost: "费用",
        colStatus: "状态",
        colUseAs: "操作",
        available: "可用",
        defaultForFmt: (types: string) => `★ 默认用于：${types}`,
        defaultForTaskFmt: (t: string) => `设为 ${t} 默认`,
        setDefaultToast: (label: string, task: string) => `${label} → ${task} 已设为默认`,
        usageTitle: "用量统计",
        usageSubtitleOk: "聚合用量数据，来自 /api/v1/usage（24 小时窗口，内存统计）。",
        usageSubtitleErr: (e: string) => `加载失败：${e}`,
        statModelsInUse: "模型使用数",
        statTotalTokens: "总 Token（24h）",
        statTotalSessions: "总会话数",
        statEstCost: "预估费用（$）",
        useAs: "用作 ▾",
      }
    : {
        registryTitle: "Model management",
        registrySubtitle: "Callable LLM catalog + 24h actual usage.",
        registrySourceBanner: "Catalog persists to ~/.cyberclaw/models.json: \"Set default\" updates the server-wide default (writes the file), \"Delete\" removes the entry, \"Use as ▾\" records a per-task-type browser-local preference. Stats below reflect real calls.",
        colModelId: "model id",
        colLabel: "label",
        colProvider: "provider",
        colRole: "role hint",
        colRequests: "requests",
        colTokens: "tokens",
        colCost: "cost",
        colStatus: "status",
        colUseAs: "actions",
        available: "available",
        defaultForFmt: (types: string) => `★ default for: ${types}`,
        defaultForTaskFmt: (t: string) => `default for ${t}`,
        setDefaultToast: (label: string, task: string) => `${label} → ${task} default`,
        usageTitle: "Usage statistics",
        usageSubtitleOk: "Aggregated usage data from /api/v1/usage (24 h window, in-memory).",
        usageSubtitleErr: (e: string) => `Failed to load: ${e}`,
        statModelsInUse: "Models in use",
        statTotalTokens: "Total tokens (24h)",
        statTotalSessions: "Total sessions",
        statEstCost: "Estimated cost ($)",
        useAs: "Use as ▾",
      };
}

export default function ModelsPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const [usage, setUsage] = useState<UsageInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [defaults, setDefaults] = useState<ModelDefaults>(loadDefaults);
  const [menuOpen, setMenuOpen] = useState<string | null>(null);
  const [catalog, setCatalog] = useState<ModelsCatalog | null>(null);
  const [catalogErr, setCatalogErr] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [pendingDelete, setPendingDelete] = useState<ModelEntry | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const toast = useToast();

  useEffect(() => {
    fetchUsage()
      .then(setUsage)
      .catch((e) => setError(String(e?.body ?? e)));
    fetchModelsCatalog()
      .then(setCatalog)
      .catch((e) => setCatalogErr(String(e?.body ?? e)));
  }, []);

  const confirmDelete = async () => {
    const m = pendingDelete;
    if (!m) return;
    setPendingDelete(null);
    const label = m.label ?? m.id;
    setBusy(m.id);
    try {
      const updated = await deleteModelFromCatalog(m.id);
      setCatalog(updated);
      toast({ tone: "success", msg: lang === "zh-CN" ? `已删除「${label}」` : `Deleted "${label}"` });
    } catch (e) {
      const msg = typeof e === "object" && e && "body" in e ? String((e as { body: unknown }).body) : String(e);
      toast({ tone: "error", msg: lang === "zh-CN" ? `删除失败：${msg}` : `Delete failed: ${msg}` });
    } finally {
      setBusy(null);
    }
  };

  const handleSetServerDefault = async (m: ModelEntry) => {
    const label = m.label ?? m.id;
    setBusy(m.id);
    try {
      const updated = await setDefaultModel(m.id);
      setCatalog(updated);
      toast({ tone: "success", msg: lang === "zh-CN" ? `服务端默认模型已设为「${label}」` : `Server default → "${label}"` });
    } catch (e) {
      const msg = typeof e === "object" && e && "body" in e ? String((e as { body: unknown }).body) : String(e);
      toast({ tone: "error", msg: lang === "zh-CN" ? `设置默认失败：${msg}` : `Set default failed: ${msg}` });
    } finally {
      setBusy(null);
    }
  };

  // Close dropdown on outside click
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setMenuOpen(null);
      }
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, []);

  const setDefault = (modelId: string, modelLabel: string, taskType: TaskType) => {
    const next = { ...defaults, [taskType]: modelId };
    setDefaults(next);
    saveDefaults(next);
    setMenuOpen(null);
    toast({ tone: "success", msg: L.setDefaultToast(modelLabel, taskType) });
  };

  const modelsInUse = usage ? Object.keys(usage.by_model).length : null;
  const totalTokens = usage
    ? usage.total_input_tokens + usage.total_output_tokens
    : null;

  const stats = [
    { label: L.statModelsInUse, value: modelsInUse !== null ? String(modelsInUse) : "—" },
    { label: L.statTotalTokens, value: totalTokens !== null ? fmtNum(totalTokens) : "—" },
    { label: L.statTotalSessions, value: usage ? fmtNum(usage.total_sessions) : "—" },
    { label: L.statEstCost, value: usage ? fmtCost(usage.estimated_cost_usd) : "—" },
  ];

  // Which task types is this model the default for?
  const defaultsFor = (modelId: string): TaskType[] =>
    TASK_TYPES.filter((t) => defaults[t] === modelId);

  return (
    <section className="space-y-5" onClick={() => setMenuOpen(null)}>
      <header className="space-y-1">
        <h2 className="text-base font-medium">{L.registryTitle}</h2>
        <p className="text-xs opacity-60">{L.registrySubtitle}</p>
      </header>

      <p className="text-[11px] text-amber-300/90 px-3 py-2 bg-amber-500/10 rounded border border-amber-500/20 leading-relaxed">
        {L.registrySourceBanner}
      </p>

      {/* B-class fix: 不能用 overflow-hidden — 内嵌 "设为默认" dropdown
          (absolute-positioned) 会被 clip。改成 overflow-visible 让 dropdown
          能溢出表格边界正常显示；表头圆角的轻微越界由内部 thead 的圆角
          补偿，视觉无显著退化。 */}
      <div className="rounded-lg border border-border overflow-visible">
        <table className="w-full text-xs mono">
          <thead className="bg-bg-3">
            <tr className="text-left">
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colModelId}</th>
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colLabel}</th>
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colProvider}</th>
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colRole}</th>
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colRequests}</th>
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colTokens}</th>
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colCost}</th>
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colStatus}</th>
              <th className="px-3 py-2 text-fg-3 font-medium">{L.colUseAs}</th>
            </tr>
          </thead>
          <tbody>
            {(catalog?.models ?? []).map((m) => {
              const snap: ModelUsageSnapshot | undefined = usage?.by_model[m.id];
              const tokens = snap
                ? fmtNum(snap.input_tokens + snap.output_tokens)
                : "—";
              const requests = snap ? fmtNum(snap.requests) : "—";
              const costStatus: string = (snap as (ModelUsageSnapshot & { cost_status?: string }) | undefined)?.cost_status ?? "estimated";
              const costUsd = (snap as (ModelUsageSnapshot & { cost_usd?: number }) | undefined)?.cost_usd;
              const cost = costUsd != null ? fmtCost(costUsd) : snap ? "—" : "—";
              const myDefaults = defaultsFor(m.id);
              const isMenuOpen = menuOpen === m.id;
              const isServerDefault = catalog?.current_default === m.id;
              const isBusy = busy === m.id;
              return (
                <tr key={m.id} className="border-t border-border">
                  <td className="px-3 py-2 text-fg-3">{m.id}</td>
                  <td className="px-3 py-2 font-medium">
                    <div className="flex items-center gap-1.5">
                      <span>{m.label ?? m.id}</span>
                      {isServerDefault && (
                        <span className="text-[10px] text-amber-400 font-semibold" title={lang === "zh-CN" ? "服务端默认" : "Server default"}>★</span>
                      )}
                    </div>
                    {myDefaults.length > 0 && (
                      <div className="text-[10px] text-amber-400 mt-0.5">
                        {L.defaultForFmt(myDefaults.join(", "))}
                      </div>
                    )}
                  </td>
                  <td className="px-3 py-2">
                    {m.provider && (
                      <span className={`px-1.5 py-0.5 rounded ${PROVIDER_TONE[m.provider] ?? "bg-bg-3"}`}>
                        {m.provider}
                      </span>
                    )}
                  </td>
                  <td className="px-3 py-2 text-fg-3">{m.role ?? ""}</td>
                  <td className="px-3 py-2 text-fg-3">{requests}</td>
                  <td className="px-3 py-2 text-fg-3">{tokens}</td>
                  <td className="px-3 py-2 text-fg-3">
                    {cost}
                    {snap && costStatus === "exact" && (
                      <span className="ml-1 text-[10px] text-emerald-400">✓ exact</span>
                    )}
                    {snap && costStatus === "estimated" && (
                      <span className="ml-1 text-[10px] text-fg-4 italic">~ est</span>
                    )}
                    {snap && costStatus === "unknown" && (
                      <span className="ml-1 text-[10px] text-fg-4">?</span>
                    )}
                  </td>
                  <td className="px-3 py-2">
                    <span className="px-1.5 py-0.5 rounded bg-emerald-500/15 text-emerald-300">
                      {L.available}
                    </span>
                  </td>
                  <td className="px-3 py-2">
                    <div className="flex items-center gap-1" onClick={(e) => e.stopPropagation()}>
                      {!isServerDefault && (
                        <button
                          disabled={isBusy}
                          onClick={() => handleSetServerDefault(m)}
                          title={lang === "zh-CN" ? "设为服务端默认（写入 models.json）" : "Set as server default (writes models.json)"}
                          className="px-1.5 py-0.5 text-[10px] rounded border border-border hover:bg-hover disabled:opacity-40 whitespace-nowrap"
                        >
                          {lang === "zh-CN" ? "设默认" : "Set default"}
                        </button>
                      )}
                      <div className="relative" ref={isMenuOpen ? menuRef : undefined}>
                        <button
                          onClick={() => setMenuOpen(isMenuOpen ? null : m.id)}
                          title={lang === "zh-CN" ? "本地任务类型默认（仅当前浏览器）" : "Local task-type default (this browser only)"}
                          className="px-1.5 py-0.5 text-[10px] rounded border border-border hover:bg-hover whitespace-nowrap"
                        >
                          {L.useAs}
                        </button>
                        {isMenuOpen && (
                          <div className="absolute right-0 top-full mt-1 z-20 bg-bg-2 border border-border rounded-md shadow-lg min-w-[160px]">
                            {TASK_TYPES.map((t) => (
                              <button
                                key={t}
                                onClick={() => setDefault(m.id, m.label ?? m.id, t)}
                                className="w-full text-left px-3 py-2 text-[11px] hover:bg-hover text-fg-2 flex items-center justify-between gap-2"
                              >
                                <span>{L.defaultForTaskFmt(t)}</span>
                                {defaults[t] === m.id && <span className="text-amber-400 text-[10px]">★</span>}
                              </button>
                            ))}
                          </div>
                        )}
                      </div>
                      <button
                        disabled={isBusy}
                        onClick={() => setPendingDelete(m)}
                        title={lang === "zh-CN" ? "从目录中删除（不影响已发起的请求）" : "Delete from catalog"}
                        className="px-1.5 py-0.5 text-[10px] rounded border border-rose-500/30 text-rose-400 hover:bg-rose-500/10 disabled:opacity-40 whitespace-nowrap"
                      >
                        {lang === "zh-CN" ? "删除" : "Delete"}
                      </button>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>

      <header className="space-y-1 mt-8">
        <h2 className="text-base font-medium">{L.usageTitle}</h2>
        <p className="text-xs opacity-60">
          {error ? L.usageSubtitleErr(error) : L.usageSubtitleOk}
        </p>
      </header>

      <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
        {stats.map((stat) => (
          <div key={stat.label} className="rounded-lg border border-border p-3">
            <div className="text-[10px] text-fg-3 uppercase tracking-wide">
              {stat.label}
            </div>
            <div className="text-2xl font-semibold mt-1 mono">{stat.value}</div>
          </div>
        ))}
      </div>

      <Modal
        open={!!pendingDelete}
        onClose={() => setPendingDelete(null)}
        title={lang === "zh-CN" ? "确认删除模型" : "Confirm delete model"}
        width={460}
        footer={
          <>
            <button
              onClick={() => setPendingDelete(null)}
              className="px-3 h-8 rounded-md text-xs text-fg-3 hover:text-fg hover:bg-hover"
            >
              {lang === "zh-CN" ? "取消" : "Cancel"}
            </button>
            <button
              onClick={confirmDelete}
              className="px-3 h-8 rounded-md bg-rose-600 text-white text-xs font-medium hover:opacity-90"
            >
              {lang === "zh-CN" ? "删除" : "Delete"}
            </button>
          </>
        }
      >
        <p className="text-sm text-fg-2">
          {pendingDelete &&
            (lang === "zh-CN"
              ? `将从目录中删除模型「${pendingDelete.label ?? pendingDelete.id}」（写入 ~/.cyberclaw/models.json）。已发起的请求不受影响。`
              : `Delete model "${pendingDelete.label ?? pendingDelete.id}" from the catalog (writes ~/.cyberclaw/models.json). In-flight requests are unaffected.`)}
        </p>
      </Modal>
    </section>
  );
}
