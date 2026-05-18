// OnboardingDialog — functional 7-step setup wizard.
//
// Replaces the Sprint 8 L4 educational dialog. After login, if GET
// /admin/onboarding/status returns { needs_onboarding: true }, this wizard
// walks the operator through the minimum configuration needed to make the
// system usable: LLM provider, default skill bundle, governance policy,
// first agent, MCP connector, local skill import, and a demo task.
//
// Design rules the wizard follows:
//   - every step commits state to localStorage['cyberclaw.onboarding.state']
//     so a page refresh mid-wizard resumes at the same step;
//   - every step posts to a backend stub endpoint; on 404 we degrade
//     gracefully (state stays in localStorage, operator gets an info toast);
//   - optional steps expose a "Skip" link at the bottom right;
//   - visual language matches the existing admin console (CSS vars,
//     monospace for identifiers, indigo accent, no hard-coded colors).
//
// Exposed as window.OnboardingDialog (app.jsx mounts it). No ES modules.

const { useState: uSOnb, useEffect: uEOnb, useMemo: uMOnb, useRef: uROnb } = React;

// ---- Persisted state helpers ----------------------------------------------

const ONB_STATE_KEY = 'cyberclaw.onboarding.state';

const ONB_DEFAULT_STATE = {
  step: 0,
  llm: { provider: 'anthropic', api_key: '', base_url: '', tested: null },
  bundle: 'developer',
  governance: 'balanced',
  agent: { name: '', role_template: 'coder', skills: [] },
  mcp: { scanned: false, servers: [] },
  local_skills: { scanned: false, skills: [] },
  demo: { task_id: null, status: null, events: [] },
  completed_steps: [],
  offline_steps: [],
};

const loadOnbState = () => {
  try {
    const raw = localStorage.getItem(ONB_STATE_KEY);
    if (!raw) return { ...ONB_DEFAULT_STATE };
    const parsed = JSON.parse(raw);
    return { ...ONB_DEFAULT_STATE, ...parsed, llm: { ...ONB_DEFAULT_STATE.llm, ...(parsed.llm || {}) },
      agent: { ...ONB_DEFAULT_STATE.agent, ...(parsed.agent || {}) },
      mcp: { ...ONB_DEFAULT_STATE.mcp, ...(parsed.mcp || {}) },
      local_skills: { ...ONB_DEFAULT_STATE.local_skills, ...(parsed.local_skills || {}) },
      demo: { ...ONB_DEFAULT_STATE.demo, ...(parsed.demo || {}) },
    };
  } catch {
    return { ...ONB_DEFAULT_STATE };
  }
};

const saveOnbState = (state) => {
  try { localStorage.setItem(ONB_STATE_KEY, JSON.stringify(state)); } catch { /* quota */ }
};

const clearOnbState = () => {
  try { localStorage.removeItem(ONB_STATE_KEY); } catch {}
};

// ---- Thin POST helper with 404 degradation --------------------------------
// Returns { ok: true, data } on success, { ok: false, offline: true } when
// the endpoint does not exist yet (any 4xx), or { ok: false, error } for
// hard failures. Never throws — caller chooses how to react.
const onbPost = async (path, body) => {
  try {
    const data = await window.cc.apiFetch(path, {
      method: 'POST',
      body: JSON.stringify(body || {}),
    });
    return { ok: true, data };
  } catch (e) {
    const msg = String(e && e.message || e);
    // apiFetch throws `${path}: HTTP ${status}` for non-2xx. Treat any
    // 4xx as "backend stub missing" and continue; only 5xx and network
    // failures are reported as errors.
    const m = msg.match(/HTTP (\d{3})/);
    if (m) {
      const status = parseInt(m[1], 10);
      if (status >= 400 && status < 500) return { ok: false, offline: true };
    }
    return { ok: false, error: msg };
  }
};

// ---- Provider presets ------------------------------------------------------

const LLM_PROVIDERS = [
  { id: 'anthropic',  label: 'Anthropic',   base_url: 'https://api.anthropic.com',        key_hint: 'sk-ant-...' },
  { id: 'openai',     label: 'OpenAI',      base_url: 'https://api.openai.com/v1',        key_hint: 'sk-...' },
  { id: 'ark',        label: 'Volcengine (Ark)', base_url: 'https://ark.cn-beijing.volces.com/api/v3', key_hint: 'ARK access key' },
  { id: 'ollama',     label: 'Ollama (local)', base_url: 'http://127.0.0.1:11434',        key_hint: '(not required)' },
  { id: 'custom',     label: 'Custom / OpenAI-compatible', base_url: '',                  key_hint: 'bearer token' },
];

const SKILL_BUNDLES = [
  { id: 'all',       count: 13, skills: ['brainstorm','plan','skill-creator','code-reviewer','debug','explore','daily-digest','verify','critic','analyst','architect','report','git-master'] },
  { id: 'developer', count: 5,  skills: ['plan','debug','code-reviewer','verify','explore'] },
  { id: 'minimal',   count: 2,  skills: ['plan','explore'] },
];

const GOV_PRESETS = [
  { id: 'strict',     rules: { default_action: 'ask', write_approval: 'required', destructive_approval: 'required', risk_floor: 'low' } },
  { id: 'balanced',   rules: { default_action: 'allow', write_approval: 'inline', destructive_approval: 'required', risk_floor: 'medium' } },
  { id: 'permissive', rules: { default_action: 'allow', write_approval: 'audit_only', destructive_approval: 'inline', risk_floor: 'high', note: 'dev_mode' } },
];

const ROLE_TEMPLATES_ONB = [
  { value: 'coder',      label: 'coder' },
  { value: 'researcher', label: 'researcher' },
  { value: 'analyst',    label: 'analyst' },
  { value: 'reviewer',   label: 'reviewer' },
];

// ---- Progress rail (sticky top) -------------------------------------------

const OnbProgressRail = ({ steps, current, completed, onJump }) => (
  <div className="flex items-center gap-0">
    {steps.map((s, idx) => {
      const done = completed.indexOf(idx) !== -1 && idx < current;
      const active = idx === current;
      const reachable = idx <= current || done;
      return (
        <React.Fragment key={s.key}>
          <button
            type="button"
            onClick={() => reachable && onJump(idx)}
            disabled={!reachable}
            className={`flex items-center gap-1.5 shrink-0 group ${reachable ? 'cursor-pointer' : 'cursor-not-allowed'}`}
            title={s.label}
          >
            <span className={`w-6 h-6 rounded-full flex items-center justify-center text-[11px] font-semibold transition-colors border
              ${done   ? 'bg-accent border-accent text-white' : ''}
              ${active ? 'bg-accent/15 border-accent text-accent ring-2 ring-accent/25' : ''}
              ${!done && !active ? 'bg-[var(--bg-3)] border-[var(--border-strong)] text-fg-4' : ''}`}>
              {done ? <I.Check size={11} /> : idx + 1}
            </span>
            <span className={`hidden sm:inline text-[10.5px] uppercase tracking-wider mono
              ${active ? 'text-fg' : done ? 'text-fg-3' : 'text-fg-4'}`}>
              {s.short}
            </span>
          </button>
          {idx < steps.length - 1 && (
            <div className={`flex-1 h-px mx-2 transition-colors ${done ? 'bg-accent/50' : 'bg-[var(--border)]'}`} style={{ minWidth: 10 }} />
          )}
        </React.Fragment>
      );
    })}
  </div>
);

// ---- Radio-style card group -----------------------------------------------

const OnbCardPick = ({ options, value, onChange, renderBody }) => (
  <div className="grid grid-cols-1 md:grid-cols-3 gap-2.5">
    {options.map((o) => {
      const selected = o.id === value || o.value === value;
      return (
        <button
          key={o.id || o.value}
          type="button"
          onClick={() => onChange(o.id || o.value)}
          className={`text-left p-3 rounded-lg border transition-all
            ${selected ? 'border-accent bg-accent/5 ring-1 ring-accent/40' : 'border-[var(--border-strong)] bg-[var(--bg-3)] hover:border-[var(--border-strong)] hover:bg-[var(--hover)]'}`}
        >
          {renderBody(o, selected)}
        </button>
      );
    })}
  </div>
);

// ---- Step 1: Welcome ------------------------------------------------------

const StepWelcome = ({ operator, t }) => (
  <div className="space-y-4">
    <div className="flex items-center gap-3">
      <div className="w-12 h-12 rounded-xl bg-accent-soft border border-accent/30 flex items-center justify-center text-accent">
        <I.Claw size={22} />
      </div>
      <div>
        <div className="text-[15px] font-semibold text-fg">{t('onb.w.hello').replace('{name}', operator?.display_name || operator?.user_id || 'operator')}</div>
        <div className="text-[12px] text-fg-3 mt-0.5">{t('onb.w.subtitle')}</div>
      </div>
    </div>
    <div className="bg-bg-3 border border-border rounded-lg p-4 space-y-2.5 text-[12.5px] text-fg-2 leading-relaxed">
      <p>{t('onb.w.body1')}</p>
      <p>{t('onb.w.body2')}</p>
    </div>
    <div className="grid grid-cols-2 md:grid-cols-4 gap-2 text-[11px] mono">
      {[
        { k: 'onb.w.chip.time', v: '5-10 min' },
        { k: 'onb.w.chip.steps', v: '7 steps' },
        { k: 'onb.w.chip.resume', v: 'auto-resume' },
        { k: 'onb.w.chip.skippable', v: 'skippable' },
      ].map((c) => (
        <div key={c.k} className="px-2.5 py-2 rounded-md border border-[var(--border)] bg-[var(--bg-3)] flex items-center gap-1.5">
          <span className="w-1.5 h-1.5 rounded-full bg-accent" />
          <span className="text-fg-3 uppercase tracking-wider">{c.v}</span>
        </div>
      ))}
    </div>
  </div>
);

// ---- Step 2: LLM Provider -------------------------------------------------

const StepLlm = ({ state, setState, t, toast }) => {
  const [showKey, setShowKey] = uSOnb(false);
  const [testing, setTesting] = uSOnb(false);
  const provider = LLM_PROVIDERS.find((p) => p.id === state.llm.provider) || LLM_PROVIDERS[0];
  const patch = (partial) => setState((s) => ({ ...s, llm: { ...s.llm, ...partial } }));

  // Prefill base_url when provider changes unless the operator edited it.
  uEOnb(() => {
    const p = LLM_PROVIDERS.find((x) => x.id === state.llm.provider);
    if (!p) return;
    if (!state.llm.base_url || LLM_PROVIDERS.some((q) => q.base_url === state.llm.base_url)) {
      patch({ base_url: p.base_url });
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.llm.provider]);

  const doTest = async () => {
    setTesting(true);
    patch({ tested: null });
    const r = await onbPost('/admin/onboarding/test-llm', {
      provider: state.llm.provider,
      api_key: state.llm.api_key,
      base_url: state.llm.base_url,
    });
    setTesting(false);
    if (r.ok) {
      patch({ tested: 'ok' });
      toast.toast({ title: t('onb.llm.test_ok'), tone: 'success' });
    } else if (r.offline) {
      patch({ tested: 'offline' });
      toast.toast({ title: t('onb.llm.test_offline'), desc: t('onb.saved_local'), tone: 'default' });
    } else {
      patch({ tested: 'error' });
      toast.toast({ title: t('onb.llm.test_err'), desc: r.error, tone: 'error' });
    }
  };

  return (
    <div className="space-y-4">
      <div>
        <label className="mono text-[11px] uppercase tracking-wider text-fg-4 mb-1.5 block">{t('onb.llm.provider')}</label>
        <Select
          value={state.llm.provider}
          onChange={(v) => patch({ provider: v })}
          options={LLM_PROVIDERS.map((p) => ({ value: p.id, label: p.label }))}
        />
      </div>
      <div>
        <label className="mono text-[11px] uppercase tracking-wider text-fg-4 mb-1.5 block">{t('onb.llm.base_url')}</label>
        <Input className="mono" value={state.llm.base_url} onChange={(e) => patch({ base_url: e.target.value })} placeholder={provider.base_url} />
      </div>
      <div>
        <label className="mono text-[11px] uppercase tracking-wider text-fg-4 mb-1.5 block">{t('onb.llm.api_key')}</label>
        <div className="relative">
          <Input
            type={showKey ? 'text' : 'password'}
            className="mono pr-9"
            value={state.llm.api_key}
            onChange={(e) => patch({ api_key: e.target.value, tested: null })}
            placeholder={provider.key_hint}
            autoComplete="off"
          />
          <button
            type="button"
            onClick={() => setShowKey((v) => !v)}
            className="absolute right-2 top-1/2 -translate-y-1/2 text-fg-4 hover:text-fg p-1 rounded focus-ring"
            title={showKey ? t('onb.llm.hide') : t('onb.llm.show')}
          >
            {showKey ? <I.EyeOff size={14} /> : <I.Eye size={14} />}
          </button>
        </div>
        <div className="mt-1.5 text-[11px] text-fg-4 leading-relaxed">
          <I.Shield size={11} className="inline-block mr-1 -mt-0.5" />
          {t('onb.llm.key_note')}
        </div>
      </div>

      <div className="flex items-center gap-2 pt-1">
        <Button variant="outline" size="sm" onClick={doTest} disabled={testing || (!state.llm.api_key && state.llm.provider !== 'ollama')}>
          {testing ? <><I.Loader size={12} className="animate-spin" /> {t('onb.llm.testing')}</> : <><I.Zap size={12} /> {t('onb.llm.test')}</>}
        </Button>
        {state.llm.tested === 'ok'      && <Badge tone="emerald"><I.CheckCircle size={10} /> {t('onb.llm.test_ok')}</Badge>}
        {state.llm.tested === 'offline' && <Badge tone="slate"><I.Clock size={10} /> {t('onb.saved_local')}</Badge>}
        {state.llm.tested === 'error'   && <Badge tone="rose"><I.XCircle size={10} /> {t('onb.llm.test_err')}</Badge>}
      </div>
    </div>
  );
};

// ---- Step 3: Skill bundle -------------------------------------------------

const StepBundle = ({ state, setState, t }) => {
  const patch = (bundle) => setState((s) => ({ ...s, bundle }));
  const current = SKILL_BUNDLES.find((b) => b.id === state.bundle) || SKILL_BUNDLES[1];
  return (
    <div className="space-y-4">
      <OnbCardPick
        options={SKILL_BUNDLES}
        value={state.bundle}
        onChange={patch}
        renderBody={(b, selected) => (
          <div>
            <div className="flex items-center justify-between">
              <span className={`text-[13px] font-medium ${selected ? 'text-accent' : 'text-fg'}`}>{t('onb.bundle.' + b.id)}</span>
              <span className="mono text-[10.5px] text-fg-4">{b.count}</span>
            </div>
            <div className="text-[11.5px] text-fg-3 mt-1 leading-relaxed">{t('onb.bundle.' + b.id + '.desc')}</div>
          </div>
        )}
      />
      <div className="bg-bg-3 border border-border rounded-md p-3">
        <div className="mono text-[10.5px] uppercase tracking-wider text-fg-4 mb-2">{t('onb.bundle.included')}</div>
        <div className="flex flex-wrap gap-1.5">
          {current.skills.map((n) => (
            <span key={n} className="mono text-[11px] px-2 py-0.5 rounded-md border border-[var(--border)] bg-[var(--bg-2)] text-fg-2">{n}</span>
          ))}
        </div>
      </div>
    </div>
  );
};

// ---- Step 4: Governance ---------------------------------------------------

const StepGovernance = ({ state, setState, t }) => {
  const current = GOV_PRESETS.find((g) => g.id === state.governance) || GOV_PRESETS[1];
  return (
    <div className="space-y-4">
      <OnbCardPick
        options={GOV_PRESETS}
        value={state.governance}
        onChange={(v) => setState((s) => ({ ...s, governance: v }))}
        renderBody={(g, selected) => (
          <div>
            <div className="flex items-center justify-between">
              <span className={`text-[13px] font-medium ${selected ? 'text-accent' : 'text-fg'}`}>{t('onb.gov.' + g.id)}</span>
              <Badge tone={g.id === 'strict' ? 'rose' : g.id === 'balanced' ? 'amber' : 'slate'}>{g.id}</Badge>
            </div>
            <div className="text-[11.5px] text-fg-3 mt-1 leading-relaxed">{t('onb.gov.' + g.id + '.desc')}</div>
          </div>
        )}
      />
      <div>
        <div className="mono text-[10.5px] uppercase tracking-wider text-fg-4 mb-1.5">{t('onb.gov.preview')}</div>
        <JsonViewer value={current.rules} />
      </div>
    </div>
  );
};

// ---- Step 5: First agent --------------------------------------------------

const StepAgent = ({ state, setState, t }) => {
  const patch = (partial) => setState((s) => ({ ...s, agent: { ...s.agent, ...partial } }));
  const bundle = SKILL_BUNDLES.find((b) => b.id === state.bundle) || SKILL_BUNDLES[1];
  const toggle = (skill) => {
    const exists = state.agent.skills.indexOf(skill) !== -1;
    patch({ skills: exists ? state.agent.skills.filter((x) => x !== skill) : [...state.agent.skills, skill] });
  };
  return (
    <div className="space-y-4">
      <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
        <div>
          <label className="mono text-[11px] uppercase tracking-wider text-fg-4 mb-1.5 block">{t('onb.agent.name')}</label>
          <Input
            className="mono"
            value={state.agent.name}
            onChange={(e) => patch({ name: e.target.value.toLowerCase().replace(/[^a-z0-9-]/g, '-').slice(0, 32) })}
            placeholder="my-first-agent"
          />
          <div className="text-[10.5px] text-fg-4 mt-1 mono">a-z, 0-9, hyphen · max 32</div>
        </div>
        <div>
          <label className="mono text-[11px] uppercase tracking-wider text-fg-4 mb-1.5 block">{t('onb.agent.role')}</label>
          <Select
            value={state.agent.role_template}
            onChange={(v) => patch({ role_template: v })}
            options={ROLE_TEMPLATES_ONB}
          />
        </div>
      </div>
      <div>
        <div className="flex items-center justify-between mb-1.5">
          <label className="mono text-[11px] uppercase tracking-wider text-fg-4">{t('onb.agent.skills')}</label>
          <span className="text-[11px] text-fg-4 mono">{state.agent.skills.length} / {bundle.skills.length}</span>
        </div>
        <div className="flex flex-wrap gap-1.5 p-2 bg-[var(--bg-3)] border border-[var(--border)] rounded-md min-h-[44px]">
          {bundle.skills.map((n) => {
            const active = state.agent.skills.indexOf(n) !== -1;
            return (
              <button
                key={n}
                type="button"
                onClick={() => toggle(n)}
                className={`mono text-[11px] px-2 py-1 rounded-md border transition-colors
                  ${active ? 'bg-accent/15 border-accent text-accent' : 'bg-[var(--bg-2)] border-[var(--border)] text-fg-3 hover:text-fg hover:border-[var(--border-strong)]'}`}
              >
                {active && <I.Check size={10} className="inline -mt-0.5 mr-1" />}{n}
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
};

// ---- Step 6: MCP Connector ------------------------------------------------

const StepMcp = ({ state, setState, t, toast }) => {
  const [scanning, setScanning] = uSOnb(false);
  const patch = (partial) => setState((s) => ({ ...s, mcp: { ...s.mcp, ...partial } }));
  const toggle = (id) => patch({
    servers: state.mcp.servers.map((srv) => srv.id === id ? { ...srv, enabled: !srv.enabled } : srv),
  });
  const doScan = async () => {
    setScanning(true);
    const r = await onbPost('/admin/onboarding/scan-mcp', { path: '~/.claude-code-config.json' });
    setScanning(false);
    if (r.ok && r.data && Array.isArray(r.data.servers)) {
      patch({ scanned: true, servers: r.data.servers.map((s) => ({ ...s, enabled: true })) });
      toast.toast({ title: t('onb.mcp.scan_ok').replace('{n}', r.data.servers.length), tone: 'success' });
    } else if (r.offline) {
      patch({ scanned: true, servers: [], scanError: 'MCP scan failed; check server connection' });
      toast.toast({ title: t('onb.mcp.scan_offline'), desc: t('onb.saved_local'), tone: 'error' });
    } else {
      toast.toast({ title: t('onb.mcp.scan_err'), desc: r.error, tone: 'error' });
    }
  };
  return (
    <div className="space-y-4">
      <div className="flex items-center gap-2">
        <Button variant="outline" size="sm" onClick={doScan} disabled={scanning}>
          {scanning ? <><I.Loader size={12} className="animate-spin" /> {t('onb.mcp.scanning')}</> : <><I.Search size={12} /> {t('onb.mcp.scan')}</>}
        </Button>
        <span className="mono text-[11px] text-fg-4">~/.claude-code-config.json</span>
      </div>
      {state.mcp.scanned && state.mcp.scanError && (
        <div className="flex items-start gap-2 rounded-md border border-red-300 bg-red-50 px-3 py-2.5 text-[12px] text-red-700">
          <span>❌</span>
          <span className="flex-1">{state.mcp.scanError}</span>
          <Button variant="outline" size="sm" onClick={doScan} disabled={scanning}>Retry</Button>
        </div>
      )}
      {state.mcp.scanned && state.mcp.servers.length === 0 && !state.mcp.scanError && (
        <EmptyState icon={I.Plug} title={t('onb.mcp.empty')} subtitle={t('onb.mcp.empty.sub')} />
      )}
      {state.mcp.servers.length > 0 && (
        <div className="border border-[var(--border)] rounded-lg divide-y divide-[var(--border)] bg-[var(--bg-3)]">
          {state.mcp.servers.map((srv) => (
            <div key={srv.id} className="px-3.5 py-2.5 flex items-center gap-3">
              <div className="w-8 h-8 rounded-md bg-[var(--bg-2)] border border-[var(--border)] flex items-center justify-center text-fg-3 shrink-0">
                <I.Plug size={13} />
              </div>
              <div className="flex-1 min-w-0">
                <div className="mono text-[12.5px] font-medium text-fg truncate">{srv.name}</div>
                <div className="text-[11px] text-fg-4 mono">{srv.tools} tools</div>
              </div>
              <Switch checked={!!srv.enabled} onChange={() => toggle(srv.id)} size="sm" />
            </div>
          ))}
        </div>
      )}
      {!state.mcp.scanned && (
        <div className="text-[11.5px] text-fg-3 leading-relaxed bg-[var(--bg-3)] border border-[var(--border)] rounded-md p-3">
          {t('onb.mcp.body')}
        </div>
      )}
    </div>
  );
};

// ---- Step 7: Local skills -------------------------------------------------

const StepLocalSkills = ({ state, setState, t, toast }) => {
  const [scanning, setScanning] = uSOnb(false);
  const patch = (partial) => setState((s) => ({ ...s, local_skills: { ...s.local_skills, ...partial } }));
  const toggle = (id) => patch({
    skills: state.local_skills.skills.map((s) => s.id === id ? { ...s, selected: !s.selected } : s),
  });
  const doScan = async () => {
    setScanning(true);
    const r = await onbPost('/admin/onboarding/scan-skills', { path: '~/.claude/skills/' });
    setScanning(false);
    if (r.ok && r.data && Array.isArray(r.data.skills)) {
      patch({ scanned: true, skills: r.data.skills.map((s) => ({ ...s, selected: true })) });
      toast.toast({ title: t('onb.ls.scan_ok').replace('{n}', r.data.skills.length), tone: 'success' });
    } else if (r.offline) {
      const demo = [
        { id: 'omc-reference',   name: 'omc-reference',   source: 'local', selected: true },
        { id: 'plan',            name: 'plan',            source: 'local', selected: true },
        { id: 'autopilot',       name: 'autopilot',       source: 'local', selected: false },
        { id: 'ralph',           name: 'ralph',           source: 'local', selected: false },
        { id: 'team',            name: 'team',            source: 'local', selected: false },
      ];
      patch({ scanned: true, skills: demo });
      toast.toast({ title: t('onb.ls.scan_offline'), desc: t('onb.saved_local'), tone: 'default' });
    } else {
      toast.toast({ title: t('onb.ls.scan_err'), desc: r.error, tone: 'error' });
    }
  };
  return (
    <div className="space-y-4">
      <div className="flex items-center gap-2">
        <Button variant="outline" size="sm" onClick={doScan} disabled={scanning}>
          {scanning ? <><I.Loader size={12} className="animate-spin" /> {t('onb.ls.scanning')}</> : <><I.Search size={12} /> {t('onb.ls.scan')}</>}
        </Button>
        <span className="mono text-[11px] text-fg-4">~/.claude/skills/</span>
      </div>
      {state.local_skills.skills.length > 0 ? (
        <div className="border border-[var(--border)] rounded-lg divide-y divide-[var(--border)] bg-[var(--bg-3)] max-h-[260px] overflow-auto">
          {state.local_skills.skills.map((s) => (
            <label key={s.id} className="px-3.5 py-2 flex items-center gap-3 cursor-pointer hover:bg-[var(--hover)]">
              <input
                type="checkbox"
                checked={!!s.selected}
                onChange={() => toggle(s.id)}
                className="w-3.5 h-3.5 rounded border-[var(--border-strong)] accent-[var(--accent)]"
              />
              <I.Book size={13} className="text-fg-4" />
              <span className="mono text-[12px] text-fg flex-1 truncate">{s.name}</span>
              <Badge tone="slate">{s.source}</Badge>
            </label>
          ))}
        </div>
      ) : (
        !state.local_skills.scanned && (
          <div className="text-[11.5px] text-fg-3 leading-relaxed bg-[var(--bg-3)] border border-[var(--border)] rounded-md p-3">
            {t('onb.ls.body')}
          </div>
        )
      )}
      {state.local_skills.scanned && state.local_skills.skills.length === 0 && (
        <EmptyState icon={I.Book} title={t('onb.ls.empty')} subtitle={t('onb.ls.empty.sub')} />
      )}
    </div>
  );
};

// ---- Final: Demo task -----------------------------------------------------

const StepDemo = ({ state, setState, t, toast }) => {
  const [running, setRunning] = uSOnb(false);
  const unsubRef = uROnb(null);
  const doneRef = uROnb(false);
  const patch = (partial) => setState((s) => ({ ...s, demo: { ...s.demo, ...partial } }));

  uEOnb(() => () => { if (unsubRef.current) { try { unsubRef.current(); } catch {} } }, []);

  const finishRun = () => {
    if (doneRef.current) return;
    doneRef.current = true;
    patch({ status: 'done' });
    setRunning(false);
  };

  const runDemo = async () => {
    doneRef.current = false;
    setRunning(true);
    patch({ events: [], status: 'queued' });
    try {
      const res = await window.cc.api.tasks.create('chat', 'Hello CyberClaw — first demo task.', {
        title: 'Hello CyberClaw',
        priority: 'low',
        tags: ['onboarding', 'demo'],
      });
      const tid = res && (res.id || res.task_id || res.execution_id) || null;
      patch({ task_id: tid, status: 'running' });
      toast.toast({ title: t('onb.demo.queued'), desc: tid || '(no id)', tone: 'success' });
      // Prefer SSE; if it never delivers, the safety cutoff below marks done.
      try {
        const unsub = window.cc.subscribeEvents((ev) => {
          if (!ev) return;
          setState((s) => ({ ...s, demo: { ...s.demo, events: [...(s.demo.events || []).slice(-7), { ts: Date.now(), ...ev }] } }));
          if (ev.kind === 'task_done' || ev.status === 'done') finishRun();
        });
        unsubRef.current = unsub;
      } catch {}
      // Safety cutoff so the button doesn't stay spinning forever even if
      // SSE is unavailable (e.g., backend stub missing).
      setTimeout(finishRun, 6000);
    } catch (e) {
      doneRef.current = true;
      patch({ status: 'error' });
      setRunning(false);
      toast.toast({ title: t('onb.demo.err'), desc: String(e && e.message || e), tone: 'error' });
    }
  };

  return (
    <div className="space-y-4">
      <div className="bg-bg-3 border border-border rounded-lg p-4">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 rounded-lg bg-accent-soft border border-accent/30 flex items-center justify-center text-accent">
            <I.Sparkles size={16} />
          </div>
          <div className="flex-1 min-w-0">
            <div className="text-[13px] font-medium text-fg">{t('onb.demo.title')}</div>
            <div className="text-[11.5px] text-fg-3 mt-0.5">{t('onb.demo.subtitle')}</div>
          </div>
          <Button size="sm" onClick={runDemo} disabled={running || state.demo.status === 'done'}>
            {running
              ? <><I.Loader size={12} className="animate-spin" /> {t('onb.demo.running')}</>
              : state.demo.status === 'done'
                ? <><I.CheckCircle size={12} /> {t('onb.demo.done')}</>
                : <><I.Play size={12} /> {t('onb.demo.run')}</>}
          </Button>
        </div>
      </div>
      {state.demo.task_id && (
        <div className="border border-[var(--border)] rounded-md bg-[var(--bg-3)] p-3">
          <div className="flex items-center justify-between mb-1.5">
            <span className="mono text-[10.5px] uppercase tracking-wider text-fg-4">{t('onb.demo.trace')}</span>
            <span className="mono text-[11px] text-fg-3">{state.demo.task_id}</span>
          </div>
          <div className="space-y-1 mono text-[11px]">
            {(state.demo.events && state.demo.events.length > 0) ? state.demo.events.map((e, i) => (
              <div key={i} className="flex items-center gap-2 text-fg-3">
                <span className="text-fg-4">{new Date(e.ts).toLocaleTimeString()}</span>
                <span className="text-accent">{e.kind || e.type || 'event'}</span>
                <span className="truncate flex-1">{e.name || e.message || ''}</span>
              </div>
            )) : (
              <div className="text-fg-4 italic">{t('onb.demo.waiting')}</div>
            )}
          </div>
        </div>
      )}
    </div>
  );
};

// ---- Main OnboardingDialog ------------------------------------------------

const OnboardingDialog = ({ open, onClose, onFinish, lang, operator }) => {
  const t = tFor(lang || 'en');
  const toast = useToast();
  const [state, setState] = uSOnb(loadOnbState);

  // Persist on every change while open.
  uEOnb(() => { if (open) saveOnbState(state); }, [state, open]);

  // Reset selectively when reopened — keep resume-from-step behaviour but
  // clear transient fields like the test-result badge.
  uEOnb(() => {
    if (!open) return;
    setState((s) => ({ ...s, llm: { ...s.llm, tested: null } }));
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const steps = uMOnb(() => [
    { key: 'welcome',    short: t('onb.step.welcome'),    label: t('onb.step.welcome'),    optional: false },
    { key: 'llm',        short: t('onb.step.llm'),        label: t('onb.step.llm'),        optional: false },
    { key: 'bundle',     short: t('onb.step.bundle'),     label: t('onb.step.bundle'),     optional: true },
    { key: 'governance', short: t('onb.step.governance'), label: t('onb.step.governance'), optional: true },
    { key: 'agent',      short: t('onb.step.agent'),      label: t('onb.step.agent'),      optional: true },
    { key: 'mcp',        short: t('onb.step.mcp'),        label: t('onb.step.mcp'),        optional: true },
    { key: 'skills',     short: t('onb.step.skills'),     label: t('onb.step.skills'),     optional: true },
    { key: 'demo',       short: t('onb.step.demo'),       label: t('onb.step.demo'),       optional: false },
  ], [lang]);

  const idx = Math.min(Math.max(0, state.step || 0), steps.length - 1);
  const cur = steps[idx];
  const isLast = idx === steps.length - 1;
  const isFirst = idx === 0;
  const isMandatory = !cur.optional;

  const markCompleted = (i, offline = false) => {
    setState((s) => ({
      ...s,
      completed_steps: s.completed_steps.indexOf(i) === -1 ? [...s.completed_steps, i] : s.completed_steps,
      offline_steps: offline && s.offline_steps.indexOf(i) === -1 ? [...s.offline_steps, i] : s.offline_steps,
    }));
  };

  // Persist the current step's payload and advance. Returns true if we
  // should advance (all non-network failures should still advance).
  const commitStep = async () => {
    if (cur.key === 'welcome') { markCompleted(idx); return true; }

    if (cur.key === 'llm') {
      if (!state.llm.api_key && state.llm.provider !== 'ollama') {
        toast.toast({ title: t('onb.llm.need_key'), tone: 'error' });
        return false;
      }
      const r = await onbPost('/admin/onboarding/llm-config', {
        provider: state.llm.provider,
        api_key: state.llm.api_key,
        base_url: state.llm.base_url,
      });
      if (r.offline) toast.toast({ title: t('onb.saved_local'), tone: 'default' });
      else if (!r.ok) toast.toast({ title: t('onb.llm.save_err'), desc: r.error, tone: 'error' });
      markCompleted(idx, r.offline);
      return true;
    }

    if (cur.key === 'bundle') {
      const r = await onbPost('/admin/onboarding/skill-bundle', { bundle: state.bundle });
      if (r.offline) toast.toast({ title: t('onb.saved_local'), tone: 'default' });
      markCompleted(idx, r.offline);
      return true;
    }

    if (cur.key === 'governance') {
      const r = await onbPost('/admin/onboarding/governance', { preset: state.governance });
      if (r.offline) toast.toast({ title: t('onb.saved_local'), tone: 'default' });
      markCompleted(idx, r.offline);
      return true;
    }

    if (cur.key === 'agent') {
      if (!state.agent.name) { markCompleted(idx); return true; } // empty = skip silently
      try {
        await window.cc.api.agents.create({
          name: state.agent.name,
          role_template: state.agent.role_template,
          default_skills: state.agent.skills,
        });
        toast.toast({ title: t('onb.agent.created'), desc: state.agent.name, tone: 'success' });
      } catch (e) {
        toast.toast({ title: t('onb.agent.err'), desc: String(e && e.message || e), tone: 'error' });
      }
      markCompleted(idx);
      return true;
    }

    if (cur.key === 'mcp') {
      if (state.mcp.servers.length > 0) {
        const r = await onbPost('/admin/onboarding/mcp-connect', {
          servers: state.mcp.servers.filter((s) => s.enabled).map((s) => s.id),
        });
        if (r.offline) toast.toast({ title: t('onb.saved_local'), tone: 'default' });
      }
      markCompleted(idx);
      return true;
    }

    if (cur.key === 'skills') {
      const selected = state.local_skills.skills.filter((s) => s.selected);
      if (selected.length > 0) {
        const r = await onbPost('/admin/onboarding/import-skills', {
          skills: selected.map((s) => s.id),
        });
        if (r.offline) toast.toast({ title: t('onb.saved_local'), tone: 'default' });
      }
      markCompleted(idx);
      return true;
    }

    if (cur.key === 'demo') {
      markCompleted(idx);
      return true;
    }
    return true;
  };

  const goNext = async () => {
    const ok = await commitStep();
    if (!ok) return;
    if (isLast) return doFinish();
    setState((s) => ({ ...s, step: idx + 1 }));
  };

  const goPrev = () => setState((s) => ({ ...s, step: Math.max(0, idx - 1) }));
  const goSkip = () => { markCompleted(idx); setState((s) => ({ ...s, step: idx + 1 })); };

  const jumpTo = (i) => {
    if (i < 0 || i > idx) return;
    setState((s) => ({ ...s, step: i }));
  };

  const doFinish = async () => {
    try { await window.cc.api.onboarding.complete(); } catch {}
    clearOnbState();
    if (onFinish) onFinish();
    else if (onClose) onClose();
  };

  const renderStep = () => {
    switch (cur.key) {
      case 'welcome':    return <StepWelcome operator={operator} t={t} />;
      case 'llm':        return <StepLlm state={state} setState={setState} t={t} toast={toast} />;
      case 'bundle':     return <StepBundle state={state} setState={setState} t={t} />;
      case 'governance': return <StepGovernance state={state} setState={setState} t={t} />;
      case 'agent':      return <StepAgent state={state} setState={setState} t={t} />;
      case 'mcp':        return <StepMcp state={state} setState={setState} t={t} toast={toast} />;
      case 'skills':     return <StepLocalSkills state={state} setState={setState} t={t} toast={toast} />;
      case 'demo':       return <StepDemo state={state} setState={setState} t={t} toast={toast} />;
      default:           return null;
    }
  };

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50">
      <div className="absolute inset-0 bg-black/60 anim-fade-in" />
      <div
        className="absolute top-1/2 left-1/2 bg-bg-2 border border-border rounded-lg shadow-2xl anim-dialog-in flex flex-col"
        style={{ width: 720, maxWidth: 'calc(100vw - 32px)', maxHeight: 'calc(100vh - 64px)', transform: 'translate(-50%,-50%)' }}
      >
        {/* Header: title + progress rail */}
        <div className="px-5 pt-4 pb-3 border-b border-border">
          <div className="flex items-start justify-between mb-3">
            <div className="min-w-0">
              <div className="flex items-center gap-2">
                <div className="text-[14px] font-semibold text-fg">{t('onb.title')}</div>
                <span className="mono text-[10.5px] text-fg-4 uppercase tracking-wider">step {idx + 1} / {steps.length}</span>
              </div>
              <div className="text-[12px] text-fg-3 mt-0.5">{cur.label} · <span className="text-fg-4">{isMandatory ? t('onb.required') : t('onb.optional')}</span></div>
            </div>
            <button onClick={onClose} className="text-fg-3 hover:text-fg p-1 -mr-1 -mt-1 rounded focus-ring" title={t('onb.close')}>
              <I.Close size={16} />
            </button>
          </div>
          <OnbProgressRail
            steps={steps}
            current={idx}
            completed={state.completed_steps}
            onJump={jumpTo}
          />
        </div>

        {/* Body */}
        <div className="p-5 overflow-y-auto flex-1">
          {renderStep()}
        </div>

        {/* Footer */}
        <div className="px-5 pb-4 pt-3 border-t border-border flex items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            {!isFirst && (
              <Button variant="outline" size="sm" onClick={goPrev}>
                <I.ArrowLeft size={12} /> {t('onb.prev')}
              </Button>
            )}
          </div>
          <div className="flex items-center gap-3">
            {!isMandatory && !isLast && (
              <button onClick={goSkip} className="text-[12px] text-fg-3 hover:text-fg hover:underline focus-ring rounded">
                {t('onb.skip_step')}
              </button>
            )}
            {isLast
              ? <Button size="sm" onClick={doFinish}><I.Check size={12} /> {t('onb.finish')}</Button>
              : <Button size="sm" onClick={goNext}>{t('onb.next')} <I.ArrowRight size={12} /></Button>}
          </div>
        </div>
      </div>
    </div>
  );
};

Object.assign(window, { OnboardingDialog });
