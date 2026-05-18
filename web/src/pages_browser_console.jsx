// pages_browser_console.jsx — R-01 Browser CDP Console.
//
// Tabs: Targets / Action / History.
// Status strip shows CDP connection, WS URL, debug URL, target count.
// Actions hit POST /api/v1/admin/browser/{capability}.
// History persists last 20 calls in sessionStorage for replay.

const { useEffect: bcU, useState: bcS, useMemo: bcM } = React;
const {
  Card: BcCard, CardHeader: BcCardHeader, Button: BcButton, Input: BcInput,
  Textarea: BcTextarea, Tabs: BcTabs, Select: BcSelect, JsonViewer: BcJson,
  EmptyState: BcEmpty, Skeleton: BcSk, ErrorBanner: BcErr, Badge: BcBadge,
  StatusBadge: BcStatusBadge, RiskBadge: BcRiskBadge, useToast: bcUseToast, I: BcI,
} = window.cc.ui || window;

const HISTORY_KEY = 'cyberclaw.admin.browser_console.history';
const HISTORY_MAX = 20;

const BROWSER_CAPS = [
  { key: 'navigate',   risk: 'low',     fields: [{ k: 'url', label: 'URL', placeholder: 'https://example.com', required: true }] },
  { key: 'click',      risk: 'medium',  fields: [{ k: 'selector', label: 'Selector', placeholder: 'button#submit', required: true }] },
  { key: 'fill',       risk: 'medium',  fields: [
    { k: 'selector', label: 'Selector', placeholder: 'input[name=q]', required: true },
    { k: 'value',    label: 'Value',    placeholder: 'hello world',    required: true },
  ] },
  { key: 'evaluate',   risk: 'high',    fields: [{ k: 'expression', label: 'JS Expression', placeholder: 'document.title', textarea: true, required: true }] },
  { key: 'screenshot', risk: 'low',     fields: [
    { k: 'selector', label: 'Selector (optional)', placeholder: 'leave blank for full page' },
    { k: 'full_page', label: 'Full page', type: 'select', options: [
      { value: '', label: 'auto' },
      { value: 'true', label: 'true' },
      { value: 'false', label: 'false' },
    ] },
  ] },
  { key: 'dialog',     risk: 'medium',  fields: [
    { k: 'action', label: 'Action', type: 'select', options: [
      { value: 'accept',  label: 'accept' },
      { value: 'dismiss', label: 'dismiss' },
    ], required: true },
    { k: 'prompt_text', label: 'Prompt text (if any)', placeholder: 'optional' },
  ] },
];

const loadHistory = () => {
  try {
    const raw = sessionStorage.getItem(HISTORY_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch { return []; }
};
const saveHistory = (hist) => {
  try { sessionStorage.setItem(HISTORY_KEY, JSON.stringify(hist.slice(0, HISTORY_MAX))); } catch {}
};

// ---- Status strip ---------------------------------------------------------

const BcStatusStrip = ({ status, loading, error, onRefresh, t }) => (
  <BcCard className="overflow-hidden">
    <div className="grid grid-cols-2 md:grid-cols-4 divide-x divide-border">
      <BcStatusCell
        label={t('browser.status.connection')}
        value={status && status.connected ? (
          <span className="inline-flex items-center gap-1.5 text-emerald-400 text-[12px] mono">
            <span className="w-1.5 h-1.5 rounded-full bg-emerald-500 pulse-dot" />
            {t('browser.status.connected')}
          </span>
        ) : (
          <span className="inline-flex items-center gap-1.5 text-rose-400 text-[12px] mono">
            <span className="w-1.5 h-1.5 rounded-full bg-rose-500" />
            {t('browser.status.disconnected')}
          </span>
        )}
        loading={loading}
      />
      <BcStatusCell
        label={t('browser.status.ws_url')}
        value={status && status.ws_url ? (
          <span className="font-mono text-[11px] text-fg-2 truncate" title={status.ws_url}>
            {status.ws_url}
          </span>
        ) : '—'}
        loading={loading}
      />
      <BcStatusCell
        label={t('browser.status.debug_url')}
        value={status && status.debug_url ? (
          <a href={status.debug_url} target="_blank" rel="noreferrer"
            className="font-mono text-[11px] text-accent hover:underline truncate">
            {status.debug_url}
          </a>
        ) : '—'}
        loading={loading}
      />
      <BcStatusCell
        label={t('browser.status.targets')}
        value={status && Array.isArray(status.targets) ? (
          <span className="text-[14px] font-semibold tabular-nums text-fg">{status.targets.length}</span>
        ) : '0'}
        loading={loading}
        right={(
          <BcButton size="xs" variant="ghost" onClick={onRefresh} title={t('browser.action.refresh_status')}>
            <BcI.Refresh size={11} />
          </BcButton>
        )}
      />
    </div>
    {error && (
      <div className="px-4 py-2 border-t border-border">
        <BcErr message={error} onRetry={onRefresh} />
      </div>
    )}
  </BcCard>
);

const BcStatusCell = ({ label, value, loading, right }) => (
  <div className="px-4 py-3 min-w-0">
    <div className="flex items-center justify-between">
      <div className="text-[10px] uppercase tracking-wider text-fg-3">{label}</div>
      {right}
    </div>
    <div className="mt-1.5 truncate">
      {loading ? <BcSk h={14} w={120} /> : value}
    </div>
  </div>
);

// ---- Targets panel --------------------------------------------------------

const BcTargetsPanel = ({ targets, loading, t }) => {
  if (loading) {
    return <BcCard><div className="p-4 space-y-2">
      <BcSk h={14} w={'80%'} /><BcSk h={14} w={'60%'} /><BcSk h={14} w={'70%'} />
    </div></BcCard>;
  }
  if (!targets || targets.length === 0) {
    return <BcCard className="py-2">
      <BcEmpty icon={BcI.Globe} title={t('browser.targets.empty.title')} subtitle={t('browser.targets.empty.subtitle')} />
    </BcCard>;
  }
  return (
    <BcCard className="overflow-hidden">
      <table className="w-full text-sm">
        <thead className="border-b border-border bg-bg-3 text-fg-3 text-[11px]">
          <tr>
            <th className="text-left px-4 py-2 font-medium">{t('browser.targets.col.title')}</th>
            <th className="text-left px-4 py-2 font-medium mono">{t('browser.targets.col.type')}</th>
            <th className="text-left px-4 py-2 font-medium mono">{t('browser.targets.col.url')}</th>
            <th className="text-left px-4 py-2 font-medium mono">{t('browser.targets.col.id')}</th>
          </tr>
        </thead>
        <tbody>
          {targets.map((tg, i) => (
            <tr key={tg.id || i} className="border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)]">
              <td className="px-4 py-2 text-fg truncate max-w-[200px]">{tg.title || '—'}</td>
              <td className="px-4 py-2 mono text-[11px] text-fg-3">{tg.type || '—'}</td>
              <td className="px-4 py-2 mono text-[11px] text-fg-2 truncate max-w-[280px]">{tg.url || '—'}</td>
              <td className="px-4 py-2 mono text-[10px] text-fg-4">{(tg.id || tg.target_id || '').slice(0, 20)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </BcCard>
  );
};

// ---- Action panel ---------------------------------------------------------

const BcActionPanel = ({ onResult, onHistory, t }) => {
  const toast = bcUseToast();
  const [cap, setCap] = bcS('navigate');
  const [args, setArgs] = bcS({});
  const [busy, setBusy] = bcS(false);
  const [result, setResult] = bcS(null);
  const [error, setError] = bcS(null);

  const def = bcM(() => BROWSER_CAPS.find((c) => c.key === cap), [cap]);

  bcU(() => { setArgs({}); setResult(null); setError(null); }, [cap]);

  const setField = (k, v) => setArgs((a) => ({ ...a, [k]: v }));

  const validate = () => {
    if (!def) return false;
    return def.fields.every((f) => !f.required || (args[f.k] && String(args[f.k]).trim()));
  };

  const run = async () => {
    if (!validate()) {
      toast && toast.toast({ title: t('browser.action.required_fields'), tone: 'error' });
      return;
    }
    setBusy(true);
    setError(null);
    setResult(null);
    const start = performance.now();
    try {
      const apiFn = window.cc.api.browser[cap];
      if (!apiFn) throw new Error('no api: ' + cap);
      const resp = await apiFn(args);
      const ms = Math.round(performance.now() - start);
      setResult({ resp, ms });
      onResult && onResult(resp);
      onHistory && onHistory({ ts: Date.now(), capability: cap, input: args, ok: true, latency_ms: ms, output_preview: previewify(resp) });
      toast && toast.toast({ title: t('browser.action.ok'), desc: cap + ' · ' + ms + 'ms', tone: 'success' });
    } catch (e) {
      const ms = Math.round(performance.now() - start);
      const msg = e && e.message ? e.message : String(e);
      setError(msg);
      onHistory && onHistory({ ts: Date.now(), capability: cap, input: args, ok: false, latency_ms: ms, error: msg });
      toast && toast.toast({ title: t('browser.action.failed'), desc: msg, tone: 'error' });
    } finally {
      setBusy(false);
    }
  };

  // Backend wraps capability output as { status, output, error? }.
  // browser.screenshot output is { path, bytes_written, format, image_b64 }
  // (image_b64 is injected by admin/browser.rs::screenshot after reading the
  // file back). Format defaults to "png".
  const out = result && result.resp && result.resp.output;
  const screenshotData = out && (out.image_b64 || out.screenshot_b64 || out.b64);
  const screenshotMime = out && out.format ? `image/${out.format}` : 'image/png';

  return (
    <div className="grid grid-cols-1 xl:grid-cols-[420px_1fr] gap-4">
      <BcCard className="overflow-hidden">
        <BcCardHeader
          title={t('browser.action.invoke')}
          subtitle={<span className="mono">POST /api/v1/admin/browser/{cap}</span>}
          right={<BcRiskBadge level={def && def.risk} />}
        />
        <div className="p-4 space-y-3">
          <div>
            <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">
              {t('browser.action.capability')}
            </div>
            <BcSelect
              value={cap}
              onChange={setCap}
              options={BROWSER_CAPS.map((c) => ({ value: c.key, label: c.key }))}
            />
          </div>
          {def && def.fields.map((f) => (
            <div key={f.k}>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">
                {f.label}{f.required && <span className="text-rose-400 ml-0.5">*</span>}
              </div>
              {f.type === 'select' ? (
                <BcSelect
                  value={args[f.k] || ''}
                  onChange={(v) => setField(f.k, v)}
                  options={f.options}
                />
              ) : f.textarea ? (
                <BcTextarea
                  rows={4}
                  value={args[f.k] || ''}
                  onChange={(e) => setField(f.k, e.target.value)}
                  placeholder={f.placeholder}
                />
              ) : (
                <BcInput
                  value={args[f.k] || ''}
                  onChange={(e) => setField(f.k, e.target.value)}
                  placeholder={f.placeholder}
                />
              )}
            </div>
          ))}
          <BcButton onClick={run} disabled={busy}>
            {busy ? <><BcI.Loader size={12} className="animate-spin" /> {t('browser.action.running')}</> : <><BcI.Play size={12} /> {t('browser.action.run')}</>}
          </BcButton>
        </div>
      </BcCard>

      <BcCard className="overflow-hidden">
        <BcCardHeader
          title={t('browser.action.result')}
          subtitle={result ? <span className="mono">{result.ms} ms</span> : null}
        />
        <div className="p-4 space-y-3">
          {error && <BcErr message={error} />}
          {!result && !error && (
            <BcEmpty icon={BcI.Activity} title={t('browser.action.empty.title')} subtitle={t('browser.action.empty.subtitle')} />
          )}
          {screenshotData && (
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">
                {t('browser.action.screenshot')}
              </div>
              <div className="border border-border rounded-md overflow-hidden bg-checkered">
                <img alt="screenshot" src={`data:${screenshotMime};base64,${screenshotData}`} className="block max-w-full" />
              </div>
            </div>
          )}
          {result && (
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">
                {t('browser.action.json')}
              </div>
              <BcJson value={result.resp} />
            </div>
          )}
        </div>
      </BcCard>
    </div>
  );
};

const previewify = (resp) => {
  try {
    const s = JSON.stringify(resp);
    return s.length > 160 ? s.slice(0, 160) + '…' : s;
  } catch { return ''; }
};

// ---- History panel --------------------------------------------------------

const BcHistoryPanel = ({ history, onReplay, onClear, t }) => {
  if (history.length === 0) {
    return <BcCard className="py-2">
      <BcEmpty
        icon={BcI.Clock}
        title={t('browser.history.empty.title')}
        subtitle={t('browser.history.empty.subtitle')}
      />
    </BcCard>;
  }
  return (
    <BcCard className="overflow-hidden">
      <BcCardHeader
        title={t('browser.history.title')}
        subtitle={<span className="mono">{history.length} / {HISTORY_MAX}</span>}
        right={<BcButton size="xs" variant="ghost" onClick={onClear}>
          <BcI.Trash size={11} /> {t('browser.history.clear')}
        </BcButton>}
      />
      <div className="divide-y divide-border">
        {history.map((h, i) => (
          <div key={i} className="px-4 py-3 flex items-start gap-3 hover:bg-[var(--hover)]">
            <div className={`mt-0.5 w-1.5 h-1.5 rounded-full ${h.ok ? 'bg-emerald-500' : 'bg-rose-500'}`} />
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2 text-[12px]">
                <span className="font-medium text-fg">{h.capability}</span>
                <span className="mono text-[10px] text-fg-4">{new Date(h.ts).toLocaleTimeString()}</span>
                <span className="mono text-[10px] text-fg-4">{h.latency_ms}ms</span>
              </div>
              <div className="mt-1 mono text-[11px] text-fg-3 truncate">
                {h.ok ? h.output_preview : h.error}
              </div>
            </div>
            <BcButton size="xs" variant="ghost" onClick={() => onReplay(h)}>
              <BcI.Refresh size={10} /> {t('browser.history.replay')}
            </BcButton>
          </div>
        ))}
      </div>
    </BcCard>
  );
};

// ---- Page -----------------------------------------------------------------

const BrowserConsolePage = ({ lang }) => {
  const t = tFor(lang || 'en');
  const [tab, setTab] = bcS('targets');
  const [status, setStatus] = bcS(null);
  const [statusLoading, setStatusLoading] = bcS(true);
  const [statusErr, setStatusErr] = bcS(null);
  const [history, setHistory] = bcS(loadHistory());

  const loadStatus = async () => {
    setStatusLoading(true);
    setStatusErr(null);
    try {
      const s = await window.cc.api.browser.status();
      setStatus(s);
    } catch (e) {
      setStatus(null);
      setStatusErr(e && e.message ? e.message : String(e));
    } finally {
      setStatusLoading(false);
    }
  };
  bcU(() => { loadStatus(); }, []);

  const pushHistory = (entry) => {
    setHistory((h) => {
      const next = [entry, ...h].slice(0, HISTORY_MAX);
      saveHistory(next);
      return next;
    });
  };
  const clearHistory = () => { saveHistory([]); setHistory([]); };
  const onReplay = (h) => { setTab('action'); /* user re-fills if desired */ };

  return (
    <div className="space-y-4">
      <BcStatusStrip status={status} loading={statusLoading} error={statusErr} onRefresh={loadStatus} t={t} />
      <BcTabs value={tab} onChange={setTab} items={[
        { value: 'targets', label: t('browser.tab.targets'), count: status && status.targets ? status.targets.length : 0 },
        { value: 'action',  label: t('browser.tab.action') },
        { value: 'history', label: t('browser.tab.history'), count: history.length || null },
      ]} />
      {tab === 'targets' && <BcTargetsPanel targets={(status && status.targets) || []} loading={statusLoading} t={t} />}
      {tab === 'action'  && <BcActionPanel t={t} onHistory={pushHistory} />}
      {tab === 'history' && <BcHistoryPanel history={history} onReplay={onReplay} onClear={clearHistory} t={t} />}
    </div>
  );
};

Object.assign(window, { BrowserConsolePage });
