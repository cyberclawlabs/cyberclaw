// pages_tools.jsx — F7 Tools Management
// Surfaces DeferredToolRegistry state: active vs deferred tools.
// Active tools shown in green, deferred in grey.
// Per-row Promote / Demote buttons call POST /tools/promote/:name or /demote/:name.
// Exposed as window.ToolsMgmtPage.

const { useState: tmS, useEffect: tmE, useCallback: tmC } = React;

const EXPOSURE_TONE = {
  LlmDefault:  'emerald',
  LlmAdvanced: 'accent',
  Internal:    'slate',
  AdminOnly:   'rose',
};

const fmtExposure = (s) => s || '—';

const ToolsMgmtPage = ({ lang }) => {
  const t = tFor(lang || 'en');
  // ui.jsx publishes via Object.assign(window, ...) — match the
  // `|| window` convention every other page uses to avoid crashing on
  // a fresh page load.
  const __ui = window.cc?.ui || window;
  const toast = __ui.useToast ? __ui.useToast() : { show: () => {} };

  const [tab, setTab] = tmS('active');
  const [data, setData] = tmS(null);
  const [loading, setLoading] = tmS(true);
  const [err, setErr] = tmS(null);
  const [pending, setPending] = tmS({}); // name -> 'promote'|'demote'

  const reload = tmC(async () => {
    setLoading(true); setErr(null);
    try {
      const res = await window.cc.api.tools.state();
      setData(res);
    } catch (e) {
      setErr(e && e.message ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  tmE(() => { reload(); }, []);

  const doPromote = async (name) => {
    setPending(p => ({ ...p, [name]: 'promote' }));
    try {
      await window.cc.api.tools.promote(name);
      toast.show && toast.show(t('tools.promoted'), 'success');
      reload();
    } catch (e) {
      toast.show && toast.show(t('tools.op_failed') + ': ' + (e && e.message ? e.message : String(e)), 'error');
    } finally {
      setPending(p => { const n = { ...p }; delete n[name]; return n; });
    }
  };

  const doDemote = async (name) => {
    setPending(p => ({ ...p, [name]: 'demote' }));
    try {
      await window.cc.api.tools.demote(name);
      toast.show && toast.show(t('tools.demoted'), 'success');
      reload();
    } catch (e) {
      toast.show && toast.show(t('tools.op_failed') + ': ' + (e && e.message ? e.message : String(e)), 'error');
    } finally {
      setPending(p => { const n = { ...p }; delete n[name]; return n; });
    }
  };

  const active   = (data && Array.isArray(data.active))   ? data.active   : [];
  const deferred = (data && Array.isArray(data.deferred)) ? data.deferred : [];

  const { Card, CardHeader, Button, Badge, Tabs, StatCard, EmptyState,
    SkeletonRows, ErrorBanner } = window.cc.ui || window;

  const tabs = [
    { value: 'active',   label: t('tools.tab.active'),   count: active.length },
    { value: 'deferred', label: t('tools.tab.deferred'),  count: deferred.length },
  ];

  const rows = tab === 'active' ? active : deferred;
  const isActive = tab === 'active';

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h1 className="text-[18px] font-semibold">{t('tools_mgmt')}</h1>
          <p className="text-[12px] text-fg-3 mt-0.5">{t('sub_tools_mgmt')}</p>
        </div>
        <Button variant="outline" size="sm" onClick={reload} disabled={loading}>
          <I.Refresh size={13} className={loading ? 'animate-spin' : ''} />
          {t('tools.refresh')}
        </Button>
      </div>

      {/* Stats — StatCard renders <IconCmp size={14}/>, so pass a component
          reference, not a pre-rendered JSX element. */}
      <div className="grid grid-cols-3 gap-3">
        <StatCard
          label={t('tools.stat.active')}
          value={loading ? '…' : active.length}
          icon={I.ToolState}
          tone="emerald"
        />
        <StatCard
          label={t('tools.stat.deferred')}
          value={loading ? '…' : deferred.length}
          icon={I.ToolState}
          tone="slate"
        />
        <StatCard
          label={t('tools.stat.total')}
          value={loading ? '…' : (active.length + deferred.length)}
          icon={I.Plug}
          tone="default"
        />
      </div>

      <Card>
        <CardHeader
          title={<Tabs items={tabs} value={tab} onChange={setTab} />}
        />
        <div className="p-0">
          {err && <ErrorBanner message={err} />}
          {loading && <SkeletonRows rows={6} cols={5} />}
          {!loading && !err && rows.length === 0 && (
            <EmptyState
              icon={I.ToolState}
              title={t(isActive ? 'tools.empty.active' : 'tools.empty.deferred')}
            />
          )}
          {!loading && !err && rows.length > 0 && (
            <div className="overflow-x-auto">
              <table className="w-full text-[12px]">
                <thead>
                  <tr className="border-b border-border text-fg-4 uppercase tracking-wider text-[10px]">
                    <th className="text-left px-4 py-2.5">{t('tools.col.name')}</th>
                    <th className="text-left px-4 py-2.5">{t('tools.col.exposure')}</th>
                    <th className="text-left px-4 py-2.5">{t('tools.col.connector')}</th>
                    <th className="text-left px-4 py-2.5">{t('tools.col.risk')}</th>
                    <th className="text-right px-4 py-2.5">{t('tools.col.actions')}</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((tool, i) => {
                    const name = tool.name || tool.tool_id || String(i);
                    const exposure = tool.exposure || tool.exposure_level || '';
                    const connector = tool.connector_id || tool.connector || '—';
                    const risk = tool.risk_level || tool.risk || '—';
                    const isPending = !!pending[name];
                    return (
                      <tr key={name} className="border-b border-border/50 hover:bg-[var(--hover)] transition-colors">
                        <td className="px-4 py-2.5">
                          <div className="flex items-center gap-2">
                            <span className={`w-1.5 h-1.5 rounded-full shrink-0 ${isActive ? 'bg-emerald-500 pulse-dot' : 'bg-slate-400'}`} />
                            <span className="mono text-fg font-medium">{name}</span>
                          </div>
                        </td>
                        <td className="px-4 py-2.5">
                          {exposure
                            ? <Badge tone={EXPOSURE_TONE[exposure] || 'slate'}>{fmtExposure(exposure)}</Badge>
                            : <span className="text-fg-4">—</span>}
                        </td>
                        <td className="px-4 py-2.5 mono text-fg-3 text-[11px]">{connector}</td>
                        <td className="px-4 py-2.5">
                          {risk !== '—'
                            ? <Badge tone={risk === 'critical' ? 'rose' : risk === 'high' ? 'orange' : risk === 'medium' ? 'amber' : 'slate'}>{risk}</Badge>
                            : <span className="text-fg-4">—</span>}
                        </td>
                        <td className="px-4 py-2.5 text-right">
                          {isActive ? (
                            <Button
                              variant="outline"
                              size="xs"
                              disabled={isPending}
                              onClick={() => doDemote(name)}
                            >
                              {isPending ? t('tools.demoting') : t('tools.demote')}
                            </Button>
                          ) : (
                            <Button
                              variant="default"
                              size="xs"
                              disabled={isPending}
                              onClick={() => doPromote(name)}
                            >
                              {isPending ? t('tools.promoting') : t('tools.promote')}
                            </Button>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </div>
      </Card>
    </div>
  );
};

window.ToolsMgmtPage = ToolsMgmtPage;
