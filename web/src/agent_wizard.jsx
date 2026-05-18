// Agent Create Wizard — Sprint 11 Admin UI Wave 1 Lane A1
// Components: EmojiAvatarPicker (~40 LOC), WizardStepper (~60 LOC), CreateAgentWizard (Sheet, 5 steps)
// Spec: docs/superpowers/specs/2026-04-20-cyberclaw-admin-console-ui-design.md §C1 §3.2

const { useState: uS_W, useEffect: uE_W, useMemo: uM_W } = React;

// ---- Emoji Avatar Picker (~40 LOC) ----
// Grid of curated platform-themed emojis. Triggers via Button → inline popover.
const AVATAR_EMOJIS = [
  '🤖','🧠','⚙️','🛡','🔍','📊','🚀','⚖','◆','✦',
  '🌐','🔧','💡','📡','🗂','🔐','🎯','🧩','⚡','🦾',
  '🏗','📝','🔬','🌊','🎛','🛠','🧪','📌','🔮','💎',
  '🦅','🌿','🏛','🎮','📈','🗃','🧭','🔑','🌀','⭐',
];

const EmojiAvatarPicker = ({ value, onChange }) => {
  const [open, setOpen] = uS_W(false);
  const ref = React.useRef();
  uE_W(() => {
    if (!open) return;
    const handler = (e) => { if (ref.current && !ref.current.contains(e.target)) setOpen(false); };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [open]);
  return (
    <div className="relative inline-block" ref={ref}>
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="w-14 h-14 rounded-xl border-2 border-[var(--border-strong)] bg-[var(--bg-3)] hover:border-accent transition-colors flex items-center justify-center text-[28px] focus-ring"
        title="Pick avatar emoji"
      >
        {value || '🤖'}
      </button>
      {open && (
        <div className="absolute left-0 top-16 z-50 bg-[var(--bg-2)] border border-[var(--border-strong)] rounded-xl shadow-2xl p-2 w-[232px] anim-fade-in">
          <div className="text-[10px] uppercase tracking-wider text-fg-4 px-1 pb-1.5 mono">avatar</div>
          <div className="grid grid-cols-8 gap-0.5">
            {AVATAR_EMOJIS.map((em) => (
              <button
                key={em}
                type="button"
                onClick={() => { onChange(em); setOpen(false); }}
                className={`w-7 h-7 rounded-md text-[16px] flex items-center justify-center hover:bg-[var(--bg-3)] transition-colors ${value === em ? 'bg-accent/20 ring-1 ring-accent' : ''}`}
              >
                {em}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
};

// ---- Wizard Stepper (~60 LOC) ----
// Top step-indicator bar + prev/next controls. Accepts step definitions and current index.
const WizardStepper = ({ steps, current, onNext, onPrev, onDraftSave, lang, submitting }) => {
  const t = tFor(lang || 'en');
  const isLast = current === steps.length - 1;
  const isFirst = current === 0;
  return (
    <div>
      {/* Step indicator */}
      <div className="flex items-center gap-0 mb-6">
        {steps.map((step, idx) => {
          const done = idx < current;
          const active = idx === current;
          return (
            <React.Fragment key={step.key}>
              <div className="flex flex-col items-center gap-1 min-w-0" style={{ flex: '0 0 auto' }}>
                <div className={`w-7 h-7 rounded-full flex items-center justify-center text-[12px] font-semibold transition-colors border
                  ${done ? 'bg-accent border-accent text-white' : active ? 'bg-accent/15 border-accent text-accent' : 'bg-[var(--bg-3)] border-[var(--border-strong)] text-fg-4'}`}
                >
                  {done ? <I.Check size={12} /> : idx + 1}
                </div>
                <div className={`text-[10px] whitespace-nowrap mono uppercase tracking-wider ${active ? 'text-fg' : done ? 'text-fg-3' : 'text-fg-4'}`}>
                  {step.label}
                </div>
              </div>
              {idx < steps.length - 1 && (
                <div className={`flex-1 h-px mx-1 mt-[-14px] transition-colors ${done ? 'bg-accent/60' : 'bg-[var(--border)]'}`} style={{ minWidth: 12 }} />
              )}
            </React.Fragment>
          );
        })}
      </div>

      {/* Nav controls */}
      <div className="flex items-center justify-between pt-4 border-t border-[var(--border)] mt-4">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="sm" onClick={onDraftSave} type="button">
            <I.Save size={12} /> {t('agents.wizard.save_draft')}
          </Button>
        </div>
        <div className="flex items-center gap-2">
          {!isFirst && (
            <Button variant="outline" size="sm" onClick={onPrev} type="button">
              <I.ArrowLeft size={12} /> {t('agents.wizard.back')}
            </Button>
          )}
          <Button size="sm" onClick={onNext} type="button" disabled={submitting}>
            {submitting
              ? <><I.Loader size={12} className="animate-spin" /> {t('create')}</>
              : isLast
                ? <><I.Zap size={12} /> {t('agents.wizard.launch')}</>
                : <>{t('agents.wizard.next')} <I.ArrowRight size={12} /></>
            }
          </Button>
        </div>
      </div>
    </div>
  );
};

// ---- Built-in Agent Cards ----
const BUILTIN_AGENTS = [
  { id: 'internal.audit.governor',    emoji: '⚖',  name: 'Audit Agent',       mission: 'Monitors approvals, self-evolves governance rules.',       role: 'governance',  level: 'L4', kpi: { label: 'approvals/mo', value: '—' } },
  { id: 'internal.pipeline.5stage',   emoji: '◆',  name: 'Team Pipeline',     mission: 'Orchestrates plan → prd → exec → verify → fix pipeline.',  role: 'orchestrator',level: 'L3', kpi: { label: 'pipelines/wk', value: '—' } },
  { id: 'internal.evolution.daemon',  emoji: '✦',  name: 'Evolution Daemon',  mission: 'Drives skill mutation, fitness scoring, and acceptance.',   role: 'evolution',   level: 'L3', kpi: { label: 'mutations/mo', value: '—' } },
  { id: 'internal.security.scanner',  emoji: '🛡', name: 'Skill Hub Scanner', mission: 'Detects CodeInjection, CommandInjection, InvisibleUnicode.', role: 'security',    level: 'L2', kpi: { label: 'scans/day',    value: '—' } },
];

const BuiltinAgentCard = ({ agent, onView, lang }) => {
  const t = tFor(lang || 'en');
  return (
    <div
      className="flex items-start gap-3 p-4 bg-[var(--bg-2)] border border-[var(--border)] rounded-lg hover:border-accent/40 transition-colors cursor-pointer"
      onClick={() => onView && onView(agent)}
    >
      <div className="w-10 h-10 rounded-lg bg-[var(--bg-3)] border border-[var(--border-strong)] flex items-center justify-center text-[22px] shrink-0">
        {agent.emoji}
      </div>
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2 flex-wrap">
          <span className="text-[13px] font-medium text-fg">{agent.name}</span>
          <Badge tone="violet">INTERNAL</Badge>
          <Badge tone="slate">{agent.role}</Badge>
          <Badge tone="cyan">{agent.level}</Badge>
        </div>
        <div className="mono text-[11px] text-fg-4 mt-0.5">{agent.id}</div>
        <div className="text-[12px] text-fg-3 mt-1">{agent.mission}</div>
        <div className="mt-2 flex items-center gap-1">
          <span className="text-[11px] mono text-fg-4">{agent.kpi.label}:</span>
          <span className="text-[12px] mono text-fg font-medium">{agent.kpi.value}</span>
        </div>
      </div>
    </div>
  );
};

// ---- Role Templates ----
const ROLE_TEMPLATES = [
  { id: 'release.gate.approver',    label: 'Release Gate Approver',   desc: 'Blocks deploys pending policy checks.',        skills: ['code-reviewer','verifier'], level: 'L3' },
  { id: 'treasury.signer.L2',       label: 'Treasury Signer L2',      desc: 'Dual-approval for financial operations.',      skills: ['verifier'],                  level: 'L4' },
  { id: 'dbops.migration-guard',    label: 'DB Migration Guard',      desc: 'Reviews and dry-runs schema migrations.',     skills: ['architect','verifier'],       level: 'L3' },
  { id: 'customer.service.L2',      label: 'Customer Service L2',     desc: 'Handles escalated customer issues.',           skills: ['analyst','report-agent'],    level: 'L2' },
  { id: 'security.audit.governor',  label: 'Security Audit Governor', desc: 'Full audit + policy enforcement principal.',   skills: ['verifier','critic'],          level: 'L4' },
  { id: 'support.analyst.L1',       label: 'Support Analyst L1',      desc: 'First-line support classification and triage.',skills: ['analyst'],                   level: 'L1' },
];

// ---- Deploy Mode ----
const DEPLOY_MODES = [
  { value: 'shadow',    label: 'Shadow',   desc: 'Observe only — zero production side-effects. Default.' },
  { value: 'canary',   label: 'Canary 10%', desc: 'Route 10% of eligible tasks to this agent.' },
  { value: 'active',   label: 'Active',   desc: 'Full production routing. Requires dual approval.' },
];

// ---- Create Agent Wizard ----
// 5-step Sheet wizard per spec §3.2
const PERSONA_LIST = [
  'analyst', 'architect', 'code-reviewer', 'critic', 'executor',
  'git-master', 'planner', 'verifier', 'master-agent', 'report-agent',
];
const NAME_RE_W = /^[a-z][a-z0-9-]{2,31}$/;

const WIZARD_STEPS_EN = [
  { key: 'identity',   label: 'Identity' },
  { key: 'role',       label: 'Role' },
  { key: 'skills',     label: 'Skills' },
  { key: 'guardrails', label: 'Guardrails' },
  { key: 'review',     label: 'Review' },
];

const CreateAgentWizard = ({ open, onClose, onCreate, lang }) => {
  const t = tFor(lang || 'en');
  const [step, setStep] = uS_W(0);
  const [submitting, setSubmitting] = uS_W(false);
  const [errors, setErrors] = uS_W({});

  // Step 1 — Identity
  const [avatar, setAvatar] = uS_W('🤖');
  const [name, setName] = uS_W('');
  const [displayName, setDisplayName] = uS_W('');
  const [mission, setMission] = uS_W('');

  // Step 2 — Role
  const [roleTemplate, setRoleTemplate] = uS_W('');

  // Step 3 — Skills
  const [selectedSkills, setSelectedSkills] = uS_W([]);
  const skillsRes = window.cc.data.useSkills();
  const skillsList = window.cc.data.extractList(skillsRes, 'skills', []);

  // Step 4 — Guardrails
  const [approvalPolicy, setApprovalPolicy] = uS_W('deny_by_default');
  const [budgetCap, setBudgetCap] = uS_W('');
  const [rateLimit, setRateLimit] = uS_W('');
  const [soul, setSoul] = uS_W('');

  // Step 5 — Deploy
  const [deployMode, setDeployMode] = uS_W('shadow');

  uE_W(() => {
    if (open) {
      setStep(0); setSubmitting(false); setErrors({});
      setAvatar('🤖'); setName(''); setDisplayName(''); setMission('');
      setRoleTemplate(''); setSelectedSkills([]);
      setApprovalPolicy('deny_by_default'); setBudgetCap(''); setRateLimit(''); setSoul('');
      setDeployMode('shadow');
    }
  }, [open]);

  const validateStep = (s) => {
    const e = {};
    if (s === 0) {
      if (!NAME_RE_W.test(name)) e.name = t('agents.new.name_invalid');
      if (!displayName.trim()) e.displayName = t('agents.new.display_required');
    }
    return e;
  };

  const handleNext = async () => {
    const e = validateStep(step);
    if (Object.keys(e).length) { setErrors(e); return; }
    setErrors({});
    if (step < WIZARD_STEPS_EN.length - 1) {
      setStep(step + 1);
    } else {
      // Final submit
      setSubmitting(true);
      try {
        await onCreate({
          avatar, name, display_name: displayName, mission,
          role_template: roleTemplate || null,
          default_skills: selectedSkills,
          approval_policy: approvalPolicy,
          budget_cap: budgetCap || null,
          rate_limit: rateLimit || null,
          soul: soul || null,
          deploy_mode: deployMode,
        });
      } finally {
        setSubmitting(false);
      }
    }
  };

  const handlePrev = () => { setErrors({}); setStep(Math.max(0, step - 1)); };
  const handleDraftSave = () => {
    try {
      const draft = { avatar, name, display_name: displayName, mission, role_template: roleTemplate, default_skills: selectedSkills, approval_policy: approvalPolicy, soul, deploy_mode: deployMode, _step: step };
      sessionStorage.setItem('cyberclaw.agent_wizard.draft', JSON.stringify(draft));
    } catch {}
  };

  const toggleSkill = (id) => setSelectedSkills(prev => prev.includes(id) ? prev.filter(s => s !== id) : [...prev, id]);

  const wizardSteps = WIZARD_STEPS_EN.map(s => ({ ...s, label: t('agents.wizard.step.' + s.key) || s.label }));
  const selectedRole = ROLE_TEMPLATES.find(r => r.id === roleTemplate);

  return (
    <Sheet open={open} onClose={onClose} title={t('agents.wizard.title')} subtitle="POST /api/v1/agents" width={620}>
      <div className="p-5 space-y-0">
        <WizardStepper
          steps={wizardSteps}
          current={step}
          onNext={handleNext}
          onPrev={handlePrev}
          onDraftSave={handleDraftSave}
          lang={lang}
          submitting={submitting}
        />

        <div className="mt-5 space-y-4">
          {/* ── Step 1: Identity ── */}
          {step === 0 && (
            <div className="space-y-4">
              <div className="text-[13px] font-medium text-fg">{t('agents.wizard.step.identity')}</div>
              {/* Avatar */}
              <div>
                <label className="text-[11px] text-fg-3 mb-2 block uppercase tracking-wider">{t('agents.wizard.avatar')}</label>
                <EmojiAvatarPicker value={avatar} onChange={setAvatar} />
              </div>
              {/* Name slug */}
              <div>
                <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('agents.new.name')}</label>
                <Input value={name} onChange={e => { setName(e.target.value); setErrors(prev => ({...prev, name: undefined})); }}
                  className="mono" placeholder="e.g. treasury-signer" autoFocus />
                {errors.name
                  ? <div className="text-[11px] text-rose-400 mt-1">{errors.name}</div>
                  : <div className="text-[11px] text-fg-4 mt-1 mono">{t('agents.new.name_hint')}</div>}
              </div>
              {/* Display name */}
              <div>
                <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('agents.new.display_name')}</label>
                <Input value={displayName} onChange={e => { setDisplayName(e.target.value); setErrors(prev => ({...prev, displayName: undefined})); }}
                  placeholder="Treasury Signer" />
                {errors.displayName && <div className="text-[11px] text-rose-400 mt-1">{errors.displayName}</div>}
              </div>
              {/* Mission statement */}
              <div>
                <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">
                  {t('agents.wizard.mission')} <span className="text-fg-4 normal-case">(≤200)</span>
                </label>
                <Textarea value={mission} onChange={e => setMission(e.target.value.slice(0, 200))} rows={3}
                  placeholder={t('agents.wizard.mission_placeholder')} />
                <div className="text-[10px] text-fg-4 mt-1 text-right mono">{mission.length}/200</div>
              </div>
            </div>
          )}

          {/* ── Step 2: Role ── */}
          {step === 1 && (
            <div className="space-y-3">
              <div className="text-[13px] font-medium text-fg">{t('agents.wizard.step.role')}</div>
              <div className="text-[12px] text-fg-3">{t('agents.wizard.role_hint')}</div>
              <div className="space-y-2">
                {ROLE_TEMPLATES.map(r => (
                  <button key={r.id} type="button" onClick={() => setRoleTemplate(roleTemplate === r.id ? '' : r.id)}
                    className={`w-full text-left p-3 rounded-lg border transition-colors ${roleTemplate === r.id ? 'border-accent bg-accent/10' : 'border-[var(--border)] bg-[var(--bg-3)] hover:border-[var(--border-strong)]'}`}
                  >
                    <div className="flex items-center justify-between">
                      <span className="text-[13px] font-medium text-fg">{r.label}</span>
                      <div className="flex items-center gap-1.5">
                        <Badge tone="cyan">{r.level}</Badge>
                        {roleTemplate === r.id && <I.Check size={13} className="text-accent" />}
                      </div>
                    </div>
                    <div className="text-[12px] text-fg-3 mt-0.5">{r.desc}</div>
                    <div className="flex items-center gap-1 mt-1.5 flex-wrap">
                      {r.skills.map(sk => <Badge key={sk} tone="slate">{sk}</Badge>)}
                    </div>
                  </button>
                ))}
              </div>
              {!roleTemplate && (
                <div className="text-[11px] text-fg-4 mono">{t('agents.wizard.role_skip')}</div>
              )}
            </div>
          )}

          {/* ── Step 3: Skills & Toolkit ── */}
          {step === 2 && (
            <div className="space-y-3">
              <div className="text-[13px] font-medium text-fg">{t('agents.wizard.step.skills')}</div>
              {selectedRole && (
                <div className="text-[12px] text-fg-3 bg-[var(--bg-3)] border border-[var(--border)] rounded-md px-3 py-2">
                  {t('agents.wizard.role_defaults')}: {selectedRole.skills.join(', ')}
                </div>
              )}
              <label className="text-[11px] text-fg-3 mb-1 block uppercase tracking-wider">{t('agents.wizard.installed_skills')}</label>
              {skillsList.length === 0 ? (
                <EmptyState icon={I.Brain} title={t('agents.wizard.no_skills')} subtitle={t('agents.wizard.no_skills_hint')} />
              ) : (
                <div className="max-h-[220px] overflow-auto border border-[var(--border-strong)] rounded-md bg-[var(--bg-2)] divide-y divide-[var(--border)]">
                  {skillsList.map(s => (
                    <label key={s.skill_id} className="flex items-center gap-2.5 px-3 py-2 hover:bg-[var(--hover)] cursor-pointer">
                      <input type="checkbox" checked={selectedSkills.includes(s.skill_id)} onChange={() => toggleSkill(s.skill_id)}
                        className="accent-[var(--accent)] w-3.5 h-3.5 shrink-0" />
                      <span className="mono text-[12px] text-fg">{s.name}</span>
                      <span className="text-[11px] text-fg-4 ml-auto">{s.category}</span>
                    </label>
                  ))}
                </div>
              )}
              {selectedSkills.length > 0 && (
                <div className="flex flex-wrap gap-1 mt-1">
                  {selectedSkills.map(id => {
                    const sk = skillsList.find(s => s.skill_id === id);
                    return sk ? (
                      <Badge key={id} tone="accent">{sk.name}
                        <button onClick={() => toggleSkill(id)} className="ml-1 opacity-60 hover:opacity-100">×</button>
                      </Badge>
                    ) : null;
                  })}
                </div>
              )}
            </div>
          )}

          {/* ── Step 4: Guardrails + Soul ── */}
          {step === 3 && (
            <div className="space-y-4">
              <div className="text-[13px] font-medium text-fg">{t('agents.wizard.step.guardrails')}</div>
              {/* Approval policy */}
              <div>
                <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('agents.wizard.approval_policy')}</label>
                <Select value={approvalPolicy} onChange={setApprovalPolicy}
                  options={[
                    { value: 'deny_by_default',  label: t('agents.wizard.policy.deny_by_default') },
                    { value: 'dual_approval',    label: t('agents.wizard.policy.dual_approval') },
                    { value: 'auto_low_risk',    label: t('agents.wizard.policy.auto_low_risk') },
                  ]}
                />
              </div>
              {/* Budget cap + rate limit side by side */}
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('agents.wizard.budget_cap')}</label>
                  <Input value={budgetCap} onChange={e => setBudgetCap(e.target.value)} className="mono" placeholder="e.g. $500/mo" />
                </div>
                <div>
                  <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('agents.wizard.rate_limit')}</label>
                  <Input value={rateLimit} onChange={e => setRateLimit(e.target.value)} className="mono" placeholder="e.g. 100/hr" />
                </div>
              </div>
              {/* Soul — free-form role description */}
              <div>
                <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">
                  {t('agents.soul.label')}
                  <span className="ml-1 text-fg-4 normal-case">{t('agents.soul.hint_short')}</span>
                </label>
                <Textarea value={soul} onChange={e => setSoul(e.target.value)} rows={5}
                  placeholder={t('agents.soul.placeholder')} />
                <div className="text-[11px] text-fg-3 mt-1.5 flex gap-1.5 items-start">
                  <I.Shield size={11} className="shrink-0 mt-0.5 text-fg-4" />
                  <span>{t('agents.soul.description')}</span>
                </div>
              </div>
            </div>
          )}

          {/* ── Step 5: Review & Launch ── */}
          {step === 4 && (
            <div className="space-y-4">
              <div className="text-[13px] font-medium text-fg">{t('agents.wizard.step.review')}</div>
              {/* Preview card */}
              <div className="border border-[var(--border-strong)] rounded-xl p-4 bg-[var(--bg-3)] space-y-3">
                <div className="flex items-center gap-3">
                  <div className="w-12 h-12 rounded-xl bg-[var(--bg-2)] border border-[var(--border)] flex items-center justify-center text-[26px]">{avatar}</div>
                  <div>
                    <div className="text-[15px] font-semibold text-fg">{displayName || '—'}</div>
                    <div className="mono text-[12px] text-fg-4">{name || '—'}</div>
                  </div>
                  <div className="ml-auto flex flex-col items-end gap-1">
                    {roleTemplate && <Badge tone="cyan">{ROLE_TEMPLATES.find(r => r.id === roleTemplate)?.label}</Badge>}
                    <Badge tone="slate">{approvalPolicy.replace(/_/g, ' ')}</Badge>
                  </div>
                </div>
                {mission && <div className="text-[12px] text-fg-2 border-t border-[var(--border)] pt-2">{mission}</div>}
                {soul && (
                  <div className="text-[12px] text-fg-3 border border-accent/20 bg-accent/5 rounded-md px-3 py-2">
                    <span className="text-[10px] uppercase tracking-wider text-fg-4 mono block mb-1">soul</span>
                    {soul.slice(0, 120)}{soul.length > 120 ? '…' : ''}
                  </div>
                )}
                {selectedSkills.length > 0 && (
                  <div className="flex flex-wrap gap-1 border-t border-[var(--border)] pt-2">
                    {selectedSkills.map(id => {
                      const sk = window.cc.data.extractList(window.cc.data.useSkills(), 'skills', []).find(s => s.skill_id === id);
                      return sk ? <Badge key={id} tone="accent">{sk.name}</Badge> : null;
                    })}
                  </div>
                )}
              </div>
              {/* Deploy mode */}
              <div>
                <label className="text-[11px] text-fg-3 mb-2 block uppercase tracking-wider">{t('agents.wizard.deploy_mode')}</label>
                <div className="space-y-2">
                  {DEPLOY_MODES.map(m => (
                    <button key={m.value} type="button" onClick={() => setDeployMode(m.value)}
                      className={`w-full text-left p-3 rounded-lg border transition-colors ${deployMode === m.value ? 'border-accent bg-accent/10' : 'border-[var(--border)] hover:border-[var(--border-strong)]'}`}
                    >
                      <div className="flex items-center justify-between">
                        <span className="text-[13px] font-medium text-fg">{m.label}</span>
                        <div className="flex items-center gap-2">
                          {m.value === 'shadow' && <Badge tone="slate">{t('agents.wizard.default')}</Badge>}
                          {deployMode === m.value && <I.Check size={13} className="text-accent" />}
                        </div>
                      </div>
                      <div className="text-[12px] text-fg-3 mt-0.5">{m.desc}</div>
                    </button>
                  ))}
                </div>
              </div>
              <div className="text-[11px] text-fg-3 bg-[var(--bg-3)] border border-[var(--border)] rounded-md p-2.5 flex gap-2">
                <I.Shield size={12} className="shrink-0 mt-0.5 text-fg-4" />
                <span>{t('agents.wizard.launch_hint')}</span>
              </div>
            </div>
          )}
        </div>
      </div>
    </Sheet>
  );
};

Object.assign(window, { EmojiAvatarPicker, WizardStepper, CreateAgentWizard, BuiltinAgentCard, BUILTIN_AGENTS });
