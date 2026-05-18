// pages_cluster.jsx — F4 Cluster Dashboard
// Brains list + Sessions list + Register Brain modal + auto-refresh every 10s.
// Exposed as window.ClusterDashboardPage.

const { useState: clS, useEffect: clE, useRef: clR, useCallback: clC } = React;

const fmtRel = (s) => {
  if (!s) return '—';
  try {
    const ms = Date.now() - new Date(s).getTime();
    if (ms < 0) return 'soon';
    const sec = Math.floor(ms / 1000); if (sec < 60) return sec + 's ago';
    const min = Math.floor(sec / 60);  if (min < 60) return min + 'm ago';
    const h = Math.floor(min / 60);    if (h < 48)   return h + 'h ago';
    return Math.floor(h / 24) + 'd ago';
  } catch { return s; }
};

const fmtTs = (s) => {
  if (!s) return '—';
  try { return new Date(s).toLocaleTimeString(); } catch { return s; }
};

// Progress bar for load metrics
const MiniBar = ({ value, max, tone = 'accent' }) => {
  const pct = max > 0 ? Math.min(100, Math.round((value / max) * 100)) : 0;
  const colorMap = { accent: 'bg-[var(--accent)]', emerald: 'bg-emerald-500', amber: 'bg-amber-400', rose: 'bg-rose-500' };
  const color = pct >= 90 ? colorMap.rose : pct >= 70 ? colorMap.amber : colorMap[tone] || colorMap.accent;
  return (
    <div className="flex items-center gap-2 min-w-[80px]">
      <div className="flex-1 h-1.5 rounded-full bg-[var(--bg-3)] overflow-hidden">
        <div className={`h-full rounded-full transition-all ${color}`} style={{ width: pct + '%' }} />
      </div>
      <span className="text-[10px] mono text-fg-4 tabular-nums w-8 text-right">{pct}%</span>
    </div>
  );
};

// Register Brain modal
const RegisterBrainModal = ({ open, onClose, onSuccess, t }) => {
  const toast = window.cc.ui && window.cc.ui.useToast ? window.cc.ui.useToast() : { show: () => {} };
  const [form, setForm] = clS({ brain_id: '', address: '', port: '8080', max_concurrent: '4' });
  const [loading, setLoading] = clS(false);
  const [err, setErr] = clS(null);

  const set = (k, v) => setForm(f => ({ ...f, [k]: v }));

  const submit = async () => {
    if (!form.brain_id.trim() || !form.address.trim()) return;
    setLoading(true); setErr(null);
    try {
      await window.cc.api.cluster.registerBrain({
        brain_id: form.brain_id.trim(),
        address: form.address.trim(),
        port: parseInt(form.port, 10) || 8080,
        max_concurrent: parseInt(form.max_concurrent, 10) || 4,
      });
      toast.show && toast.show(t('cluster.reg.success'), 'success');
      setForm({ brain_id: '', address: '', port: '8080', max_concurrent: '4' });
      onSuccess && onSuccess();
      onClose();
    } catch (e) {
      setErr(e && e.message ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  const { Dialog, Button, Input, ErrorBanner } = window.cc.ui || window;
  if (!open) return null;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={t('cluster.reg.title')}
      subtitle="POST /api/v1/cluster/brain/register"
      width={440}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>{t('cluster.reg.cancel')}</Button>
          <Button onClick={submit} disabled={loading || !form.brain_id.trim() || !form.address.trim()}>
            <I.BrainNode size={13} />
            {loading ? '…' : t('cluster.reg.submit')}
          </Button>
        </>
      }
    >
      <div className="space-y-3">
        {err && <ErrorBanner message={err} />}
        {[
          { key: 'brain_id',        label: t('cluster.reg.brain_id'),        placeholder: 'brain-01' },
          { key: 'address',         label: t('cluster.reg.address'),          placeholder: '10.0.0.1' },
          { key: 'port',            label: t('cluster.reg.port'),              placeholder: '8080' },
          { key: 'max_concurrent',  label: t('cluster.reg.max_concurrent'),   placeholder: '4' },
        ].map(f => (
          <div key={f.key}>
            <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider mono">{f.label}</label>
            <Input
              value={form[f.key]}
              onChange={e => set(f.key, e.target.value)}
              placeholder={f.placeholder}
              className="mono"
            />
          </div>
        ))}
      </div>
    </Dialog>
  );
};

// Brains table
const BrainsTab = ({ brains, loading, t, onHeartbeat }) => {
  const { EmptyState, SkeletonRows, Badge, Button } = window.cc.ui || window;

  if (loading) return <SkeletonRows rows={4} cols={6} />;
  if (!brains || brains.length === 0) {
    return <EmptyState icon={I.BrainNode} title={t('cluster.empty.brains')} />;
  }
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-[12px]">
        <thead>
          <tr className="border-b border-border text-fg-4 uppercase tracking-wider text-[10px]">
            {['cluster.col.id','cluster.col.address','cluster.col.status',
              'cluster.col.last_seen','cluster.col.load','cluster.col.sessions'].map(k => (
              <th key={k} className="text-left px-4 py-2.5">{t(k)}</th>
            ))}
            <th className="text-right px-4 py-2.5"></th>
          </tr>
        </thead>
        <tbody>
          {brains.map((b, i) => {
            const status = b.status || 'unknown';
            const statusTone = status === 'healthy' ? 'emerald' : status === 'dead' || status === 'offline' ? 'rose' : 'amber';
            const sessions = b.active_sessions ?? b.sessions ?? 0;
            const capacity = b.max_concurrent ?? b.capacity ?? 0;
            const cpuPct = b.cpu_pct ?? b.cpu ?? 0;
            const memPct = b.mem_pct ?? b.mem ?? 0;
            const addr = [b.address, b.port ? ':' + b.port : ''].join('');
            return (
              <tr key={b.brain_id || i} className="border-b border-border/50 hover:bg-[var(--hover)] transition-colors">
                <td className="px-4 py-2.5 mono text-fg font-medium">{b.brain_id || '—'}</td>
                <td className="px-4 py-2.5 mono text-fg-3 text-[11px]">{addr || '—'}</td>
                <td className="px-4 py-2.5">
                  <Badge tone={statusTone}>
                    <span className={`inline-block w-1.5 h-1.5 rounded-full mr-1.5 ${statusTone === 'emerald' ? 'bg-emerald-500 pulse-dot' : statusTone === 'rose' ? 'bg-rose-500' : 'bg-amber-400'}`} />
                    {status}
                  </Badge>
                </td>
                <td className="px-4 py-2.5 text-fg-3">{fmtRel(b.last_seen)}</td>
                <td className="px-4 py-2.5">
                  <div className="space-y-1">
                    <div className="flex items-center gap-1.5 text-[10px] text-fg-4">
                      <span className="w-6">CPU</span>
                      <MiniBar value={cpuPct} max={100} />
                    </div>
                    <div className="flex items-center gap-1.5 text-[10px] text-fg-4">
                      <span className="w-6">MEM</span>
                      <MiniBar value={memPct} max={100} tone="accent" />
                    </div>
                  </div>
                </td>
                <td className="px-4 py-2.5">
                  {capacity > 0
                    ? <MiniBar value={sessions} max={capacity} tone="emerald" />
                    : <span className="text-fg-4 mono">{sessions}</span>}
                </td>
                <td className="px-4 py-2.5 text-right">
                  <Button
                    variant="ghost"
                    size="xs"
                    title="Send heartbeat"
                    onClick={() => onHeartbeat && onHeartbeat(b.brain_id)}
                  >
                    <I.Radio size={12} />
                  </Button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
};

// Sessions table
const SessionsTab = ({ sessions, loading, t }) => {
  const { EmptyState, SkeletonRows } = window.cc.ui || window;
  if (loading) return <SkeletonRows rows={4} cols={3} />;
  if (!sessions || sessions.length === 0) {
    return <EmptyState icon={I.Cluster} title={t('cluster.empty.sessions')} />;
  }
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-[12px]">
        <thead>
          <tr className="border-b border-border text-fg-4 uppercase tracking-wider text-[10px]">
            {['cluster.col.session_id','cluster.col.brain','cluster.col.last_touched'].map(k => (
              <th key={k} className="text-left px-4 py-2.5">{t(k)}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {sessions.map((s, i) => (
            <tr key={s.session_id || i} className="border-b border-border/50 hover:bg-[var(--hover)] transition-colors">
              <td className="px-4 py-2.5 mono text-fg">{s.session_id || '—'}</td>
              <td className="px-4 py-2.5 mono text-fg-3">{s.brain_id || '—'}</td>
              <td className="px-4 py-2.5 text-fg-3">{fmtRel(s.last_touched || s.last_seen)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
};

const ClusterDashboardPage = ({ lang }) => {
  const t = tFor(lang || 'en');
  const toast = window.cc.ui && window.cc.ui.useToast ? window.cc.ui.useToast() : { show: () => {} };

  const [tab, setTab] = clS('brains');
  const [data, setData] = clS(null);
  const [loading, setLoading] = clS(true);
  const [err, setErr] = clS(null);
  const [autoRefresh, setAutoRefresh] = clS(true);
  const [regOpen, setRegOpen] = clS(false);
  const timerRef = clR(null);

  const reload = clC(async (silent = false) => {
    if (!silent) setLoading(true);
    setErr(null);
    try {
      const res = await window.cc.api.cluster.state();
      setData(res);
    } catch (e) {
      setErr(e && e.message ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  // Auto-refresh every 10s
  clE(() => {
    reload();
    if (autoRefresh) {
      timerRef.current = setInterval(() => reload(true), 10000);
    }
    return () => { if (timerRef.current) clearInterval(timerRef.current); };
  }, [autoRefresh]);

  const handleHeartbeat = async (brain_id) => {
    try {
      await window.cc.api.cluster.heartbeat(brain_id);
      toast.show && toast.show(t('cluster.heartbeat.ok'), 'success');
    } catch (e) {
      toast.show && toast.show(t('cluster.heartbeat.failed') + ': ' + (e && e.message ? e.message : String(e)), 'error');
    }
  };

  const brains   = (data && (data.brains   || data.nodes   || data.brain_nodes)) || [];
  const sessions = (data && (data.sessions || data.active_sessions))              || [];

  const healthyCount = brains.filter(b => (b.status || '') === 'healthy').length;

  const { Card, CardHeader, Button, Badge, Tabs, StatCard, ErrorBanner, Switch } = window.cc.ui || window;

  const tabs = [
    { value: 'brains',   label: t('cluster.tab.brains'),   count: brains.length },
    { value: 'sessions', label: t('cluster.tab.sessions'), count: sessions.length },
  ];

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-3 flex-wrap">
        <div>
          <h1 className="text-[18px] font-semibold">{t('cluster_dashboard')}</h1>
          <p className="text-[12px] text-fg-3 mt-0.5">{t('sub_cluster_dashboard')}</p>
        </div>
        <div className="flex items-center gap-2 flex-wrap">
          <label className="flex items-center gap-1.5 text-[12px] text-fg-3 cursor-pointer select-none">
            <Switch checked={autoRefresh} onChange={setAutoRefresh} size="sm" />
            {t('cluster.auto_refresh')}
          </label>
          <Button variant="outline" size="sm" onClick={() => reload(false)} disabled={loading}>
            <I.Refresh size={13} className={loading ? 'animate-spin' : ''} />
            {t('cluster.refresh')}
          </Button>
          <Button variant="default" size="sm" onClick={() => setRegOpen(true)}>
            <I.Plus size={13} />
            {t('cluster.register')}
          </Button>
        </div>
      </div>

      {/* Stats — StatCard expects a component reference for `icon`,
          not a pre-rendered JSX element (it does `<IconCmp size={14}/>`). */}
      <div className="grid grid-cols-3 gap-3">
        <StatCard
          label={t('cluster.stat.brains')}
          value={loading ? '…' : brains.length}
          icon={I.BrainNode}
          tone="default"
        />
        <StatCard
          label={t('cluster.stat.healthy')}
          value={loading ? '…' : healthyCount}
          icon={I.CheckCircle}
          tone="emerald"
        />
        <StatCard
          label={t('cluster.stat.sessions')}
          value={loading ? '…' : sessions.length}
          icon={I.Cluster}
          tone="accent"
        />
      </div>

      {err && <ErrorBanner message={err} />}

      <Card>
        <CardHeader title={<Tabs items={tabs} value={tab} onChange={setTab} />} />
        <div className="p-0">
          {tab === 'brains' && (
            <BrainsTab brains={brains} loading={loading} t={t} onHeartbeat={handleHeartbeat} />
          )}
          {tab === 'sessions' && (
            <SessionsTab sessions={sessions} loading={loading} t={t} />
          )}
        </div>
      </Card>

      <RegisterBrainModal
        open={regOpen}
        onClose={() => setRegOpen(false)}
        onSuccess={() => reload(false)}
        t={t}
      />
    </div>
  );
};

window.ClusterDashboardPage = ClusterDashboardPage;
