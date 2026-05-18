// Shell: Sidebar, Topbar, Layout, Tweaks panel, Command Palette

// ---- Command Palette ----
const CMDK_PAGES = [
  { key: 'status',       label: 'Status',       icon: I.Home },
  { key: 'chat',         label: 'Chat',         icon: I.MessageSquare || I.Book },
  { key: 'agents',       label: 'Agents',       icon: I.Bot },
  { key: 'skills',       label: 'Skills',       icon: I.Brain },
  { key: 'tasks',        label: 'Tasks',        icon: I.List },
  { key: 'executions',   label: 'Executions',   icon: I.Activity },
  { key: 'reviews',      label: 'Reviews',      icon: I.Check },
  { key: 'clarifications', label: 'Clarifications', icon: I.MessageSquare || I.Book },
  { key: 'handoffs',     label: 'Handoffs',     icon: I.Activity },
  { key: 'audit',        label: 'Audit',        icon: I.Shield },
  { key: 'learning',     label: 'Learning',     icon: I.Book },
  { key: 'workbench',    label: 'Workbench',    icon: I.Terminal },
  { key: 'capabilities', label: 'Capabilities', icon: I.Plug },
  { key: 'channels',     label: 'Channels',     icon: I.Radio },
  { key: 'nodes',        label: 'Nodes',        icon: I.Server },
  { key: 'memory_console', label: 'Memory Console', icon: I.Brain },
  { key: 'admin_ops',    label: 'Admin Ops',    icon: I.Settings },
  { key: 'kanban',       label: 'Kanban',        icon: I.List },
  { key: 'browser_console', label: 'Browser Console', icon: I.Globe },
  { key: 'multimodal',   label: 'Multimodal',    icon: I.Sparkles },
  { key: 'im_platforms', label: 'IM Platforms',  icon: I.Radio },
  { key: 'curator',      label: 'Curator',       icon: I.Brain },
  { key: 'moa',          label: 'Mixture-of-Agents', icon: I.Layers },
  { key: 'capability_monitor', label: 'Capability Monitor', icon: I.AlertTriangle || I.Activity },
  { key: 'tools_mgmt',    label: 'Tools',          icon: I.ToolState },
  { key: 'cluster',       label: 'Cluster',         icon: I.Cluster },
  { key: 'settings',     label: 'Settings',     icon: I.Settings },
];

const CommandPalette = ({ open, onClose, lang, onNavigate, onOpenExec }) => {
  const t = tFor(lang || 'en');
  const [query, setQuery] = useState('');
  const [activeIdx, setActiveIdx] = useState(0);
  const inputRef = useRef(null);
  const listRef = useRef(null);

  const agentsRes = window.cc.data.useAgents();
  const skillsRes = window.cc.data.useSkills();
  const tasksRes = window.cc.data.useTasks('all');
  const execsRes = window.cc.data.useExecutions({});

  const agents = window.cc.data.extractList(agentsRes, 'agents', []);
  const skills = window.cc.data.extractList(skillsRes, 'skills', []);
  const tasks = window.cc.data.extractList(tasksRes, 'tasks:all', []);
  const execs = window.cc.data.extractList(execsRes, 'executions:{}', []);

  useEffect(() => {
    if (open) {
      setQuery('');
      setActiveIdx(0);
      setTimeout(() => inputRef.current && inputRef.current.focus(), 30);
    }
  }, [open]);

  const q = query.trim().toLowerCase();

  const results = useMemo(() => {
    const items = [];
    // Pages always shown when empty query, filtered when not
    CMDK_PAGES.forEach(p => {
      if (!q || p.label.toLowerCase().includes(q) || p.key.includes(q)) {
        items.push({ kind: 'page', id: 'page:' + p.key, icon: p.icon, label: p.label, sub: '/' + p.key, pageKey: p.key });
      }
    });
    if (q) {
      agents.slice(0, 50).forEach(a => {
        if ((a.name || '').toLowerCase().includes(q) || (a.agent_id || '').toLowerCase().includes(q) || (a.description || '').toLowerCase().includes(q)) {
          items.push({ kind: 'agent', id: 'agent:' + a.agent_id, icon: I.Bot, label: a.name || a.agent_id, sub: a.agent_id, data: a });
        }
      });
      skills.slice(0, 50).forEach(s => {
        if ((s.name || '').toLowerCase().includes(q) || (s.skill_id || '').toLowerCase().includes(q) || (s.description || '').toLowerCase().includes(q)) {
          items.push({ kind: 'skill', id: 'skill:' + s.skill_id, icon: I.Brain, label: s.name || s.skill_id, sub: s.skill_id, data: s });
        }
      });
      tasks.slice(0, 50).forEach(t => {
        const desc = t.description || t.title || '';
        if (desc.toLowerCase().includes(q) || (t.task_id || '').toLowerCase().includes(q)) {
          items.push({ kind: 'task', id: 'task:' + t.task_id, icon: I.List, label: desc.slice(0, 60) || t.task_id, sub: t.task_id, data: t });
        }
      });
      execs.slice(0, 50).forEach(e => {
        if ((e.execution_id || '').toLowerCase().includes(q) || (e.agent || '').toLowerCase().includes(q) || (e.capability || '').toLowerCase().includes(q)) {
          items.push({ kind: 'execution', id: 'exec:' + e.execution_id, icon: I.Activity, label: e.execution_id, sub: (e.agent || '') + ' · ' + (e.capability || ''), data: e });
        }
      });
    }
    return items.slice(0, 30);
  }, [q, agents, skills, tasks, execs]);

  useEffect(() => { setActiveIdx(0); }, [results.length, q]);

  const kindLabel = (kind) => ({
    page: t('cmdk.pages'),
    agent: t('cmdk.agents'),
    skill: t('cmdk.skills'),
    task: t('cmdk.tasks'),
    execution: t('cmdk.executions'),
  }[kind] || kind);

  const kindTone = { page: 'slate', agent: 'accent', skill: 'violet', task: 'amber', execution: 'cyan' };

  const activate = (item) => {
    onClose();
    if (item.kind === 'page') { onNavigate(item.pageKey); return; }
    if (item.kind === 'agent') { onNavigate('agents'); return; }
    if (item.kind === 'skill') { onNavigate('skills'); return; }
    if (item.kind === 'task') { onNavigate('tasks'); return; }
    if (item.kind === 'execution') { onOpenExec && onOpenExec(item.data); onNavigate('executions'); return; }
  };

  const onKey = (e) => {
    if (e.key === 'Escape') { onClose(); return; }
    if (e.key === 'ArrowDown') { e.preventDefault(); setActiveIdx(i => Math.min(i + 1, results.length - 1)); }
    if (e.key === 'ArrowUp') { e.preventDefault(); setActiveIdx(i => Math.max(i - 1, 0)); }
    if (e.key === 'Enter' && results[activeIdx]) { activate(results[activeIdx]); }
  };

  useEffect(() => {
    if (!open) return;
    const handler = (e) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [open, onClose]);

  // Scroll active item into view
  useEffect(() => {
    if (!listRef.current) return;
    const el = listRef.current.querySelector('[data-active="true"]');
    if (el) el.scrollIntoView({ block: 'nearest' });
  }, [activeIdx]);

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-[70]">
      <div className="absolute inset-0 bg-black/60 anim-fade-in" onClick={onClose} />
      <div
        className="absolute left-1/2 top-[15vh] bg-bg-2 border border-border rounded-xl shadow-2xl anim-dialog-in overflow-hidden"
        style={{ width: 580, maxWidth: 'calc(100vw - 32px)', transform: 'translateX(-50%)' }}
      >
        {/* Search input */}
        <div className="flex items-center gap-3 px-4 py-3 border-b border-border">
          <I.Search size={15} className="text-fg-4 shrink-0" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKey}
            placeholder={t('cmdk.placeholder')}
            className="flex-1 bg-transparent text-[14px] text-fg placeholder:text-fg-4 outline-none"
          />
          {query && (
            <button onClick={() => setQuery('')} className="text-fg-4 hover:text-fg">
              <I.Close size={14} />
            </button>
          )}
          <Kbd>ESC</Kbd>
        </div>

        {/* Results */}
        <div ref={listRef} className="max-h-[360px] overflow-auto py-1">
          {results.length === 0 && (
            <div className="px-4 py-10 text-center text-[13px] text-fg-4">{t('cmdk.no_results')}</div>
          )}
          {results.map((item, idx) => {
            const Icon = item.icon || I.Circle;
            const active = idx === activeIdx;
            return (
              <button
                key={item.id}
                data-active={active}
                onClick={() => activate(item)}
                onMouseEnter={() => setActiveIdx(idx)}
                className={`w-full flex items-center gap-3 px-4 py-2.5 text-left transition-colors ${active ? 'bg-[var(--bg-3)]' : 'hover:bg-[var(--hover)]'}`}
              >
                <div className={`h-7 w-7 rounded-md flex items-center justify-center shrink-0 ${active ? 'bg-accent-soft text-accent' : 'bg-[var(--bg-3)] text-fg-3'}`}>
                  <Icon size={14} />
                </div>
                <div className="flex-1 min-w-0">
                  <div className="text-[13px] text-fg font-medium truncate">{item.label}</div>
                  {item.sub && <div className="text-[11px] text-fg-4 mono truncate">{item.sub}</div>}
                </div>
                <Badge tone={kindTone[item.kind] || 'slate'}>{kindLabel(item.kind)}</Badge>
              </button>
            );
          })}
        </div>

        {/* Footer hint */}
        <div className="px-4 py-2 border-t border-border flex items-center gap-3 text-[10px] mono text-fg-4">
          <span><Kbd>↑↓</Kbd> navigate</span>
          <span><Kbd>↵</Kbd> open</span>
          <span><Kbd>ESC</Kbd> close</span>
          <span className="ml-auto">{results.length} results</span>
        </div>
      </div>
    </div>
  );
};

const NAV = [
  { key: 'status', icon: I.Home, label: 'Status' },
  { key: 'chat', icon: I.MessageSquare || I.Book, label: 'Chat' },
  { key: 'agents', icon: I.Bot, label: 'Agents' },
  { key: 'skills', icon: I.Brain, label: 'Skills' },
  { key: 'tasks', icon: I.List, label: 'Tasks' },
  { key: 'executions', icon: I.Activity, label: 'Executions' },
  { key: 'reviews', icon: I.Check, label: 'Reviews' },
  { key: 'clarifications', icon: I.MessageSquare || I.Book, label: 'Clarifications' },
  { key: 'handoffs',     icon: I.Activity, label: 'Handoffs' },
  { key: 'audit', icon: I.Shield, label: 'Audit' },
  { key: 'learning', icon: I.Book, label: 'Learning' },
  { key: 'workbench', icon: I.Terminal, label: 'Workbench' },
  { key: 'capabilities', icon: I.Plug, label: 'Capabilities' },
  { key: 'channels', icon: I.Radio, label: 'Channels' },
  { key: 'nodes', icon: I.Server, label: 'Nodes' },
  { key: 'memory_console', icon: I.Brain, label: 'Memory Console' },
  { key: 'admin_ops', icon: I.Settings, label: 'Admin Ops' },
  { key: 'kanban', icon: I.List, label: 'Kanban' },
  { key: 'browser_console', icon: I.Globe, label: 'Browser Console' },
  { key: 'multimodal', icon: I.Sparkles, label: 'Multimodal' },
  { key: 'im_platforms', icon: I.Radio, label: 'IM Platforms' },
  { key: 'curator', icon: I.Brain, label: 'Curator' },
  { key: 'moa', icon: I.Layers, label: 'Mixture-of-Agents' },
  { key: 'capability_monitor', icon: I.AlertTriangle || I.Activity, label: 'Capability Monitor' },
  { key: 'tools_mgmt', icon: I.ToolState, label: 'Tools' },
  { key: 'cluster',    icon: I.Cluster,   label: 'Cluster' },
  { key: 'settings', icon: I.Settings, label: 'Settings' },
];

const Sidebar = ({ current, onChange, collapsed, onToggle, lang, onLogout, onBackToOperate }) => {
  const t = tFor(lang);
  const aboutRes = window.cc.data.useAbout();
  const pendingReviewsRes = window.cc.data.useReviews('pending');
  const version = (aboutRes.data && aboutRes.data.version) || '—';
  const pendingList = window.cc.data.extractList(pendingReviewsRes, 'reviews:pending', null);
  const pendingCount = pendingList.length;
  return (
    <aside
      className="border-r border-border flex flex-col bg-bg-2"
      style={{ width: collapsed ? 64 : 240, transition: 'width .18s ease' }}
    >
      <div className={`h-11 border-b border-border flex items-center ${collapsed ? 'justify-center' : 'px-4'} shrink-0`}>
        <div className="flex items-center gap-2 min-w-0">
          <span className="text-accent shrink-0"><I.Claw size={18} /></span>
          {!collapsed && (
            <div className="min-w-0">
              <div className="text-[13px] font-semibold tracking-tight leading-none">CyberClaw</div>
              <div className="text-[10px] text-fg-4 mono leading-none mt-1">v{version}</div>
            </div>
          )}
        </div>
      </div>
      <nav className="flex-1 py-2 overflow-auto">
        {NAV.map(item => {
          const Icon = item.icon;
          const active = current === item.key;
          return (
            <button key={item.key} onClick={() => onChange(item.key)} title={collapsed ? t(item.key) : ''}
              className={`w-full flex items-center gap-2.5 ${collapsed ? 'justify-center px-0' : 'px-3'} h-8 mx-2 my-0.5 rounded-md text-[13px] transition-colors ${active ? 'bg-[var(--bg-3)] text-fg border border-[var(--border)]' : 'text-fg-2 hover:bg-[var(--hover)] hover:text-fg border border-transparent'}`}
              style={{ width: collapsed ? 48 : 'calc(100% - 16px)' }}
            >
              <Icon size={15} className={active ? 'text-accent' : 'text-fg-3'} />
              {!collapsed && <span className="truncate">{t(item.key)}</span>}
              {!collapsed && item.key === 'reviews' && pendingCount > 0 && (
                <Badge tone="rose" className="ml-auto">{pendingCount}</Badge>
              )}
            </button>
          );
        })}
      </nav>
      <div className="border-t border-border p-2 space-y-1">
        {onBackToOperate && (
          <button onClick={onBackToOperate}
            title={collapsed ? t('mode.switch.to_operate') : ''}
            className={`w-full flex items-center gap-2 ${collapsed ? 'justify-center' : 'px-3'} h-7 rounded-md text-[12px] text-accent hover:text-fg hover:bg-accent-soft border border-transparent hover:border-[var(--accent-ring)] transition-colors`}>
            <I.MessageSquare size={13} />
            {!collapsed && <span className="truncate">{t('mode.switch.to_operate')}</span>}
          </button>
        )}
        <button onClick={onToggle}
          className={`w-full flex items-center gap-2 ${collapsed ? 'justify-center' : 'px-3'} h-7 rounded-md text-[12px] text-fg-3 hover:text-fg hover:bg-[var(--hover)]`}>
          {collapsed ? <I.ChevronsRight size={14} /> : <><I.ChevronsLeft size={14} /><span>{t('collapse')}</span></>}
        </button>
        <button onClick={onLogout}
          className={`w-full flex items-center gap-2 ${collapsed ? 'justify-center' : 'px-3'} h-7 rounded-md text-[12px] text-fg-3 hover:text-rose-400 hover:bg-[var(--hover)]`}>
          <I.Logout size={14} />
          {!collapsed && <span>{t('logout')}</span>}
        </button>
      </div>
    </aside>
  );
};

// Sprint 14 S14-1: segmented mode switcher. Renders two pills (Operate | Govern).
// The active pill slides using an absolutely-positioned highlight layer so the
// transition is a tactile slide, not a color flip.
const ModeSwitcher = ({ mode, onMode, lang }) => {
  const t = tFor(lang);
  const isGovern = mode === 'govern';
  return (
    <div
      role="tablist"
      aria-label="Console mode"
      className="relative inline-flex items-center h-8 p-0.5 rounded-lg bg-bg-3 border border-border select-none"
      style={{ minWidth: 184 }}
    >
      {/* sliding highlight */}
      <span
        aria-hidden="true"
        className="absolute top-0.5 bottom-0.5 rounded-md bg-bg-2 border border-[var(--border-strong)] shadow-[0_1px_2px_rgba(0,0,0,0.25)]"
        style={{
          width: 'calc(50% - 2px)',
          left: isGovern ? 'calc(50%)' : '2px',
          transition: 'left .18s cubic-bezier(.4,0,.2,1)',
        }}
      />
      <button
        role="tab"
        aria-selected={!isGovern}
        onClick={() => onMode('operate')}
        className={`relative z-10 flex-1 h-7 px-3 inline-flex items-center justify-center gap-1.5 text-[12px] rounded-md transition-colors ${isGovern ? 'text-fg-3 hover:text-fg-2' : 'text-fg'}`}
      >
        <I.MessageSquare size={12} className={isGovern ? 'text-fg-4' : 'text-accent'} />
        <span className="font-medium tracking-tight">{t('mode.operate')}</span>
      </button>
      <button
        role="tab"
        aria-selected={isGovern}
        onClick={() => onMode('govern')}
        className={`relative z-10 flex-1 h-7 px-3 inline-flex items-center justify-center gap-1.5 text-[12px] rounded-md transition-colors ${isGovern ? 'text-fg' : 'text-fg-3 hover:text-fg-2'}`}
      >
        <I.Shield size={12} className={isGovern ? 'text-accent' : 'text-fg-4'} />
        <span className="font-medium tracking-tight">{t('mode.govern')}</span>
      </button>
    </div>
  );
};

const Topbar = ({ current, lang, onLang, theme, onTheme, operator, onOpenTweaks, scenario, onOpenPalette, mode, onMode, rightRailOpen, onToggleRightRail }) => {
  const t = tFor(lang);
  const crumbs = [{ label: 'cyberclaw' }, { label: t(current) }];
  const [userOpen, setUserOpen] = useState(false);
  const [sheet, setSheet] = useState(null); // 'profile' | 'tokens' | 'docs' | null
  const healthMap = { healthy: 'bg-emerald-500', degraded: 'bg-amber-400', incident: 'bg-rose-500' };
  const openSheet = (k) => { setSheet(k); setUserOpen(false); };
  const showCrumbs = mode !== 'operate'; // Operate mode is single-surface — breadcrumbs add noise

  // Global ⌘K / Ctrl+K shortcut
  useEffect(() => {
    const handler = (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
        e.preventDefault();
        onOpenPalette && onOpenPalette();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [onOpenPalette]);

  return (
    <header className="h-11 border-b border-border flex items-center px-4 gap-3 shrink-0 bg-bg-2">
      <ModeSwitcher mode={mode} onMode={onMode} lang={lang} />
      {showCrumbs && (
        <nav className="flex items-center gap-1.5 text-[12px] min-w-0 pl-1">
          {crumbs.map((c, i) => (
            <React.Fragment key={i}>
              {i > 0 && <I.ChevronRight size={12} className="text-fg-4" />}
              <span className={`${i === crumbs.length - 1 ? 'text-fg' : 'text-fg-3'} ${i === 0 ? 'mono' : 'font-medium capitalize'}`}>{c.label}</span>
            </React.Fragment>
          ))}
        </nav>
      )}
      <div className="flex-1" />
      <button
        onClick={onOpenPalette}
        className="hidden lg:flex items-center gap-2 text-[11px] text-fg-3 px-2.5 py-1 rounded-md bg-bg-3 border border-border hover:border-[var(--border-strong)] hover:text-fg transition-colors"
        style={{ lineHeight: 1.5 }}
      >
        <I.Search size={11} className="text-fg-4" />
        <span className="whitespace-nowrap">{t('search_ellipsis')}</span>
        <Kbd>⌘K</Kbd>
      </button>
      <div className="flex items-center gap-1">
        {mode === 'operate' && (
          <button
            onClick={onToggleRightRail}
            title={rightRailOpen ? t('right_rail.hide') : t('right_rail.show')}
            aria-pressed={rightRailOpen}
            className={`h-8 w-8 inline-flex items-center justify-center rounded-md transition-colors ${rightRailOpen ? 'text-accent bg-accent-soft' : 'text-fg-3 hover:text-fg hover:bg-[var(--hover)]'}`}
          >
            <I.Layers size={14} />
          </button>
        )}
        <button onClick={onOpenTweaks} title={t('tweaks')} className="h-8 w-8 inline-flex items-center justify-center rounded-md text-fg-3 hover:text-fg hover:bg-[var(--hover)]">
          <I.Sliders size={14} />
        </button>
        <LangMenu lang={lang} onLang={onLang} />

        <button onClick={() => onTheme(theme === 'dark' ? 'light' : 'dark')} title="Theme" className="h-8 w-8 inline-flex items-center justify-center rounded-md text-fg-3 hover:text-fg hover:bg-[var(--hover)]">
          {theme === 'dark' ? <I.Sun size={14} /> : <I.Moon size={14} />}
        </button>
      </div>
      <div className="h-5 w-px bg-border mx-1" />
      <div className="flex items-center gap-1.5 text-[11px] mono text-fg-3 px-2 py-1 rounded-md">
        <span className={`w-1.5 h-1.5 rounded-full pulse-dot ${healthMap[scenario]}`} />
        <span>{t('scenario_' + scenario)}</span>
      </div>
      <div className="relative">
        <button onClick={() => setUserOpen(v => !v)} onBlur={() => setTimeout(() => setUserOpen(false), 150)}
          className="flex items-center gap-2 h-8 pl-1 pr-2 rounded-md hover:bg-[var(--hover)]">
          <div className="h-6 w-6 rounded-full bg-accent-soft text-accent flex items-center justify-center text-[10px] mono font-semibold">{operator.avatar_initials || 'AC'}</div>
          <span className="text-[12px] text-fg hidden md:inline">{operator.display_name}</span>
          <I.ChevronDown size={11} className="text-fg-4" />
        </button>
        {userOpen && (
          <div className="absolute right-0 top-9 w-56 bg-bg-2 border border-border rounded-md shadow-xl anim-slide-up overflow-hidden z-40">
            <div className="p-3 border-b border-border">
              <div className="text-[12px] font-medium">{operator.display_name}</div>
              <div className="text-[11px] text-fg-3 mono">{operator.user_id} · {operator.role}</div>
            </div>
            <div className="py-1 text-[12px]">
              <MenuItem icon={I.User} label={t('profile')} onClick={() => openSheet('profile')} />
              <MenuItem icon={I.Key} label={t('api_tokens')} onClick={() => openSheet('tokens')} />
              <MenuItem icon={I.Book} label={t('docs')} onClick={() => openSheet('docs')} />
            </div>
          </div>
        )}
      </div>
      <ProfileSheet open={sheet === 'profile'} onClose={() => setSheet(null)} operator={operator} lang={lang} />
      <TokensSheet open={sheet === 'tokens'} onClose={() => setSheet(null)} lang={lang} />
      <DocsSheet open={sheet === 'docs'} onClose={() => setSheet(null)} lang={lang} />
    </header>
  );
};
const LangMenu = ({ lang, onLang }) => {
  const [open, setOpen] = useState(false);
  const meta = LANG_META.find(l => l.code === lang) || LANG_META[0];
  return (
    <div className="relative">
      <button onClick={() => setOpen(v => !v)} onBlur={() => setTimeout(() => setOpen(false), 150)}
        title="Language" className="h-8 px-2 inline-flex items-center gap-1 rounded-md text-fg-3 hover:text-fg hover:bg-[var(--hover)] text-[11px] mono">
        <I.Globe size={12} /> {meta.short}
      </button>
      {open && (
        <div className="absolute right-0 top-9 w-40 bg-bg-2 border border-border rounded-md shadow-xl anim-slide-up overflow-hidden z-40 py-1">
          {LANG_META.map(l => (
            <button key={l.code} onMouseDown={(e) => { e.preventDefault(); onLang(l.code); setOpen(false); }}
              className={`w-full px-3 py-1.5 text-left flex items-center justify-between hover:bg-[var(--hover)] ${l.code === lang ? 'text-fg' : 'text-fg-2'}`}>
              <span className="text-[12px]">{l.label}</span>
              <span className="mono text-[10px] text-fg-4">{l.short}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
};

const MenuItem = ({ icon: Icon, label, onClick }) => (
  <button onMouseDown={(e) => { e.preventDefault(); onClick && onClick(); }}
    className="w-full px-3 py-1.5 text-left hover:bg-[var(--hover)] flex items-center gap-2 text-fg-2">
    <Icon size={12} className="text-fg-4" /> {label}
  </button>
);

// ---- Tweaks panel ----
const ACCENTS = [
  { key: 'indigo', h: 243, s: 75, l: 59, label: 'Indigo' },
  { key: 'violet', h: 270, s: 70, l: 62, label: 'Violet' },
  { key: 'teal',   h: 174, s: 65, l: 48, label: 'Teal' },
  { key: 'amber',  h: 32,  s: 85, l: 55, label: 'Amber' },
];
const SCENARIOS = ['healthy', 'degraded', 'incident'];
const ROLES = ['admin', 'operator', 'viewer'];
const DENSITIES = ['comfortable', 'compact'];
const LANGS = ['en', 'zh-CN', 'ja', 'es', 'fr', 'de'];

const TweaksPanel = ({ open, onClose, state, setState }) => {
  if (!open) return null;
  const t = tFor(state.lang);
  return (
    <div className="fixed right-4 top-16 z-50 w-[280px] anim-slide-up">
      <Card className="overflow-hidden shadow-2xl">
        <div className="px-4 py-2.5 border-b border-border flex items-center justify-between">
          <div className="flex items-center gap-2">
            <I.Sliders size={13} className="text-accent" />
            <div className="text-[13px] font-semibold">{t('tweaks')}</div>
          </div>
          <button onClick={onClose} className="text-fg-3 hover:text-fg"><I.Close size={14} /></button>
        </div>
        <div className="p-4 space-y-4">
          <TweakRow label={t('tweaks_accent')}>
            <div className="flex gap-1.5">
              {ACCENTS.map(a => (
                <button key={a.key} onClick={() => setState(s => ({ ...s, accent: a.key }))}
                  className={`h-6 w-6 rounded-md border-2 transition-all ${state.accent === a.key ? 'border-fg scale-110' : 'border-border'}`}
                  style={{ background: `hsl(${a.h} ${a.s}% ${a.l}%)` }}
                  title={a.label}
                />
              ))}
            </div>
          </TweakRow>
          <TweakRow label={t('tweaks_theme')}>
            <Pills value={state.theme} options={['dark', 'light']} onChange={v => setState(s => ({ ...s, theme: v }))} />
          </TweakRow>
          <TweakRow label={t('tweaks_density')}>
            <Pills value={state.density} options={DENSITIES} onChange={v => setState(s => ({ ...s, density: v }))} />
          </TweakRow>
          <TweakRow label={t('tweaks_sidebar')}>
            <Pills value={state.collapsed ? 'collapsed' : 'expanded'} options={['expanded', 'collapsed']}
              onChange={v => setState(s => ({ ...s, collapsed: v === 'collapsed' }))} />
          </TweakRow>
          <TweakRow label={t('tweaks_language')}>
            <Pills value={state.lang} options={LANGS} onChange={v => setState(s => ({ ...s, lang: v }))} />
          </TweakRow>
          <TweakRow label={t('tweaks_scenario')}>
            <Pills value={state.scenario} options={SCENARIOS} onChange={v => setState(s => ({ ...s, scenario: v }))} />
          </TweakRow>
          {/* Role selector removed — role is authoritative from backend
              OperatorRecord. Sprint 8 vuln fix (user-reported). */}
        </div>
      </Card>
    </div>
  );
};
const TweakRow = ({ label, children }) => (
  <div className="flex items-center justify-between gap-3">
    <div className="text-[11px] uppercase tracking-wider text-fg-3">{label}</div>
    <div>{children}</div>
  </div>
);
const Pills = ({ value, options, onChange }) => (
  <div className="inline-flex items-center rounded-md border border-border bg-bg-3 p-0.5">
    {options.map(o => (
      <button key={o} onClick={() => onChange(o)}
        className={`px-2 h-6 text-[11px] rounded-[4px] mono ${value === o ? 'bg-bg-2 text-fg border border-border' : 'text-fg-3 hover:text-fg'}`}>
        {o}
      </button>
    ))}
  </div>
);

// Sprint 14 S14-1: Minimal left column for Operate mode.
// Fixed 260px. Single primary affordance (Chat), no 13-tab sidebar. Bottom
// "Switch to Govern" link feels like a secondary door rather than a peer tab.
const OperateSideNav = ({ current, onChange, lang, onSwitchToGovern, operator, onLogout }) => {
  const t = tFor(lang);
  const aboutRes = window.cc.data.useAbout();
  const version = (aboutRes.data && aboutRes.data.version) || '—';
  return (
    <aside className="border-r border-border flex flex-col bg-bg-2 shrink-0" style={{ width: 260 }}>
      <div className="h-11 border-b border-border flex items-center px-4 shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          <span className="text-accent shrink-0"><I.Claw size={18} /></span>
          <div className="min-w-0">
            <div className="text-[13px] font-semibold tracking-tight leading-none">CyberClaw</div>
            <div className="text-[10px] text-fg-4 mono leading-none mt-1">v{version}</div>
          </div>
        </div>
      </div>
      <div className="px-3 pt-4 pb-2">
        <div className="text-[10px] uppercase tracking-[0.14em] text-fg-4 mono px-2">{t('operate.sessions_label')}</div>
      </div>
      <nav className="flex-1 px-2 overflow-auto">
        <button
          onClick={() => onChange('chat')}
          className={`w-full flex items-center gap-2.5 px-3 h-9 my-0.5 rounded-md text-[13px] transition-colors ${current === 'chat' ? 'bg-accent-soft text-fg border border-[var(--accent-ring)]' : 'text-fg-2 hover:bg-[var(--hover)] hover:text-fg border border-transparent'}`}
        >
          <I.MessageSquare size={14} className={current === 'chat' ? 'text-accent' : 'text-fg-3'} />
          <span className="truncate flex-1 text-left font-medium">{t('chat')}</span>
          <span className="text-[10px] mono text-fg-4">●</span>
        </button>
        <div className="px-3 pt-3 pb-1 text-[10px] mono text-fg-4 uppercase tracking-wider">
          {t('operate.new_conversation')}
        </div>
        <div className="px-3 py-4 text-[11px] text-fg-4 leading-relaxed">
          {t('operate.empty_hint')}
        </div>
      </nav>
      <div className="border-t border-border p-2 space-y-1">
        <button onClick={onSwitchToGovern}
          className="w-full flex items-center gap-2 px-3 h-7 rounded-md text-[11px] text-fg-3 hover:text-fg hover:bg-[var(--hover)] transition-colors">
          <I.Shield size={12} className="text-fg-4" />
          <span className="truncate">{t('mode.switch.to_govern')}</span>
          <I.ChevronRight size={11} className="ml-auto text-fg-4" />
        </button>
        <button onClick={onLogout}
          className="w-full flex items-center gap-2 px-3 h-7 rounded-md text-[11px] text-fg-3 hover:text-rose-400 hover:bg-[var(--hover)]">
          <I.Logout size={12} />
          <span className="truncate">{t('logout')}</span>
        </button>
      </div>
    </aside>
  );
};

// Sprint 14 S14-1: Right-rail scaffold for Operate mode.
// Three tabs: Workspace | Memory | Skills. Content is an empty state for this
// story — S14-5 fills Memory. The rail collapses fully (width: 0) when closed,
// so it never leaves a residual column.
const RightRail = ({ open, lang, onClose }) => {
  const t = tFor(lang);
  const [tab, setTab] = useState('workspace');
  if (!open) return null;
  const tabs = [
    { key: 'workspace', label: t('right_rail.workspace'), icon: I.Terminal },
    { key: 'memory',    label: t('right_rail.memory'),    icon: I.Brain },
    { key: 'skills',    label: t('right_rail.skills'),    icon: I.Plug },
  ];
  return (
    <aside
      className="border-l border-border bg-bg-2 flex flex-col shrink-0 anim-slide-up"
      style={{ width: 320 }}
    >
      <div className="h-11 border-b border-border flex items-center px-3 shrink-0 gap-1">
        <div className="inline-flex items-center rounded-md border border-border bg-bg-3 p-0.5">
          {tabs.map(tb => {
            const Icon = tb.icon;
            const active = tab === tb.key;
            return (
              <button key={tb.key} onClick={() => setTab(tb.key)}
                className={`h-7 px-2 inline-flex items-center gap-1.5 text-[11px] rounded-[4px] transition-colors ${active ? 'bg-bg-2 text-fg border border-border' : 'text-fg-3 hover:text-fg'}`}>
                <Icon size={11} className={active ? 'text-accent' : 'text-fg-4'} />
                <span>{tb.label}</span>
              </button>
            );
          })}
        </div>
        <div className="flex-1" />
        <button onClick={onClose} title={t('close')}
          className="h-7 w-7 inline-flex items-center justify-center rounded-md text-fg-3 hover:text-fg hover:bg-[var(--hover)]">
          <I.Close size={12} />
        </button>
      </div>
      <div className="flex-1 overflow-auto">
        <div className="flex flex-col items-center justify-center h-full px-6 py-10 text-center">
          <div className="h-10 w-10 rounded-lg bg-bg-3 border border-border flex items-center justify-center mb-3 text-fg-4">
            {tab === 'workspace' && <I.Terminal size={16} />}
            {tab === 'memory' && <I.Brain size={16} />}
            {tab === 'skills' && <I.Plug size={16} />}
          </div>
          <div className="text-[12px] text-fg-2 font-medium">{tabs.find(x => x.key === tab).label}</div>
          <div className="text-[11px] text-fg-4 mt-1 max-w-[220px] leading-relaxed">
            {t('right_rail.empty_state')}
          </div>
        </div>
      </div>
    </aside>
  );
};

const Layout = ({ current, setCurrent, lang, setLang, theme, setTheme, collapsed, setCollapsed, mode, setMode, rightRailOpen, setRightRailOpen, onLogout, operator, scenario, role, openTweaks, onOpenTweaks, tweakState, setTweakState, onOpenExec, children }) => {
  const t = tFor(lang);
  const aboutRes = window.cc.data.useAbout();
  const about = aboutRes.data || {};
  const [paletteOpen, setPaletteOpen] = useState(false);

  const topbar = (
    <Topbar
      current={current}
      lang={lang}
      onLang={setLang}
      theme={theme}
      onTheme={setTheme}
      operator={operator}
      scenario={scenario}
      onOpenTweaks={onOpenTweaks}
      onOpenPalette={() => setPaletteOpen(true)}
      mode={mode}
      onMode={(m) => {
        setMode(m);
        // Entering Operate mode should land on Chat (only page exposed in Operate).
        if (m === 'operate') setCurrent('chat');
      }}
      rightRailOpen={rightRailOpen}
      onToggleRightRail={() => setRightRailOpen(!rightRailOpen)}
    />
  );

  const modals = (
    <>
      <TweaksPanel open={openTweaks} onClose={onOpenTweaks} state={tweakState} setState={setTweakState} />
      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        lang={lang}
        onNavigate={(key) => { setCurrent(key); setPaletteOpen(false); }}
        onOpenExec={(exec) => { onOpenExec && onOpenExec(exec); setPaletteOpen(false); }}
      />
    </>
  );

  // ── Operate mode: three-pane (session nav | chat | right rail) ───────────
  if (mode === 'operate') {
    return (
      <div className="h-screen flex flex-col">
        <div className="flex flex-1 min-h-0">
          <OperateSideNav
            current={current}
            onChange={setCurrent}
            lang={lang}
            onSwitchToGovern={() => { setMode('govern'); setCurrent('status'); }}
            operator={operator}
            onLogout={onLogout}
          />
          <div className="flex-1 flex flex-col min-w-0">
            {topbar}
            <main className="flex-1 overflow-hidden min-h-0">
              {/* ChatPage owns its own layout and scroll — no title header, no footer. */}
              <div className="h-full">
                {children}
              </div>
            </main>
          </div>
          <RightRail open={rightRailOpen} lang={lang} onClose={() => setRightRailOpen(false)} />
        </div>
        {modals}
      </div>
    );
  }

  // ── Govern mode: original 13-tab shell (preserved verbatim, do not alter) ─
  const pageTitle = {
    status: { title: t('status'), sub: t('sub_status') },
    chat: { title: t('chat'), sub: t('sub_chat') },
    agents: { title: t('agents'), sub: t('sub_agents') },
    skills: { title: t('skills'), sub: t('sub_skills') },
    tasks: { title: t('tasks'), sub: t('sub_tasks') },
    executions: { title: t('executions'), sub: t('sub_executions') },
    reviews: { title: t('reviews'), sub: t('sub_reviews') },
    clarifications: { title: t('govern.tab.clarifications'), sub: t('clarify.govern.readonly_label') },
    handoffs: { title: t('handoffs.page.title') || 'Handoffs', sub: t('handoffs.page.subtitle') || 'read-only' },
    audit: { title: t('audit'), sub: t('sub_audit') },
    learning: { title: t('learning'), sub: t('sub_learning') },
    workbench: { title: t('workbench'), sub: t('sub_workbench') },
    capabilities: { title: t('capabilities'), sub: t('sub_capabilities') },
    channels: { title: t('channels'), sub: t('sub_channels') },
    nodes: { title: t('nodes'), sub: t('sub_nodes') },
    settings: { title: t('settings'), sub: t('sub_settings') },
  }[current] || { title: '', sub: '' };
  return (
    <div className="h-screen flex flex-col">
      <div className="flex flex-1 min-h-0">
        <Sidebar
          current={current}
          onChange={setCurrent}
          collapsed={collapsed}
          onToggle={() => setCollapsed(!collapsed)}
          lang={lang}
          onLogout={onLogout}
          onBackToOperate={() => { setMode('operate'); setCurrent('chat'); }}
        />
        <div className="flex-1 flex flex-col min-w-0">
          {topbar}
          <main className="flex-1 overflow-auto">
            <div className="mx-auto max-w-[1440px] px-6 py-5">
              <div className="mb-5">
                <div className="text-[20px] font-semibold tracking-tight">{pageTitle.title}</div>
                <div className="text-[12px] text-fg-3 mt-0.5">{pageTitle.sub}</div>
              </div>
              {children}
              <div className="mt-10 pt-4 border-t border-border flex items-center justify-between text-[10px] mono text-fg-4">
                <span>cyberclaw v{about.version || '—'} · {about.commit || '—'}</span>
                <span>{t('footer_operator')} {operator.user_id} · {t('footer_role')} {role}</span>
              </div>
            </div>
          </main>
        </div>
      </div>
      {modals}
    </div>
  );
};

Object.assign(window, { Layout, Sidebar, Topbar, NAV, ACCENTS, CommandPalette, ModeSwitcher, OperateSideNav, RightRail });
