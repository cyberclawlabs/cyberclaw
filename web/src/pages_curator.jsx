// pages_curator.jsx — Autonomous Curator + Skill Usage.
//
// Stat cards: Last run / Next run / Total runs.
// Tabs: Verdicts / Skill Usage / Actions.
//
// Backend contract:
//   GET  /api/v1/admin/curator/status      → { last_run, next_run, total_runs, recent_verdicts:[...] }
//   POST /api/v1/admin/curator/run-now     → { ok, run_id }
//   GET  /api/v1/admin/curator/skill-usage → { records:[...] }

const { useEffect: cuU, useState: cuS, useMemo: cuM } = React;
const {
  Card: CuCard, CardHeader: CuCardHeader, Button: CuButton, Tabs: CuTabs,
  StatCard: CuStat, Badge: CuBadge, RiskBadge: CuRiskBadge, Dialog: CuDialog,
  EmptyState: CuEmpty, SkeletonRows: CuSkeletonRows, ErrorBanner: CuErr,
  Skeleton: CuSk, useToast: cuUseToast, I: CuI,
} = window.cc.ui || window;

const VERDICT_TONE = {
  approved:    'emerald',
  rejected:    'rose',
  flagged:     'amber',
  retired:     'slate',
  promoted:    'cyan',
  quarantined: 'orange',
};

const ORIGIN_TONE = {
  builtin: 'slate',
  hub:     'accent',
  local:   'amber',
  learned: 'violet',
  remote:  'cyan',
};

const fmtTs = (s) => {
  if (!s) return '—';
  try { return new Date(s).toLocaleString(); } catch { return s; }
};
const fmtRelative = (s, t) => {
  if (!s) return '—';
  try {
    const ms = Date.now() - new Date(s).getTime();
    if (ms < 0) return t('curator.in_future', { ms: Math.abs(ms) });
    const sec = Math.floor(ms / 1000);
    if (sec < 60) return sec + 's';
    const min = Math.floor(sec / 60);
    if (min < 60) return min + 'm';
    const h = Math.floor(min / 60);
    if (h < 48) return h + 'h';
    return Math.floor(h / 24) + 'd';
  } catch { return s; }
};

// ---- Verdicts timeline ---------------------------------------------------

const VerdictsTimeline = ({ verdicts, t }) => {
  if (!verdicts || verdicts.length === 0) {
    return <CuCard className="py-2">
      <CuEmpty icon={CuI.Activity} title={t('curator.verdicts.empty.title')} subtitle={t('curator.verdicts.empty.subtitle')} />
    </CuCard>;
  }
  return (
    <CuCard className="overflow-hidden">
      <CuCardHeader title={t('curator.verdicts.title')}
                    subtitle={<span className="mono">{verdicts.length} {t('curator.verdicts.count_label')}</span>} />
      <div className="relative pl-10 pr-4 py-4">
        <div className="absolute left-[24px] top-0 bottom-0 w-px bg-border" />
        {verdicts.map((v, i) => {
          const tone = VERDICT_TONE[(v.verdict || '').toLowerCase()] || 'slate';
          return (
            <div key={v.id || i} className="relative pb-5 last:pb-0">
              <div className={`absolute -left-[18px] top-1.5 w-3 h-3 rounded-full border-2 border-bg-2 ${
                tone === 'emerald' ? 'bg-emerald-500' :
                tone === 'rose'    ? 'bg-rose-500' :
                tone === 'amber'   ? 'bg-amber-400' :
                tone === 'cyan'    ? 'bg-cyan-500' :
                tone === 'orange'  ? 'bg-orange-500' : 'bg-slate-400'}`} />
              <div className="flex items-center gap-2 mb-1.5">
                <CuBadge tone={tone}>{v.verdict || 'unknown'}</CuBadge>
                {v.skill && <span className="text-[12px] mono text-fg">{v.skill}</span>}
                <span className="text-[11px] mono text-fg-4 ml-auto" title={fmtTs(v.timestamp)}>
                  {fmtRelative(v.timestamp, t)}
                </span>
              </div>
              {v.reason && (
                <div className="text-[12.5px] text-fg-2 leading-relaxed">{v.reason}</div>
              )}
              {v.evidence && (
                <details className="mt-1.5 text-[11px]">
                  <summary className="cursor-pointer text-fg-4 hover:text-fg-3">{t('curator.verdicts.evidence')}</summary>
                  <pre className="mt-1.5 bg-bg-3 border border-border rounded p-2 mono text-[11px] text-fg-3 whitespace-pre-wrap break-words overflow-x-auto">
                    {typeof v.evidence === 'string' ? v.evidence : JSON.stringify(v.evidence, null, 2)}
                  </pre>
                </details>
              )}
            </div>
          );
        })}
      </div>
    </CuCard>
  );
};

// ---- Skill usage table ---------------------------------------------------

const SkillUsageTable = ({ data, loading, error, onReload, t }) => {
  if (loading) return <CuCard><CuSkeletonRows rows={6} cols={5} /></CuCard>;
  if (error)   return <CuCard className="p-4"><CuErr message={error} onRetry={onReload} /></CuCard>;
  const records = (data && data.records) || data || [];
  if (!Array.isArray(records) || records.length === 0) {
    return <CuCard className="py-2">
      <CuEmpty icon={CuI.Brain} title={t('curator.usage.empty.title')} subtitle={t('curator.usage.empty.subtitle')} />
    </CuCard>;
  }
  // Sort by days_idle desc to surface stale skills first
  const sorted = [...records].sort((a, b) => (b.days_idle || 0) - (a.days_idle || 0));
  return (
    <CuCard className="overflow-hidden">
      <CuCardHeader
        title={t('curator.usage.title')}
        subtitle={<span className="mono">{records.length} skills</span>}
        right={<CuButton size="sm" variant="ghost" onClick={onReload}><CuI.Refresh size={12} /> {t('curator.action.reload')}</CuButton>}
      />
      <table className="w-full text-sm">
        <thead className="border-b border-border bg-bg-3 text-fg-3 text-[11px]">
          <tr>
            <th className="text-left px-4 py-2 font-medium">{t('curator.usage.col.skill')}</th>
            <th className="text-left px-4 py-2 font-medium">{t('curator.usage.col.origin')}</th>
            <th className="text-right px-4 py-2 font-medium">{t('curator.usage.col.use_count')}</th>
            <th className="text-left px-4 py-2 font-medium">{t('curator.usage.col.last_used')}</th>
            <th className="text-right px-4 py-2 font-medium">{t('curator.usage.col.days_idle')}</th>
          </tr>
        </thead>
        <tbody>
          {sorted.map((r, i) => {
            const idle = r.days_idle || 0;
            const idleTone = idle > 30 ? 'rose' : idle > 14 ? 'amber' : 'slate';
            return (
              <tr key={r.skill_id || r.skill || i} className="border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)]">
                <td className="px-4 py-2 text-fg mono text-[12px]">{r.skill || r.skill_id || r.name}</td>
                <td className="px-4 py-2">
                  <CuBadge tone={ORIGIN_TONE[(r.origin || '').toLowerCase()] || 'slate'}>{r.origin || 'unknown'}</CuBadge>
                </td>
                <td className="px-4 py-2 text-right mono tabular-nums text-fg-2">{r.use_count != null ? r.use_count : '—'}</td>
                <td className="px-4 py-2 mono text-[11px] text-fg-3" title={fmtTs(r.last_used_at)}>
                  {r.last_used_at ? fmtRelative(r.last_used_at, t) : '—'}
                </td>
                <td className="px-4 py-2 text-right">
                  <CuBadge tone={idleTone}>{idle}d</CuBadge>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </CuCard>
  );
};

// ---- Actions panel -------------------------------------------------------

const ActionsPanel = ({ status, onRan, t }) => {
  const toast = cuUseToast();
  const [confirm, setConfirm] = cuS(false);
  const [running, setRunning] = cuS(false);

  const run = async () => {
    setRunning(true);
    try {
      const resp = await window.cc.api.curator.runNow();
      toast && toast.toast({
        title: t('curator.run.ok'),
        desc: resp && resp.run_id ? 'run_id ' + resp.run_id : '',
        tone: 'success',
      });
      onRan && onRan();
    } catch (e) {
      toast && toast.toast({ title: t('curator.run.failed'), desc: String(e), tone: 'error' });
    } finally {
      setRunning(false);
      setConfirm(false);
    }
  };

  return (
    <CuCard className="overflow-hidden">
      <CuCardHeader title={t('curator.actions.title')} subtitle={<span className="mono">POST /api/v1/admin/curator/run-now</span>} right={<CuRiskBadge level="high" />} />
      <div className="p-5 space-y-4">
        <div className="text-[13px] text-fg-2 leading-relaxed max-w-prose">
          {t('curator.actions.body')}
        </div>
        <div className="grid grid-cols-1 md:grid-cols-3 gap-3 text-[12px]">
          <div className="bg-bg-3 border border-border rounded-md p-3">
            <div className="text-[10px] uppercase tracking-wider text-fg-3">{t('curator.stat.last_run')}</div>
            <div className="mono text-fg-2 mt-1.5">{status && status.last_run ? fmtTs(status.last_run) : '—'}</div>
          </div>
          <div className="bg-bg-3 border border-border rounded-md p-3">
            <div className="text-[10px] uppercase tracking-wider text-fg-3">{t('curator.stat.next_run')}</div>
            <div className="mono text-fg-2 mt-1.5">{status && status.next_run ? fmtTs(status.next_run) : '—'}</div>
          </div>
          <div className="bg-bg-3 border border-border rounded-md p-3">
            <div className="text-[10px] uppercase tracking-wider text-fg-3">{t('curator.stat.total_runs')}</div>
            <div className="text-[18px] tabular-nums font-semibold text-fg mt-1">{status && status.total_runs != null ? status.total_runs : '—'}</div>
          </div>
        </div>
        <div className="pt-2">
          <CuButton variant="danger-outline" onClick={() => setConfirm(true)} disabled={running}>
            {running ? <><CuI.Loader size={12} className="animate-spin" /> {t('curator.run.running')}</>
                     : <><CuI.Zap size={12} /> {t('curator.run.cta')}</>}
          </CuButton>
        </div>
      </div>
      <CuDialog
        open={confirm}
        onClose={() => setConfirm(false)}
        title={t('curator.run.confirm.title')}
        subtitle={t('curator.run.confirm.subtitle')}
        footer={<>
          <CuButton variant="ghost" onClick={() => setConfirm(false)}>{t('curator.run.confirm.cancel')}</CuButton>
          <CuButton variant="danger" onClick={run} disabled={running}>
            <CuI.Zap size={12} /> {t('curator.run.confirm.go')}
          </CuButton>
        </>}>
        <p className="text-[13px] text-fg-2">{t('curator.run.confirm.body')}</p>
      </CuDialog>
    </CuCard>
  );
};

// ---- Page ----------------------------------------------------------------

const CuratorPage = ({ lang }) => {
  const t = tFor(lang || 'en');
  const [tab, setTab] = cuS('verdicts');
  const [status, setStatus] = cuS(null);
  const [statusLoading, setStatusLoading] = cuS(true);
  const [statusErr, setStatusErr] = cuS(null);
  const [usage, setUsage] = cuS(null);
  const [usageLoading, setUsageLoading] = cuS(true);
  const [usageErr, setUsageErr] = cuS(null);

  const loadStatus = async () => {
    setStatusLoading(true); setStatusErr(null);
    try { setStatus(await window.cc.api.curator.status()); }
    catch (e) { setStatusErr(e && e.message ? e.message : String(e)); setStatus(null); }
    finally { setStatusLoading(false); }
  };
  const loadUsage = async () => {
    setUsageLoading(true); setUsageErr(null);
    try { setUsage(await window.cc.api.curator.skillUsage()); }
    catch (e) { setUsageErr(e && e.message ? e.message : String(e)); setUsage(null); }
    finally { setUsageLoading(false); }
  };
  cuU(() => { loadStatus(); loadUsage(); }, []);

  const verdicts = (status && status.recent_verdicts) || [];

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
        <CuStat
          label={t('curator.stat.last_run')}
          value={statusLoading ? <CuSk h={20} w={120} /> : (status && status.last_run ? fmtRelative(status.last_run, t) + ' ago' : '—')}
          icon={CuI.Clock}
        />
        <CuStat
          label={t('curator.stat.next_run')}
          value={statusLoading ? <CuSk h={20} w={120} /> : (status && status.next_run ? 'in ' + fmtRelative(status.next_run, t) : '—')}
          icon={CuI.Activity}
        />
        <CuStat
          label={t('curator.stat.total_runs')}
          value={statusLoading ? <CuSk h={20} w={60} /> : (status && status.total_runs != null ? String(status.total_runs) : '—')}
          icon={CuI.Refresh}
        />
      </div>

      {statusErr && <CuErr message={statusErr} onRetry={loadStatus} />}

      <CuTabs value={tab} onChange={setTab} items={[
        { value: 'verdicts', label: t('curator.tab.verdicts'), count: verdicts.length || null },
        { value: 'usage',    label: t('curator.tab.usage') },
        { value: 'actions',  label: t('curator.tab.actions') },
      ]} />

      {tab === 'verdicts' && (statusLoading
        ? <CuCard><CuSkeletonRows rows={4} cols={3} /></CuCard>
        : <VerdictsTimeline verdicts={verdicts} t={t} />)}
      {tab === 'usage'    && <SkillUsageTable data={usage} loading={usageLoading} error={usageErr} onReload={loadUsage} t={t} />}
      {tab === 'actions'  && <ActionsPanel status={status} onRan={() => { loadStatus(); }} t={t} />}
    </div>
  );
};

Object.assign(window, { CuratorPage });
