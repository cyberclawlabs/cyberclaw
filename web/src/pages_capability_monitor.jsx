// pages_capability_monitor.jsx — W4 D-3 Capability Monitor SPA.
//
// Surfaces capability_requests (failure clusters), skill usage telemetry,
// and curator activity in one operator-facing view. Backend:
//   GET  /api/v1/admin/capability-requests?status=&limit=
//   PUT  /api/v1/admin/capability-requests/:id/status
//   POST /api/v1/admin/capability-requests/run-feedback
//   GET  /api/v1/admin/curator/status   GET /skill-usage   POST /run-now
// Exposed as window.CapabilityMonitorPage (wired in app.jsx).

const { useEffect: cmU, useState: cmS, useMemo: cmM } = React;
const { Card: CmCard, CardHeader: CmCardHeader, Button: CmButton, Tabs: CmTabs,
  StatCard: CmStat, Badge: CmBadge, RiskBadge: CmRiskBadge, Sheet: CmSheet,
  Dialog: CmDialog, EmptyState: CmEmpty, SkeletonRows: CmSkeletonRows,
  ErrorBanner: CmErr, Skeleton: CmSk, useToast: cmUseToast, I: CmI } = window.cc.ui || window;

const FAIL_TONE = { not_found: 'amber', governance_denied: 'rose', execution_error: 'orange' };
const STATUS_TONE = { pending: 'amber', implemented: 'emerald', rejected: 'slate' };
const ORIGIN_TONE = { builtin: 'slate', hub: 'accent', local: 'amber', learned: 'violet', remote: 'cyan' };
const VERDICT_TONE = { approved: 'emerald', rejected: 'rose', flagged: 'amber',
  retired: 'slate', promoted: 'cyan', quarantined: 'orange' };
const VERDICT_DOT = { emerald: 'bg-emerald-500', rose: 'bg-rose-500', amber: 'bg-amber-400',
  cyan: 'bg-cyan-500', orange: 'bg-orange-500', slate: 'bg-slate-400' };

const fmtTs = (s) => { if (!s) return '—'; try { return new Date(s).toLocaleString(); } catch { return s; } };
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

const DetailField = ({ label, value }) => (
  <div>
    <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1">{label}</div>
    <div>{value}</div>
  </div>
);

// ---- Tab 1: Failure Queue ------------------------------------------------

const FailureQueueTab = ({ t }) => {
  const toast = cmUseToast();
  const [rows, setRows] = cmS([]);
  const [loading, setLoading] = cmS(true);
  const [err, setErr] = cmS(null);
  const [statusFilter, setStatusFilter] = cmS('pending');
  const [activeRow, setActiveRow] = cmS(null);
  const [running, setRunning] = cmS(false);
  const [updating, setUpdating] = cmS(false);

  const reload = async () => {
    setLoading(true); setErr(null);
    try {
      const params = statusFilter === 'all' ? { limit: 200 } : { status: statusFilter, limit: 200 };
      const res = await window.cc.api.capabilityRequests.list(params);
      setRows((res && res.entries) || []);
    } catch (e) { setErr(e && e.message ? e.message : String(e)); setRows([]); }
    finally { setLoading(false); }
  };
  cmU(() => { reload(); }, [statusFilter]);

  const runFeedback = async () => {
    setRunning(true);
    try {
      const r = await window.cc.api.capabilityRequests.runFeedback();
      toast && toast.toast({ title: t('capmon.feedback.ok'),
        desc: t('capmon.feedback.summary', { aggregated: (r && r.aggregated) || 0, written: (r && r.written_to_queue) || 0 }),
        tone: 'success' });
      reload();
    } catch (e) {
      toast && toast.toast({ title: t('capmon.feedback.failed'), desc: String(e), tone: 'error' });
    } finally { setRunning(false); }
  };

  const updateStatus = async (id, status) => {
    setUpdating(true);
    try {
      await window.cc.api.capabilityRequests.updateStatus(id, { status, notes: null });
      toast && toast.toast({ title: t('capmon.status.ok_' + status), tone: 'success' });
      setActiveRow(null); reload();
    } catch (e) {
      toast && toast.toast({ title: t('capmon.status.failed'), desc: String(e), tone: 'error' });
    } finally { setUpdating(false); }
  };

  const sorted = cmM(() => [...rows].sort((a, b) => (b.count || 0) - (a.count || 0)), [rows]);
  const totals = cmM(() => {
    let total = 0, hot = 0; const byReason = {};
    for (const r of rows) {
      total += (r.count || 0); hot = Math.max(hot, r.count || 0);
      const k = r.failure_reason || 'unknown'; byReason[k] = (byReason[k] || 0) + (r.count || 0);
    }
    return { total, hot, byReason, clusters: rows.length };
  }, [rows]);

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 md:grid-cols-4 gap-3">
        <CmStat label={t('capmon.stat.clusters')}    value={loading ? <CmSk h={20} w={50} /> : String(totals.clusters)}    icon={CmI.Activity} />
        <CmStat label={t('capmon.stat.attempts')}    value={loading ? <CmSk h={20} w={50} /> : String(totals.total)}       icon={CmI.Filter} />
        <CmStat label={t('capmon.stat.hottest')}     value={loading ? <CmSk h={20} w={50} /> : String(totals.hot)}         icon={CmI.Zap} />
        <CmStat label={t('capmon.stat.governance')}  value={loading ? <CmSk h={20} w={50} /> : String(totals.byReason.governance_denied || 0)} icon={CmI.Shield} />
      </div>

      <CmCard className="overflow-hidden">
        <CmCardHeader
          title={t('capmon.queue.title')}
          subtitle={<span className="mono">GET /api/v1/admin/capability-requests</span>}
          right={<div className="flex items-center gap-2">
            <div className="inline-flex items-center rounded-md border border-border bg-bg-3 p-0.5">
              {['pending', 'all', 'implemented', 'rejected'].map((s) => (
                <button key={s} onClick={() => setStatusFilter(s)}
                  className={`px-2 h-6 text-[11px] rounded-[4px] mono transition-colors ${
                    statusFilter === s ? 'bg-bg-2 text-fg border border-border' : 'text-fg-3 hover:text-fg'}`}>
                  {t('capmon.filter.' + s)}
                </button>
              ))}
            </div>
            <CmButton size="sm" variant="ghost" onClick={reload}><CmI.Refresh size={12} /> {t('capmon.action.reload')}</CmButton>
            <CmButton size="sm" variant="default" onClick={runFeedback} disabled={running}>
              {running
                ? <><CmI.Loader size={12} className="animate-spin" /> {t('capmon.feedback.running')}</>
                : <><CmI.Zap size={12} /> {t('capmon.feedback.cta')}</>}
            </CmButton>
          </div>}
        />
        {err && <div className="px-4 pt-3"><CmErr message={err} onRetry={reload} /></div>}
        {loading ? <CmSkeletonRows rows={5} cols={6} />
          : sorted.length === 0
          ? <CmEmpty icon={CmI.Activity} title={t('capmon.queue.empty.title')} subtitle={t('capmon.queue.empty.subtitle')} />
          : <table className="w-full text-sm">
              <thead className="border-b border-border bg-bg-3 text-fg-3 text-[11px]">
                <tr>
                  <th className="text-left px-4 py-2 font-medium">{t('capmon.col.tool')}</th>
                  <th className="text-left px-4 py-2 font-medium">{t('capmon.col.reason')}</th>
                  <th className="text-left px-4 py-2 font-medium">{t('capmon.col.attempted_cap')}</th>
                  <th className="text-right px-4 py-2 font-medium">{t('capmon.col.count')}</th>
                  <th className="text-left px-4 py-2 font-medium">{t('capmon.col.last_seen')}</th>
                  <th className="text-left px-4 py-2 font-medium">{t('capmon.col.status')}</th>
                  <th className="text-right px-4 py-2 font-medium" />
                </tr>
              </thead>
              <tbody>
                {sorted.map((r) => {
                  const c = r.count || 0;
                  const cTone = c >= 5 ? 'text-rose-400' : c >= 2 ? 'text-amber-400' : 'text-fg-2';
                  return (
                    <tr key={r.id} onClick={() => setActiveRow(r)}
                      className="border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)] cursor-pointer transition-colors">
                      <td className="px-4 py-2 mono text-[12.5px] text-fg">{r.tool_name || '—'}</td>
                      <td className="px-4 py-2"><CmBadge tone={FAIL_TONE[r.failure_reason] || 'slate'}>{r.failure_reason || 'unknown'}</CmBadge></td>
                      <td className="px-4 py-2 mono text-[11.5px] text-fg-3 truncate max-w-[260px]">
                        {r.attempted_capability_id || r.attempted_connector_id || '—'}
                      </td>
                      <td className="px-4 py-2 text-right">
                        <span className={`mono tabular-nums font-semibold ${cTone}`} style={{ fontSize: 13 }}>
                          {r.count != null ? r.count : '—'}
                        </span>
                      </td>
                      <td className="px-4 py-2 mono text-[11px] text-fg-3" title={fmtTs(r.requested_at)}>
                        {r.requested_at ? fmtRel(r.requested_at) : '—'}
                      </td>
                      <td className="px-4 py-2"><CmBadge tone={STATUS_TONE[r.status] || 'slate'}>{r.status || 'pending'}</CmBadge></td>
                      <td className="px-4 py-2 text-right"><CmI.ChevronRight size={12} className="text-fg-4" /></td>
                    </tr>
                  );
                })}
              </tbody>
            </table>}
      </CmCard>

      <CmSheet
        open={!!activeRow}
        onClose={() => setActiveRow(null)}
        title={activeRow ? activeRow.tool_name : ''}
        subtitle={activeRow ? <span className="mono">id {activeRow.id}</span> : ''}
        right={activeRow && <CmBadge tone={FAIL_TONE[activeRow.failure_reason] || 'slate'}>{activeRow.failure_reason}</CmBadge>}>
        {activeRow && (
          <div className="p-5 space-y-5">
            <div className="grid grid-cols-2 gap-3 text-[12px]">
              <DetailField label={t('capmon.detail.count')}      value={<span className="mono tabular-nums text-[18px] font-semibold text-fg">{activeRow.count}</span>} />
              <DetailField label={t('capmon.detail.status')}     value={<CmBadge tone={STATUS_TONE[activeRow.status] || 'slate'}>{activeRow.status}</CmBadge>} />
              <DetailField label={t('capmon.detail.first_seen')} value={<span className="mono text-fg-2">{fmtTs(activeRow.requested_at)}</span>} />
              <DetailField label={t('capmon.detail.last_seen')}  value={<span className="mono text-fg-2">{fmtTs(activeRow.last_seen_at || activeRow.requested_at)}</span>} />
              <DetailField label={t('capmon.detail.attempted_cap')}        value={<span className="mono text-fg-2 break-all">{activeRow.attempted_capability_id || '—'}</span>} />
              <DetailField label={t('capmon.detail.attempted_connector')}  value={<span className="mono text-fg-2 break-all">{activeRow.attempted_connector_id || '—'}</span>} />
            </div>
            {activeRow.notes && (
              <div>
                <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('capmon.detail.notes')}</div>
                <div className="bg-bg-3 border border-border rounded p-3 text-[12.5px] text-fg-2 whitespace-pre-wrap">{activeRow.notes}</div>
              </div>
            )}
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-2">{t('capmon.detail.raw')}</div>
              <pre className="bg-bg-3 border border-border rounded p-3 mono text-[11px] text-fg-3 overflow-x-auto whitespace-pre-wrap break-words">
                {JSON.stringify(activeRow, null, 2)}
              </pre>
            </div>
            {activeRow.status === 'pending' && (
              <div className="flex items-center gap-2 pt-3 border-t border-border">
                <CmButton variant="default" disabled={updating} onClick={() => updateStatus(activeRow.id, 'implemented')}>
                  <CmI.Check size={12} /> {t('capmon.action.mark_implemented')}
                </CmButton>
                <CmButton variant="danger-outline" disabled={updating} onClick={() => updateStatus(activeRow.id, 'rejected')}>
                  <CmI.Close size={12} /> {t('capmon.action.mark_rejected')}
                </CmButton>
              </div>
            )}
          </div>
        )}
      </CmSheet>
    </div>
  );
};

// ---- Tab 2: Skill Usage --------------------------------------------------

const SkillUsageTab = ({ t }) => {
  const [data, setData] = cmS(null);
  const [loading, setLoading] = cmS(true);
  const [err, setErr] = cmS(null);
  const reload = async () => {
    setLoading(true); setErr(null);
    try { setData(await window.cc.api.curator.skillUsage()); }
    catch (e) { setErr(e && e.message ? e.message : String(e)); setData(null); }
    finally { setLoading(false); }
  };
  cmU(() => { reload(); }, []);

  const records = (data && data.records) || [];
  const sorted = cmM(() => [...records].sort((a, b) => (b.use_count || 0) - (a.use_count || 0)), [records]);
  const idleCount = records.filter((r) => (r.days_idle || 0) >= 30).length;
  const activeCount = records.filter((r) => (r.use_count || 0) > 0).length;

  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
        <CmStat label={t('capmon.usage.stat.total')}  value={loading ? <CmSk h={20} w={40} /> : String(records.length)} icon={CmI.Brain} />
        <CmStat label={t('capmon.usage.stat.active')} value={loading ? <CmSk h={20} w={40} /> : String(activeCount)}    icon={CmI.Activity} />
        <CmStat label={t('capmon.usage.stat.idle')}   value={loading ? <CmSk h={20} w={40} /> : String(idleCount)}      icon={CmI.Clock} />
      </div>
      <CmCard className="overflow-hidden">
        <CmCardHeader
          title={t('capmon.usage.title')}
          subtitle={<span className="mono">GET /api/v1/admin/curator/skill-usage</span>}
          right={<CmButton size="sm" variant="ghost" onClick={reload}><CmI.Refresh size={12} /> {t('capmon.action.reload')}</CmButton>}
        />
        {err && <div className="px-4 pt-3"><CmErr message={err} onRetry={reload} /></div>}
        {loading ? <CmSkeletonRows rows={5} cols={5} />
          : sorted.length === 0
          ? <CmEmpty icon={CmI.Brain} title={t('capmon.usage.empty.title')} subtitle={t('capmon.usage.empty.subtitle')} />
          : <table className="w-full text-sm">
              <thead className="border-b border-border bg-bg-3 text-fg-3 text-[11px]">
                <tr>
                  <th className="text-left px-4 py-2 font-medium">{t('capmon.usage.col.skill')}</th>
                  <th className="text-left px-4 py-2 font-medium">{t('capmon.usage.col.origin')}</th>
                  <th className="text-right px-4 py-2 font-medium">{t('capmon.usage.col.use_count')}</th>
                  <th className="text-left px-4 py-2 font-medium">{t('capmon.usage.col.last_used')}</th>
                  <th className="text-right px-4 py-2 font-medium">{t('capmon.usage.col.days_idle')}</th>
                </tr>
              </thead>
              <tbody>
                {sorted.map((r, i) => {
                  const idle = r.days_idle || 0;
                  const idleTone = idle >= 30 ? 'rose' : idle >= 14 ? 'amber' : 'slate';
                  return (
                    <tr key={r.skill_id || r.skill || i} className="border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)]">
                      <td className="px-4 py-2 mono text-[12px] text-fg">{r.skill || r.skill_id || r.name}</td>
                      <td className="px-4 py-2"><CmBadge tone={ORIGIN_TONE[(r.origin || '').toLowerCase()] || 'slate'}>{r.origin || 'unknown'}</CmBadge></td>
                      <td className="px-4 py-2 text-right mono tabular-nums text-fg-2">{r.use_count != null ? r.use_count : '—'}</td>
                      <td className="px-4 py-2 mono text-[11px] text-fg-3" title={fmtTs(r.last_used_at)}>{r.last_used_at ? fmtRel(r.last_used_at) : '—'}</td>
                      <td className="px-4 py-2 text-right"><CmBadge tone={idleTone}>{idle}d</CmBadge></td>
                    </tr>
                  );
                })}
              </tbody>
            </table>}
      </CmCard>
    </div>
  );
};

// ---- Tab 3: Curator Activity ---------------------------------------------

const CuratorActivityTab = ({ t }) => {
  const toast = cmUseToast();
  const [status, setStatus] = cmS(null);
  const [loading, setLoading] = cmS(true);
  const [err, setErr] = cmS(null);
  const [confirm, setConfirm] = cmS(false);
  const [running, setRunning] = cmS(false);

  const reload = async () => {
    setLoading(true); setErr(null);
    try { setStatus(await window.cc.api.curator.status()); }
    catch (e) { setErr(e && e.message ? e.message : String(e)); setStatus(null); }
    finally { setLoading(false); }
  };
  cmU(() => { reload(); }, []);

  const run = async () => {
    setRunning(true);
    try {
      const r = await window.cc.api.curator.runNow();
      toast && toast.toast({ title: t('capmon.curator.run.ok'), desc: r && r.run_id ? 'run_id ' + r.run_id : '', tone: 'success' });
      reload();
    } catch (e) {
      toast && toast.toast({ title: t('capmon.curator.run.failed'), desc: String(e), tone: 'error' });
    } finally { setRunning(false); setConfirm(false); }
  };

  const verdicts = (status && status.recent_verdicts) || [];
  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
        <CmStat label={t('capmon.curator.last_run')}   value={loading ? <CmSk h={20} w={120} /> : (status && status.last_run_at ? fmtRel(status.last_run_at) : '—')} icon={CmI.Clock} />
        <CmStat label={t('capmon.curator.next_run')}   value={loading ? <CmSk h={20} w={120} /> : (status && status.next_run_at ? fmtRel(status.next_run_at) : '—')} icon={CmI.Activity} />
        <CmStat label={t('capmon.curator.total_runs')} value={loading ? <CmSk h={20} w={40} />  : (status && status.total_runs != null ? String(status.total_runs) : '—')} icon={CmI.Refresh} />
      </div>
      {err && <CmErr message={err} onRetry={reload} />}

      <CmCard className="overflow-hidden">
        <CmCardHeader title={t('capmon.curator.actions.title')}
          subtitle={<span className="mono">POST /api/v1/admin/curator/run-now</span>}
          right={<CmRiskBadge level="high" />} />
        <div className="p-5 space-y-3">
          <div className="text-[13px] text-fg-2 leading-relaxed max-w-prose">{t('capmon.curator.actions.body')}</div>
          <CmButton variant="danger-outline" onClick={() => setConfirm(true)} disabled={running}>
            {running
              ? <><CmI.Loader size={12} className="animate-spin" /> {t('capmon.curator.run.running')}</>
              : <><CmI.Zap size={12} /> {t('capmon.curator.run.cta')}</>}
          </CmButton>
        </div>
      </CmCard>

      {verdicts.length === 0
        ? <CmCard className="py-2"><CmEmpty icon={CmI.Activity}
            title={t('capmon.curator.verdicts.empty.title')}
            subtitle={t('capmon.curator.verdicts.empty.subtitle')} /></CmCard>
        : <CmCard className="overflow-hidden">
            <CmCardHeader title={t('capmon.curator.verdicts.title')}
              subtitle={<span className="mono">{verdicts.length} {t('capmon.curator.verdicts.count_label')}</span>} />
            <div className="relative pl-10 pr-4 py-4">
              <div className="absolute left-[24px] top-0 bottom-0 w-px bg-border" />
              {verdicts.map((v, i) => {
                const tone = VERDICT_TONE[(v.verdict || '').toLowerCase()] || 'slate';
                return (
                  <div key={v.id || i} className="relative pb-5 last:pb-0">
                    <div className={`absolute -left-[18px] top-1.5 w-3 h-3 rounded-full border-2 border-bg-2 ${VERDICT_DOT[tone] || VERDICT_DOT.slate}`} />
                    <div className="flex items-center gap-2 mb-1.5">
                      <CmBadge tone={tone}>{v.verdict || 'unknown'}</CmBadge>
                      {v.skill && <span className="text-[12px] mono text-fg">{v.skill}</span>}
                      <span className="text-[11px] mono text-fg-4 ml-auto" title={fmtTs(v.timestamp)}>{fmtRel(v.timestamp)}</span>
                    </div>
                    {v.reason && <div className="text-[12.5px] text-fg-2 leading-relaxed">{v.reason}</div>}
                  </div>
                );
              })}
            </div>
          </CmCard>}

      <CmDialog open={confirm} onClose={() => setConfirm(false)}
        title={t('capmon.curator.confirm.title')}
        subtitle={t('capmon.curator.confirm.subtitle')}
        footer={<>
          <CmButton variant="ghost" onClick={() => setConfirm(false)}>{t('capmon.curator.confirm.cancel')}</CmButton>
          <CmButton variant="danger" onClick={run} disabled={running}>
            <CmI.Zap size={12} /> {t('capmon.curator.confirm.go')}
          </CmButton>
        </>}>
        <p className="text-[13px] text-fg-2">{t('capmon.curator.confirm.body')}</p>
      </CmDialog>
    </div>
  );
};

// ---- Page shell ----------------------------------------------------------

const CapabilityMonitorPage = ({ lang }) => {
  const t = tFor(lang || 'en');
  const [tab, setTab] = cmS('queue');
  return (
    <div className="space-y-4">
      <CmTabs value={tab} onChange={setTab} items={[
        { value: 'queue',   label: t('capmon.tab.queue') },
        { value: 'usage',   label: t('capmon.tab.usage') },
        { value: 'curator', label: t('capmon.tab.curator') },
      ]} />
      {tab === 'queue'   && <FailureQueueTab t={t} />}
      {tab === 'usage'   && <SkillUsageTab t={t} />}
      {tab === 'curator' && <CuratorActivityTab t={t} />}
    </div>
  );
};

Object.assign(window, { CapabilityMonitorPage });
