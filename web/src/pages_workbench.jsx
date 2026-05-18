// pages_workbench.jsx — /workbench page
// C5: Operator Workbench — Diagnose / What-If / Inspect / Dry-Run
// Sprint 11 Admin UI Wave 1 Lane A3
// TranscriptPane is defined in pages_learning.jsx (loaded first via cyberclaw.html)

// ---- Workbench mode configs ----
const WB_MODES = [
  {
    key: 'diagnose',
    label: 'Diagnose',
    labelZh: '诊断',
    icon: I.Search,
    placeholder: 'Ask any principal to explain a recent action…\ne.g. "Release Guard, why did you block v2.4.1 at 14:12?"',
    hint: 'Read-only · returns rationale + linked trace + policy version',
    endpoint: '/api/v1/workbench/diagnose',
  },
  {
    key: 'what-if',
    label: 'What-If',
    labelZh: '假设',
    icon: I.Layers,
    placeholder: 'Project the effect of a policy change against historical data…\ne.g. "If I drop shadow-diff threshold to 3%, what would auto-approve last 7 days?"',
    hint: 'Runs against historical data only · no production side-effects',
    endpoint: '/api/v1/workbench/what-if',
  },
  {
    key: 'inspect',
    label: 'Inspect',
    labelZh: '检查',
    icon: I.Eye,
    placeholder: 'System-wide cross-agent query…\ne.g. "Show all agents that touched wallet.transfer in last 24h, ordered by total value"',
    hint: 'Read-only · scoped to your reachable principals',
    endpoint: '/api/v1/workbench/inspect',
  },
  {
    key: 'dry-run',
    label: 'Dry-Run',
    labelZh: '预演',
    icon: I.Play,
    placeholder: 'Sandbox-start an unreleased agent and validate behavior…\nPaste a sample ticket or describe a scenario.',
    hint: 'Zero production side-effects · promote to /reviews to execute for real',
    endpoint: '/api/v1/workbench/dry-run',
  },
];

// Sprint 18 W2 — workbench responses now come from
// `POST /api/v1/workbench/chat` (apps/cyberclaw-server/src/api/workbench.rs).
// The previous in-SPA MOCK_RESPONSES fixture has been retired.

// ---- Right Rail: referenced objects ----
const WorkbenchRightRail = ({ refs, onClear }) => {
  if (!refs || refs.length === 0) return null;
  const typeIcon = { trace: I.Activity, policy: I.Shield, agent: I.Bot, fact: I.Book };
  const typeTone = { trace: 'cyan', policy: 'amber', agent: 'accent', fact: 'violet' };
  return (
    <div className="w-56 shrink-0 space-y-2">
      <div className="flex items-center justify-between">
        <div className="text-[10px] mono text-fg-4 uppercase tracking-wider">Referenced</div>
        <button onClick={onClear} className="text-[10px] mono text-fg-4 hover:text-fg">clear</button>
      </div>
      {refs.map((r, i) => {
        const Icon = typeIcon[r.type] || I.Circle;
        return (
          <button key={i} className="w-full text-left flex items-center gap-2 px-2 py-1.5 rounded border border-border bg-bg-2 hover:bg-[var(--hover)] text-[11px] mono">
            <Icon size={11} className="text-fg-4 shrink-0" />
            <div className="flex-1 min-w-0">
              <div className="text-fg-2 truncate">{r.id}</div>
              <div className="text-fg-4 truncate">{r.label}</div>
            </div>
            <Badge tone={typeTone[r.type] || 'slate'}>{r.type}</Badge>
          </button>
        );
      })}
      <div className="text-[10px] mono text-fg-4 leading-snug">click to jump to page</div>
    </div>
  );
};

// ---- Workbench Tab Content ----
const WorkbenchModePanel = ({ modeKey, lang }) => {
  const { useState } = React;
  const t = tFor(lang);
  const mode = WB_MODES.find(m => m.key === modeKey) || WB_MODES[0];
  const [messages, setMessages] = useState([
    {
      role: 'system',
      actor: 'workbench',
      content: `${mode.label} mode active. ${mode.hint}`,
      ts: new Date().toLocaleTimeString(),
      mode: modeKey,
    },
  ]);
  const [loading, setLoading] = useState(false);
  const [refs, setRefs] = useState([]);
  const [railOpen, setRailOpen] = useState(true);

  const [targetPrincipal, setTargetPrincipal] = useState('any');

  const handleSend = async (text) => {
    const userMsg = {
      role: 'user',
      actor: 'operator',
      content: text,
      ts: new Date().toLocaleTimeString(),
      mode: modeKey,
    };
    setMessages(prev => [...prev, userMsg]);
    setLoading(true);

    try {
      // Sprint 18 W2 — POST /api/v1/workbench/chat unified entrypoint.
      const apiResp = await window.cc.api.workbench.chat(modeKey, text);
      const resp = {
        role: apiResp.role || 'assistant',
        actor: apiResp.actor || 'Workbench',
        content: apiResp.content || '',
        ts: new Date().toLocaleTimeString(),
        mode: apiResp.mode || modeKey,
        traceRef: apiResp.trace_ref,
        provenance: apiResp.provenance,
        actions: apiResp.actions || [],
      };
      setMessages(prev => [...prev, resp]);
      if (resp.traceRef) {
        setRefs(prev => [{ type: 'trace', id: resp.traceRef, label: 'execution trace' }, ...prev].slice(0, 8));
      }
    } catch (err) {
      // On API error, surface the error message in the chat flow rather than
      // silently dropping. Network/auth issues should be visible.
      setMessages(prev => [...prev, {
        role: 'assistant',
        actor: 'Workbench',
        content: 'Error: ' + ((err && err.message) || String(err)),
        ts: new Date().toLocaleTimeString(),
        mode: modeKey,
      }]);
    } finally {
      setLoading(false);
    }
  };

  const modePills = WB_MODES.map(m => ({
    key: m.key,
    label: m.label,
    active: m.key === modeKey,
    onClick: () => {},  // switching handled by parent tab
  }));

  const targetSel = (
    <select value={targetPrincipal} onChange={e => setTargetPrincipal(e.target.value)}
      className="h-6 px-1.5 bg-bg-3 border border-border rounded text-[11px] mono text-fg outline-none focus:border-[var(--accent)]">
      <option value="any">any principal</option>
      <option value="internal.audit.governor">Audit Agent</option>
      <option value="internal.pipeline.5stage">Team Pipeline</option>
      <option value="internal.evolution.daemon">Evolution Daemon</option>
      <option value="internal.security.scanner">Skill Hub Scanner</option>
    </select>
  );

  return (
    <div className="flex gap-3 h-full" style={{ minHeight: 500 }}>
      {/* Main transcript area */}
      <div className="flex-1 min-w-0 flex flex-col border border-border rounded-lg overflow-hidden bg-bg-2">
        {/* Mode header */}
        <div className="flex items-center gap-3 px-4 py-2.5 border-b border-border bg-bg-3 shrink-0">
          {React.createElement(mode.icon, { size: 14, className: 'text-accent shrink-0' })}
          <span className="text-[13px] font-semibold">{mode.label}</span>
          <span className="text-[11px] mono text-fg-4 flex-1">{mode.hint}</span>
          <button onClick={() => setRailOpen(v => !v)} title="Toggle references rail"
            className="text-fg-4 hover:text-fg h-6 w-6 flex items-center justify-center rounded hover:bg-[var(--hover)]">
            <I.Layers size={13} />
          </button>
          <button onClick={() => setMessages([messages[0]])}
            className="text-[10px] mono text-fg-4 hover:text-fg px-1.5 py-0.5 rounded hover:bg-[var(--hover)]">
            clear
          </button>
        </div>
        <div className="flex-1 min-h-0">
          <TranscriptPane
            messages={messages}
            onSend={handleSend}
            placeholder={mode.placeholder}
            targetSelector={targetSel}
            modePills={null}
            loading={loading}
          />
        </div>
      </div>

      {/* Right rail */}
      {railOpen && (
        <WorkbenchRightRail refs={refs} onClear={() => setRefs([])} />
      )}
    </div>
  );
};

// ---- Workbench Page Root ----
const WorkbenchPage = ({ lang }) => {
  const { useState } = React;
  const t = tFor(lang);
  const [tab, setTab] = useState('diagnose');

  const tabs = WB_MODES.map(m => ({ value: m.key, label: t(`workbench.${m.key.replace('-', '_')}`) || m.label }));

  return (
    <div className="space-y-4">
      {/* Safety notice banner */}
      <div className="flex items-center gap-3 px-3 py-2 rounded-lg border border-amber-500/30 bg-amber-500/5 text-[12px]">
        <I.Shield size={14} className="text-amber-400 shrink-0" />
        <span className="text-amber-300/80">
          {t('workbench.safety_notice') || 'Workbench is read-only by default. Promote messages to /reviews to execute for real. Every interaction creates a Provenance record in the Audit log.'}
        </span>
      </div>

      <Tabs items={tabs} value={tab} onChange={setTab} />

      {WB_MODES.map(m => tab === m.key && (
        <WorkbenchModePanel key={m.key} modeKey={m.key} lang={lang} />
      ))}
    </div>
  );
};

Object.assign(window, { WorkbenchPage });
