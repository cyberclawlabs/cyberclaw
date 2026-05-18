// AgentDetailPage — L10 Agent detail page
// Triggered from AgentsPage by clicking any agent row.
// Breadcrumb: cyberclaw > Agents > {name}
// Sections: Overview / Daily Digest / Recent Executions / Bound Skills

const { useState: uS_D, useEffect: uE_D } = React;

const AgentDetailPage = ({ agentId, lang, onBack }) => {
  const t = tFor(lang || 'en');
  const [tab, setTab] = uS_D('overview');
  const [digest, setDigest] = uS_D(null);
  const [digestLoading, setDigestLoading] = uS_D(false);
  const [digestErr, setDigestErr] = uS_D(null);

  const agentsRes = window.cc.data.useAgents();
  const execRes = window.cc.data.useExecutions({});
  const skillsRes = window.cc.data.useSkills();

  const agentsList = window.cc.data.extractList(agentsRes, 'agents', MOCK.agents);
  const agent = agentsList.find(a => a.agent_id === agentId) || null;

  const execList = window.cc.data.extractList(execRes, 'executions:{}', MOCK.executions);
  const recentExecs = agent
    ? execList.filter(e => e.agent === agent.name || e.agent === agentId).slice(0, 5)
    : [];

  const skillsList = window.cc.data.extractList(skillsRes, 'skills', MOCK.skills);
  const agentSkillKeys = agent && (
    Array.isArray(agent.skill_ids) ? agent.skill_ids :
    (Array.isArray(agent.skills_list) ? agent.skills_list : null)
  );
  const boundSkills = agentSkillKeys
    ? skillsList.filter(s => agentSkillKeys.includes(s.skill_id) || agentSkillKeys.includes(s.name))
    : [];

  // Fetch daily digest when tab is opened (L9 wires real endpoint later)
  uE_D(() => {
    if (tab !== 'digest' || !agent) return;
    let cancelled = false;
    setDigestLoading(true); setDigestErr(null);
    (async () => {
      try {
        const res = await window.cc.api.get(`/api/v1/agents/${agent.agent_id}/digest`);
        if (!cancelled) setDigest(res);
      } catch (e) {
        if (!cancelled) { setDigestErr(e && e.message ? e.message : String(e)); setDigest(null); }
      } finally {
        if (!cancelled) setDigestLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [tab, agent && agent.agent_id]);

  const tabs = [
    { value: 'overview',   label: t('agent.detail.overview') },
    { value: 'digest',     label: t('agent.detail.digest') },
    { value: 'executions', label: t('agent.detail.executions'), count: recentExecs.length },
    { value: 'skills',     label: t('agent.detail.skills'), count: boundSkills.length || undefined },
    { value: 'delegate',   label: t('agent.delegate.tab') },
  ];

  // Avatar color derived from agent_id
  const avatarColors = ['bg-indigo-500', 'bg-violet-500', 'bg-cyan-500', 'bg-emerald-500', 'bg-amber-500', 'bg-rose-500'];
  const avatarColor = agent
    ? avatarColors[agent.agent_id.charCodeAt(agent.agent_id.length - 1) % avatarColors.length]
    : 'bg-slate-500';

  const initials = agent
    ? (agent.display_name || agent.name || agent.agent_id)
        .split(/[\s_-]/).map(s => s[0]).join('').slice(0, 2).toUpperCase()
    : '??';

  if (!agent && !agentsRes.loading) {
    return (
      <div className="space-y-4">
        <_AgentDetailBreadcrumb t={t} name="—" onBack={onBack} />
        <EmptyState icon={I.Bot} title="Agent not found" subtitle={`agent_id: ${agentId}`}
          action={<Button size="sm" onClick={onBack}><I.ChevronLeft size={12} /> {t('agent.detail.back')}</Button>} />
      </div>
    );
  }

  return (
    <div className="space-y-5">
      {/* Breadcrumb */}
      <_AgentDetailBreadcrumb t={t} name={agent ? (agent.display_name || agent.name) : '…'} onBack={onBack} />

      {/* Hero header */}
      <Card className="p-5">
        <div className="flex items-start gap-4">
          <div className={`w-14 h-14 rounded-xl ${avatarColor} flex items-center justify-center text-white text-[18px] font-bold shrink-0 shadow-lg`}>
            {initials}
          </div>
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2 flex-wrap">
              <h1 className="text-[18px] font-semibold text-fg leading-tight">
                {agent ? (agent.display_name || agent.name) : <span className="text-fg-4">Loading…</span>}
              </h1>
              {agent && (
                <Badge tone={agent.trust_level === 'high' ? 'emerald' : agent.trust_level === 'medium' ? 'amber' : 'slate'}>
                  trust: {agent.trust_level}
                </Badge>
              )}
              {agent && <StatusBadge status={agent.status} />}
            </div>
            {agent && (
              <div className="mono text-[11px] text-fg-4 mt-0.5">{agent.agent_id}</div>
            )}
            {agent && agent.description && (
              <p className="text-[13px] text-fg-2 mt-2 leading-relaxed">{agent.description}</p>
            )}
          </div>
          {agent && (
            <div className="flex items-center gap-2 shrink-0">
              <div className="text-right text-[11px] text-fg-3 mono">
                <div>{agent.executions_24h || 0} <span className="text-fg-4">exec / 24h</span></div>
                <div>{agent.tools || 0} <span className="text-fg-4">tools</span> · {agent.skills || 0} <span className="text-fg-4">skills</span></div>
              </div>
            </div>
          )}
        </div>
      </Card>

      {/* Tabs */}
      <div>
        <Tabs value={tab} onChange={setTab} items={tabs} />
      </div>

      {/* Tab content */}
      {tab === 'overview' && agent && (
        <Card className="overflow-hidden">
          <CardHeader title={t('agent.detail.overview')} subtitle="GET /api/v1/agents/:id" />
          <div className="p-4 grid grid-cols-2 gap-3">
            <InfoRow label="agent_id"       value={agent.agent_id} mono />
            <InfoRow label="display_name"   value={agent.display_name || agent.name} />
            <InfoRow label="user_id"        value={agent.user_id || '—'} mono />
            <InfoRow label="workspace_mode" value={agent.workspace_mode || '—'} />
            <InfoRow label="persona_file"   value={agent.persona_file ? agent.persona_file + '.md' : '—'} mono />
            <InfoRow label="trust_level"    value={<Badge tone={agent.trust_level === 'high' ? 'emerald' : agent.trust_level === 'medium' ? 'amber' : 'slate'}>{agent.trust_level}</Badge>} />
            <InfoRow label="status"         value={<StatusBadge status={agent.status} />} />
            <InfoRow label="executions_24h" value={agent.executions_24h ?? '—'} mono />
            {Array.isArray(agent.default_skills) && agent.default_skills.length > 0 && (
              <div className="col-span-2 flex items-start justify-between px-3 py-2 bg-bg-3 rounded-md border border-border gap-3">
                <span className="text-[11px] text-fg-3 uppercase tracking-wider shrink-0">default_skills</span>
                <div className="flex flex-wrap gap-1 justify-end">
                  {agent.default_skills.map(sk => (
                    <Badge key={sk} tone="accent">{sk}</Badge>
                  ))}
                </div>
              </div>
            )}
          </div>
        </Card>
      )}

      {tab === 'digest' && (
        <Card className="overflow-hidden">
          <CardHeader
            title={t('agent.detail.digest')}
            subtitle={agent ? `GET /api/v1/agents/${agent.agent_id}/digest` : ''}
            right={
              <Button variant="ghost" size="icon-sm" onClick={() => {
                setDigest(null); setDigestErr(null);
                // Re-trigger fetch by briefly toggling tab
                setTab('overview');
                setTimeout(() => setTab('digest'), 10);
              }}>
                <I.Refresh size={12} />
              </Button>
            }
          />
          <div className="p-4">
            {digestLoading && <SkeletonRows rows={4} cols={2} />}
            {!digestLoading && digestErr && (
              <_DigestEmptyState t={t} />
            )}
            {!digestLoading && !digestErr && digest && (
              <_DigestContent digest={digest} />
            )}
            {!digestLoading && !digestErr && !digest && (
              <_DigestEmptyState t={t} />
            )}
          </div>
        </Card>
      )}

      {tab === 'executions' && (
        <Card className="overflow-hidden">
          <CardHeader title={t('agent.detail.executions')} subtitle="last 5 · filtered by agent" />
          {recentExecs.length > 0 ? (
            <div className="border-t border-border">
              <div className="grid grid-cols-[120px_1fr_110px_110px_80px] gap-3 px-4 py-2 text-[10px] uppercase tracking-wider text-fg-4 mono border-b border-border">
                <div>exec_id</div><div>capability</div><div>status</div><div>started</div><div className="text-right">dur</div>
              </div>
              {recentExecs.map(e => (
                <div key={e.execution_id}
                  className="grid grid-cols-[120px_1fr_110px_110px_80px] gap-3 px-4 row-pad border-b border-border/60 last:border-b-0 text-[12px] items-center hover:bg-[var(--hover)]">
                  <div className="mono text-fg-3 truncate">{e.execution_id}</div>
                  <div className="mono text-fg-2 truncate">{e.capability}</div>
                  <div><StatusBadge status={e.status} /></div>
                  <div className="mono text-fg-3">{relTime(e.started_at)}</div>
                  <div className="mono text-fg text-right tabular-nums">{e.duration}</div>
                </div>
              ))}
            </div>
          ) : (
            <EmptyState icon={I.Activity} title={t('no_recent_exec')} subtitle={t('no_recent_exec_sub')} />
          )}
        </Card>
      )}

      {tab === 'skills' && (
        <Card className="overflow-hidden">
          <CardHeader title={t('agent.detail.skills')} subtitle="bound skill list · join not yet in API" />
          <div className="p-4">
            {boundSkills.length > 0 ? (
              <div className="grid grid-cols-2 gap-2">
                {boundSkills.map(s => (
                  <div key={s.skill_id} className="p-3 bg-bg-3 rounded-md border border-border">
                    <div className="flex items-center justify-between gap-2">
                      <div className="mono text-[12px] text-fg font-medium truncate">{s.name}</div>
                      <Badge tone={s.source === 'builtin' ? 'accent' : s.source === 'hub' ? 'violet' : 'cyan'}>{s.source}</Badge>
                    </div>
                    <div className="text-[11px] text-fg-3 mt-1">{s.category}</div>
                  </div>
                ))}
              </div>
            ) : (
              <EmptyState icon={I.Brain}
                title={t('no_skills_bound') !== 'no_skills_bound' ? t('no_skills_bound') : 'No skill bindings reported'}
                subtitle={agent && agent.skills ? `Agent advertises ${agent.skills} skills — backend not yet returning per-skill list` : 'Bind skills via the New Agent dialog or API.'} />
            )}
          </div>
        </Card>
      )}

      {tab === 'delegate' && agent && (
        <_AgentDelegatePane agentId={agent.agent_id} lang={lang} t={t} />
      )}
    </div>
  );
};

// Breadcrumb sub-component
const _AgentDetailBreadcrumb = ({ t, name, onBack }) => (
  <nav className="flex items-center gap-1.5 text-[12px] text-fg-4 mono">
    <span className="text-accent"><I.Claw size={14} /></span>
    <I.ChevronRight size={11} className="text-fg-4" />
    <button onClick={onBack} className="hover:text-fg transition-colors">
      {t('agent.detail.back')}
    </button>
    <I.ChevronRight size={11} className="text-fg-4" />
    <span className="text-fg truncate max-w-[240px]">{name}</span>
  </nav>
);

// Digest empty state — shown before L9 wires the real endpoint
const _DigestEmptyState = ({ t }) => (
  <div className="flex flex-col items-center gap-3 py-8 text-center">
    <div className="w-10 h-10 rounded-full bg-bg-3 border border-border flex items-center justify-center text-fg-4">
      <I.Clock size={16} />
    </div>
    <div>
      <div className="text-[13px] font-medium text-fg mb-1">{t('agent.detail.no_digest')}</div>
      <div className="text-[11px] text-fg-4 mono max-w-[320px] leading-relaxed">
        GET /api/v1/agents/:id/digest · 404 expected until L9 lane ships
      </div>
    </div>
  </div>
);

// Digest content renderer — used when L9 endpoint returns data
const _DigestContent = ({ digest }) => {
  const entries = Array.isArray(digest.entries) ? digest.entries : [];
  const summary = digest.summary || null;
  return (
    <div className="space-y-4">
      {summary && (
        <div className="bg-bg-3 border border-border rounded-md p-3 text-[12px] text-fg-2 leading-relaxed">
          {summary}
        </div>
      )}
      {entries.length > 0 && (
        <div className="relative pl-5">
          <div className="absolute left-2 top-0 bottom-0 w-px bg-border" />
          {entries.map((entry, i) => (
            <div key={i} className="relative mb-4 last:mb-0">
              <div className="absolute -left-3 top-1 w-2 h-2 rounded-full bg-accent border-2 border-bg" />
              <div className="text-[10px] mono text-fg-4 mb-0.5">{entry.time || entry.ts || '—'}</div>
              <div className="text-[12px] text-fg">{entry.title || entry.text || JSON.stringify(entry)}</div>
              {entry.detail && <div className="text-[11px] text-fg-3 mt-0.5">{entry.detail}</div>}
            </div>
          ))}
        </div>
      )}
      {!summary && entries.length === 0 && (
        <div className="text-[12px] text-fg-4 mono py-4 text-center">digest returned empty payload</div>
      )}
    </div>
  );
};

// F5 — Delegate Subtask pane (tab inside AgentDetailPage)
const _AgentDelegatePane = ({ agentId, lang, t }) => {
  const toast = window.cc.ui && window.cc.ui.useToast ? window.cc.ui.useToast() : { show: () => {} };
  const [form, setForm] = uS_D({ task_description: '', max_iterations: '10' });
  const [result, setResult] = uS_D(null);
  const [loading, setLoading] = uS_D(false);
  const [err, setErr] = uS_D(null);
  const [history, setHistory] = uS_D([]);

  const set = (k, v) => setForm(f => ({ ...f, [k]: v }));

  const submit = async () => {
    if (!form.task_description.trim()) return;
    setLoading(true); setErr(null); setResult(null);
    try {
      const res = await window.cc.api.agents.delegate({
        parent_agent_id: agentId,
        task_description: form.task_description.trim(),
        max_iterations: parseInt(form.max_iterations, 10) || 10,
      });
      setResult(res);
      setHistory(h => [{ ts: new Date().toLocaleTimeString(), task: form.task_description.trim(), res }, ...h].slice(0, 10));
      toast.show && toast.show(t('agent.delegate.success'), 'success');
    } catch (e) {
      const msg = e && e.message ? e.message : String(e);
      setErr(msg);
      toast.show && toast.show(t('agent.delegate.failed') + ': ' + msg, 'error');
    } finally {
      setLoading(false);
    }
  };

  const { Card, CardHeader, Button, Badge, Input, Textarea, ErrorBanner, EmptyState } = window.cc.ui || window;

  return (
    <div className="space-y-4">
      <Card>
        <CardHeader
          title={t('agent.delegate.title')}
          subtitle={'POST /api/v1/agents/delegate · parent: ' + agentId}
        />
        <div className="p-4 space-y-3">
          {err && <ErrorBanner message={err} />}
          <div>
            <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider mono">
              {t('agent.delegate.task_description')}
            </label>
            <Textarea
              rows={4}
              value={form.task_description}
              onChange={e => set('task_description', e.target.value)}
              placeholder={t('agent.delegate.task_placeholder')}
            />
          </div>
          <div className="flex items-end gap-3">
            <div className="w-36">
              <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider mono">
                {t('agent.delegate.max_iterations')}
              </label>
              <Input
                type="number"
                min="1"
                max="50"
                value={form.max_iterations}
                onChange={e => set('max_iterations', e.target.value)}
                className="mono"
              />
            </div>
            <Button
              onClick={submit}
              disabled={loading || !form.task_description.trim()}
            >
              <I.Delegate size={13} />
              {loading ? t('agent.delegate.submitting') : t('agent.delegate.submit')}
            </Button>
          </div>
        </div>
      </Card>

      {result && (
        <Card>
          <CardHeader title={t('agent.delegate.result_title')} />
          <div className="p-4 space-y-3">
            {[
              { label: 'child_agent_id',  value: result.child_agent_id  || result.agent_id  || '—' },
              { label: 'task_id',         value: result.task_id         || result.id         || '—' },
              { label: 'status',          value: result.status          || '—' },
            ].map(row => (
              <div key={row.label} className="flex items-start gap-3">
                <span className="text-[11px] text-fg-4 uppercase tracking-wider mono w-32 shrink-0 mt-0.5">{row.label}</span>
                <span className="mono text-[13px] text-fg">{String(row.value)}</span>
              </div>
            ))}
            {result.output && (
              <div>
                <div className="text-[11px] text-fg-4 uppercase tracking-wider mono mb-1.5">output</div>
                <pre className="p-3 bg-bg-3 rounded text-[12px] mono text-fg-2 overflow-auto max-h-48 whitespace-pre-wrap">
                  {typeof result.output === 'string' ? result.output : JSON.stringify(result.output, null, 2)}
                </pre>
              </div>
            )}
            <details>
              <summary className="text-[11px] text-fg-4 cursor-pointer hover:text-fg-3 mono">raw json</summary>
              <pre className="mt-2 p-3 bg-bg-3 rounded text-[11px] mono text-fg-2 overflow-auto max-h-48">{JSON.stringify(result, null, 2)}</pre>
            </details>
          </div>
        </Card>
      )}

      {history.length > 0 && (
        <Card>
          <CardHeader title="Recent delegations" subtitle="session only · max 10" />
          <div className="divide-y divide-border/50">
            {history.map((h, i) => (
              <div key={i} className="px-4 py-2.5 flex items-start gap-3">
                <span className="text-[10px] mono text-fg-4 w-16 shrink-0 mt-0.5">{h.ts}</span>
                <span className="text-[12px] text-fg-2 truncate flex-1">{h.task}</span>
                <Badge tone={(h.res && h.res.status === 'success') ? 'emerald' : 'slate'}>
                  {h.res && h.res.status ? h.res.status : 'sent'}
                </Badge>
              </div>
            ))}
          </div>
        </Card>
      )}
    </div>
  );
};

Object.assign(window, { AgentDetailPage });
