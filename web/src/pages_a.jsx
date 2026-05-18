// Pages group A: Login, Status, Agents
const { useState: uS_A, useEffect: uE_A, useMemo: uM_A, useRef: uR_A } = React;

// ---- Login ----
const LoginPage = ({ onLogin, lang }) => {
  const t = tFor(lang);
  const [uid, setUid] = useState('op_ada');
  const [pw, setPw] = useState('');
  const [showPw, setShowPw] = useState(false);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState(null);
  const submit = async (e) => {
    e.preventDefault();
    setLoading(true); setErr(null);
    if (!uid.trim()) { setErr(t('unknown_user')); setLoading(false); return; }
    try {
      const res = await window.cc.api.login(uid.trim(), pw);
      if (res && res.jwt) {
        try { sessionStorage.setItem('cyberclaw.admin.jwt', res.jwt); } catch {}
      }
      // Backend LoginResponse shape is `{jwt, user: OperatorRecord}`.
      // Accept either `user` (current backend) or `operator` (older mocks)
      // so the admin/login footer always reflects the real role from
      // users.toml — not the hard-coded 'viewer' fallback.
      const op =
        (res && (res.user || res.operator)) ||
        { user_id: uid.trim(), display_name: uid === 'op_ada' ? 'Ada Chen' : uid };
      onLogin(op);
    } catch (err) {
      // Real failure surfaces to the user — no silent mock fallback.
      // (Silent fallback was the Sprint 7 bug that looked "logged in" but
      // every subsequent API call returned 401 and bounced back to login.)
      console.warn('login failed:', err);
      setErr(err.message || t('unknown_user'));
    } finally {
      setLoading(false);
    }
  };
  return (
    <div className="min-h-screen w-full relative flex items-center justify-center bg-bg overflow-hidden">
      <div className="absolute inset-0 grid-bg opacity-40 pointer-events-none" />
      <div className="absolute top-5 left-6 flex items-center gap-2 text-fg-3 mono text-[11px]">
        <span className="text-accent"><I.Claw size={18} /></span>
        <span className="text-fg font-semibold tracking-tight">CYBERCLAW</span>
        <span className="text-fg-4">admin</span>
      </div>
      <div className="absolute top-5 right-6 flex items-center gap-3 text-[11px] text-fg-4 mono">
        <span className="flex items-center gap-1.5"><span className="w-1.5 h-1.5 rounded-full bg-emerald-500 pulse-dot" /> admin.cyberclaw.local</span>
        <span>·</span>
        <span>7 nodes online</span>
      </div>

      <div className="relative z-10 w-[420px] max-w-[92vw]">
        <div className="flex flex-col items-center mb-3">
          <div className="text-accent mb-2" style={{ filter: 'drop-shadow(0 0 12px var(--accent-ring))' }}>
            <I.ClawMascot size={96} />
          </div>
          <div className="mono text-[10px] text-fg-4 uppercase tracking-[0.3em] ascii-sep">— clawz · admin console —</div>
        </div>
        <Card className="p-6">
          <div className="flex items-center gap-2 mb-5">
            <div className="h-8 w-8 rounded-md bg-accent-soft flex items-center justify-center text-accent">
              <I.Claw size={20} />
            </div>
            <div>
              <div className="text-[15px] font-semibold">{t('login_cta')}</div>
              <div className="text-[11px] text-fg-3 mono">{t('ascii_subtitle')}</div>
            </div>
          </div>
          <form onSubmit={submit} className="space-y-3.5">
            <div>
              <label className="text-[11px] text-fg-3 mb-1.5 block">{t('user_id')}</label>
              <div className="relative">
                <I.User size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-fg-4" />
                <Input value={uid} onChange={(e) => setUid(e.target.value)} className="pl-8 mono" placeholder="op_ada" autoFocus />
              </div>
            </div>
            <div>
              <label className="text-[11px] text-fg-3 mb-1.5 block">{t('password')}</label>
              <div className="relative">
                <I.Key size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-fg-4" />
                <Input type={showPw ? 'text' : 'password'} value={pw} onChange={(e) => setPw(e.target.value)} className="pl-8 pr-8" placeholder="••••••••" />
                <button type="button" onClick={() => setShowPw(!showPw)} className="absolute right-2 top-1/2 -translate-y-1/2 text-fg-4 hover:text-fg">
                  {showPw ? <I.EyeOff size={13} /> : <I.Eye size={13} />}
                </button>
              </div>
            </div>
            {err && (
              <div className="text-[12px] text-rose-400 bg-rose-500/10 border border-rose-500/20 rounded-md px-3 py-2 flex items-center gap-2">
                <I.AlertTriangle size={13} /> {err}
              </div>
            )}
            <Button type="submit" className="w-full" size="lg" disabled={loading}>
              {loading ? <><I.Loader size={13} className="animate-spin" /> {t('authenticating')}</> : <>{t('login')} <I.ArrowRight size={13} /></>}
            </Button>
          </form>
          <div className="mt-4 pt-4 border-t border-border text-[11px] text-fg-3 mono flex items-start gap-2">
            <I.Shield size={12} className="mt-0.5 text-fg-4" />
            <span>{t('login_hint')}</span>
          </div>
        </Card>
        <div className="mt-3 flex justify-between text-[10px] mono text-fg-4">
          <span>dev build</span>
          <span>POST /admin/login</span>
        </div>
      </div>
    </div>
  );
};

// ---- Status / Dashboard ----
const LiveEventStream = ({ scenario, lang }) => {
  const t = tFor(lang || 'en');
  const [paused, setPaused] = useState(false);
  const [events, setEvents] = useState([]);
  // Live SSE — invalidate caches + append to event log when backend emits events.
  // Note: `scenario` is still accepted as a prop for header styling continuity,
  // but the event list is driven purely by the backend SSE stream. When there
  // are no events yet, we show a friendly empty state.
  useEffect(() => {
    if (!window.cc || !window.cc.subscribeEvents) return;
    if (paused) return;
    const unsub = window.cc.subscribeEvents((ev) => {
      if (!ev) return;
      if (ev.kind === 'execution_started' || ev.kind === 'exec.started' || ev.kind === 'exec.done' || ev.kind === 'exec.failed') {
        window.cc.data.invalidate('executions:*');
        window.cc.data.invalidate('dashboard');
      }
      if (ev.kind === 'review_created' || ev.kind === 'review.requested') {
        window.cc.data.invalidate('reviews:pending');
        window.cc.data.invalidate('dashboard');
      }
      setEvents(evts => [...evts.slice(-40), {
        id: Math.random().toString(36).slice(2),
        t: new Date().toISOString().slice(11, 19),
        kind: ev.kind || 'event',
        color: ev.color || 'accent',
        text: ev.text || JSON.stringify(ev),
      }]);
    });
    return unsub;
  }, [paused]);
  const colorMap = { accent: 'text-accent', rose: 'text-rose-400', amber: 'text-amber-400', emerald: 'text-emerald-400', muted: 'text-fg-3' };
  const ref = useRef();
  useEffect(() => {
    if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
  }, [events]);
  return (
    <Card className="overflow-hidden flex flex-col h-full">
      <CardHeader
        title={<div className="flex items-center gap-2"><span className="w-1.5 h-1.5 rounded-full bg-emerald-500 pulse-dot" /> {t('live_events')}</div>}
        subtitle={<span className="mono">GET /admin/events · SSE · {events.length} {t('buffered')}</span>}
        right={
          <div className="flex items-center gap-1">
            <Button variant="ghost" size="icon-sm" onClick={() => setPaused(!paused)} title={paused ? t('resume') : t('pause')}>
              {paused ? <I.Play size={12} /> : <I.Pause size={12} />}
            </Button>
            <Button variant="ghost" size="icon-sm" onClick={() => setEvents([])} title={t('clear')}>
              <I.Trash size={12} />
            </Button>
          </div>
        }
      />
      <div ref={ref} className="flex-1 overflow-auto px-4 pb-3 mono text-[11.5px] space-y-[3px]">
        {events.length === 0 && (
          <div className="text-fg-4 italic py-4 text-center">
            {t('no_events_yet') !== 'no_events_yet' ? t('no_events_yet') : 'no events yet — waiting for backend activity'}
          </div>
        )}
        {events.map(e => (
          <div key={e.id} className="flex gap-2.5 leading-[1.45] anim-fade-in">
            <span className="text-fg-4 shrink-0">{e.t}</span>
            <span className={`shrink-0 w-[120px] truncate ${colorMap[e.color]}`}>{e.kind}</span>
            <span className="text-fg-2 truncate">{e.text}</span>
          </div>
        ))}
      </div>
    </Card>
  );
};

const StatusPage = ({ scenario, lang, onNavigate, onOpenExec }) => {
  const t = tFor(lang);
  const dashRes = window.cc.data.useDashboard();
  const execRes = window.cc.data.useExecutions({});
  const reviewsRes = window.cc.data.useReviews('pending');
  const aboutRes = window.cc.data.useAbout();
  // Prefer real backend shape. Only fall back to MOCK.dashboard when a
  // fetch error populated it (via _ccMockFallback). `dashRes.error` is the
  // trigger — on success we show whatever the backend returned, sparse or not.
  const dash = (dashRes.data && typeof dashRes.data === 'object')
    ? dashRes.data
    : (dashRes.error ? MOCK.dashboard : {});
  const counts = dash.counts || {};
  const execList = window.cc.data.extractList(execRes, 'executions:{}', MOCK.executions);
  const reviewList = window.cc.data.extractList(reviewsRes, 'reviews:pending', MOCK.reviews);
  const about = aboutRes.data || {};
  const uptimeSecs = Number.isFinite(about.uptime_secs) ? about.uptime_secs : 0;
  const version = about.version || '—';
  const healthMap = { healthy: { tone: 'emerald', label: t('all_systems_ok') }, degraded: { tone: 'amber', label: t('degraded_msg') }, incident: { tone: 'rose', label: t('incident_msg') } };
  const h = healthMap[scenario] || healthMap.healthy;

  const spark = {
    health: [95,96,94,97,98,98,97,99,98,98],
    agents: [10,11,11,12,12,11,12,13,12,12],
    reviews: [1,2,2,3,4,3,3,3,3,3],
    tasks: [132,148,165,171,180,183,176,182,187,187],
  };
  if (scenario === 'incident') spark.health = [98,96,92,84,72,68,64,62,58,55];
  if (scenario === 'degraded') spark.health = [97,96,96,93,91,90,88,87,86,85];

  return (
    <div className="space-y-5">
      {/* Health banner */}
      <div className={`border rounded-lg px-4 py-3 flex items-center gap-3 ${scenario === 'incident' ? 'border-rose-600/30 bg-rose-600/5' : scenario === 'degraded' ? 'border-amber-500/30 bg-amber-500/5' : 'border-border bg-bg-2'}`}>
        <span className={`inline-block w-2 h-2 rounded-full pulse-dot ${scenario === 'incident' ? 'bg-rose-500' : scenario === 'degraded' ? 'bg-amber-400' : 'bg-emerald-500'}`} />
        <div className="flex-1">
          <div className="text-[13px] font-medium">{h.label}</div>
          <div className="text-[11px] text-fg-3 mono">
            {t('uptime')} {Math.floor(uptimeSecs / 86400)}d {Math.floor((uptimeSecs % 86400) / 3600)}h · admin/cyberclaw-v{version}
          </div>
        </div>
        <Button variant="outline" size="sm"><I.Refresh size={12} /> {t('refresh')}</Button>
      </div>

      {/* Stats */}
      <div className="grid grid-cols-4 gap-4">
        <StatCard label={t('system_health')} value={scenario === 'incident' ? '55%' : scenario === 'degraded' ? '85%' : (counts.system_health_score != null ? counts.system_health_score + '%' : '—')}
          trend={scenario === 'incident' ? '-43%' : scenario === 'degraded' ? '-13%' : t('stable')}
          trendTone={scenario === 'incident' || scenario === 'degraded' ? 'rose' : 'emerald'}
          icon={I.Activity} sparkline={<Sparkline values={spark.health} tone={scenario === 'incident' ? 'rose' : 'accent'} />} />
        <StatCard label={t('active_agents')} value={counts.active_agents != null ? counts.active_agents : '—'} trend="" icon={I.Bot} sparkline={<Sparkline values={spark.agents} />} />
        <StatCard label={t('pending_reviews')} value={reviewList.length} trend={`${reviewList.length} ${t('need_attention')}`} trendTone="rose" icon={I.Check} sparkline={<Sparkline values={spark.reviews} tone="rose" />} />
        <StatCard label={t('tasks_today')} value={counts.tasks_today != null ? counts.tasks_today : '—'} trend="" icon={I.List} sparkline={<Sparkline values={spark.tasks} />} />
      </div>

      {/* Two-col */}
      <div className="grid grid-cols-[1fr_380px] gap-4">
        <Card className="overflow-hidden">
          <CardHeader
            title={t('recent_executions')}
            subtitle={<span className="mono">GET /admin/dashboard · last 10</span>}
            right={<Button variant="link" size="sm" onClick={() => onNavigate('executions')}>{t('view_all')} <I.ArrowRight size={12} /></Button>}
          />
          <div className="border-t border-border">
            <div className="grid grid-cols-[120px_1fr_1fr_100px_110px_80px] gap-3 px-4 py-2 text-[10px] uppercase tracking-wider text-fg-4 mono border-b border-border">
              <div>id</div><div>agent</div><div>capability</div><div>status</div><div>started</div><div className="text-right">dur</div>
            </div>
            {execList.slice(0, 10).map((e) => (
              <button key={e.execution_id} onClick={() => onOpenExec(e)}
                className="w-full text-left grid grid-cols-[120px_1fr_1fr_100px_110px_80px] gap-3 px-4 row-pad border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)] text-[12px] items-center">
                <div className="mono text-fg-3 truncate">{e.execution_id}</div>
                <div className="text-fg truncate">{e.agent}</div>
                <div className="mono text-fg-3 truncate">{e.capability}</div>
                <div><StatusBadge status={e.status} /></div>
                <div className="mono text-fg-3">{relTime(e.started_at)}</div>
                <div className="mono text-fg text-right tabular-nums">{e.duration}</div>
              </button>
            ))}
          </div>
        </Card>

        <LiveEventStream scenario={scenario} lang={lang} />
      </div>
    </div>
  );
};

function relTime(iso) {
  const diff = (Date.now() - new Date(iso).getTime()) / 1000;
  if (diff < 60) return Math.round(diff) + 's ago';
  if (diff < 3600) return Math.round(diff / 60) + 'm ago';
  if (diff < 86400) return Math.round(diff / 3600) + 'h ago';
  return Math.round(diff / 86400) + 'd ago';
}

// ---- Agents ----
const AgentDetailSheet = ({ agent, open, onClose, onInvoke, lang }) => {
  const t = tFor(lang || 'en');
  const [tab, setTab] = useState('overview');
  const execRes = window.cc.data.useExecutions({});
  const skillsRes = window.cc.data.useSkills();
  if (!agent) return null;
  const tabs = [
    { value: 'overview', label: t('overview') },
    { value: 'tools', label: t('tools'), count: agent.tools },
    { value: 'skills', label: t('skills'), count: agent.skills },
    { value: 'recent', label: t('recent') },
  ];
  const execList = window.cc.data.extractList(execRes, 'executions:{}', MOCK.executions);
  const recent = execList.filter(e => e.agent === agent.name).slice(0, 6);
  const skillsList = window.cc.data.extractList(skillsRes, 'skills', MOCK.skills);
  // Filter by association if the backend attaches a `skills` array of
  // skill names/ids to the agent. Otherwise fall back to showing nothing
  // rather than fabricating an association.
  const agentSkillKeys = Array.isArray(agent.skill_ids)
    ? agent.skill_ids
    : (Array.isArray(agent.skills_list) ? agent.skills_list : null);
  const associatedSkills = agentSkillKeys
    ? skillsList.filter(s => agentSkillKeys.includes(s.skill_id) || agentSkillKeys.includes(s.name))
    : [];
  const agentToolList = Array.isArray(agent.tool_ids)
    ? agent.tool_ids
    : (Array.isArray(agent.tools_list) ? agent.tools_list : []);
  return (
      <Sheet open={open} onClose={onClose} title={agent.name} subtitle={agent.agent_id} width={680}
      right={<Button variant="outline" size="sm" onClick={() => onInvoke && onInvoke(agent)}><I.Zap size={12} /> {t('invoke')}</Button>}>
      <div className="p-5 space-y-4">
        <div className="flex items-center gap-2 flex-wrap">
          <StatusBadge status={agent.status} />
          <Badge tone={agent.trust_level === 'high' ? 'emerald' : agent.trust_level === 'medium' ? 'amber' : 'slate'}>
            trust: {agent.trust_level}
          </Badge>
          <Badge>{t('executions_24h')}: {agent.executions_24h}</Badge>
        </div>
        <p className="text-[13px] text-fg-2">{agent.description}</p>
        <Tabs value={tab} onChange={setTab} items={tabs} />

        {tab === 'overview' && (
          <div className="grid grid-cols-2 gap-3">
            <InfoRow label="agent_id" value={agent.agent_id} mono />
            <InfoRow label="name" value={agent.name} />
            <InfoRow label="trust_level" value={agent.trust_level} />
            <InfoRow label="status" value={<StatusBadge status={agent.status} />} />
            <InfoRow label="tools" value={agent.tools} />
            <InfoRow label="skills" value={agent.skills} />
            <InfoRow label="executions_24h" value={agent.executions_24h} mono />
            <InfoRow label="model" value="claude-sonnet-4-5" mono />
          </div>
        )}
        {tab === 'tools' && (
          <div className="space-y-1.5">
            {agentToolList.length ? agentToolList.map(toolName => (
              <div key={toolName} className="flex items-center justify-between px-3 py-2 bg-bg-3 rounded-md border border-border">
                <span className="mono text-[12px] text-fg">{toolName}</span>
                <Badge tone="slate">allowed</Badge>
              </div>
            )) : (
              <EmptyState icon={I.Plug} title={t('no_tools_bound') !== 'no_tools_bound' ? t('no_tools_bound') : 'no tool bindings reported'}
                subtitle={agent.tools ? `agent advertises ${agent.tools} tools — backend not yet returning per-tool list` : ''} />
            )}
          </div>
        )}
        {tab === 'skills' && (
          <div className="grid grid-cols-2 gap-2">
            {associatedSkills.length ? associatedSkills.map(s => (
              <div key={s.skill_id} className="p-3 bg-bg-3 rounded-md border border-border">
                <div className="mono text-[12px] text-fg">{s.name}</div>
                <div className="text-[11px] text-fg-3 mt-0.5">{s.category}</div>
              </div>
            )) : (
              <div className="col-span-2">
                <EmptyState icon={I.Brain} title={t('no_skills_bound') !== 'no_skills_bound' ? t('no_skills_bound') : 'no skill bindings reported'}
                  subtitle={agent.skills ? `agent advertises ${agent.skills} skills — backend not yet returning per-skill list` : ''} />
              </div>
            )}
          </div>
        )}
        {tab === 'recent' && (
          <div className="border border-border rounded-md overflow-hidden">
            {recent.length ? recent.map(e => (
              <div key={e.execution_id} className="grid grid-cols-[1fr_auto_80px] gap-3 px-3 py-2 border-b border-border/60 last:border-b-0 text-[12px] items-center">
                <div className="mono text-fg-3 truncate">{e.execution_id} <span className="text-fg-4">·</span> <span className="text-fg-2">{e.capability}</span></div>
                <StatusBadge status={e.status} />
                <div className="mono text-fg-3 text-right">{e.duration}</div>
              </div>
            )) : <EmptyState icon={I.Activity} title={t('no_recent_exec')} subtitle={t('no_recent_exec_sub')} />}
          </div>
        )}
      </div>
    </Sheet>
  );
};
const InfoRow = ({ label, value, mono: isMono }) => (
  <div className="flex items-center justify-between px-3 py-2 bg-bg-3 rounded-md border border-border">
    <span className="text-[11px] text-fg-3 uppercase tracking-wider">{label}</span>
    <span className={`text-[12px] text-fg ${isMono ? 'mono' : ''}`}>{value}</span>
  </div>
);

const InvokeDialog = ({ open, agent, onClose, onInvoke, lang }) => {
  const t = tFor(lang || 'en');
  const [input, setInput] = useState('{\n  "prompt": "Summarize the last 7 days of incidents"\n}');
  useEffect(() => { if (open) setInput('{\n  "prompt": "Summarize the last 7 days of incidents"\n}'); }, [open, agent]);
  if (!agent) return null;
  return (
    <Dialog open={open} onClose={onClose} title={`${t('invoke_dialog_title')} · ${agent.name}`} subtitle="POST /agents/invoke" width={560}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>{t('cancel')}</Button>
          <Button onClick={() => onInvoke(agent, input)}><I.Zap size={13} /> {t('invoke')}</Button>
        </>
      }
    >
      <div className="space-y-3">
        <div className="flex items-center gap-2 text-[12px]">
          <span className="text-fg-3">agent_id</span>
          <span className="mono text-fg">{agent.agent_id}</span>
          <span className="text-fg-4">·</span>
          <Badge tone={agent.trust_level === 'high' ? 'emerald' : 'amber'}>trust: {agent.trust_level}</Badge>
        </div>
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('input_json')}</label>
          <Textarea value={input} onChange={(e) => setInput(e.target.value)} rows={9} />
        </div>
        <div className="text-[11px] text-fg-3 bg-bg-3 border border-border rounded-md p-2.5 flex gap-2">
          <I.Shield size={12} className="shrink-0 mt-0.5 text-fg-4" />
          <span>{t('invoke_warn')}</span>
        </div>
      </div>
    </Dialog>
  );
};

// ---- New Agent Dialog ----
const PERSONA_FILES = [
  'analyst', 'architect', 'code-reviewer', 'critic', 'executor',
  'git-master', 'planner', 'verifier', 'master-agent', 'report-agent',
];
const WORKSPACE_MODES = ['isolated', 'shared', 'ephemeral'];
const NAME_RE = /^[a-z][a-z0-9-]{2,31}$/;

const NewAgentDialog = ({ open, onClose, onCreate, lang }) => {
  const t = tFor(lang || 'en');
  const [name, setName] = useState('');
  const [displayName, setDisplayName] = useState('');
  const [persona, setPersona] = useState('');
  const [workspace, setWorkspace] = useState('isolated');
  const [selectedSkills, setSelectedSkills] = useState([]);
  const [errors, setErrors] = useState({});
  const [submitting, setSubmitting] = useState(false);
  const skillsRes = window.cc.data.useSkills();
  const skillsList = window.cc.data.extractList(skillsRes, 'skills', []);

  useEffect(() => {
    if (open) {
      setName(''); setDisplayName(''); setPersona(''); setWorkspace('isolated');
      setSelectedSkills([]); setErrors({}); setSubmitting(false);
    }
  }, [open]);

  const validate = () => {
    const e = {};
    if (!NAME_RE.test(name)) e.name = t('agents.new.name_invalid');
    if (!displayName.trim()) e.displayName = t('agents.new.display_required');
    return e;
  };

  const toggleSkill = (skillId) => {
    setSelectedSkills(prev =>
      prev.includes(skillId) ? prev.filter(s => s !== skillId) : [...prev, skillId]
    );
  };

  const handleSubmit = async () => {
    const e = validate();
    if (Object.keys(e).length) { setErrors(e); return; }
    setSubmitting(true);
    try {
      await onCreate({ name, display_name: displayName, persona_file: persona || null, workspace_mode: workspace, default_skills: selectedSkills });
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={open} onClose={onClose} title={t('agents.new')} subtitle="POST /api/v1/agents" width={560}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>{t('cancel')}</Button>
          <Button onClick={handleSubmit} disabled={submitting}>
            {submitting ? <><I.Loader size={13} className="animate-spin" /> {t('create')}</> : <><I.Plus size={13} /> {t('create')}</>}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        {/* name */}
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('agents.new.name')}</label>
          <Input
            value={name}
            onChange={(e) => { setName(e.target.value); setErrors(prev => ({ ...prev, name: undefined })); }}
            className="mono"
            placeholder="e.g. my-analyst"
            autoFocus
          />
          {errors.name
            ? <div className="text-[11px] text-rose-400 mt-1">{errors.name}</div>
            : <div className="text-[11px] text-fg-4 mt-1 mono">{t('agents.new.name_hint')}</div>
          }
        </div>

        {/* display_name */}
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('agents.new.display_name')}</label>
          <Input
            value={displayName}
            onChange={(e) => { setDisplayName(e.target.value); setErrors(prev => ({ ...prev, displayName: undefined })); }}
            placeholder="My Analyst"
          />
          {errors.displayName && <div className="text-[11px] text-rose-400 mt-1">{errors.displayName}</div>}
        </div>

        {/* persona + workspace side by side */}
        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('agents.new.persona')}</label>
            <Select
              value={persona}
              onChange={setPersona}
              placeholder="— none —"
              options={PERSONA_FILES.map(p => ({ value: p, label: p + '.md' }))}
            />
          </div>
          <div>
            <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('agents.new.workspace')}</label>
            <Select
              value={workspace}
              onChange={setWorkspace}
              options={WORKSPACE_MODES.map(m => ({ value: m, label: m }))}
            />
          </div>
        </div>

        {/* default_skills multi-select */}
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('agents.new.skills')}</label>
          {skillsList.length === 0 ? (
            <div className="text-[11px] text-fg-4 mono px-2.5 py-2 border border-border rounded-md bg-bg-3">
              {/* TODO(L3): persist via admin_store — backend GET /api/v1/skills not reachable */}
              no skills available
            </div>
          ) : (
            <div className="max-h-[140px] overflow-auto border border-[var(--border-strong)] rounded-md bg-[var(--bg-2)] divide-y divide-[var(--border)]">
              {skillsList.map(s => (
                <label key={s.skill_id} className="flex items-center gap-2.5 px-3 py-1.5 hover:bg-[var(--hover)] cursor-pointer">
                  <input
                    type="checkbox"
                    checked={selectedSkills.includes(s.skill_id)}
                    onChange={() => toggleSkill(s.skill_id)}
                    className="accent-[var(--accent)] w-3.5 h-3.5 shrink-0"
                  />
                  <span className="mono text-[12px] text-fg">{s.name}</span>
                  <span className="text-[11px] text-fg-4 ml-auto">{s.category}</span>
                </label>
              ))}
            </div>
          )}
          {selectedSkills.length > 0 && (
            <div className="mt-1.5 flex flex-wrap gap-1">
              {selectedSkills.map(id => {
                const sk = skillsList.find(s => s.skill_id === id);
                return sk ? (
                  <Badge key={id} tone="accent">
                    {sk.name}
                    <button onClick={() => toggleSkill(id)} className="ml-1 opacity-60 hover:opacity-100">×</button>
                  </Badge>
                ) : null;
              })}
            </div>
          )}
        </div>

        <div className="text-[11px] text-fg-3 bg-bg-3 border border-border rounded-md p-2.5 flex gap-2">
          <I.Shield size={12} className="shrink-0 mt-0.5 text-fg-4" />
          <span>Agent will be registered under the default connector with medium trust. Adjust trust_level via the agent detail sheet.</span>
        </div>
      </div>
    </Dialog>
  );
};

// ---- Agent list table (shared between My Agents and All tabs) ----
const AgentListTable = ({ agentsRaw, loading, q, trust, status, onViewAgent, setSelected, setInvoking, t }) => {
  const list = agentsRaw.filter(a =>
    (!q || (a.name || '').includes(q) || (a.agent_id || '').includes(q) || (a.description || '').toLowerCase().includes(q.toLowerCase())) &&
    (!trust || a.trust_level === trust) && (!status || a.status === status)
  );
  return (
    <Card className="overflow-hidden">
      <div className="grid grid-cols-[140px_180px_1fr_120px_110px_120px] gap-3 px-4 py-2 text-[10px] uppercase tracking-wider text-fg-4 mono border-b border-border">
        <div>agent_id</div><div>name</div><div>description</div><div>trust</div><div>status</div><div className="text-right">actions</div>
      </div>
      {loading && !agentsRaw.length ? <SkeletonRows rows={6} cols={6} /> : list.map(a => (
        <div key={a.agent_id} className="grid grid-cols-[140px_180px_1fr_120px_110px_120px] gap-3 px-4 row-pad border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)] text-[12px] items-center cursor-pointer"
          onClick={() => onViewAgent ? onViewAgent(a.agent_id) : setSelected(a)}
        >
          <div className="mono text-fg-3 truncate">{a.agent_id}</div>
          <div className="text-fg font-medium truncate">{a.name}</div>
          <div className="text-fg-2 truncate">{a.description}</div>
          <div><Badge tone={a.trust_level === 'high' ? 'emerald' : a.trust_level === 'medium' ? 'amber' : 'slate'}>{a.trust_level}</Badge></div>
          <div><StatusBadge status={a.status} /></div>
          <div className="text-right" onClick={(e) => e.stopPropagation()}>
            <Button variant="outline" size="xs" onClick={() => setInvoking(a)}><I.Zap size={11} /> {t('invoke')}</Button>
          </div>
        </div>
      ))}
      {!list.length && <EmptyState icon={I.Bot} title={t('no_agents_match')} subtitle={t('try_clear_filters')} />}
    </Card>
  );
};

const AgentsPage = ({ lang, onNavigate, onViewAgent }) => {
  const t = tFor(lang);
  // Tab state: my | builtin | all
  const [activeTab, setActiveTab] = useState('my');
  const [q, setQ] = useState('');
  const [trust, setTrust] = useState('');
  const [status, setStatus] = useState('');
  const [selected, setSelected] = useState(null);
  const [invoking, setInvoking] = useState(null);
  const [wizardOpen, setWizardOpen] = useState(false);
  const toast = useToast();
  const agentsRes = window.cc.data.useAgents();
  const { loading, error } = agentsRes;
  const agentsRaw = window.cc.data.extractList(agentsRes, 'agents', MOCK.agents);

  // Partition: user agents vs built-in (by source field; fall back to id prefix)
  const userAgents = agentsRaw.filter(a => a.source !== 'builtin' && !(a.agent_id || '').startsWith('internal.'));
  const allAgents  = agentsRaw;

  const tabItems = [
    { value: 'my',      label: t('agents.tab.my'),      count: userAgents.length },
    { value: 'builtin', label: t('agents.tab.builtin')  },
    { value: 'all',     label: t('agents.tab.all'),     count: allAgents.length },
  ];

  const handleCreate = async (payload) => {
    setWizardOpen(false);
    try {
      const res = await window.cc.api.agents.create(payload);
      window.cc.data.invalidate('agents');
      const id = (res && (res.agent_id || res.id)) || 'ag_' + Math.random().toString(36).slice(2, 8).toUpperCase();
      toast.toast({ title: t('agents.created'), desc: `${payload.name} (${id}) · ${t('agents.created.desc')}`, tone: 'success' });
    } catch (e) {
      // TODO(L3): persist via admin_store — backend POST /api/v1/agents may not yet exist
      const mockId = 'ag_' + Math.random().toString(36).slice(2, 8).toUpperCase();
      try {
        const stored = JSON.parse(sessionStorage.getItem('cyberclaw.admin.pending_agents') || '[]');
        stored.push({ ...payload, agent_id: mockId, status: 'idle', trust_level: 'medium', created_at: new Date().toISOString() });
        sessionStorage.setItem('cyberclaw.admin.pending_agents', JSON.stringify(stored));
      } catch {}
      toast.toast({ title: t('agents.created'), desc: `${payload.name} (${mockId}) · ${t('agents.wizard.saved_draft')}`, tone: 'success' });
    }
  };

  return (
    <div className="space-y-4">
      <PageToolbar
        left={
          <>
            <Tabs value={activeTab} onChange={setActiveTab} items={tabItems} />
            {activeTab !== 'builtin' && (
              <>
                <div className="relative">
                  <I.Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-fg-4" />
                  <Input value={q} onChange={(e) => setQ(e.target.value)} className="pl-7 w-56" placeholder={t('search_agents')} />
                </div>
                <Select value={trust} onChange={setTrust} className="w-32" placeholder={t('all_trust_levels')}
                  options={[{ value: '', label: t('all_trust_levels') }, { value: 'high', label: t('trust_high') }, { value: 'medium', label: t('trust_medium') }, { value: 'low', label: t('trust_low') }]} />
                <Select value={status} onChange={setStatus} className="w-28" placeholder={t('all_status')}
                  options={[{ value: '', label: t('all_status') }, { value: 'active', label: 'active' }, { value: 'idle', label: 'idle' }, { value: 'paused', label: 'paused' }]} />
              </>
            )}
          </>
        }
        right={
          <div className="flex items-center gap-2">
            <Button size="sm" onClick={() => setWizardOpen(true)}><I.Plus size={12} /> {t('agents.wizard.create_btn')}</Button>
            <Button variant="outline" size="sm" onClick={() => window.cc.data.invalidate('agents')}><I.Refresh size={12} /> {t('refresh')}</Button>
          </div>
        }
      />

      {error && <ErrorBanner message={error} onRetry={() => window.cc.data.invalidate('agents')} />}

      {/* Tab: My Agents */}
      {activeTab === 'my' && (
        <AgentListTable agentsRaw={userAgents} loading={loading} q={q} trust={trust} status={status}
          onViewAgent={onViewAgent} setSelected={setSelected} setInvoking={setInvoking} t={t} />
      )}

      {/* Tab: Built-in */}
      {activeTab === 'builtin' && (
        <div className="space-y-3">
          <div className="text-[12px] text-fg-3 px-1">{t('agents.tab.builtin_desc')}</div>
          <div className="grid grid-cols-1 gap-3">
            {BUILTIN_AGENTS.map(agent => (
              <BuiltinAgentCard key={agent.id} agent={agent} lang={lang}
                onView={(a) => toast.toast({ title: a.name, desc: a.mission, tone: 'default' })} />
            ))}
          </div>
        </div>
      )}

      {/* Tab: All */}
      {activeTab === 'all' && (
        <AgentListTable agentsRaw={allAgents} loading={loading} q={q} trust={trust} status={status}
          onViewAgent={onViewAgent} setSelected={setSelected} setInvoking={setInvoking} t={t} />
      )}

      <AgentDetailSheet agent={selected} open={!!selected} lang={lang} onClose={() => setSelected(null)} onInvoke={(a) => { setSelected(null); setInvoking(a); }} />

      {/* 5-step Create Agent Wizard (Sheet) */}
      <CreateAgentWizard open={wizardOpen} lang={lang} onClose={() => setWizardOpen(false)} onCreate={handleCreate} />

      <InvokeDialog open={!!invoking} agent={invoking} lang={lang} onClose={() => setInvoking(null)}
        onInvoke={async (a, input) => {
          setInvoking(null);
          let parsed = input;
          try { parsed = JSON.parse(input); } catch {}
          try {
            const res = await window.cc.api.agents.invoke(a.agent_id, parsed);
            const execId = (res && (res.execution_id || res.id)) || 'ex_' + Math.random().toString(36).slice(2,8).toUpperCase();
            window.cc.data.invalidate('executions:*');
            window.cc.data.invalidate('dashboard');
            toast.toast({ title: t('invocation_queued'), desc: `execution_id: ${execId} (agent ${a.name})`, tone: 'success', action: { label: t('view'), onClick: () => onNavigate('executions') } });
          } catch (e) {
            toast.toast({ title: t('invocation_queued'), desc: `(offline) ${a.name}`, tone: 'error' });
          }
        }}
      />
    </div>
  );
};

const PageToolbar = ({ left, right }) => (
  <div className="flex items-center justify-between gap-3 flex-wrap">
    <div className="flex items-center gap-2 flex-wrap">{left}</div>
    <div className="flex items-center gap-2">{right}</div>
  </div>
);

Object.assign(window, { LoginPage, StatusPage, AgentsPage, PageToolbar, InfoRow, LiveEventStream, relTime });
