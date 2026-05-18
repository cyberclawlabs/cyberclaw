// pages_memory_panel.jsx — Memory right-rail panel
// Sprint 14 S14-5: Memory tab — User Profile / Agent Notes / Skills Index
// Exported on window.MemoryPanel for shell.jsx to mount.

const { useState: mpS, useEffect: mpE, useCallback: mpC } = React;

// ── helpers ──────────────────────────────────────────────────────────────────

function mpApiFetch(path) {
  return window.cc && window.cc.apiFetch
    ? window.cc.apiFetch(path)
    : fetch(path).then(r => {
        if (!r.ok) throw new Error(path + ': HTTP ' + r.status);
        return r.json();
      });
}

// ── Sub-tab: User Profile ─────────────────────────────────────────────────────

const ProfileTab = ({ lang }) => {
  const t = tFor(lang);
  const [cfg, setCfg] = mpS(null);
  const [err, setErr] = mpS(null);
  const [loading, setLoading] = mpS(true);

  mpE(() => {
    let cancelled = false;
    setLoading(true);
    const api = window.cc && window.cc.api;
    const p = api && api.settings && api.settings.config
      ? api.settings.config()
      : mpApiFetch('/api/v1/settings/config');
    p.then(d => { if (!cancelled) { setCfg(d); setErr(null); } })
      .catch(e => { if (!cancelled) setErr(String(e)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, []);

  const toast = (type, msg) => {
    if (window.cc && window.cc.toast) window.cc.toast(type, msg);
  };

  if (loading) return (
    <div className="p-3 space-y-2">
      {[80, 60, 70, 55, 65].map((w, i) => (
        <div key={i} className="skeleton h-3 rounded" style={{ width: w + '%' }} />
      ))}
    </div>
  );

  if (err) return (
    <div className="p-3 text-[12px] text-fg-3 mono">
      <span className="text-rose-400">error:</span> {err}
    </div>
  );

  const op = (cfg && cfg.operator) || {};
  const prefs = (cfg && cfg.preferences) || cfg || {};

  const Section = ({ title, children }) => (
    <div className="space-y-1.5">
      <div className="text-[10px] mono text-fg-4 uppercase tracking-wider px-1">{title}</div>
      <div className="rounded-md border border-[var(--border)] bg-[var(--bg-2)] divide-y divide-[var(--border)]">
        {children}
      </div>
    </div>
  );

  const Row = ({ label, value }) => (
    <div className="flex items-center justify-between px-2.5 py-1.5 gap-2">
      <span className="text-[11px] text-fg-3 shrink-0">{label}</span>
      <span className="text-[11px] mono text-fg-2 text-right truncate max-w-[160px]" title={String(value ?? '—')}>
        {value != null && value !== '' ? String(value) : <span className="text-fg-4">—</span>}
      </span>
    </div>
  );

  return (
    <div className="p-3 space-y-3 anim-slide-up">
      <Section title={t('memory.section.identity') || 'Operator identity'}>
        <Row label="display_name" value={op.display_name || prefs.display_name} />
        <Row label="user_id"      value={op.user_id      || prefs.user_id} />
        <Row label="role"         value={op.role         || prefs.role} />
      </Section>

      <Section title={t('memory.section.preferences') || 'Preferences'}>
        <Row label="lang"  value={prefs.lang  || lang} />
        <Row label="theme" value={prefs.theme} />
        {prefs.tweaks && typeof prefs.tweaks === 'object'
          ? Object.entries(prefs.tweaks).slice(0, 4).map(([k, v]) => (
              <Row key={k} label={k} value={String(v)} />
            ))
          : null}
      </Section>

      <button
        onClick={() => toast('info', t('memory.edit_toast') || 'Edit lives in Settings page (Govern mode)')}
        className="w-full flex items-center justify-center gap-1.5 py-1.5 rounded-md border border-[var(--border)] bg-[var(--bg-3)] hover:bg-[var(--hover)] text-[11px] text-fg-3 hover:text-fg transition-colors"
      >
        <I.Edit size={11} />
        {t('edit') || 'Edit'}
      </button>
    </div>
  );
};

// ── Sub-tab: Agent Notes ──────────────────────────────────────────────────────

const NotesTab = ({ lang }) => {
  const t = tFor(lang);
  const [notes, setNotes] = mpS(null);
  const [loading, setLoading] = mpS(true);
  const [empty, setEmpty] = mpS(false);

  mpE(() => {
    let cancelled = false;
    setLoading(true);
    mpApiFetch('/api/v1/memory/agent_notes')
      .then(d => {
        if (!cancelled) {
          const list = Array.isArray(d) ? d : (d && d.notes) ? d.notes : [];
          setNotes(list);
          setEmpty(list.length === 0);
        }
      })
      .catch(e => {
        if (!cancelled) {
          // 404 or any error → graceful empty state
          setNotes([]);
          setEmpty(true);
        }
      })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, []);

  if (loading) return (
    <div className="p-3 space-y-2">
      {[100, 90, 75, 85].map((w, i) => (
        <div key={i} className="skeleton h-14 rounded-md" />
      ))}
    </div>
  );

  if (empty || !notes || notes.length === 0) return (
    <div className="p-4 flex flex-col items-center gap-3 text-center anim-slide-up">
      <div className="w-9 h-9 rounded-full bg-[var(--bg-3)] border border-[var(--border)] flex items-center justify-center">
        <I.Book size={15} className="text-fg-4" />
      </div>
      <div className="space-y-1">
        <div className="text-[12px] font-medium text-fg-2">
          {t('memory.empty_state.notes') || 'No agent notes yet.'}
        </div>
        <div className="text-[11px] text-fg-4 leading-snug">
          Notes will appear here as agents accrue knowledge across executions.
        </div>
      </div>
    </div>
  );

  return (
    <div className="p-3 space-y-2 overflow-y-auto anim-slide-up" style={{ maxHeight: 420 }}>
      {notes.map((n, i) => {
        const agent = n.agent_id || n.agent || 'unknown';
        const content = n.content || n.note || n.text || JSON.stringify(n);
        const ts = n.created_at || n.ts || '';
        return (
          <div key={n.id || i} className="rounded-md border border-[var(--border)] bg-[var(--bg-2)] p-2.5 space-y-1">
            <div className="flex items-center justify-between gap-2">
              <span className="text-[10px] mono text-accent truncate">{agent}</span>
              {ts && <span className="text-[10px] mono text-fg-4 shrink-0">{ts}</span>}
            </div>
            <p className="text-[11px] text-fg-2 leading-snug whitespace-pre-wrap break-words">{content}</p>
          </div>
        );
      })}
    </div>
  );
};

// ── Sub-tab: Skills Index ─────────────────────────────────────────────────────

const TRUST_TONE = {
  verified:  'emerald',
  trusted:   'accent',
  community: 'amber',
  sandbox:   'slate',
  untrusted: 'rose',
};

const SkillsTab = ({ lang }) => {
  const t = tFor(lang);
  const [skills, setSkills] = mpS(null);
  const [loading, setLoading] = mpS(true);
  const [err, setErr] = mpS(null);
  const [selected, setSelected] = mpS(null); // {skill, content}
  const [contentLoading, setContentLoading] = mpS(false);

  mpE(() => {
    let cancelled = false;
    setLoading(true);
    const api = window.cc && window.cc.api;
    const p = api && api.skills && api.skills.list
      ? api.skills.list()
      : mpApiFetch('/api/v1/skills');
    p.then(d => {
      if (!cancelled) {
        const list = Array.isArray(d) ? d : (d && d.skills) ? d.skills : [];
        setSkills(list);
      }
    })
    .catch(e => { if (!cancelled) setErr(String(e)); })
    .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, []);

  const openSkill = mpC(async (skill) => {
    setContentLoading(true);
    setSelected({ skill, content: null });
    try {
      const api = window.cc && window.cc.api;
      const raw = api && api.skills && api.skills.content
        ? await api.skills.content(skill.id || skill.name)
        : await mpApiFetch('/api/v1/skills/' + encodeURIComponent(skill.id || skill.name) + '/content');
      const text = typeof raw === 'string' ? raw : (raw && raw.content) ? raw.content : JSON.stringify(raw, null, 2);
      const preview = text.split('\n').slice(0, 30).join('\n');
      setSelected({ skill, content: preview });
    } catch (e) {
      setSelected({ skill, content: '(content unavailable: ' + String(e) + ')' });
    } finally {
      setContentLoading(false);
    }
  }, []);

  if (loading) return (
    <div className="p-3 space-y-1.5">
      {[1,2,3,4].map(i => (
        <div key={i} className="skeleton h-8 rounded" />
      ))}
    </div>
  );

  if (err) return (
    <div className="p-3 text-[12px] mono text-fg-3">
      <span className="text-rose-400">error:</span> {err}
    </div>
  );

  if (!skills || skills.length === 0) return (
    <div className="p-4 flex flex-col items-center gap-3 text-center anim-slide-up">
      <div className="w-9 h-9 rounded-full bg-[var(--bg-3)] border border-[var(--border)] flex items-center justify-center">
        <I.Zap size={15} className="text-fg-4" />
      </div>
      <div className="text-[12px] font-medium text-fg-2">{t('no_skills') || 'No skills installed'}</div>
      <div className="text-[11px] text-fg-4">{t('no_skills_sub') || 'Install a skill from the hub or a local path.'}</div>
    </div>
  );

  return (
    <div className="anim-slide-up">
      {/* Column headers */}
      <div className="grid px-2.5 py-1 border-b border-[var(--border)] bg-[var(--bg-3)]"
           style={{ gridTemplateColumns: '1fr 80px 70px' }}>
        {['name', 'category', 'trust'].map(h => (
          <span key={h} className="text-[10px] mono text-fg-4 uppercase tracking-wider">{h}</span>
        ))}
      </div>

      <div className="overflow-y-auto" style={{ maxHeight: 380 }}>
        {skills.map((sk, i) => {
          const name      = sk.name || sk.id || '—';
          const category  = sk.category || sk.skill_type || '—';
          const trust     = (sk.trust_tier || sk.trust || 'community').toLowerCase();
          const tone      = TRUST_TONE[trust] || 'slate';
          return (
            <button
              key={sk.id || i}
              onClick={() => openSkill(sk)}
              className="w-full grid text-left px-2.5 py-1.5 hover:bg-[var(--hover)] border-b border-[var(--border)] last:border-0 transition-colors"
              style={{ gridTemplateColumns: '1fr 80px 70px' }}
            >
              <span className="text-[11px] mono text-fg-2 truncate pr-1">{name}</span>
              <span className="text-[11px] text-fg-3 truncate pr-1">{category}</span>
              <span><Badge tone={tone}>{trust}</Badge></span>
            </button>
          );
        })}
      </div>

      {/* Methodology Dialog */}
      {selected && (
        <Dialog
          open={true}
          onClose={() => setSelected(null)}
          title={selected.skill.name || selected.skill.id || 'Skill'}
          subtitle={selected.skill.description || 'Methodology preview (first 30 lines of SKILL.md)'}
          width={560}
          footer={
            <div className="flex items-center gap-2 justify-end">
              <button
                onClick={() => {
                  // Navigate to Skills page via cc router if available
                  if (window.cc && window.cc.navigate) {
                    window.cc.navigate('skills');
                  } else if (window.dispatchEvent) {
                    window.dispatchEvent(new CustomEvent('cyberclaw:navigate', { detail: { page: 'skills' } }));
                  }
                  setSelected(null);
                }}
                className="px-3 py-1.5 rounded-md text-[12px] text-accent border border-[var(--accent)]/40 hover:bg-accent-soft transition-colors"
              >
                Open in Skills page →
              </button>
              <button
                onClick={() => setSelected(null)}
                className="px-3 py-1.5 rounded-md text-[12px] text-fg-2 border border-[var(--border)] bg-[var(--bg-3)] hover:bg-[var(--hover)] transition-colors"
              >
                {t('close') || 'Close'}
              </button>
            </div>
          }
        >
          {contentLoading ? (
            <div className="space-y-1.5 p-1">
              {[100,90,85,95,70].map((w, i) => (
                <div key={i} className="skeleton h-3 rounded" style={{ width: w + '%' }} />
              ))}
            </div>
          ) : (
            <pre className="mono text-[11px] text-fg-2 whitespace-pre-wrap break-words leading-relaxed p-1 max-h-64 overflow-y-auto">
              {selected.content || '(no content)'}
            </pre>
          )}
        </Dialog>
      )}
    </div>
  );
};

// ── MemoryPanel root ──────────────────────────────────────────────────────────

const MEMORY_TABS = [
  { value: 'profile', labelKey: 'memory.tab.profile', fallback: 'User Profile', Icon: I.User      },
  { value: 'notes',   labelKey: 'memory.tab.notes',   fallback: 'Agent Notes',  Icon: I.Book      },
  { value: 'skills',  labelKey: 'memory.tab.skills',  fallback: 'Skills Index', Icon: I.Zap       },
];

const MemoryPanel = ({ lang }) => {
  const t = tFor(lang || 'en');
  const [tab, setTab] = mpS('profile');

  const tabItems = MEMORY_TABS.map(({ value, labelKey, fallback }) => ({
    value,
    label: t(labelKey) || fallback,
  }));

  return (
    <div className="flex flex-col h-full" style={{ minWidth: 0 }}>
      {/* Tab bar */}
      <div className="px-2 pt-2 pb-1 shrink-0">
        <Tabs items={tabItems} value={tab} onChange={setTab} className="w-full" />
      </div>

      {/* Tab body */}
      <div className="flex-1 overflow-y-auto min-h-0">
        {tab === 'profile' && <ProfileTab lang={lang || 'en'} />}
        {tab === 'notes'   && <NotesTab   lang={lang || 'en'} />}
        {tab === 'skills'  && <SkillsTab  lang={lang || 'en'} />}
      </div>
    </div>
  );
};

// Expose on window so shell.jsx can mount it
Object.assign(window, { MemoryPanel });
