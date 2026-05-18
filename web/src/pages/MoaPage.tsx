import { useEffect, useState } from "react";
import { type MoaConfig, type MoaProposer, fetchMoaConfig, updateMoaConfig } from "@/lib/api";
import { type Lang } from "@/lib/i18n";
import { useToast } from "@/components/ToastBar";
import PageHeader from "@/components/PageHeader";
import Field, { TextInput } from "@/components/Field";

const PROVIDERS = ["anthropic", "openai", "minimax", "ollama"] as const;
type Provider = (typeof PROVIDERS)[number];

function dict(lang: Lang) {
  return lang === "zh-CN"
    ? {
        title: "Mixture-of-Agents",
        subtitle: "聚合多个 LLM 以获得更高质量的推理",
        subtitleDirty: "聚合多个 LLM 以获得更高质量的推理 — 有未保存改动",
        discard: "放弃",
        save: "保存",
        saving: "保存中…",
        savedToast: "配置已保存。",
        loading: "加载中…",
        sectionEnable: "启用",
        enableLabel: "对 /v1/chat/completions 启用 MoA",
        sectionAggregator: "聚合器",
        sectionProposers: "提议者",
        emptyProposers: "尚未配置提议者。点下面添加一个。",
        sourceBanner: "Model 字段填你 LLM provider 支持的任意 model id（例：gpt-4o、claude-sonnet-4-6、MiniMax-M2.7-HighSpeed）。保存即生效，对所有走 chat completions 的请求生效。要换或加 LLM provider，去 设置 → 配置 编辑 [llm] 段。",
        addProposer: "+ 添加提议者",
        labelProvider: "Provider",
        labelModel: "Model",
        labelWeight: "权重（0–1）",
        modelPlaceholder: "例如 gpt-4o",
        modelMiniPlaceholder: "例如 gpt-4o-mini",
        removeProposer: "删除提议者",
      }
    : {
        title: "Mixture-of-Agents",
        subtitle: "Aggregate multiple LLMs for higher-quality reasoning",
        subtitleDirty: "Aggregate multiple LLMs for higher-quality reasoning — unsaved changes",
        discard: "Discard",
        save: "Save",
        saving: "Saving…",
        savedToast: "Configuration saved.",
        loading: "Loading…",
        sectionEnable: "Enable",
        enableLabel: "Enable MoA for /v1/chat/completions",
        sectionAggregator: "Aggregator",
        sectionProposers: "Proposers",
        emptyProposers: "No proposers configured. Add one below.",
        sourceBanner: "Enter any model id your LLM provider supports in the Model field (e.g. gpt-4o, claude-sonnet-4-6, MiniMax-M2.7-HighSpeed). Saving applies the change to every chat completion immediately. To switch or add an LLM provider, go to Settings → Config and edit the [llm] section.",
        addProposer: "+ Add proposer",
        labelProvider: "Provider",
        labelModel: "Model",
        labelWeight: "Weight (0–1)",
        modelPlaceholder: "e.g. gpt-4o",
        modelMiniPlaceholder: "e.g. gpt-4o-mini",
        removeProposer: "Remove proposer",
      };
}

function providerOpts() {
  return PROVIDERS.map((p) => (
    <option key={p} value={p}>
      {p}
    </option>
  ));
}

function emptyProposer(): MoaProposer {
  return { model: "", provider: "openai", weight: 0.5 };
}

export default function MoaPage({ lang }: { lang: Lang }) {
  const L = dict(lang);
  const toast = useToast();
  const [config, setConfig] = useState<MoaConfig | null>(null);
  const [draft, setDraft] = useState<MoaConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);

  const load = () => {
    setLoading(true);
    fetchMoaConfig()
      .then((c) => {
        setConfig(c);
        setDraft(JSON.parse(JSON.stringify(c)) as MoaConfig);
      })
      .catch(() => {
        const fallback: MoaConfig = { enabled: false, proposers: [], aggregator: { model: "", provider: "openai" } };
        setConfig(fallback);
        setDraft(JSON.parse(JSON.stringify(fallback)) as MoaConfig);
      })
      .finally(() => setLoading(false));
  };

  useEffect(() => { load(); }, []);

  const dirty =
    draft !== null && config !== null && JSON.stringify(draft) !== JSON.stringify(config);

  const setDraftField = <K extends keyof MoaConfig>(k: K, v: MoaConfig[K]) => {
    setDraft((d) => d ? { ...d, [k]: v } : d);
  };

  const setAggField = (k: "model" | "provider", v: string) => {
    setDraft((d) =>
      d ? { ...d, aggregator: { ...(d.aggregator ?? { model: "", provider: "openai" }), [k]: v } } : d
    );
  };

  const setProposer = (idx: number, patch: Partial<MoaProposer>) => {
    setDraft((d) => {
      if (!d) return d;
      const proposers = d.proposers.map((p, i) => (i === idx ? { ...p, ...patch } : p));
      return { ...d, proposers };
    });
  };

  const addProposer = () => {
    setDraft((d) => d ? { ...d, proposers: [...d.proposers, emptyProposer()] } : d);
  };

  const removeProposer = (idx: number) => {
    setDraft((d) => d ? { ...d, proposers: d.proposers.filter((_, i) => i !== idx) } : d);
  };

  const save = async () => {
    if (!draft) return;
    setSaving(true);
    try {
      await updateMoaConfig(draft);
      setConfig(JSON.parse(JSON.stringify(draft)) as MoaConfig);
      toast({ msg: L.savedToast, tone: "success" });
    } catch (e: unknown) {
      const err = e as { body?: string; status?: number };
      toast({ msg: `Save failed: ${err.body ?? err.status ?? "unknown error"}`, tone: "error" });
    } finally {
      setSaving(false);
    }
  };

  const discard = () => {
    if (config) setDraft(JSON.parse(JSON.stringify(config)) as MoaConfig);
  };

  if (loading || !draft) {
    return (
      <section className="space-y-4">
        <PageHeader title={L.title} subtitle={L.subtitle} />
        <div className="text-[12px] text-fg-4 mono">{L.loading}</div>
      </section>
    );
  }

  return (
    <section className="space-y-5 anim-fade-in">
      <PageHeader
        title={L.title}
        subtitle={dirty ? L.subtitleDirty : L.subtitle}
        actions={
          <div className="flex items-center gap-2">
            <button
              onClick={discard}
              disabled={!dirty || saving}
              className="px-3 py-1.5 rounded-md border border-border text-[12px] text-fg-2 hover:text-fg hover:border-border-strong disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
            >
              {L.discard}
            </button>
            <button
              onClick={save}
              disabled={!dirty || saving}
              className="px-3 py-1.5 rounded-md bg-accent text-accent-fg text-[12px] hover:opacity-90 disabled:opacity-40 disabled:cursor-not-allowed transition-opacity"
            >
              {saving ? L.saving : L.save}
            </button>
          </div>
        }
      />

      <p className="text-[11px] text-amber-300/90 px-3 py-2 bg-amber-500/10 rounded border border-amber-500/20 leading-relaxed">
        {L.sourceBanner}
      </p>

      {/* Section 1: Enabled */}
      <div className="rounded-xl border border-border bg-bg-2 p-5 space-y-3">
        <h3 className="text-[11px] mono text-fg-4 uppercase tracking-widest">{L.sectionEnable}</h3>
        <label className="flex items-center gap-3 cursor-pointer select-none">
          <input
            type="checkbox"
            checked={draft.enabled}
            onChange={(e) => setDraftField("enabled", e.target.checked)}
            className="h-4 w-4 rounded border-border accent-[var(--color-accent)]"
          />
          <span className="text-[13px] text-fg-2">{L.enableLabel}</span>
        </label>
      </div>

      {/* Section 2: Aggregator */}
      <div className="rounded-xl border border-border bg-bg-2 p-5 space-y-3">
        <h3 className="text-[11px] mono text-fg-4 uppercase tracking-widest">{L.sectionAggregator}</h3>
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
          <Field label={L.labelProvider}>
            <select
              value={draft.aggregator?.provider ?? "openai"}
              onChange={(e) => setAggField("provider", e.target.value)}
              className="w-full px-3 py-2 rounded-md bg-bg-3 border border-border text-fg outline-none focus-ring"
            >
              {providerOpts()}
            </select>
          </Field>
          <Field label={L.labelModel}>
            <TextInput
              value={draft.aggregator?.model ?? ""}
              onChange={(v) => setAggField("model", v)}
              placeholder={L.modelPlaceholder}
            />
          </Field>
        </div>
      </div>

      {/* Section 3: Proposers */}
      <div className="rounded-xl border border-border bg-bg-2 p-5 space-y-3">
        <h3 className="text-[11px] mono text-fg-4 uppercase tracking-widest">
          {L.sectionProposers} ({draft.proposers.length})
        </h3>

        {draft.proposers.length === 0 && (
          <p className="text-[12px] text-fg-4">{L.emptyProposers}</p>
        )}

        <div className="space-y-2">
          {draft.proposers.map((p, idx) => (
            <div
              key={idx}
              className="grid grid-cols-[1fr_1fr_120px_auto] gap-2 items-end p-3 rounded-lg bg-bg-3 border border-border"
            >
              <Field label={L.labelModel}>
                <TextInput
                  value={p.model}
                  onChange={(v) => setProposer(idx, { model: v })}
                  placeholder={L.modelMiniPlaceholder}
                />
              </Field>
              <Field label={L.labelProvider}>
                <select
                  value={p.provider ?? "openai"}
                  onChange={(e) => setProposer(idx, { provider: e.target.value as Provider })}
                  className="w-full px-3 py-2 rounded-md bg-bg-2 border border-border text-fg outline-none focus-ring"
                >
                  {providerOpts()}
                </select>
              </Field>
              <Field label={L.labelWeight}>
                <input
                  type="number"
                  min={0}
                  max={1}
                  step={0.01}
                  value={p.weight ?? 0.5}
                  onChange={(e) => setProposer(idx, { weight: Number(e.target.value) })}
                  className="w-full px-3 py-2 rounded-md bg-bg-2 border border-border text-fg outline-none focus-ring mono"
                />
              </Field>
              <div className="pb-0.5">
                <button
                  onClick={() => removeProposer(idx)}
                  className="px-2.5 py-2 rounded-md border border-border text-[12px] text-fg-3 hover:text-rose-400 hover:border-rose-400/40 transition-colors"
                  title={L.removeProposer}
                >
                  ✕
                </button>
              </div>
            </div>
          ))}
        </div>

        <button
          onClick={addProposer}
          className="mt-1 px-3 py-1.5 rounded-md border border-dashed border-border text-[12px] text-fg-3 hover:text-fg hover:border-border-strong transition-colors"
        >
          {L.addProposer}
        </button>
      </div>
    </section>
  );
}
