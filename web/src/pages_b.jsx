// Pages group B: Skills, Tasks, Executions + Trace

// ---- Skills ----

// 后端 installed_at 可能是 unix epoch 秒（数字）或 ISO 字符串。统一渲染为本地短日期。
function fmtSkillDate(v) {
  if (v == null) return '—';
  const d = typeof v === 'number' ? new Date(v * 1000) : new Date(v);
  if (isNaN(d.getTime())) return String(v);
  return d.toLocaleDateString();
}

const SkillSheet = ({ skill, open, onClose, onUninstall, lang }) => {
  const t = tFor(lang || 'en');
  if (!skill) return null;
  const md = `# ${skill.name}

**Category:** ${skill.category}  
**Source:** ${skill.source}  
**Installed:** ${fmtSkillDate(skill.installed_at)}

## Description

${skill.description}

## Usage

Invoke this skill by including \`${skill.name}\` in the agent's skill list, or call it directly:

\`\`\`json
{
  "skill": "${skill.name}",
  "input": { "target": "<object>", "mode": "default" }
}
\`\`\`

## Inputs

- \`target\` — the subject to operate on.
- \`mode\` — one of \`default\`, \`strict\`, \`dry_run\`.

## Outputs

Returns a structured result with \`summary\`, \`findings\`, and \`next_steps\`.

## Notes

This skill is governed by the \`ToolPermissionMatcher\` — only agents with \`trust_level >= medium\` can invoke it.`;
  return (
    <Sheet open={open} onClose={onClose} title={skill.name} subtitle={skill.skill_id} width={680}
      right={<Button variant="danger-outline" size="sm" onClick={() => onUninstall(skill)}><I.Trash size={12} /> {t('uninstall')}</Button>}
    >
      <div className="p-5 space-y-4">
        <div className="flex flex-wrap gap-2">
          <Badge tone="slate">{skill.category}</Badge>
          <Badge tone={skill.source === 'builtin' ? 'accent' : skill.source === 'hub' ? 'violet' : 'cyan'}>{skill.source}</Badge>
          <Badge>{t('installed_at') !== 'installed_at' ? t('installed_at') : 'installed'} {fmtSkillDate(skill.installed_at)}</Badge>
        </div>
        <div className="bg-bg-3 border border-border rounded-md p-4 mono text-[12px] whitespace-pre-wrap leading-relaxed text-fg-2">{md}</div>
      </div>
    </Sheet>
  );
};

const InstallDialog = ({ open, onClose, onInstall, lang }) => {
  const t = tFor(lang || 'en');
  const [name, setName] = useState('');
  const [source, setSource] = useState('hub');
  return (
    <Dialog open={open} onClose={onClose} title={t('install_skill')} subtitle="POST /skills/install" width={460}
      footer={<><Button variant="ghost" onClick={onClose}>{t('cancel')}</Button><Button onClick={() => onInstall({ name, source })} disabled={!name}><I.Download size={13} /> {t('install')}</Button></>}>
      <div className="space-y-3">
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('skill_name')}</label>
          <Input value={name} onChange={(e) => setName(e.target.value)} className="mono" placeholder="e.g. db-migration-writer" />
        </div>
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('source')}</label>
          <Select value={source} onChange={setSource} options={[{ value: 'hub', label: t('source_hub') }, { value: 'local', label: t('source_local') }, { value: 'git', label: t('source_git') }]} />
        </div>
        <div className="text-[11px] text-fg-3 bg-bg-3 border border-border rounded-md p-2.5">
          {t('install_warn')}
        </div>
      </div>
    </Dialog>
  );
};

const CATEGORIES = ['Development', 'Research', 'Productivity', 'Creative', 'Agents', 'Other'];

// NewSkillDialog must be declared BEFORE SkillsPage references it in JSX.
// `const` declarations do NOT hoist, and Babel's in-browser transform
// honors JS scoping rules — referencing `NewSkillDialog` inside SkillsPage's
// render body while it was declared later in the module triggered the
// "Element type is invalid: got undefined" crash on first load (B03).
const NewSkillDialog = ({ open, onClose, onCreate, lang }) => {
  const t = tFor(lang);
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [methodology, setMethodology] = useState('');
  const [triggerCsv, setTriggerCsv] = useState('');
  const [err, setErr] = useState({});
  const NAME_RE = /^[a-z][a-z0-9-]{2,31}$/;
  useEffect(() => {
    if (!open) {
      setName(''); setDescription(''); setMethodology(''); setTriggerCsv(''); setErr({});
    }
  }, [open]);
  const onSubmit = () => {
    const e = {};
    if (!NAME_RE.test(name)) e.name = t('skill.new.name_invalid');
    if (!description.trim()) e.description = t('skill.new.description_required');
    setErr(e);
    if (Object.keys(e).length) return;
    const trigger_examples = triggerCsv.split(/\n|,/).map(s => s.trim()).filter(Boolean);
    onCreate({ name: name.trim(), description: description.trim(), methodology: methodology.trim() || null, trigger_examples });
  };
  return (
    <Dialog open={open} onClose={onClose} title={t('create_skill')} subtitle="POST /api/v1/skills/create" width={580}
      footer={<><Button variant="secondary" onClick={onClose}>{t('cancel')}</Button><Button onClick={onSubmit}><I.Sparkles size={12} /> {t('skill.new.submit')}</Button></>}
    >
      <div className="space-y-3">
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('skill.new.name')}</label>
          <Input value={name} onChange={(e) => setName(e.target.value.toLowerCase())} placeholder="json-validator" className="mono" />
          {err.name
            ? <div className="text-[11px] text-rose-400 mt-1">{err.name}</div>
            : <div className="text-[11px] text-fg-4 mt-1 mono">{t('skill.new.name_hint')}</div>}
        </div>
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('skill.new.description')}</label>
          <textarea value={description} onChange={(e) => setDescription(e.target.value)} rows={2}
            placeholder={t('skill.new.description_placeholder')}
            className="w-full px-2 py-1.5 rounded-md border border-border bg-bg-2 text-[12px] text-fg focus:outline-none focus:border-[var(--border-strong)]" />
          {err.description && <div className="text-[11px] text-rose-400 mt-1">{err.description}</div>}
        </div>
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('skill.new.methodology')}</label>
          <textarea value={methodology} onChange={(e) => setMethodology(e.target.value)} rows={4}
            placeholder={t('skill.new.methodology_placeholder')}
            className="w-full px-2 py-1.5 rounded-md border border-border bg-bg-2 text-[12px] text-fg focus:outline-none focus:border-[var(--border-strong)]" />
        </div>
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('skill.new.triggers')}</label>
          <textarea value={triggerCsv} onChange={(e) => setTriggerCsv(e.target.value)} rows={3}
            placeholder={t('skill.new.triggers_placeholder')}
            className="w-full px-2 py-1.5 rounded-md border border-border bg-bg-2 text-[12px] text-fg focus:outline-none focus:border-[var(--border-strong)]" />
        </div>
      </div>
    </Dialog>
  );
};

// ---- C4: Skill Poison Verdict tab ----
// Mock data — TODO: wire GET /api/v1/skills/poison-verdict
const MOCK_POISON_VERDICTS = [
  {
    skill_id: 'sk_exfil_01',
    name: 'data-exporter-v2',
    submitter: 'op_unknown',
    submitted_at: new Date(Date.now() - 3600000 * 2).toISOString(),
    verdict: 'CommandInjection',
    trust_level: 'untrusted',
    signature_valid: false,
    findings: [
      '• Line 47: `subprocess.run(user_input, shell=True)` — unsanitized shell execution',
      '• Line 112: base64-encoded payload decoded at runtime — obfuscation pattern',
    ],
    scan_report: { scanner_version: '1.4.2', patterns_matched: 3, risk_score: 0.94 },
  },
  {
    skill_id: 'sk_unicode_02',
    name: 'text-processor',
    submitter: 'op_alice',
    submitted_at: new Date(Date.now() - 3600000 * 8).toISOString(),
    verdict: 'InvisibleUnicode',
    trust_level: 'low',
    signature_valid: false,
    findings: [
      '• Multiple U+202A–U+202E bidirectional override characters found in SKILL.md',
      '• Potential text direction spoofing for social engineering',
    ],
    scan_report: { scanner_version: '1.4.2', patterns_matched: 1, risk_score: 0.61 },
  },
  {
    skill_id: 'sk_inject_03',
    name: 'sql-helper',
    submitter: 'op_bob',
    submitted_at: new Date(Date.now() - 3600000 * 24).toISOString(),
    verdict: 'CodeInjection',
    trust_level: 'low',
    signature_valid: true,
    findings: [
      '• Methodology contains raw SQL template with unquoted string interpolation',
      '• No parameterized query pattern detected',
    ],
    scan_report: { scanner_version: '1.4.2', patterns_matched: 2, risk_score: 0.73 },
  },
];

const VERDICT_TONE = {
  CommandInjection: 'rose',
  CodeInjection: 'amber',
  InvisibleUnicode: 'violet',
  Pass: 'emerald',
  Warn: 'amber',
  Quarantine: 'rose',
};
const TRUST_TONE = { untrusted: 'rose', low: 'amber', medium: 'slate', high: 'emerald' };

const SkillPoisonVerdictPane = ({ lang }) => {
  const t = tFor(lang || 'en');
  const [selected, setSelected] = useState(null);
  const [detailOpen, setDetailOpen] = useState(false);
  const toast = useToast();
  // Sprint 18 W2 — wired to GET /api/v1/skills/poison-verdicts.
  const verdictsRes = window.cc.data.useSkillPoisonVerdicts();
  const verdicts = verdictsRes.data?.verdicts ?? (verdictsRes.error ? MOCK_POISON_VERDICTS : []);

  const handleDecide = (skill, action) => {
    toast.toast({
      title: action === 'reject'
        ? (t('skills.poison.rejected') || 'Skill rejected')
        : (t('skills.poison.override_requires_audit') || 'Override requires Audit Agent high-risk entry'),
      desc: `${skill.name} → ${action === 'reject' ? '/audit log' : '/reviews'}`,
      tone: action === 'reject' ? 'error' : 'error',
    });
    // TODO: POST /api/v1/skills/poison-verdict/{id}/decide
  };

  return (
    <div className="space-y-4">
      {/* KPI row */}
      <div className="grid grid-cols-3 gap-3">
        <StatCard label={t('skills.poison.kpi_quarantined') || 'Quarantined'} value={verdicts.length} tone="rose" />
        <StatCard label={t('skills.poison.kpi_sig_invalid') || 'Sig invalid'} value={verdicts.filter(v => !v.signature_valid).length} tone="amber" />
        <StatCard label={t('skills.poison.kpi_untrusted') || 'Untrusted level'} value={verdicts.filter(v => v.trust_level === 'untrusted').length} tone="rose" />
      </div>

      <div className="space-y-3">
        {verdicts.map(v => (
          <Card key={v.skill_id} className="overflow-hidden border-l-2 border-rose-600/60">
            {/* Header row */}
            <div className="flex items-center justify-between px-4 py-3 bg-bg-3/40 border-b border-border">
              <div className="flex items-center gap-3 flex-wrap">
                <span className="mono text-[13px] font-medium text-fg">{v.name}</span>
                <Badge tone={VERDICT_TONE[v.verdict] || 'rose'}>{v.verdict}</Badge>
                <Badge tone={TRUST_TONE[v.trust_level] || 'slate'}>
                  {t('skills.poison.trust') || 'trust'}: {v.trust_level}
                </Badge>
                {v.signature_valid
                  ? <Badge tone="emerald"><I.Check size={10} /> {t('skills.poison.sig_valid') || 'sig valid'}</Badge>
                  : <Badge tone="rose"><I.XCircle size={10} /> {t('skills.poison.sig_invalid') || 'sig invalid'}</Badge>}
              </div>
              <span className="mono text-[11px] text-fg-3">{relTime(v.submitted_at)}</span>
            </div>

            {/* Findings */}
            <div className="px-4 py-3 space-y-1">
              <div className="text-[10px] mono uppercase tracking-wider text-fg-4 mb-2">
                {t('skills.poison.scanner_findings') || 'Scanner findings'}
                <span className="ml-2 text-fg-3">· risk score: <span className="text-rose-400">{v.scan_report.risk_score}</span></span>
              </div>
              {v.findings.map((f, fi) => (
                <div key={fi} className="mono text-[11.5px] text-rose-300/90 bg-rose-500/5 border border-rose-500/15 rounded px-2.5 py-1.5">{f}</div>
              ))}
            </div>

            {/* Actions footer */}
            <div className="px-4 py-3 border-t border-border flex items-center justify-between">
              <div className="flex items-center gap-1.5 text-[11px] mono text-fg-4">
                <span>submitter: {v.submitter}</span>
                <span className="text-fg-5">·</span>
                <span>scanner v{v.scan_report.scanner_version}</span>
                <span className="text-fg-5">·</span>
                <span>{v.scan_report.patterns_matched} patterns</span>
              </div>
              <div className="flex items-center gap-2">
                <Button variant="ghost" size="xs" onClick={() => { setSelected(v); setDetailOpen(true); }}>
                  <I.Eye size={11} /> {t('skills.poison.inspect_diff') || 'Inspect diff'}
                </Button>
                <Button variant="danger-outline" size="xs" onClick={() => handleDecide(v, 'reject')}>
                  <I.XCircle size={11} /> {t('skills.poison.reject') || 'Reject'}
                </Button>
                <Button variant="outline" size="xs" onClick={() => handleDecide(v, 'override')}>
                  <I.Shield size={11} /> {t('skills.poison.override') || 'Override (with audit)'}
                </Button>
              </div>
            </div>
          </Card>
        ))}
        {!verdicts.length && (
          <EmptyState icon={I.Shield}
            title={t('skills.poison.empty') || 'No quarantined skills'}
            subtitle={t('skills.poison.empty_sub') || 'All installed skills passed the scanner.'}
          />
        )}
      </div>

      {/* Full scan report sheet */}
      <Sheet open={detailOpen} onClose={() => { setDetailOpen(false); setSelected(null); }}
        title={selected ? selected.name : ''}
        subtitle={selected ? `${selected.verdict} · ${selected.skill_id}` : ''}
        width={620}>
        {selected && (
          <div className="p-5 space-y-4">
            <div className="flex items-center gap-2 flex-wrap">
              <Badge tone={VERDICT_TONE[selected.verdict] || 'rose'}>{selected.verdict}</Badge>
              <Badge tone={TRUST_TONE[selected.trust_level] || 'slate'}>trust: {selected.trust_level}</Badge>
              {selected.signature_valid
                ? <Badge tone="emerald">sig valid</Badge>
                : <Badge tone="rose">sig invalid</Badge>}
            </div>
            <div>
              <div className="text-[11px] text-fg-3 uppercase tracking-wider mb-1.5">{t('skills.poison.scanner_findings') || 'Scanner findings'}</div>
              <div className="space-y-1.5">
                {selected.findings.map((f, fi) => (
                  <div key={fi} className="mono text-[11.5px] text-rose-300 bg-rose-500/5 border border-rose-500/15 rounded px-3 py-2">{f}</div>
                ))}
              </div>
            </div>
            <div>
              <div className="text-[11px] text-fg-3 uppercase tracking-wider mb-1.5">{t('skills.poison.full_report') || 'Full scan report'}</div>
              <JsonViewer value={selected.scan_report} />
            </div>
            <div className="grid grid-cols-2 gap-3">
              <InfoRow label="skill_id" value={selected.skill_id} mono />
              <InfoRow label="submitter" value={selected.submitter} mono />
              <InfoRow label="submitted_at" value={selected.submitted_at} mono />
              <InfoRow label="risk_score" value={String(selected.scan_report.risk_score)} mono />
            </div>
            <div className="text-[11px] text-fg-4 mono">
              {t('skills.poison.todo_endpoint') || '// TODO: GET /api/v1/skills/poison-verdict · POST /api/v1/skills/poison-verdict/{id}/decide'}
            </div>
          </div>
        )}
      </Sheet>
    </div>
  );
};

const SkillsPage = ({ lang }) => {
  const t = tFor(lang);
  const [skillsTab, setSkillsTab] = useState('skills');
  const [selected, setSelected] = useState(null);
  const [installing, setInstalling] = useState(false);
  const [creating, setCreating] = useState(false);
  const [q, setQ] = useState('');
  const skillsRes = window.cc.data.useSkills();
  const { loading, error, data } = skillsRes;
  const toast = useToast();
  const skills = window.cc.data.extractList(skillsRes, 'skills', MOCK.skills);
  // Sprint 18 W2 — poison-tab badge count from API.
  const poisonRes = window.cc.data.useSkillPoisonVerdicts();
  const poisonCount = poisonRes.data?.verdicts?.length ?? (poisonRes.error ? MOCK_POISON_VERDICTS.length : 0);
  const groups = useMemo(() => {
    const filtered = skills.filter(s => !q || (s.name || '').includes(q) || (s.description || '').toLowerCase().includes(q.toLowerCase()));
    return CATEGORIES.map(c => ({ cat: c, items: filtered.filter(s => s.category === c) })).filter(g => g.items.length);
  }, [q, skills]);

  return (
    <div className="space-y-4">
      <Tabs value={skillsTab} onChange={setSkillsTab} items={[
        { value: 'skills', label: t('skills.tab_skills') || 'Skills' },
        // 重命名 "Poison Verdict" → "Quarantined"，对运营更直白；count=0 时不显
        // 示徽标（避免 "Poison Verdict0" 这种字符贴脸）。
        {
          value: 'poison',
          label: t('skills.tab_poison') || 'Quarantined',
          count: poisonCount > 0 ? poisonCount : undefined,
        },
      ]} />

      {skillsTab === 'skills' && (
        <div className="space-y-4">
          <PageToolbar
            left={
              <div className="relative">
                <I.Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-fg-4" />
                <Input value={q} onChange={(e) => setQ(e.target.value)} className="pl-7 w-64" placeholder={t('search_skills')} />
              </div>
            }
            right={<div className="flex items-center gap-2">
              <Button size="sm" variant="secondary" onClick={() => setCreating(true)}><I.Sparkles size={12} /> {t('create_skill')}</Button>
              <Button size="sm" onClick={() => setInstalling(true)}><I.Plus size={12} /> {t('install_hub')}</Button>
            </div>}
          />

          {groups.map(g => (
            <div key={g.cat}>
              <div className="flex items-center gap-2 mb-2">
                <h3 className="text-[12px] font-semibold text-fg uppercase tracking-wider">{t('cat_' + g.cat) || g.cat}</h3>
                <span className="text-[11px] text-fg-4 mono">{g.items.length}</span>
                <div className="flex-1 h-px bg-border ml-1" />
              </div>
              <div className="grid grid-cols-3 gap-3">
                {g.items.map(s => (
                  <button key={s.skill_id} onClick={() => setSelected(s)} className="text-left">
                    <Card className="p-4 hover:border-[var(--border-strong)] transition-colors h-full">
                      <div className="flex items-start justify-between gap-2">
                        <div className="mono text-[13px] text-fg font-medium">{s.name}</div>
                        <Badge tone={s.source === 'builtin' ? 'accent' : s.source === 'hub' ? 'violet' : 'cyan'}>{s.source}</Badge>
                      </div>
                      <div className="text-[12px] text-fg-2 mt-2 line-clamp-2 leading-relaxed">{s.description}</div>
                      <div className="mt-3 pt-3 border-t border-border flex items-center justify-between text-[11px] text-fg-4 mono">
                        <span>{s.skill_id}</span>
                        <span>{fmtSkillDate(s.installed_at)}</span>
                      </div>
                    </Card>
                  </button>
                ))}
              </div>
            </div>
          ))}
          {error && !skills.length && <ErrorBanner message={error} onRetry={() => window.cc.data.invalidate('skills')} />}
          {loading && !skills.length && <SkeletonRows rows={6} cols={3} />}
          {!loading && !groups.length && <EmptyState icon={I.Brain} title={t('no_skills')} subtitle={t('no_skills_sub')} action={<Button size="sm" onClick={() => setInstalling(true)}><I.Plus size={12} /> {t('install')}</Button>} />}
        </div>
      )}

      {skillsTab === 'poison' && <SkillPoisonVerdictPane lang={lang} />}

      <SkillSheet skill={selected} open={!!selected} lang={lang} onClose={() => setSelected(null)}
        onUninstall={async (s) => {
          setSelected(null);
          try {
            await window.cc.api.skills.uninstall(s.skill_id);
            window.cc.data.invalidate('skills');
            toast.toast({ title: t('uninstalled'), desc: `${s.name} ${t('uninstalled_desc')}`, tone: 'success' });
          } catch (e) {
            toast.toast({ title: t('uninstalled'), desc: `(offline) ${s.name}`, tone: 'error' });
          }
        }}
      />
      <InstallDialog open={installing} lang={lang} onClose={() => setInstalling(false)}
        onInstall={async (p) => {
          setInstalling(false);
          try {
            const res = await window.cc.api.skills.install(p.name, p.source);
            window.cc.data.invalidate('skills');
            const id = (res && (res.skill_id || res.id)) || 'sk_' + Math.random().toString(36).slice(2, 7);
            toast.toast({ title: t('skill_installed'), desc: `${p.name} (${id})`, tone: 'success' });
          } catch (e) {
            toast.toast({ title: t('skill_installed'), desc: `(offline) ${p.name}`, tone: 'error' });
          }
        }}
      />
      <NewSkillDialog open={creating} lang={lang} onClose={() => setCreating(false)}
        onCreate={async (payload) => {
          setCreating(false);
          try {
            const res = await window.cc.api.skills.create(payload);
            window.cc.data.invalidate('skills');
            const id = (res && (res.skill_id || res.id)) || 'sk_' + payload.name;
            toast.toast({ title: t('skill_created'), desc: `${payload.name} (${id})`, tone: 'success' });
          } catch (e) {
            toast.toast({ title: t('skill_created'), desc: `(offline) ${payload.name}`, tone: 'error' });
          }
        }}
      />
    </div>
  );
};

// ---- Tasks ----
const TasksPage = ({ lang, role }) => {
  const t = tFor(lang);
  const [tab, setTab] = useState('all');
  const [selected, setSelected] = useState(null);
  const [creating, setCreating] = useState(false);
  const toast = useToast();
  const tasksRes = window.cc.data.useTasks('all');
  const { loading, error, data } = tasksRes;
  const tasks = window.cc.data.extractList(tasksRes, 'tasks:all', MOCK.tasks);
  const counts = { all: tasks.length, pending: tasks.filter(t => t.status === 'pending').length, running: tasks.filter(t => t.status === 'running').length, done: tasks.filter(t => t.status === 'done').length, failed: tasks.filter(t => t.status === 'failed').length, cancelled: tasks.filter(t => t.status === 'cancelled').length };
  const filtered = tab === 'all' ? tasks : tasks.filter(t => t.status === tab);
  return (
    <div className="space-y-4">
      <Card className="p-4 bg-accent-soft border border-[var(--accent)]/30">
        <div className="flex items-start gap-3 mb-4">
          <div className="text-accent mt-0.5 shrink-0"><I.List size={16} /></div>
          <div className="text-[12px] text-fg-2">
            <div className="font-semibold text-fg mb-1">{t('tasks_intro_title')}</div>
            <div className="text-[12px] leading-relaxed text-fg-3">{t('tasks_intro_body')}</div>
          </div>
        </div>
        <div className="border-t border-[var(--accent)]/20 pt-3">
          <div className="text-[10px] uppercase tracking-wider text-fg-4 mono mb-2">{t('tasks.intro.usecase_title')}</div>
          <div className="grid grid-cols-4 gap-2">
            {TASK_TEMPLATES.map(tmpl => {
              const UseCaseIcon = TEMPLATE_ICONS[tmpl.key] || I.List;
              const tone = TEMPLATE_TONES[tmpl.key] || 'default';
              const exampleKey = 'tasks.intro.' + tmpl.key + '_example';
              const toneClasses = {
                rose:    'border-rose-500/30 hover:border-rose-500/60 hover:bg-rose-500/8 text-fg-3 hover:text-rose-400',
                accent:  'border-[var(--accent)]/30 hover:border-[var(--accent)]/60 hover:bg-accent-soft/80 text-fg-3 hover:text-accent',
                emerald: 'border-emerald-500/30 hover:border-emerald-500/60 hover:bg-emerald-500/8 text-fg-3 hover:text-emerald-400',
                violet:  'border-violet-500/30 hover:border-violet-500/60 hover:bg-violet-500/8 text-fg-3 hover:text-violet-400',
              };
              const iconClasses = {
                rose: 'text-rose-400', accent: 'text-accent', emerald: 'text-emerald-400', violet: 'text-violet-400',
              };
              return (
                <button key={tmpl.key}
                  onClick={() => { setCreating(true); setTimeout(() => { window.__ccTaskTemplate = tmpl.key; window.dispatchEvent(new CustomEvent('cc:preselect-template', { detail: tmpl.key })); }, 50); }}
                  className={`text-left p-3 rounded-md border bg-bg-3/60 transition-all duration-150 ${toneClasses[tone]}`}
                >
                  <div className={`mb-1.5 ${iconClasses[tone]}`}><UseCaseIcon size={14} /></div>
                  <div className="text-[11px] font-semibold mb-1">{t('tasks.template.' + tmpl.key)}</div>
                  <div className="text-[10px] leading-relaxed opacity-70 line-clamp-2">{t(exampleKey)}</div>
                </button>
              );
            })}
          </div>
        </div>
      </Card>
      <PageToolbar
        left={<Tabs value={tab} onChange={setTab} items={[
          { value: 'all', label: t('tab_all'), count: counts.all },
          { value: 'pending', label: t('tab_pending'), count: counts.pending },
          { value: 'running', label: t('tab_running'), count: counts.running },
          { value: 'done', label: t('tab_done'), count: counts.done },
          { value: 'failed', label: t('tab_failed'), count: counts.failed },
          { value: 'cancelled', label: t('tab_cancelled'), count: counts.cancelled },
        ]} />}
        right={role !== 'viewer' && <Button size="sm" onClick={() => setCreating(true)}><I.Plus size={12} /> {t('new_task')}</Button>}
      />
      <Card className="overflow-hidden">
        <div className="grid grid-cols-[130px_1fr_110px_110px_90px_130px] gap-3 px-4 py-2 text-[10px] uppercase tracking-wider text-fg-4 mono border-b border-border">
          <div>task_id</div><div>description</div><div>status</div><div>created</div><div className="text-right">elapsed</div><div className="text-right">actions</div>
        </div>
        {loading && !tasks.length ? <SkeletonRows rows={6} cols={6} /> : filtered.map(t => (
          <div key={t.task_id} className="grid grid-cols-[130px_1fr_110px_110px_90px_130px] gap-3 px-4 row-pad border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)] text-[12px] items-center">
            <div className="mono text-fg-3 truncate">{t.task_id}</div>
            <div className="text-fg truncate">{t.description}</div>
            <div><StatusBadge status={t.status} /></div>
            <div className="mono text-fg-3">{relTime(t.created_at)}</div>
            <div className="mono text-fg text-right tabular-nums">{t.elapsed}</div>
            <div className="flex justify-end gap-1">
              <Button variant="ghost" size="xs" onClick={() => setSelected(t)}><I.Eye size={11} /> {tFor(lang)('output_tab')}</Button>
              {t.status === 'running' && role !== 'viewer' && (
                <Button variant="danger-outline" size="xs" onClick={async () => {
                  try {
                    await window.cc.api.tasks.cancel(t.task_id);
                    window.cc.data.invalidate('tasks:all');
                    toast.toast({ title: tFor(lang)('task_cancelled'), desc: t.task_id, tone: 'success' });
                  } catch (e) {
                    toast.toast({ title: tFor(lang)('task_cancelled'), desc: `(offline) ${t.task_id}`, tone: 'error' });
                  }
                }}><I.Stop size={10} /> {tFor(lang)('cancel')}</Button>
              )}
            </div>
          </div>
        ))}
        {!filtered.length && <EmptyState icon={I.List} title={t('no_tasks')} subtitle={t('no_tasks_sub_tmpl', { tab: t('tab_' + tab) })} />}
      </Card>

      <Sheet open={!!selected} onClose={() => setSelected(null)} title={selected?.task_id || ''} subtitle={selected?.task_type} width={680}>
        {selected && (
          <div className="p-5 space-y-4">
            <div className="flex items-center gap-2 flex-wrap">
              <StatusBadge status={selected.status} />
              <Badge>{selected.task_type}</Badge>
              <Badge>{t('col_elapsed')} {selected.elapsed}</Badge>
            </div>
            <div className="text-[13px] text-fg-2">{selected.description}</div>
            <div>
              <div className="text-[11px] text-fg-3 mb-1.5 uppercase tracking-wider">{t('output_md')}</div>
              <div className="bg-bg-3 border border-border rounded-md p-4 text-[12.5px] text-fg-2 leading-relaxed space-y-2">
                <div className="font-semibold text-fg">{t('exec_summary')}</div>
                <div>{t('exec_summary_body') !== 'exec_summary_body' ? t('exec_summary_body') : 'Q2 brief synthesized from 14 sources. Primary theme: velocity returns after a Q1 correction; risk is concentration in two customer segments.'}</div>
                <div className="font-semibold text-fg mt-3">{t('key_findings')}</div>
                <ul className="list-disc pl-5 space-y-1">
                  <li>{t('finding_1') !== 'finding_1' ? t('finding_1') : 'Weekly active agents ↑ 18% QoQ (892 → 1053).'}</li>
                  <li>{t('finding_2') !== 'finding_2' ? t('finding_2') : 'Review rejection rate dropped from 7.1% to 3.4%.'}</li>
                  <li>{t('finding_3') !== 'finding_3' ? t('finding_3') : 'p95 capability latency regressed in eu-west-1 (188ms → 243ms).'}</li>
                </ul>
                <div className="font-semibold text-fg mt-3">{t('next_steps')}</div>
                <div>{t('next_steps_body') !== 'next_steps_body' ? t('next_steps_body') : 'Investigate eu-west-1 regression, escalate customer concentration to GTM.'}</div>
              </div>
            </div>
            <div>
              <div className="text-[11px] text-fg-3 mb-1.5 uppercase tracking-wider">{t('structured_result')}</div>
              <JsonViewer value={{ status: selected.status, artifacts: [{ kind: 'markdown', bytes: 4820 }], metrics: { sources_cited: 14, duration_ms: 252000 } }} />
            </div>
          </div>
        )}
      </Sheet>

      <NewTaskDialog open={creating} lang={lang} onClose={() => setCreating(false)} onCreate={async (p) => {
        setCreating(false);
        try {
          const res = await window.cc.api.tasks.create(
            p.task_type,
            p.title ? `${p.title}\n\n${p.description || ''}`.trim() : (p.description || ''),
            { title: p.title, priority: p.priority, tags: p.tags, assigned_agent: p.assigned_agent }
          );
          window.cc.data.invalidate('tasks:all');
          const id = (res && (res.task_id || res.id)) || 't_' + Math.random().toString(36).slice(2, 8).toUpperCase();
          toast.toast({ title: t('task_queued'), desc: `${p.title || id}`, tone: 'success' });
        } catch (e) {
          toast.toast({ title: t('task_queued'), desc: `(offline) ${p.title || ''}`, tone: 'error' });
        }
      }} />
    </div>
  );
};

// Backend enforces an allowlist of task labels (see `Task::validate` in
// cyberclaw-core). Pre-filled template tags must match that list — values
// outside the allowlist (`bug`, `feature`, `research`, `fix`, ...) cause
// `process_ingress` to reject the submission with a 500. See
// docs/implementation/reports/2026-04-22-admin-regression-full.md B06/B07.
const TASK_TEMPLATES = [
  {
    key: 'bug',
    titlePrefix: '[BUG] ',
    descTemplate: '## 症状\n\n\n## 复现步骤\n1. \n2. \n\n## 期望行为\n\n\n## 实际行为\n\n',
    tags: 'investigation,review',
    priority: 'high',
    task_type: 'triage',
  },
  {
    key: 'feature',
    titlePrefix: '[FEAT] ',
    descTemplate: '## 用户价值\n\n\n## 验收标准\n- [ ] \n- [ ] \n\n## 非功能要求\n\n',
    tags: 'normal,development',
    priority: 'medium',
    task_type: 'synthesis',
  },
  {
    key: 'review',
    titlePrefix: '[REVIEW] ',
    descTemplate: '## 审查范围\n\n\n## 重点关注\n- \n\n## 风险等级\nlow / medium / high\n',
    tags: 'review,governance',
    priority: 'medium',
    task_type: 'code_review',
  },
  {
    key: 'research',
    titlePrefix: '[RESEARCH] ',
    descTemplate: '## 目标\n\n\n## 参考材料\n- \n\n## 产出形式\nmarkdown report\n',
    tags: 'analysis,reporting',
    priority: 'low',
    task_type: 'synthesis',
  },
];

const PRIORITY_OPTIONS = ['low', 'medium', 'high', 'critical'];
const TEMPLATE_ICONS = { bug: I.AlertTriangle, feature: I.Zap, review: I.Check, research: I.Brain };
const TEMPLATE_TONES = { bug: 'rose', feature: 'accent', review: 'emerald', research: 'violet' };

const NewTaskDialog = ({ open, onClose, onCreate, lang }) => {
  const t = tFor(lang || 'en');
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [priority, setPriority] = useState('medium');
  const [tags, setTags] = useState('');
  const [assignedAgent, setAssignedAgent] = useState('');
  const [taskType, setTaskType] = useState('synthesis');
  const [activeTemplate, setActiveTemplate] = useState(null);

  const agentsRes = window.cc.data.useAgents();
  const agentsList = window.cc.data.extractList(agentsRes, 'agents', []);

  useEffect(() => {
    if (open) {
      setTitle(''); setDescription(''); setPriority('medium');
      setTags(''); setAssignedAgent(''); setTaskType('synthesis');
      setActiveTemplate(null);
      // Check if a template key was pre-selected from the intro card shortcut.
      const prekey = window.__ccTaskTemplate;
      if (prekey) {
        window.__ccTaskTemplate = null;
        const tmpl = TASK_TEMPLATES.find(t => t.key === prekey);
        if (tmpl) setTimeout(() => applyTemplate(tmpl), 0);
      }
    }
  }, [open]);

  useEffect(() => {
    const handler = (e) => {
      const tmpl = TASK_TEMPLATES.find(t => t.key === e.detail);
      if (tmpl) applyTemplate(tmpl);
    };
    window.addEventListener('cc:preselect-template', handler);
    return () => window.removeEventListener('cc:preselect-template', handler);
  }, []);

  const applyTemplate = (tmpl) => {
    setActiveTemplate(tmpl.key);
    setTitle(prev => {
      // if title already has a prefix from a previous template, strip it
      const stripped = TASK_TEMPLATES.reduce((s, tt) => s.startsWith(tt.titlePrefix) ? s.slice(tt.titlePrefix.length) : s, prev);
      return tmpl.titlePrefix + stripped;
    });
    setDescription(tmpl.descTemplate);
    setPriority(tmpl.priority);
    setTags(tmpl.tags);
    setTaskType(tmpl.task_type);
  };

  const canSubmit = title.trim().length > 0;

  return (
    <Dialog open={open} onClose={onClose} title={t('tasks.new_template')} subtitle="POST /api/v1/tasks" width={580}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>{t('cancel')}</Button>
          <Button onClick={() => onCreate({ title, description, priority, tags: tags.split(',').map(s => s.trim()).filter(Boolean), assigned_agent: assignedAgent || null, task_type: taskType })} disabled={!canSubmit}>
            <I.Plus size={13} /> {t('create')}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        {/* Template picker */}
        <div>
          <div className="text-[11px] text-fg-3 mb-2 uppercase tracking-wider flex items-center gap-2">
            <span>{t('tasks.template.pick')}</span>
            <span className="text-fg-4">·</span>
            <span className="text-fg-4 normal-case">{t('tasks.template.or_blank')}</span>
          </div>
          <div className="grid grid-cols-4 gap-2">
            {TASK_TEMPLATES.map(tmpl => {
              const Icon = TEMPLATE_ICONS[tmpl.key] || I.List;
              const tone = TEMPLATE_TONES[tmpl.key] || 'default';
              const active = activeTemplate === tmpl.key;
              const toneClasses = {
                rose:   active ? 'border-rose-500/60 bg-rose-500/10 text-rose-400' : 'border-border hover:border-rose-500/40 hover:bg-rose-500/5 text-fg-3',
                accent: active ? 'border-[var(--accent)]/60 bg-accent-soft text-accent' : 'border-border hover:border-[var(--accent)]/40 hover:bg-accent-soft/50 text-fg-3',
                emerald:active ? 'border-emerald-500/60 bg-emerald-500/10 text-emerald-400' : 'border-border hover:border-emerald-500/40 hover:bg-emerald-500/5 text-fg-3',
                violet: active ? 'border-violet-500/60 bg-violet-500/10 text-violet-400' : 'border-border hover:border-violet-500/40 hover:bg-violet-500/5 text-fg-3',
              };
              return (
                <button key={tmpl.key} onClick={() => applyTemplate(tmpl)}
                  className={`flex flex-col items-center gap-1.5 p-3 rounded-md border transition-colors ${toneClasses[tone]}`}>
                  <Icon size={16} />
                  <span className="text-[11px] font-medium">{t('tasks.template.' + tmpl.key)}</span>
                </button>
              );
            })}
          </div>
        </div>

        <div className="h-px bg-border" />

        {/* title */}
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('tasks.template.title')}</label>
          <Input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="e.g. [BUG] Login fails on Safari" autoFocus />
        </div>

        {/* description */}
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('tasks.template.description')}</label>
          <Textarea rows={6} value={description} onChange={(e) => setDescription(e.target.value)} placeholder={t('describe_task')} />
        </div>

        {/* priority + tags */}
        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('tasks.template.priority')}</label>
            <Select value={priority} onChange={setPriority}
              options={PRIORITY_OPTIONS.map(p => ({ value: p, label: p }))} />
          </div>
          <div>
            <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('tasks.template.tags')}</label>
            <Input value={tags} onChange={(e) => setTags(e.target.value)} placeholder="bug,fix,auth" className="mono" />
          </div>
        </div>

        {/* assigned agent */}
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('tasks.template.agent')}</label>
          <Select value={assignedAgent} onChange={setAssignedAgent}
            placeholder={t('tasks.template.no_agent')}
            options={agentsList.map(a => ({ value: a.agent_id, label: a.name + (a.status === 'active' ? '' : ' (' + a.status + ')') }))}
          />
        </div>
      </div>
    </Dialog>
  );
};

// ---- Executions + Trace sheet ----
const TraceSheet = ({ exec, open, onClose, lang }) => {
  const t = tFor(lang || 'en');
  const [trace, setTrace] = useState(null);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState(null);
  useEffect(() => {
    if (!open || !exec) { setTrace(null); setErr(null); return; }
    let cancelled = false;
    setLoading(true); setErr(null);
    window.cc.api.executions.trace(exec.execution_id)
      .then(data => { if (!cancelled) setTrace(data); })
      .catch(e => { if (!cancelled) { setErr(e && e.message ? e.message : String(e)); setTrace(MOCK.trace); } })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [open, exec && exec.execution_id]);
  if (!exec) return null;
  const legend = [
    { k: 'agent', color: 'bg-indigo-500' }, { k: 'llm', color: 'bg-violet-500' },
    { k: 'skill', color: 'bg-cyan-500' }, { k: 'tool', color: 'bg-emerald-500' },
    { k: 'capability', color: 'bg-amber-500' }, { k: 'policy', color: 'bg-rose-500' },
    { k: 'wait', color: 'bg-slate-500' },
  ];
  const spans = (trace && Array.isArray(trace.spans)) ? trace.spans : [];
  const totalMs = (trace && trace.total_ms) || 0;
  const proposedInput = (trace && trace.proposed_input) || null;
  return (
    <Sheet open={open} onClose={onClose} title={t('trace')} subtitle={exec.execution_id} width={860}
      right={<Button variant="outline" size="sm"><I.Download size={12} /> {t('export')}</Button>}>
      <div className="p-5 space-y-4">
        <div className="flex items-center gap-2 flex-wrap">
          <StatusBadge status={exec.status} />
          <RiskBadge level={exec.risk_level} />
          <Badge>agent: {exec.agent}</Badge>
          <Badge>capability: {exec.capability}</Badge>
          <Badge>duration: {exec.duration}</Badge>
        </div>
        <div className="flex items-center gap-3 flex-wrap text-[11px] mono text-fg-3">
          {legend.map(l => <span key={l.k} className="flex items-center gap-1.5"><span className={`w-2 h-2 rounded-sm ${l.color}`} /> {l.k}</span>)}
        </div>
        {loading && <SkeletonRows rows={4} cols={3} />}
        {err && !loading && <ErrorBanner message={err} />}
        {!loading && spans.length > 0 && (
          <Card className="overflow-hidden">
            <TraceTimeline spans={spans} totalMs={totalMs} />
          </Card>
        )}
        {!loading && !spans.length && !err && (
          <EmptyState icon={I.Activity} title={t('trace_empty') !== 'trace_empty' ? t('trace_empty') : 'no spans returned'} subtitle={exec.execution_id} />
        )}
        {proposedInput && (
          <div>
            <div className="text-[11px] text-fg-3 mb-1.5 uppercase tracking-wider">Proposed input</div>
            <JsonViewer value={proposedInput} />
          </div>
        )}
      </div>
    </Sheet>
  );
};

const ExecutionsPage = ({ lang, role, preOpen, onClearPreOpen }) => {
  const t = tFor(lang);
  const [q, setQ] = useState('');
  const [st, setSt] = useState('');
  const [risk, setRisk] = useState('');
  const [traceExec, setTraceExec] = useState(null);
  const toast = useToast();
  const execRes = window.cc.data.useExecutions({});
  const { loading, error, data } = execRes;
  const execs = window.cc.data.extractList(execRes, 'executions:{}', MOCK.executions);
  useEffect(() => { if (preOpen) { setTraceExec(preOpen); onClearPreOpen && onClearPreOpen(); } }, [preOpen]);
  const list = execs.filter(e =>
    (!q || (e.agent || '').includes(q) || (e.capability || '').includes(q) || (e.execution_id || '').includes(q)) &&
    (!st || e.status === st) && (!risk || e.risk_level === risk)
  );
  return (
    <div className="space-y-4">
      <PageToolbar
        left={
          <>
            <div className="relative">
              <I.Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-fg-4" />
              <Input value={q} onChange={(e) => setQ(e.target.value)} className="pl-7 w-56" placeholder={t('search_exec')} />
            </div>
            <Select value={st} onChange={setSt} className="w-40" options={[{ value: '', label: t('all_status') }, { value: 'running', label: 'running' }, { value: 'done', label: 'done' }, { value: 'failed', label: 'failed' }, { value: 'pending_review', label: 'pending_review' }, { value: 'cancelled', label: 'cancelled' }]} />
            <Select value={risk} onChange={setRisk} className="w-36" options={[{ value: '', label: t('all_risks') }, { value: 'low', label: t('risk_low') }, { value: 'medium', label: t('risk_medium') }, { value: 'high', label: t('risk_high') }, { value: 'critical', label: t('risk_critical') }]} />
            <Select value="24h" onChange={() => {}} className="w-32" options={[{ value: '1h', label: t('last_1h') }, { value: '24h', label: t('last_24h') }, { value: '7d', label: t('last_7d') }]} />
          </>
        }
        right={<Button variant="outline" size="sm" onClick={() => window.cc.data.invalidate('executions:*')}><I.Refresh size={12} /> {t('refresh')}</Button>}
      />
      {error && <ErrorBanner message={error} onRetry={() => window.cc.data.invalidate('executions:*')} />}
      <Card className="overflow-hidden">
        <div className="grid grid-cols-[120px_140px_1fr_110px_110px_110px_80px_130px] gap-3 px-4 py-2 text-[10px] uppercase tracking-wider text-fg-4 mono border-b border-border">
          <div>exec_id</div><div>agent</div><div>capability</div><div>status</div><div>risk</div><div>started</div><div className="text-right">dur</div><div className="text-right">actions</div>
        </div>
        {loading && !execs.length ? <SkeletonRows rows={8} cols={8} /> : list.map(e => (
          <div key={e.execution_id} className="grid grid-cols-[120px_140px_1fr_110px_110px_110px_80px_130px] gap-3 px-4 row-pad border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)] text-[12px] items-center">
            <div className="mono text-fg-3 truncate">{e.execution_id}</div>
            <div className="text-fg truncate">{e.agent}</div>
            <div className="mono text-fg-2 truncate">{e.capability}</div>
            <div><StatusBadge status={e.status} /></div>
            <div><RiskBadge level={e.risk_level} /></div>
            <div className="mono text-fg-3">{relTime(e.started_at)}</div>
            <div className="mono text-fg text-right tabular-nums">{e.duration}</div>
            <div className="flex justify-end gap-1">
              <Button variant="ghost" size="xs" onClick={() => setTraceExec(e)}><I.Activity size={11} /> {t('trace')}</Button>
              {e.status === 'running' && role !== 'viewer' && (
                <Button variant="danger-outline" size="xs" onClick={async () => {
                  try {
                    await window.cc.api.executions.cancel(e.execution_id);
                    window.cc.data.invalidate('executions:*');
                    toast.toast({ title: t('exec_cancelled'), desc: e.execution_id, tone: 'success' });
                  } catch (err) {
                    toast.toast({ title: t('exec_cancelled'), desc: `(offline) ${e.execution_id}`, tone: 'error' });
                  }
                }}><I.Stop size={10} /></Button>
              )}
            </div>
          </div>
        ))}
        {!list.length && <EmptyState icon={I.Activity} title={t('no_executions')} subtitle={t('no_executions_sub')} />}
      </Card>
      <TraceSheet exec={traceExec} open={!!traceExec} lang={lang} onClose={() => setTraceExec(null)} />
    </div>
  );
};

Object.assign(window, { SkillsPage, TasksPage, ExecutionsPage, TraceSheet });
