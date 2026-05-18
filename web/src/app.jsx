// App — orchestrator

const { useState: uS, useEffect: uE } = React;

const ACCENT_MAP = {
  indigo: { h: 243, s: 75, l: 59 },
  violet: { h: 270, s: 70, l: 62 },
  teal:   { h: 174, s: 65, l: 48 },
  amber:  { h: 32,  s: 85, l: 55 },
};

const DEFAULT_TWEAKS = /*EDITMODE-BEGIN*/{
  "accent": "indigo",
  "theme": "dark",
  "density": "comfortable",
  "collapsed": false,
  "lang": "en",
  "scenario": "healthy",
  "mode": "operate",
  "right_rail_open": false
}/*EDITMODE-END*/;
// Role is authoritative from backend OperatorRecord (GET /admin/me).
// NEVER put role in tweaks — that was a Sprint 8 vuln where anyone could
// self-promote to admin via the client-side tweaks panel.

function applyTheme(theme) {
  document.body.classList.remove('dark', 'light');
  document.body.classList.add(theme);
}
function applyDensity(d) {
  document.body.classList.remove('density-compact', 'density-comfortable');
  document.body.classList.add('density-' + d);
}
function applyAccent(key) {
  const a = ACCENT_MAP[key] || ACCENT_MAP.indigo;
  document.documentElement.style.setProperty('--accent-h', a.h);
  document.documentElement.style.setProperty('--accent-s', a.s + '%');
  document.documentElement.style.setProperty('--accent-l', a.l + '%');
}

function App() {
  const [loggedIn, setLoggedIn] = uS(() => {
    try { return !!sessionStorage.getItem('cyberclaw.admin.jwt'); } catch { return false; }
  });
  // Global auth-expired handler — api.jsx dispatches this on 401.
  uE(() => {
    const onExpired = () => {
      try { sessionStorage.removeItem('cyberclaw.admin.jwt'); sessionStorage.removeItem('cyberclaw.admin.op'); } catch {}
      setLoggedIn(false);
    };
    window.addEventListener('cyberclaw:auth-expired', onExpired);
    return () => window.removeEventListener('cyberclaw:auth-expired', onExpired);
  }, []);
  const [operator, setOperator] = uS(() => {
    try {
      const stored = JSON.parse(sessionStorage.getItem('cyberclaw.admin.op') || 'null');
      return stored || { user_id: '', display_name: '', role: 'viewer', avatar_initials: '' };
    } catch {
      return { user_id: '', display_name: '', role: 'viewer', avatar_initials: '' };
    }
  });
  // Rehydrate from /admin/me whenever we have a JWT. This is the
  // authoritative source of truth for `role` (admin/operator/viewer)
  // and `display_name`. Running on every login transition — not just
  // when cached operator is blank — ensures stale/incorrect client
  // state (e.g. a prior session that defaulted role to 'viewer')
  // gets overwritten with the real record from users.toml.
  uE(() => {
    if (!loggedIn) return;
    let cancelled = false;
    (async () => {
      try {
        const meRes = await window.cc.api.me();
        if (cancelled || !meRes) return;
        // Backend shape: { user: { user_id, display_name, role, created_at, last_login } }
        // Tolerate either {user:{...}} or a flat object.
        const me = (meRes && meRes.user) ? meRes.user : meRes;
        const initials = (me.display_name || me.user_id || 'OP')
          .split(' ').map(s => s[0]).join('').slice(0, 2).toUpperCase();
        const next = { ...me, avatar_initials: initials };
        try { sessionStorage.setItem('cyberclaw.admin.op', JSON.stringify(next)); } catch {}
        setOperator(next);
      } catch (e) {
        // 401 handled globally via cyberclaw:auth-expired event
      }
    })();
    return () => { cancelled = true; };
  }, [loggedIn]);
  const [current, setCurrent] = uS(() => {
    try { return sessionStorage.getItem('cyberclaw.admin.page') || 'status'; } catch { return 'status'; }
  });
  const [tweaks, setTweaks] = uS(() => {
    try { const saved = JSON.parse(localStorage.getItem('cyberclaw.admin.tweaks') || 'null'); return { ...DEFAULT_TWEAKS, ...(saved || {}) }; } catch { return { ...DEFAULT_TWEAKS }; }
  });
  const [tweaksOpen, setTweaksOpen] = uS(false);
  const [execToOpen, setExecToOpen] = uS(null);
  const [agentToView, setAgentToView] = uS(null);
  // Sprint 8 L4 — show OnboardingDialog when the operator has no onboarded_at
  // timestamp. Status check runs after /admin/me returns operator identity.
  const [onboardingOpen, setOnboardingOpen] = uS(false);
  uE(() => {
    if (!loggedIn || !operator || !operator.user_id) return;
    let cancelled = false;
    (async () => {
      try {
        const s = await window.cc.api.onboarding.status();
        if (!cancelled && s && s.needs_onboarding) setOnboardingOpen(true);
      } catch (_) { /* offline / unsupported — stay silent */ }
    })();
    return () => { cancelled = true; };
  }, [loggedIn, operator && operator.user_id]);

  // Apply tweaks
  uE(() => { applyTheme(tweaks.theme); }, [tweaks.theme]);
  uE(() => { applyDensity(tweaks.density); }, [tweaks.density]);
  uE(() => { applyAccent(tweaks.accent); }, [tweaks.accent]);
  uE(() => { try { localStorage.setItem('cyberclaw.admin.tweaks', JSON.stringify(tweaks)); } catch {} }, [tweaks]);
  uE(() => { try { sessionStorage.setItem('cyberclaw.admin.page', current); } catch {} }, [current]);

  // Tweaks host protocol
  uE(() => {
    const onMsg = (e) => {
      if (!e.data || typeof e.data !== 'object') return;
      if (e.data.type === '__activate_edit_mode') setTweaksOpen(true);
      if (e.data.type === '__deactivate_edit_mode') setTweaksOpen(false);
    };
    window.addEventListener('message', onMsg);
    try { window.parent.postMessage({ type: '__edit_mode_available' }, '*'); } catch {}
    return () => window.removeEventListener('message', onMsg);
  }, []);
  uE(() => {
    try { window.parent.postMessage({ type: '__edit_mode_set_keys', edits: tweaks }, '*'); } catch {}
  }, [tweaks]);

  const doLogin = (op) => {
    const fullOp = {
      ...op,
      avatar_initials: (op.display_name || op.user_id || 'OP')
        .split(' ').map(s => s[0]).join('').slice(0, 2).toUpperCase(),
      // role comes from backend OperatorRecord — never client-overridden
      role: op.role || 'viewer',
    };
    // NOTE: LoginPage already wrote the real JWT into sessionStorage before
    // calling onLogin. Do NOT overwrite it here — that was the Sprint 7 bug
    // that caused "login looks successful but all API calls get 401".
    try {
      sessionStorage.setItem('cyberclaw.admin.op', JSON.stringify(fullOp));
    } catch {}
    setOperator(fullOp);
    setLoggedIn(true);
  };
  const doLogout = () => {
    try { sessionStorage.removeItem('cyberclaw.admin.jwt'); sessionStorage.removeItem('cyberclaw.admin.op'); } catch {}
    setLoggedIn(false);
  };

  if (!loggedIn) {
    return (
      <ToastProvider>
        <LoginPage onLogin={doLogin} lang={tweaks.lang} />
        <TweaksPanel open={tweaksOpen} onClose={() => setTweaksOpen(false)} state={tweaks} setState={setTweaks} />
      </ToastProvider>
    );
  }

  const pages = {
    status:       <StatusPage scenario={tweaks.scenario} lang={tweaks.lang} onNavigate={setCurrent} onOpenExec={(e) => { setExecToOpen(e); setCurrent('executions'); }} />,
    agents:       <AgentsPage lang={tweaks.lang} onNavigate={setCurrent} onViewAgent={(id) => { setAgentToView(id); setCurrent('agent-detail'); }} />,
    'agent-detail': <AgentDetailPage agentId={agentToView} lang={tweaks.lang} onBack={() => { setAgentToView(null); setCurrent('agents'); }} />,
    skills:       <SkillsPage lang={tweaks.lang} />,
    tasks:        <TasksPage lang={tweaks.lang} role={operator.role || 'viewer'} />,
    executions:   <ExecutionsPage lang={tweaks.lang} role={operator.role || 'viewer'} preOpen={execToOpen} onClearPreOpen={() => setExecToOpen(null)} />,
    reviews:      <ReviewsPage lang={tweaks.lang} role={operator.role || 'viewer'} onOpenTrace={(e) => { setExecToOpen(e); setCurrent('executions'); }} />,
    clarifications: <ClarificationsPage lang={tweaks.lang} />,
    handoffs:       <HandoffsPage lang={tweaks.lang} />,
    audit:        <AuditPage lang={tweaks.lang} />,
    learning:     <LearningPage lang={tweaks.lang} />,
    workbench:    <WorkbenchPage lang={tweaks.lang} />,
    chat:         <ChatPage lang={tweaks.lang} />,
    capabilities: <CapabilitiesPage lang={tweaks.lang} />,
    channels:     <ChannelsPage lang={tweaks.lang} role={operator.role || 'viewer'} />,
    nodes:          <NodesPage lang={tweaks.lang} />,
    memory_console: <MemoryConsolePage lang={tweaks.lang} />,
    settings:       <SettingsPage lang={tweaks.lang} role={operator.role || 'viewer'} />,
    admin_ops:      <AdminOpsPage lang={tweaks.lang} />,
    kanban:         <KanbanPage lang={tweaks.lang} />,
    browser_console:<BrowserConsolePage lang={tweaks.lang} />,
    multimodal:     <MultimodalPage lang={tweaks.lang} />,
    im_platforms:   <ImPlatformsPage lang={tweaks.lang} />,
    curator:        <CuratorPage lang={tweaks.lang} />,
    moa:            <MoaPage lang={tweaks.lang} />,
    capability_monitor: <CapabilityMonitorPage lang={tweaks.lang} />,
    tools_mgmt:         <ToolsMgmtPage lang={tweaks.lang} />,
    cluster:            <ClusterDashboardPage lang={tweaks.lang} />,
  };

  return (
    <ToastProvider>
      <Layout
        current={current}
        setCurrent={setCurrent}
        lang={tweaks.lang}
        setLang={(v) => setTweaks(s => ({ ...s, lang: v }))}
        theme={tweaks.theme}
        setTheme={(v) => setTweaks(s => ({ ...s, theme: v }))}
        collapsed={tweaks.collapsed}
        setCollapsed={(v) => setTweaks(s => ({ ...s, collapsed: v }))}
        mode={tweaks.mode || 'operate'}
        setMode={(v) => setTweaks(s => ({ ...s, mode: v }))}
        rightRailOpen={!!tweaks.right_rail_open}
        setRightRailOpen={(v) => setTweaks(s => ({ ...s, right_rail_open: v }))}
        scenario={tweaks.scenario}
        role={operator.role || 'viewer'}
        operator={operator}
        onLogout={doLogout}
        openTweaks={tweaksOpen}
        onOpenTweaks={() => setTweaksOpen(v => !v)}
        tweakState={tweaks}
        setTweakState={setTweaks}
        onOpenExec={(e) => { setExecToOpen(e); setCurrent('executions'); }}
      >
        {pages[current]}
      </Layout>
      <OnboardingDialog
        open={onboardingOpen}
        lang={tweaks.lang}
        operator={operator}
        onClose={() => setOnboardingOpen(false)}
        onFinish={() => setOnboardingOpen(false)}
      />
    </ToastProvider>
  );
}

ReactDOM.createRoot(document.getElementById('root')).render(<App />);
