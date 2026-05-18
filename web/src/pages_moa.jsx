// pages_moa.jsx — Mixture-of-Agents config + test harness.
//
// Tabs: Config (CRUD proposers/aggregator) / Test (3-column compare).
//
// Backend contract:
//   GET  /api/v1/admin/moa/config     → { enabled, proposers:[{model,provider,weight}], aggregator:{model,provider} }
//   PUT  /api/v1/admin/moa/config     ← same shape
//   POST /api/v1/admin/moa/test       ← { prompt } → { proposers:[{model,output,latency_ms}], aggregated, latency_ms }

const { useEffect: moU, useState: moS, useMemo: moM } = React;
const {
  Card: MoCard, CardHeader: MoCardHeader, Button: MoButton, Input: MoInput,
  Textarea: MoTextarea, Switch: MoSwitch, Tabs: MoTabs, Select: MoSelect,
  EmptyState: MoEmpty, Skeleton: MoSk, ErrorBanner: MoErr, Badge: MoBadge,
  useToast: moUseToast, I: MoI,
} = window.cc.ui || window;

const PROVIDERS = [
  { value: '',          label: 'auto' },
  { value: 'anthropic', label: 'anthropic' },
  { value: 'openai',    label: 'openai' },
  { value: 'google',    label: 'google' },
  { value: 'mistral',   label: 'mistral' },
  { value: 'groq',      label: 'groq' },
  { value: 'local',     label: 'local' },
];

const PROVIDER_TONE = {
  anthropic: 'orange',
  openai:    'emerald',
  google:    'cyan',
  mistral:   'violet',
  groq:      'amber',
  local:     'slate',
};

// ---- Config panel --------------------------------------------------------

const ConfigPanel = ({ cfg, onCfg, onSave, busy, t }) => {
  if (!cfg) return null;

  const setProposer = (i, key, v) =>
    onCfg({ ...cfg, proposers: cfg.proposers.map((p, j) => j === i ? { ...p, [key]: v } : p) });
  const addProposer = () =>
    onCfg({ ...cfg, proposers: [...(cfg.proposers || []), { model: '', provider: '', weight: 1 }] });
  const removeProposer = (i) =>
    onCfg({ ...cfg, proposers: cfg.proposers.filter((_, j) => j !== i) });

  const setAggregator = (key, v) =>
    onCfg({ ...cfg, aggregator: { ...(cfg.aggregator || {}), [key]: v } });

  const totalWeight = (cfg.proposers || []).reduce((s, p) => s + (Number(p.weight) || 0), 0);

  return (
    <div className="space-y-4">
      <MoCard className="overflow-hidden">
        <MoCardHeader
          title={t('moa.section.global')}
          subtitle={<span className="mono">enabled = {cfg.enabled ? 'true' : 'false'}</span>}
          right={
            <div className="flex items-center gap-3">
              <span className={`text-[12px] mono ${cfg.enabled ? 'text-emerald-400' : 'text-fg-4'}`}>
                {cfg.enabled ? t('moa.enabled') : t('moa.disabled')}
              </span>
              <MoSwitch checked={!!cfg.enabled} onChange={(v) => onCfg({ ...cfg, enabled: v })} />
            </div>
          }
        />
      </MoCard>

      <MoCard className="overflow-hidden">
        <MoCardHeader
          title={t('moa.section.proposers')}
          subtitle={<span className="mono">{(cfg.proposers || []).length} · Σweight = {totalWeight.toFixed(2)}</span>}
          right={<MoButton size="sm" variant="ghost" onClick={addProposer}>
            <MoI.Plus size={12} /> {t('moa.action.add_proposer')}
          </MoButton>}
        />
        <div className="p-4 space-y-2">
          {(cfg.proposers || []).length === 0 ? (
            <div className="border border-dashed border-border rounded-md py-6 text-center text-[12px] text-fg-4">
              {t('moa.proposers.empty')}
            </div>
          ) : (
            <div className="space-y-2">
              <div className="grid grid-cols-[1fr_140px_100px_40px] gap-2 text-[10px] uppercase tracking-wider text-fg-3 px-2">
                <div>{t('moa.col.model')}</div>
                <div>{t('moa.col.provider')}</div>
                <div className="text-right">{t('moa.col.weight')}</div>
                <div></div>
              </div>
              {(cfg.proposers || []).map((p, i) => (
                <div key={i} className="grid grid-cols-[1fr_140px_100px_40px] gap-2 items-center bg-bg-3/40 border border-border/60 rounded-md p-2">
                  <MoInput value={p.model || ''} onChange={(e) => setProposer(i, 'model', e.target.value)} placeholder="claude-sonnet-4-5" />
                  <MoSelect
                    value={p.provider || ''}
                    onChange={(v) => setProposer(i, 'provider', v)}
                    options={PROVIDERS}
                  />
                  <MoInput
                    type="number"
                    step="0.1"
                    min="0"
                    value={p.weight != null ? p.weight : 1}
                    onChange={(e) => setProposer(i, 'weight', Number(e.target.value))}
                    className="text-right tabular-nums"
                  />
                  <MoButton size="sm" variant="ghost" onClick={() => removeProposer(i)}>
                    <MoI.Trash size={11} />
                  </MoButton>
                </div>
              ))}
            </div>
          )}
        </div>
      </MoCard>

      <MoCard className="overflow-hidden">
        <MoCardHeader title={t('moa.section.aggregator')}
                      subtitle={<span className="mono">single model that fuses proposer outputs</span>} />
        <div className="p-4 grid grid-cols-1 md:grid-cols-2 gap-3">
          <div>
            <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('moa.col.model')}</div>
            <MoInput
              value={(cfg.aggregator && cfg.aggregator.model) || ''}
              onChange={(e) => setAggregator('model', e.target.value)}
              placeholder="claude-opus-4-7"
            />
          </div>
          <div>
            <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('moa.col.provider')}</div>
            <MoSelect
              value={(cfg.aggregator && cfg.aggregator.provider) || ''}
              onChange={(v) => setAggregator('provider', v)}
              options={PROVIDERS}
            />
          </div>
        </div>
      </MoCard>

      <div className="flex justify-end gap-2">
        <MoButton onClick={onSave} disabled={busy}>
          {busy ? <><MoI.Loader size={12} className="animate-spin" /> {t('moa.saving')}</>
                : <><MoI.Save size={12} /> {t('moa.save_config')}</>}
        </MoButton>
      </div>
    </div>
  );
};

// ---- Test harness --------------------------------------------------------

const TestPanel = ({ cfg, t }) => {
  const toast = moUseToast();
  const [prompt, setPrompt] = moS('Compare these three approaches and recommend one with reasoning.');
  const [busy, setBusy] = moS(false);
  const [result, setResult] = moS(null);
  const [error, setError] = moS(null);

  const run = async () => {
    if (!prompt.trim()) {
      toast && toast.toast({ title: t('moa.test.need_prompt'), tone: 'error' });
      return;
    }
    setBusy(true); setError(null); setResult(null);
    try {
      const start = performance.now();
      const resp = await window.cc.api.moa.test({ prompt });
      const ms = Math.round(performance.now() - start);
      setResult({ ...resp, _ms: ms });
    } catch (e) {
      setError(e && e.message ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const proposers = (result && result.proposers) || [];
  const aggregated = result && (result.aggregated || result.aggregate || result.output);

  return (
    <div className="space-y-4">
      <MoCard className="overflow-hidden">
        <MoCardHeader title={t('moa.test.title')} subtitle={<span className="mono">POST /api/v1/admin/moa/test</span>} />
        <div className="p-4 space-y-3">
          <MoTextarea
            rows={4}
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            placeholder={t('moa.test.prompt_placeholder')}
          />
          <div className="flex items-center justify-between">
            <div className="text-[11px] mono text-fg-4">
              {(cfg && cfg.proposers ? cfg.proposers.length : 0)} proposers · agg:{' '}
              <span className="text-fg-2">{(cfg && cfg.aggregator && cfg.aggregator.model) || '—'}</span>
            </div>
            <MoButton onClick={run} disabled={busy}>
              {busy ? <><MoI.Loader size={12} className="animate-spin" /> {t('moa.test.running')}</>
                    : <><MoI.Play size={12} /> {t('moa.test.run')}</>}
            </MoButton>
          </div>
          {error && <MoErr message={error} />}
        </div>
      </MoCard>

      {!result && !busy && !error && (
        <MoCard className="py-2">
          <MoEmpty
            icon={MoI.Sparkles}
            title={t('moa.test.empty.title')}
            subtitle={t('moa.test.empty.subtitle')}
          />
        </MoCard>
      )}

      {result && (
        <>
          <div className="grid grid-cols-1 lg:grid-cols-3 gap-3">
            {proposers.length === 0 ? (
              <div className="lg:col-span-3 text-[12px] text-fg-4 italic">{t('moa.test.no_proposer_outputs')}</div>
            ) : (
              proposers.map((p, i) => (
                <MoCard key={i} className="overflow-hidden flex flex-col">
                  <MoCardHeader
                    title={p.model || `proposer #${i + 1}`}
                    subtitle={p.provider ? <MoBadge tone={PROVIDER_TONE[p.provider] || 'slate'}>{p.provider}</MoBadge> : null}
                    right={p.latency_ms != null ? <span className="text-[11px] mono text-fg-4">{p.latency_ms}ms</span> : null}
                  />
                  <div className="p-4 text-[12.5px] text-fg-2 whitespace-pre-wrap leading-relaxed flex-1 overflow-auto max-h-[420px]">
                    {p.output || p.text || <span className="text-fg-4 italic">{t('moa.test.no_output')}</span>}
                  </div>
                </MoCard>
              ))
            )}
          </div>

          {aggregated && (
            <MoCard className="overflow-hidden border-2 border-[var(--accent-ring)]">
              <MoCardHeader
                title={
                  <span className="inline-flex items-center gap-2">
                    <MoI.Sparkles size={13} className="text-accent" />
                    {t('moa.test.aggregated')}
                  </span>
                }
                subtitle={
                  <span className="mono">
                    {result._ms} ms total · {(cfg && cfg.aggregator && cfg.aggregator.model) || 'aggregator'}
                  </span>
                }
              />
              <div className="p-4 text-[13px] text-fg whitespace-pre-wrap leading-relaxed">
                {typeof aggregated === 'string' ? aggregated : JSON.stringify(aggregated, null, 2)}
              </div>
            </MoCard>
          )}
        </>
      )}
    </div>
  );
};

// ---- Page ----------------------------------------------------------------

const MoaPage = ({ lang }) => {
  const t = tFor(lang || 'en');
  const toast = moUseToast();
  const [tab, setTab] = moS('config');
  const [cfg, setCfg] = moS(null);
  const [original, setOriginal] = moS(null);
  const [loading, setLoading] = moS(true);
  const [error, setError] = moS(null);
  const [saving, setSaving] = moS(false);

  const reload = async () => {
    setLoading(true); setError(null);
    try {
      const c = await window.cc.api.moa.getConfig();
      const normalized = {
        enabled: !!(c && c.enabled),
        proposers: (c && Array.isArray(c.proposers)) ? c.proposers : [],
        aggregator: (c && c.aggregator) || { model: '', provider: '' },
      };
      setCfg(normalized);
      setOriginal(JSON.stringify(normalized));
    } catch (e) {
      setError(e && e.message ? e.message : String(e));
      setCfg({ enabled: false, proposers: [], aggregator: { model: '', provider: '' } });
    } finally { setLoading(false); }
  };
  moU(() => { reload(); }, []);

  const save = async () => {
    setSaving(true);
    try {
      await window.cc.api.moa.putConfig(cfg);
      toast && toast.toast({ title: t('moa.toast.saved'), tone: 'success' });
      setOriginal(JSON.stringify(cfg));
    } catch (e) {
      toast && toast.toast({ title: t('moa.toast.save_failed'), desc: String(e), tone: 'error' });
    } finally { setSaving(false); }
  };

  const dirty = cfg && original && JSON.stringify(cfg) !== original;

  return (
    <div className="space-y-4">
      <MoTabs value={tab} onChange={setTab} items={[
        { value: 'config', label: t('moa.tab.config') },
        { value: 'test',   label: t('moa.tab.test') },
      ]} />

      {error && <MoErr message={error} onRetry={reload} />}

      {tab === 'config' && (loading || !cfg
        ? <MoCard className="p-6"><MoSk h={120} /></MoCard>
        : <>
            {dirty && (
              <div className="border border-amber-500/30 bg-amber-500/5 text-amber-300 rounded-md px-3 py-2 text-[12px] flex items-center gap-2">
                <MoI.AlertTriangle size={13} />
                <span>{t('moa.dirty_warning')}</span>
              </div>
            )}
            <ConfigPanel cfg={cfg} onCfg={setCfg} onSave={save} busy={saving} t={t} />
          </>)}
      {tab === 'test' && <TestPanel cfg={cfg} t={t} />}
    </div>
  );
};

Object.assign(window, { MoaPage });
