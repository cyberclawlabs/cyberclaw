// pages_learning.jsx — /learning page
// C3: Learning cluster — Daily Digest / Org Memory / Evolution Timeline
// Sprint 11 Admin UI Wave 1 Lane A3

// ---- TranscriptPane (shared 120-LOC component, also used by pages_workbench.jsx) ----
// Defined here first so workbench can reference window.TranscriptPane after this file loads.
const TranscriptPane = ({ messages, onSend, placeholder, targetSelector, modePills, loading }) => {
  const { useState, useRef, useEffect } = React;
  const [text, setText] = useState('');
  const bottomRef = useRef(null);

  useEffect(() => {
    if (bottomRef.current) bottomRef.current.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  const send = () => {
    const v = text.trim();
    if (!v) return;
    onSend && onSend(v);
    setText('');
  };

  const onKey = (e) => {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); send(); }
  };

  const roleColor = { user: 'text-accent', assistant: 'text-emerald-400', system: 'text-fg-4' };
  const roleBg   = { user: 'bg-accent-soft border-[var(--accent-ring)]', assistant: 'bg-[var(--bg-3)] border-[var(--border)]', system: 'bg-[var(--bg-3)] border-[var(--border)]' };

  return (
    <div className="flex flex-col h-full min-h-0">
      {/* Transcript scroll area */}
      <div className="flex-1 overflow-auto px-4 py-3 space-y-3">
        {(!messages || messages.length === 0) && (
          <div className="flex items-center justify-center h-32 text-[12px] text-fg-4 mono">
            No messages yet. Start a query below.
          </div>
        )}
        {(messages || []).map((msg, i) => (
          <div key={i} className={`rounded-lg border px-3 py-2.5 ${roleBg[msg.role] || roleBg.system}`}>
            <div className="flex items-center gap-2 mb-1">
              <span className={`text-[10px] mono font-semibold uppercase tracking-wider ${roleColor[msg.role] || 'text-fg-4'}`}>
                {msg.role === 'user' ? (msg.actor || 'operator') : (msg.actor || msg.role)}
              </span>
              {msg.ts && (
                <span className="text-[10px] mono text-fg-4">{msg.ts}</span>
              )}
              {msg.mode && (
                <Badge tone="slate">{msg.mode}</Badge>
              )}
              {msg.provenance && (
                <Badge tone="violet">provenance</Badge>
              )}
            </div>
            <div className="text-[13px] text-fg leading-relaxed whitespace-pre-wrap">{msg.content}</div>
            {msg.traceRef && (
              <div className="mt-1.5 flex items-center gap-1.5">
                <I.Activity size={11} className="text-fg-4" />
                <span className="text-[11px] mono text-fg-4">{msg.traceRef}</span>
              </div>
            )}
            {msg.actions && msg.actions.length > 0 && (
              <div className="mt-2 flex gap-2 flex-wrap">
                {msg.actions.map((a, ai) => (
                  <button key={ai} onClick={() => a.onClick && a.onClick()}
                    className="text-[11px] mono px-2 py-0.5 rounded border border-[var(--border)] text-fg-3 hover:text-fg hover:bg-[var(--hover)]">
                    {a.label}
                  </button>
                ))}
              </div>
            )}
          </div>
        ))}
        <div ref={bottomRef} />
      </div>

      {/* Composer */}
      <div className="border-t border-border p-3 space-y-2">
        {/* Mode pills + target selector row */}
        {(modePills || targetSelector) && (
          <div className="flex items-center gap-2 flex-wrap">
            {modePills && (
              <div className="flex gap-1">
                {modePills.map(p => (
                  <button key={p.key} onClick={() => p.onClick && p.onClick()}
                    className={`px-2 h-6 text-[11px] mono rounded border transition-colors ${p.active ? 'bg-accent text-white border-transparent' : 'border-[var(--border)] text-fg-3 hover:text-fg hover:bg-[var(--hover)]'}`}>
                    {p.label}
                  </button>
                ))}
              </div>
            )}
            {targetSelector && (
              <div className="flex items-center gap-1.5 text-[11px] mono text-fg-4">
                <I.Bot size={11} />
                {targetSelector}
              </div>
            )}
          </div>
        )}
        <div className="flex gap-2">
          <textarea
            value={text}
            onChange={e => setText(e.target.value)}
            onKeyDown={onKey}
            placeholder={placeholder || 'Type a message… (⌘↵ to send)'}
            rows={2}
            className="flex-1 bg-bg-3 border border-border rounded-md px-3 py-2 text-[13px] text-fg placeholder:text-fg-4 resize-none outline-none focus:border-[var(--accent)] transition-colors"
          />
          <button
            onClick={send}
            disabled={!text.trim() || loading}
            className="h-full px-3 rounded-md bg-accent text-white text-[12px] font-medium disabled:opacity-40 hover:opacity-90 transition-opacity flex items-center gap-1.5"
          >
            {loading ? <I.Loader size={14} className="animate-spin" /> : <I.Send size={14} />}
          </button>
        </div>
        <div className="text-[10px] mono text-fg-4">⌘↵ send · read-only by default · promote to /reviews to execute</div>
      </div>
    </div>
  );
};

// ---- Daily Digest Tab ----
const DailyDigestTab = ({ lang }) => {
  const { useState, useEffect } = React;
  const t = tFor(lang);
  const [date, setDate] = useState(() => new Date().toISOString().slice(0, 10));
  const [data, setData] = useState(null);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState(null);

  const load = async (d) => {
    setLoading(true); setErr(null);
    try {
      const jwt = sessionStorage.getItem('cc_jwt');
      const dateParam = d || date;
      const res = await fetch(`/api/v1/learning/daily-digest?date=${dateParam}`, {
        headers: jwt ? { Authorization: 'Bearer ' + jwt } : {},
      });
      if (!res.ok) throw new Error('HTTP ' + res.status);
      setData(await res.json());
    } catch (e) {
      setErr('Failed to load digest: ' + String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { load(date); }, [date]);

  const tagTone = { learned: 'violet', incident: 'rose', mutation: 'accent', blocked: 'amber', 'auto-approved': 'emerald' };

  const adjustDate = (delta) => {
    const d = new Date(date);
    d.setDate(d.getDate() + delta);
    setDate(d.toISOString().slice(0, 10));
  };

  const today = new Date().toISOString().slice(0, 10);

  return (
    <div className="space-y-5">
      {/* Header + date picker */}
      <div className="flex items-center gap-3 flex-wrap">
        <div>
          <div className="text-[14px] font-semibold">{t('learning.daily_digest')}</div>
          <div className="text-[11px] text-fg-4 mono mt-0.5">LLM-authored by Audit Agent · manually annotatable</div>
        </div>
        <div className="ml-auto flex items-center gap-1.5">
          <button onClick={() => adjustDate(-1)} className="h-7 w-7 flex items-center justify-center rounded border border-border text-fg-3 hover:text-fg hover:bg-[var(--hover)]">
            <I.ChevronLeft size={13} />
          </button>
          <input type="date" value={date} onChange={e => setDate(e.target.value)} max={today}
            className="h-7 px-2 bg-bg-3 border border-border rounded text-[12px] mono text-fg outline-none focus:border-[var(--accent)]" />
          <button onClick={() => adjustDate(1)} disabled={date >= today}
            className="h-7 w-7 flex items-center justify-center rounded border border-border text-fg-3 hover:text-fg hover:bg-[var(--hover)] disabled:opacity-30">
            <I.ChevronRight size={13} />
          </button>
          <button onClick={() => setDate(today)} className="h-7 px-2.5 text-[11px] mono rounded border border-border text-fg-3 hover:text-fg hover:bg-[var(--hover)]">today</button>
        </div>
      </div>

      {loading && <SkeletonRows n={4} />}
      {err && <ErrorBanner message={err} />}

      {data && !loading && (
        <>
          {/* KPI row */}
          <div className="grid grid-cols-2 lg:grid-cols-4 gap-3">
            <StatCard label={t('learning.stat_executions')} value={data.stats.executions.value} sub={data.stats.executions.delta} />
            <StatCard label={t('learning.stat_approvals')} value={data.stats.approvals.value} sub={data.stats.approvals.delta} />
            <StatCard label={t('learning.stat_learning')} value={data.stats.learning_entries.value} sub={data.stats.learning_entries.delta} />
            <StatCard label={t('learning.stat_incidents')} value={data.stats.incidents.value} sub={data.stats.incidents.delta} tone="rose" />
          </div>

          {/* Main layout: feed + right rail */}
          <div className="flex gap-4 items-start">
            {/* Feed */}
            <div className="flex-1 min-w-0 space-y-5">
              {data.feed.map((bucket, bi) => (
                <div key={bi}>
                  <div className="text-[11px] mono text-fg-4 uppercase tracking-wider mb-2 flex items-center gap-2">
                    <span>{bucket.bucket}</span>
                    <span className="flex-1 h-px bg-border" />
                  </div>
                  <div className="space-y-2.5">
                    {bucket.entries.map((entry, ei) => (
                      <Card key={ei}>
                        <div className="px-3 py-2.5">
                          <div className="flex items-center gap-2 mb-1.5">
                            <span className="text-[16px] leading-none">{entry.avatar}</span>
                            <span className="text-[13px] font-medium">{entry.name}</span>
                            <span className="text-[11px] mono text-fg-4">{entry.ts}</span>
                            <div className="flex gap-1 ml-auto flex-wrap">
                              {entry.tags.map(tag => (
                                <Badge key={tag} tone={tagTone[tag] || 'slate'}>{tag}</Badge>
                              ))}
                            </div>
                          </div>
                          <div className="text-[13px] text-fg-2 leading-relaxed">{entry.body}</div>
                        </div>
                      </Card>
                    ))}
                  </div>
                </div>
              ))}
            </div>

            {/* Right rail */}
            <div className="w-64 shrink-0 space-y-3">
              <Card>
                <div className="px-3 py-2 border-b border-border">
                  <div className="text-[11px] mono text-fg-4 uppercase tracking-wider">{t('learning.highlights')}</div>
                </div>
                <div className="px-3 py-2 space-y-2">
                  {data.highlights.map((h, i) => (
                    <div key={i} className="flex items-start gap-2 text-[12px]">
                      <span className="text-[14px] leading-none mt-0.5">{h.icon}</span>
                      <span className="text-fg-2 leading-snug">{h.label}</span>
                    </div>
                  ))}
                </div>
              </Card>
              <Card>
                <div className="px-3 py-2 border-b border-border">
                  <div className="text-[11px] mono text-fg-4 uppercase tracking-wider">{t('learning.recommendations')}</div>
                </div>
                <div className="px-3 py-2 space-y-2.5">
                  {data.recommendations.map((r, i) => (
                    <div key={i} className="space-y-1">
                      <div className="text-[12px] text-fg-2 leading-snug">{r.text}</div>
                      <button className="text-[11px] mono text-accent hover:underline">{r.action} →</button>
                    </div>
                  ))}
                </div>
              </Card>
            </div>
          </div>
        </>
      )}
    </div>
  );
};

// ---- Org Memory Browser Tab ----
const OrgMemoryTab = ({ lang }) => {
  const { useState, useEffect } = React;
  const t = tFor(lang);
  const [scope, setScope] = useState('Org');
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState(null);
  const [items, setItems] = useState(null);
  const [loading, setLoading] = useState(false);

  const KIND_AVATAR = { daily_digest: '📋', reflection: '◆', rule: '⚖', free_text: '◦' };
  const KIND_NAME = { daily_digest: 'Daily Digest', reflection: 'Reflection', rule: 'Rule Engine', free_text: 'Operator' };

  const load = async () => {
    setLoading(true);
    try {
      const jwt = sessionStorage.getItem('cc_jwt');
      const res = await fetch('/api/v1/learning/org-memory?limit=100', {
        headers: jwt ? { Authorization: 'Bearer ' + jwt } : {},
      });
      if (!res.ok) throw new Error('HTTP ' + res.status);
      const json = await res.json();
      setItems((json.entries || []).map(e => ({
        id: e.id,
        contributor: { avatar: KIND_AVATAR[e.kind] || '◦', name: KIND_NAME[e.kind] || e.kind },
        snippet: e.content,
        confidence: 1.0,
        ts: e.created_at ? e.created_at.slice(0, 16).replace('T', ' ') : '—',
        scope: typeof e.scope === 'string' ? e.scope : 'Org',
        signature: e.provenance?.source ? `src:${e.provenance.source}` : '—',
      })));
    } catch (_e) {
      setItems([]);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { load(); }, [scope]);

  const filtered = (items || []).filter(m =>
    !query || m.snippet.toLowerCase().includes(query.toLowerCase()) || m.contributor.name.toLowerCase().includes(query.toLowerCase())
  );

  const confColor = (c) => c >= 0.95 ? 'emerald' : c >= 0.85 ? 'amber' : 'slate';

  return (
    <div className="space-y-4">
      <div>
        <div className="text-[14px] font-semibold">{t('learning.org_memory')}</div>
        <div className="text-[11px] text-fg-4 mono mt-0.5">SemanticMemory · Ed25519 signed · hierarchical ACL</div>
      </div>

      <div className="flex gap-3 items-center flex-wrap">
        {['Team', 'Department', 'Org'].map(s => (
          <button key={s} onClick={() => setScope(s)}
            className={`h-7 px-3 text-[12px] mono rounded-md border transition-colors ${scope === s ? 'bg-bg-3 text-fg border-[var(--border-strong)]' : 'border-border text-fg-3 hover:text-fg hover:bg-[var(--hover)]'}`}>
            {s}
          </button>
        ))}
        <div className="flex-1 relative min-w-[200px]">
          <I.Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-fg-4" />
          <input value={query} onChange={e => setQuery(e.target.value)} placeholder={t('search_ellipsis')}
            className="w-full h-7 pl-8 pr-3 bg-bg-3 border border-border rounded-md text-[12px] text-fg placeholder:text-fg-4 outline-none focus:border-[var(--accent)]" />
        </div>
        <button onClick={load} className="h-7 px-2.5 text-[11px] mono rounded border border-border text-fg-3 hover:text-fg hover:bg-[var(--hover)] flex items-center gap-1">
          <I.Plus size={12} /> {t('learning.add_fact')}
        </button>
      </div>

      {loading && <SkeletonRows n={5} />}

      {!loading && (
        <div className="flex gap-3 items-start">
          {/* Memory list */}
          <div className="flex-1 min-w-0 space-y-1.5">
            {filtered.length === 0 && <EmptyState title={t('empty_title')} sub={t('empty_sub')} />}
            {filtered.map(item => (
              <button key={item.id} onClick={() => setSelected(item)}
                className={`w-full text-left rounded-lg border px-3 py-2.5 transition-colors ${selected && selected.id === item.id ? 'bg-bg-3 border-[var(--border-strong)]' : 'bg-bg-2 border-border hover:bg-[var(--hover)]'}`}>
                <div className="flex items-center gap-2 mb-1">
                  <span className="text-[14px]">{item.contributor.avatar}</span>
                  <span className="text-[12px] text-fg-3">{item.contributor.name}</span>
                  <Badge tone={confColor(item.confidence)}>{(item.confidence * 100).toFixed(0)}%</Badge>
                  <span className="ml-auto text-[10px] mono text-fg-4">{item.ts}</span>
                </div>
                <div className="text-[12px] text-fg-2 leading-snug line-clamp-2">{item.snippet}</div>
              </button>
            ))}
          </div>

          {/* Detail pane */}
          {selected && (
            <div className="w-80 shrink-0">
              <Card>
                <CardHeader
                  title={<div className="flex items-center gap-2"><span>{selected.contributor.avatar}</span><span>Memory Detail</span></div>}
                  right={<button onClick={() => setSelected(null)} className="text-fg-4 hover:text-fg"><I.Close size={14} /></button>}
                />
                <div className="px-4 py-3 space-y-3">
                  <div className="text-[13px] text-fg leading-relaxed">{selected.snippet}</div>
                  <div className="space-y-1.5 text-[12px]">
                    <div className="flex items-center gap-2">
                      <span className="text-fg-4 w-20 shrink-0">contributor</span>
                      <span className="text-fg-2">{selected.contributor.name}</span>
                    </div>
                    <div className="flex items-center gap-2">
                      <span className="text-fg-4 w-20 shrink-0">confidence</span>
                      <Badge tone={confColor(selected.confidence)}>{(selected.confidence * 100).toFixed(0)}%</Badge>
                    </div>
                    <div className="flex items-center gap-2">
                      <span className="text-fg-4 w-20 shrink-0">scope</span>
                      <Badge tone="slate">{selected.scope}</Badge>
                    </div>
                    <div className="flex items-center gap-2">
                      <span className="text-fg-4 w-20 shrink-0">timestamp</span>
                      <span className="mono text-[11px] text-fg-3">{selected.ts}</span>
                    </div>
                  </div>
                  <div>
                    <div className="text-[10px] mono text-fg-4 uppercase tracking-wider mb-1.5">signature</div>
                    <div className="bg-bg-3 rounded px-2 py-1.5 text-[10px] mono text-fg-3 break-all">{selected.signature}</div>
                  </div>
                  <JsonViewer data={{ id: selected.id, snippet: selected.snippet, contributor: selected.contributor, confidence: selected.confidence, scope: selected.scope, signed: true }} />
                </div>
              </Card>
            </div>
          )}
        </div>
      )}
    </div>
  );
};

// ---- Evolution Timeline projection helpers ----
//
// Backend `EvolutionCycle` shape:
//   { id, started_at, ended_at?, outcome, gene_changed?, meta }
// where outcome ∈ {"converged", "max_iterations", "failed"} and meta carries
// {pass_rate?, iterations?, model?, skill_id?, reason?}.
//
// UI shape needs principal / mutation / fitness_delta / status / diff /
// evaluator. The projection below is intentionally a thin one-way mapping —
// when the contract grows, prefer adding a new column to meta over
// renegotiating outcome.
const cycleStatusForUi = (outcome) =>
  outcome === 'converged' ? 'accepted'
  : outcome === 'failed' ? 'rolled-back'
  : 'pending-observe';

const fitnessDeltaForUi = (cycle) => {
  const pr = cycle?.meta?.pass_rate;
  if (typeof pr !== 'number') return '—';
  const sign = pr >= 0.5 ? '+' : '';
  return `${sign}${pr.toFixed(2)}`;
};

const projectCycleEvent = (cycle) => {
  const meta = cycle?.meta || {};
  const skill = meta.skill_id || '—';
  const model = meta.model || '—';
  const iterations = meta.iterations;
  const reason = meta.reason;
  const outcome = cycle?.outcome || 'unknown';

  const mutation =
    outcome === 'converged' ? `${skill} converged at pass_rate ${meta.pass_rate?.toFixed?.(2) ?? '?'}`
    : outcome === 'max_iterations' ? `${skill} hit max iterations`
    : outcome === 'failed' ? `${skill} failed: ${reason || 'no reason recorded'}`
    : `${skill} (${outcome})`;

  const evaluator =
    `LlmEvolutionDispatcher · model=${model}` +
    (typeof iterations === 'number' ? ` · iterations=${iterations}` : '') +
    (typeof meta.pass_rate === 'number' ? ` · pass_rate=${meta.pass_rate.toFixed(2)}` : '');

  const diff =
    outcome === 'failed' ? (reason || '(no reason recorded)')
    : cycle?.gene_changed ? `winning variant: ${cycle.gene_changed}`
    : '(no diff captured)';

  return {
    id: cycle.id,
    ts: (cycle.started_at || '').replace('T', ' ').replace(/\.\d+Z?$/, ''),
    principal: `${skill} (skill)`,
    mutation,
    fitness_delta: fitnessDeltaForUi(cycle),
    status: cycleStatusForUi(outcome),
    diff,
    evaluator,
  };
};

const projectCyclesForUi = (cycles) => {
  const events = cycles.map(projectCycleEvent);
  const tried = events.length;
  const accepted = events.filter((e) => e.status === 'accepted').length;
  const rolled_back = events.filter((e) => e.status === 'rolled-back').length;
  const passRates = cycles
    .map((c) => c?.meta?.pass_rate)
    .filter((v) => typeof v === 'number');
  const avg_delta = passRates.length
    ? `${(passRates.reduce((a, b) => a + b, 0) / passRates.length).toFixed(2)}`
    : '—';
  const skillCounts = {};
  for (const c of cycles) {
    const sk = c?.meta?.skill_id;
    if (sk) skillCounts[sk] = (skillCounts[sk] || 0) + 1;
  }
  const top_evolving = Object.entries(skillCounts)
    .sort((a, b) => b[1] - a[1])
    .slice(0, 3)
    .map(([k]) => k);
  return {
    stats: { tried, accepted, rolled_back, avg_delta, top_evolving },
    events,
  };
};

// ---- Evolution Timeline Tab ----
const EvolutionTimelineTab = ({ lang }) => {
  const { useState, useEffect } = React;
  const t = tFor(lang);
  const [data, setData] = useState(null);
  const [loading, setLoading] = useState(false);
  const [expanded, setExpanded] = useState(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    const jwt = sessionStorage.getItem('cyberclaw.admin.jwt');
    fetch('/api/v1/learning/evolution/timeline?days=30', {
      headers: { ...(jwt ? { Authorization: 'Bearer ' + jwt } : {}) },
    })
      .then((res) => {
        if (!res.ok) throw new Error('HTTP ' + res.status);
        return res.json();
      })
      .then((payload) => {
        if (cancelled) return;
        const cycles = Array.isArray(payload?.cycles) ? payload.cycles : [];
        setData(projectCyclesForUi(cycles));
        setLoading(false);
      })
      .catch(() => {
        if (cancelled) return;
        // Empty state — log unreadable / endpoint unavailable.
        setData(projectCyclesForUi([]));
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const statusTone = { accepted: 'emerald', 'rolled-back': 'rose', 'pending-observe': 'amber' };
  const deltaColor = (d) => d.startsWith('+') ? 'text-emerald-400' : d.startsWith('-') ? 'text-rose-400' : 'text-fg-3';

  return (
    <div className="space-y-5">
      <div>
        <div className="text-[14px] font-semibold">{t('learning.evolution_timeline')}</div>
        <div className="text-[11px] text-fg-4 mono mt-0.5">evolution_daemon · fitness_evaluator · SkillArchive</div>
      </div>

      {loading && <SkeletonRows n={5} />}

      {data && !loading && (
        <>
          {/* Stats */}
          <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
            <StatCard label={t('learning.evo_tried')} value={data.stats.tried} />
            <StatCard label={t('learning.evo_accepted')} value={data.stats.accepted} tone="emerald" />
            <StatCard label={t('learning.evo_rolled_back')} value={data.stats.rolled_back} tone="rose" />
            <StatCard label={t('learning.evo_avg_delta')} value={data.stats.avg_delta} tone="accent" />
          </div>

          <div className="flex items-center gap-2 text-[11px] mono text-fg-4">
            <span>{t('learning.top_evolving')}:</span>
            {data.stats.top_evolving.map(p => <Badge key={p} tone="violet">{p}</Badge>)}
          </div>

          {/* Timeline */}
          <div className="relative space-y-2">
            {/* Vertical line */}
            <div className="absolute left-[19px] top-0 bottom-0 w-px bg-border" />
            {data.events.map((ev, i) => (
              <div key={ev.id} className="relative pl-10">
                {/* Dot */}
                <div className={`absolute left-[14px] top-3 w-2.5 h-2.5 rounded-full border-2 ${ev.status === 'accepted' ? 'bg-emerald-500 border-emerald-500' : ev.status === 'rolled-back' ? 'bg-rose-500 border-rose-500' : 'bg-amber-400 border-amber-400'}`} />
                <Card>
                  <button className="w-full text-left px-3 py-2.5" onClick={() => setExpanded(expanded === ev.id ? null : ev.id)}>
                    <div className="flex items-center gap-2 flex-wrap">
                      <span className="text-[11px] mono text-fg-4">{ev.ts}</span>
                      <span className="text-[13px] font-medium text-fg flex-1">{ev.principal}</span>
                      <Badge tone={statusTone[ev.status]}>{ev.status}</Badge>
                      <span className={`text-[12px] mono font-semibold ${deltaColor(ev.fitness_delta)}`}>{ev.fitness_delta}</span>
                      <I.ChevronDown size={13} className={`text-fg-4 transition-transform ${expanded === ev.id ? 'rotate-180' : ''}`} />
                    </div>
                    <div className="text-[12px] text-fg-3 mt-1 leading-snug">{ev.mutation}</div>
                  </button>
                  {expanded === ev.id && (
                    <div className="border-t border-border px-3 py-2.5 space-y-2.5">
                      <div>
                        <div className="text-[10px] mono text-fg-4 uppercase tracking-wider mb-1">mutation diff</div>
                        <pre className="text-[11px] mono text-fg-2 bg-bg-3 rounded px-2 py-1.5 overflow-auto whitespace-pre-wrap">{ev.diff}</pre>
                      </div>
                      <div>
                        <div className="text-[10px] mono text-fg-4 uppercase tracking-wider mb-1">fitness evaluator output</div>
                        <div className="text-[11px] mono text-fg-3 bg-bg-3 rounded px-2 py-1.5">{ev.evaluator}</div>
                      </div>
                    </div>
                  )}
                </Card>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
};

// ---- Learning Page Root ----
const LearningPage = ({ lang }) => {
  const { useState } = React;
  const t = tFor(lang);
  const [tab, setTab] = useState('digest');

  const tabs = [
    { value: 'digest', label: t('learning.digest_tab') },
    { value: 'memory', label: t('learning.memory_tab') },
    { value: 'evolution', label: t('learning.evolution_tab') },
  ];

  return (
    <div className="space-y-4">
      <Tabs items={tabs} value={tab} onChange={setTab} />
      {tab === 'digest' && <DailyDigestTab lang={lang} />}
      {tab === 'memory' && <OrgMemoryTab lang={lang} />}
      {tab === 'evolution' && <EvolutionTimelineTab lang={lang} />}
    </div>
  );
};

// Export for app.jsx
Object.assign(window, { LearningPage, TranscriptPane });
