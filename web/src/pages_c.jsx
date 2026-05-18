// Pages group C: Reviews, Capabilities, Channels, Nodes, Settings

// ---- C2 Governance helpers (NL Approval Bar, Audit Agent, Policy Browser) ----

// parseNLIntent: parses natural-language approval intent into a PolicyModifier preview.
// Syntax examples:
//   "approve all low-risk fs writes from agent planner"
//     → { action:'approve', target:'all', risk:'low', capability_pattern:'fs.*', agent:'planner' }
//   "reject + tell them reduce scope"
//     → { action:'reject', target:'all', modifier_note:'reduce scope' }
//   "trust this agent's next 5 calls"
//     → { action:'trust_n', trust_count:5 }
function parseNLIntent(text) {
  if (!text || !text.trim()) return null;
  const lower = text.toLowerCase();
  let action = 'approve';
  if (/\breject\b/.test(lower)) action = 'reject';
  else if (/\btrust\b/.test(lower)) action = 'trust_n';
  else if (/\bblock\b|\bdeny\b/.test(lower)) action = 'reject';
  let risk = null;
  if (/\bcritical\b/.test(lower)) risk = 'critical';
  else if (/high[-\s]?risk|(?<![a-z])high(?![a-z])/.test(lower)) risk = 'high';
  else if (/medium[-\s]?risk|(?<![a-z])medium(?![a-z])/.test(lower)) risk = 'medium';
  else if (/low[-\s]?risk|(?<![a-z])low(?![a-z])/.test(lower)) risk = 'low';
  const agentMatch = lower.match(/(?:from\s+agent|agent)\s+([\w-]+)/);
  const agent = agentMatch ? agentMatch[1] : null;
  let capability_pattern = null;
  if (/\bfs\b|\bfile\b/.test(lower)) capability_pattern = 'fs.*';
  else if (/\bdb\b|\bdatabase\b/.test(lower)) capability_pattern = 'db.*';
  else if (/\bslack\b/.test(lower)) capability_pattern = 'slack.*';
  else if (/\baws\b/.test(lower)) capability_pattern = 'aws.*';
  else if (/\bwallet\b/.test(lower)) capability_pattern = 'wallet.*';
  else if (/\bdbops\b/.test(lower)) capability_pattern = 'db.*';
  const trustMatch = lower.match(/next\s+(\d+)/);
  const trust_count = trustMatch ? parseInt(trustMatch[1], 10) : null;
  const noteMatch = text.match(/tell\s+them\s+(.+)/i);
  const modifier_note = noteMatch ? noteMatch[1].trim() : null;
  const target = /\ball\b/.test(lower) ? 'all' : 'selection';
  return { action, target, risk, agent, capability_pattern, trust_count, modifier_note };
}

const NL_CHIPS = [
  { key: 'approve_low',  label: 'approve all low-risk' },
  { key: 'reject_scope', label: 'reject + tell them reduce scope' },
  { key: 'trust5',       label: "trust this agent's next 5 calls" },
  { key: 'auto_dbops',   label: 'auto-approve dbops if dry-run passed' },
  { key: 'show_blocked', label: 'show everything blocked last hour' },
];

// @deprecated — moved to primary Chat page (Sprint 12 L2)
const NLApprovalBar = ({ lang, onIntent }) => {
  const t = tFor(lang || 'en');
  const [text, setText] = useState('');
  const [preview, setPreview] = useState(null);
  const [confirming, setConfirming] = useState(false);

  const handleSubmit = () => {
    const parsed = parseNLIntent(text);
    if (!parsed) return;
    setPreview(parsed);
    setConfirming(true);
  };

  const handleConfirm = () => {
    onIntent && onIntent(preview, text);
    setText(''); setPreview(null); setConfirming(false);
  };

  const handleChip = (chip) => {
    setText(chip.label);
    setPreview(parseNLIntent(chip.label));
    setConfirming(true);
  };

  return (
    <div className="space-y-2">
      <div className="flex gap-2 items-start">
        <div className="flex-1 relative">
          <Textarea rows={2} value={text}
            onChange={(e) => { setText(e.target.value); setPreview(null); setConfirming(false); }}
            placeholder={t('reviews.nl.placeholder')} className="resize-none pr-10" />
          <button className="absolute right-2 top-2 text-fg-3 hover:text-fg transition-colors" title="Voice (coming soon)">
            <I.Mic size={14} />
          </button>
        </div>
        <Button onClick={handleSubmit} disabled={!text.trim()} className="shrink-0 mt-0.5">
          <I.Send size={13} /> {t('reviews.nl.parse')}
        </Button>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {NL_CHIPS.map(chip => (
          <button key={chip.key} onClick={() => handleChip(chip)}
            className="text-[11px] px-2.5 py-1 rounded-full border border-border bg-bg-3 hover:border-[var(--border-strong)] hover:bg-[var(--hover)] text-fg-3 hover:text-fg transition-colors">
            {chip.label}
          </button>
        ))}
      </div>
      {confirming && preview && (
        <Card className="border-[var(--accent)]/30 bg-accent-soft/40">
          <div className="px-4 py-3">
            <div className="text-[11px] text-fg-3 uppercase tracking-wider mb-2">{t('reviews.nl.preview_title')}</div>
            <div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[12px]">
              <span className="text-fg-3 mono">action</span><span className="text-fg font-medium">{preview.action}</span>
              <span className="text-fg-3 mono">target</span><span className="text-fg">{preview.target}</span>
              {preview.risk && <><span className="text-fg-3 mono">risk</span><RiskBadge level={preview.risk} /></>}
              {preview.agent && <><span className="text-fg-3 mono">agent</span><span className="mono text-fg-2">{preview.agent}</span></>}
              {preview.capability_pattern && <><span className="text-fg-3 mono">capability</span><span className="mono text-fg-2">{preview.capability_pattern}</span></>}
              {preview.trust_count && <><span className="text-fg-3 mono">trust_next</span><span className="text-fg">{preview.trust_count} calls</span></>}
              {preview.modifier_note && <><span className="text-fg-3 mono">note</span><span className="text-fg-2 italic">{preview.modifier_note}</span></>}
            </div>
            <div className="flex gap-2 mt-3">
              <Button variant="success" size="sm" onClick={handleConfirm}><I.Check size={12} /> {t('reviews.nl.confirm')}</Button>
              <Button variant="ghost" size="sm" onClick={() => { setConfirming(false); setPreview(null); }}>{t('cancel')}</Button>
            </div>
          </div>
        </Card>
      )}
    </div>
  );
};

// ---- Audit Agent tab ----
// Sprint 18 W2 — wired to GET /api/v1/reviews/audit-agent/profile and
// GET /api/v1/reviews/audit-agent/accuracy-curve (governance.rs).
// The previous MOCK_AUDIT_AGENT_PROFILE included SPA-only fields
// (approvals_month, auto_threshold, learned_events) with no real data
// source — those panels were removed; this view now renders only
// what the backend can honestly compute.
const AuditAgentTab = ({ lang }) => {
  const t = tFor(lang || 'en');
  // Sprint 18 W2 — wired to GET /api/v1/reviews/audit-agent/profile
  // and /api/v1/reviews/audit-agent/accuracy-curve. Schema is what the
  // backend can honestly compute from the audit log right now:
  //   { agent_id, accuracy, total_decisions, auto_approved,
  //     manual_override, learned_patterns: [{pattern, count, last_seen}] }
  // The MOCK fields auto_threshold / learned_events / approvals_month
  // / auto_handled_pct were SPA-only product fiction with no real data
  // source — dropped. accuracy_curve comes from the dedicated endpoint.
  const profileRes = window.cc.data.useAuditAgentProfile();
  const curveRes = window.cc.data.useAuditAgentAccuracyCurve(30);
  const profile = profileRes.data ?? {
    accuracy: 0,
    total_decisions: 0,
    auto_approved: 0,
    manual_override: 0,
    learned_patterns: [],
  };
  const autoHandledPct = profile.total_decisions > 0
    ? profile.auto_approved / profile.total_decisions
    : 0;
  const accuracyCurve = (curveRes.data?.points ?? []).map(p => p.accuracy);

  return (
    <div className="space-y-5">
      <Card className="p-5">
        <div className="flex items-start gap-4">
          <div className="w-12 h-12 rounded-xl bg-amber-500/15 border border-amber-500/30 flex items-center justify-center text-2xl shrink-0">⚖</div>
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-[15px] font-semibold text-fg">Audit Agent</span>
              <Badge tone="slate">internal.audit.governor</Badge>
              <Badge tone="amber">{t('reviews.audit_agent.internal_badge')}</Badge>
            </div>
            <div className="text-[12px] text-fg-3 italic mt-1">{t('reviews.audit_agent.mission')}</div>
          </div>
        </div>
      </Card>

      <div className="grid grid-cols-4 gap-3">
        <StatCard label={t('reviews.audit_agent.kpi_approvals')} value={profile.total_decisions} icon={I.Check} />
        <StatCard label={t('reviews.audit_agent.kpi_accuracy')} value={`${Math.round(profile.accuracy * 100)}%`} icon={I.Activity} />
        <StatCard label={t('reviews.audit_agent.kpi_auto')} value={`${Math.round(autoHandledPct * 100)}%`} icon={I.Zap} />
        <StatCard label={t('reviews.audit_agent.kpi_patterns')} value={profile.learned_patterns?.length ?? 0} icon={I.Brain} />
      </div>

      {accuracyCurve.length > 0 && (
        <Card className="p-4">
          <div className="text-[11px] text-fg-3 uppercase tracking-wider mb-3">{t('reviews.audit_agent.accuracy_curve')}</div>
          <Sparkline values={accuracyCurve} tone="accent" height={48} />
          <div className="mt-2 text-[11px] text-fg-4 mono">{t('reviews.audit_agent.curve_hint')}</div>
        </Card>
      )}

      <Card className="overflow-hidden">
        <div className="px-4 py-3 border-b border-border flex items-center justify-between">
          <span className="text-[12px] font-semibold text-fg">{t('reviews.audit_agent.learned_title')}</span>
          <span className="text-[11px] text-fg-4 mono">{profile.learned_patterns?.length ?? 0} entries</span>
        </div>
        {(profile.learned_patterns || []).length === 0 ? (
          <div className="px-4 py-6 text-center text-[12px] text-fg-4">
            {t('reviews.audit_agent.no_patterns') || 'No learned patterns yet.'}
          </div>
        ) : (
          (profile.learned_patterns || []).map((p, i) => (
            <div key={`${p.pattern}-${i}`} className="px-4 py-3 border-b border-border/60 last:border-b-0 flex items-start justify-between gap-3">
              <div className="flex-1 min-w-0">
                <div className="text-[12px] text-fg mono truncate">{p.pattern}</div>
                <div className="text-[11px] text-fg-4 mt-0.5">
                  {p.last_seen ? new Date(p.last_seen).toLocaleString() : '—'} · count {p.count}
                </div>
              </div>
            </div>
          ))
        )}
      </Card>
    </div>
  );
};

// ---- Policy Browser tab ----
// Rules from:
//   crates/cyberclaw-governance/src/dangerous_capability_filter.rs  (7 defaults)
//   crates/cyberclaw-governance/src/tool_permission_matcher.rs       (24 defaults)
// backend aggregation needed: GET /api/v1/security/permission/rules
// backend aggregation needed: PATCH /api/v1/security/permission/rules/{id} → enters /reviews flow
const MOCK_POLICY_RULES = [
  { id: 'dcf_01', source: 'DangerousCapabilityFilter', pattern: 'fs.delete_recursive',   action: 'DENY',  severity: 'Critical', enabled: true },
  { id: 'dcf_02', source: 'DangerousCapabilityFilter', pattern: 'aws.iam_attach_policy', action: 'DENY',  severity: 'Critical', enabled: true },
  { id: 'dcf_03', source: 'DangerousCapabilityFilter', pattern: 'db.drop_table',         action: 'ASK',   severity: 'High',     enabled: true },
  { id: 'dcf_04', source: 'DangerousCapabilityFilter', pattern: 'wallet.transfer',        action: 'ASK',   severity: 'High',     enabled: true },
  { id: 'dcf_05', source: 'DangerousCapabilityFilter', pattern: 'shell.exec',             action: 'ASK',   severity: 'High',     enabled: true },
  { id: 'dcf_06', source: 'DangerousCapabilityFilter', pattern: 'network.outbound_raw',   action: 'ASK',   severity: 'Medium',   enabled: true },
  { id: 'dcf_07', source: 'DangerousCapabilityFilter', pattern: 'config.write_global',    action: 'ASK',   severity: 'Medium',   enabled: true },
  { id: 'tpm_01', source: 'ToolPermissionMatcher', pattern: 'rm -rf /',             action: 'DENY',  severity: 'Critical', enabled: true },
  { id: 'tpm_02', source: 'ToolPermissionMatcher', pattern: 'mkfs.*',               action: 'DENY',  severity: 'Critical', enabled: true },
  { id: 'tpm_03', source: 'ToolPermissionMatcher', pattern: 'sudo *',               action: 'ASK',   severity: 'High',     enabled: true },
  { id: 'tpm_04', source: 'ToolPermissionMatcher', pattern: 'curl * | bash',        action: 'DENY',  severity: 'Critical', enabled: true },
  { id: 'tpm_05', source: 'ToolPermissionMatcher', pattern: 'git push --force',     action: 'ASK',   severity: 'High',     enabled: true },
  { id: 'tpm_06', source: 'ToolPermissionMatcher', pattern: 'kubectl delete *',     action: 'ASK',   severity: 'High',     enabled: true },
  { id: 'tpm_07', source: 'ToolPermissionMatcher', pattern: 'psql * DROP DATABASE', action: 'DENY',  severity: 'Critical', enabled: true },
  { id: 'tpm_08', source: 'ToolPermissionMatcher', pattern: 'fs.read',              action: 'ALLOW', severity: 'Low',      enabled: true },
  { id: 'tpm_09', source: 'ToolPermissionMatcher', pattern: 'fs.write',             action: 'ASK',   severity: 'Medium',   enabled: true },
  { id: 'tpm_10', source: 'ToolPermissionMatcher', pattern: 'db.select',            action: 'ALLOW', severity: 'Low',      enabled: true },
  { id: 'tpm_11', source: 'ToolPermissionMatcher', pattern: 'db.insert',            action: 'ASK',   severity: 'Medium',   enabled: true },
  { id: 'tpm_12', source: 'ToolPermissionMatcher', pattern: 'slack.send_message',   action: 'ASK',   severity: 'Medium',   enabled: true },
  { id: 'tpm_13', source: 'ToolPermissionMatcher', pattern: 'slack.read',           action: 'ALLOW', severity: 'Low',      enabled: true },
  { id: 'tpm_14', source: 'ToolPermissionMatcher', pattern: 'http.get',             action: 'ALLOW', severity: 'Low',      enabled: true },
  { id: 'tpm_15', source: 'ToolPermissionMatcher', pattern: 'http.post',            action: 'ASK',   severity: 'Medium',   enabled: true },
  { id: 'tpm_16', source: 'ToolPermissionMatcher', pattern: 'email.send',           action: 'ASK',   severity: 'Medium',   enabled: true },
  { id: 'tpm_17', source: 'ToolPermissionMatcher', pattern: 'code.execute',         action: 'ASK',   severity: 'High',     enabled: true },
  { id: 'tpm_18', source: 'ToolPermissionMatcher', pattern: 'package.install',      action: 'ASK',   severity: 'High',     enabled: true },
  { id: 'tpm_19', source: 'ToolPermissionMatcher', pattern: 'secret.read',          action: 'ASK',   severity: 'High',     enabled: true },
  { id: 'tpm_20', source: 'ToolPermissionMatcher', pattern: 'secret.write',         action: 'DENY',  severity: 'Critical', enabled: true },
  { id: 'tpm_21', source: 'ToolPermissionMatcher', pattern: 'agent.spawn',          action: 'ASK',   severity: 'High',     enabled: true },
  { id: 'tpm_22', source: 'ToolPermissionMatcher', pattern: 'agent.kill',           action: 'ASK',   severity: 'High',     enabled: true },
  { id: 'tpm_23', source: 'ToolPermissionMatcher', pattern: 'policy.write',         action: 'DENY',  severity: 'Critical', enabled: true },
  { id: 'tpm_24', source: 'ToolPermissionMatcher', pattern: 'audit.clear',          action: 'DENY',  severity: 'Critical', enabled: true },
];

const ACTION_TONE = { ALLOW: 'emerald', ASK: 'amber', DENY: 'rose' };
const SEV_TONE    = { Low: 'slate', Medium: 'amber', High: 'orange', Critical: 'rose' };

// PermissionRuleRow — inline row editor (~50 LOC): pattern + source badge + severity + action select + enable toggle
const PermissionRuleRow = ({ rule, lang, onEdit }) => {
  const t = tFor(lang || 'en');
  const [editing, setEditing] = useState(false);
  const [draftAction, setDraftAction] = useState(rule.action);
  const [draftEnabled, setDraftEnabled] = useState(rule.enabled);

  const handleSave = () => {
    onEdit && onEdit(rule.id, { action: draftAction, enabled: draftEnabled });
    setEditing(false);
  };
  const handleCancel = () => { setEditing(false); setDraftAction(rule.action); setDraftEnabled(rule.enabled); };

  return (
    <div className={`px-4 py-2.5 border-b border-border/60 last:border-b-0 flex items-center gap-3 text-[12px] hover:bg-[var(--hover)] transition-colors ${editing ? 'bg-accent-soft/20' : ''}`}>
      <div className="w-2 h-2 rounded-full shrink-0" style={{ backgroundColor: rule.enabled ? 'var(--accent)' : 'var(--fg-4)' }} />
      <div className="flex-1 mono text-fg-2 truncate min-w-0">{rule.pattern}</div>
      <Badge tone="slate" className="shrink-0 text-[10px]">{rule.source === 'DangerousCapabilityFilter' ? 'DCF' : 'TPM'}</Badge>
      <Badge tone={SEV_TONE[rule.severity] || 'slate'} className="shrink-0">{rule.severity}</Badge>
      {editing ? (
        <div className="flex items-center gap-2 shrink-0">
          <Select value={draftAction} onChange={setDraftAction} className="w-24 text-[11px]"
            options={[{ value: 'ALLOW', label: 'ALLOW' }, { value: 'ASK', label: 'ASK' }, { value: 'DENY', label: 'DENY' }]} />
          <Switch checked={draftEnabled} onChange={setDraftEnabled} />
          <Button size="xs" variant="success" onClick={handleSave}><I.Check size={10} /></Button>
          <Button size="xs" variant="ghost" onClick={handleCancel}><I.Close size={10} /></Button>
        </div>
      ) : (
        <div className="flex items-center gap-2 shrink-0">
          <Badge tone={ACTION_TONE[rule.action] || 'slate'}>{rule.action}</Badge>
          <Button size="xs" variant="ghost" onClick={() => setEditing(true)}><I.Edit size={10} /> {t('reviews.policy.edit')}</Button>
        </div>
      )}
    </div>
  );
};

const PolicyBrowserTab = ({ lang }) => {
  const t = tFor(lang || 'en');
  // Sprint 18 W2 — wired to GET /api/v1/security/permission/rules.
  // Local state still holds the rules so inline edits (handleEdit) can
  // optimistically mutate; on mount or refetch we sync from API.
  const rulesRes = window.cc.data.usePermissionRules();
  const [rules, setRules] = useState(MOCK_POLICY_RULES);
  useEffect(() => {
    if (rulesRes.data?.rules) setRules(rulesRes.data.rules);
  }, [rulesRes.data]);
  const [srcFilter, setSrcFilter] = useState('all');
  const [actionFilter, setActionFilter] = useState('all');
  const [q, setQ] = useState('');
  const toast = useToast();

  const filtered = rules.filter(r =>
    (srcFilter === 'all' || r.source === srcFilter) &&
    (actionFilter === 'all' || r.action === actionFilter) &&
    (!q || r.pattern.toLowerCase().includes(q.toLowerCase()))
  );

  const handleEdit = (id, patch) => {
    // TODO: PATCH /api/v1/security/permission/rules/{id} — enters /reviews approval flow
    setRules(prev => prev.map(r => r.id === id ? { ...r, ...patch } : r));
    toast.toast({ title: t('reviews.policy.edit_queued'), desc: t('reviews.policy.edit_queued_desc'), tone: 'success' });
  };

  return (
    <div className="space-y-4">
      <Card className="p-3 bg-amber-500/5 border border-amber-500/20">
        <div className="text-[12px] text-fg-2 flex items-start gap-2">
          <I.AlertTriangle size={13} className="text-amber-400 shrink-0 mt-0.5" />
          <span>{t('reviews.policy.edit_warn')}</span>
        </div>
      </Card>
      <div className="flex gap-2 items-center flex-wrap">
        <div className="relative">
          <I.Search size={12} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-fg-4" />
          <Input value={q} onChange={(e) => setQ(e.target.value)} className="pl-7 w-52 text-[12px]" placeholder={t('reviews.policy.search')} />
        </div>
        <Select value={srcFilter} onChange={setSrcFilter} className="w-52"
          options={[
            { value: 'all',                       label: t('reviews.policy.all_sources') },
            { value: 'DangerousCapabilityFilter',  label: 'DangerousCapabilityFilter (7)' },
            { value: 'ToolPermissionMatcher',      label: 'ToolPermissionMatcher (24)' },
          ]} />
        <Select value={actionFilter} onChange={setActionFilter} className="w-32"
          options={[
            { value: 'all',   label: t('reviews.policy.all_actions') },
            { value: 'ALLOW', label: 'ALLOW' },
            { value: 'ASK',   label: 'ASK' },
            { value: 'DENY',  label: 'DENY' },
          ]} />
        <span className="text-[11px] text-fg-4 mono ml-auto">{filtered.length} / {rules.length} {t('reviews.policy.rules_count')}</span>
      </div>
      <Card className="overflow-hidden">
        <div className="flex items-center gap-3 px-4 py-2 text-[10px] uppercase tracking-wider text-fg-4 mono border-b border-border">
          <div className="w-2 shrink-0" />
          <div className="flex-1">pattern</div>
          <div className="w-12 shrink-0">src</div>
          <div className="w-20 shrink-0">severity</div>
          <div className="w-16 shrink-0">action</div>
          <div className="w-16 shrink-0 text-right">edit</div>
        </div>
        {filtered.length
          ? filtered.map(r => <PermissionRuleRow key={r.id} rule={r} lang={lang} onEdit={handleEdit} />)
          : <EmptyState icon={I.Shield} title={t('reviews.policy.no_match')} />}
      </Card>
    </div>
  );
};

// ---- Settings/About helpers — defensive formatters so a missing backend field
// renders as '—' rather than "Invalid Date" or "NaNd NaNh".
function formatBuildTime(s) {
  if (!s) return '—';
  try {
    const d = new Date(s);
    if (Number.isNaN(d.getTime())) return s;
    return d.toISOString().slice(0, 16).replace('T', ' ');
  } catch {
    return s;
  }
}
function formatUptime(secs) {
  if (!Number.isFinite(secs)) return '—';
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  return `${d}d ${h}h`;
}

// ---- Reviews ----
const RejectDialog = ({ open, onClose, onReject, review, lang }) => {
  const t = tFor(lang || 'en');
  const [reason, setReason] = useState('');
  useEffect(() => { if (open) setReason(''); }, [open]);
  return (
    <Dialog open={open} onClose={onClose} title={t('reject_title')} subtitle={review?.review_id} width={500}
      footer={<><Button variant="ghost" onClick={onClose}>{t('cancel')}</Button><Button variant="danger" onClick={() => onReject(reason)} disabled={!reason.trim()}><I.XCircle size={13} /> {t('reject')}</Button></>}>
      <div className="space-y-3">
        <div className="text-[12px] text-fg-2">{t('reject_warn')}</div>
        <div>
          <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider">{t('reason_required')}</label>
          <Textarea rows={4} value={reason} onChange={(e) => setReason(e.target.value)} placeholder={t('reason_placeholder')} />
        </div>
      </div>
    </Dialog>
  );
};

const TargetBadge = ({ type, lang }) => {
  if (type !== 'execution' && type !== 'handoff') return null;
  const t = tFor(lang || 'en');
  const isHandoff = type === 'handoff';
  const label = t(`review.target.${type}`);
  const tone = isHandoff ? 'violet' : 'cyan';
  return <Badge tone={tone}>{isHandoff && '🔀 '}{label}</Badge>;
};

const ReviewsPage = ({ lang, role, onOpenTrace }) => {
  const t = tFor(lang);
  const [tab, setTab] = useState('pending');
  const pendingRes = window.cc.data.useReviews('pending');
  const approvedRes = window.cc.data.useReviews('approved');
  const rejectedRes = window.cc.data.useReviews('rejected');
  const [rejectOf, setRejectOf] = useState(null);
  const toast = useToast();
  const pending = window.cc.data.extractList(pendingRes, 'reviews:pending', MOCK.reviews);
  // History tabs come from real backend calls (?status=approved, ?status=rejected).
  // On fetch error we fall back to the MOCK history slice, but on success we
  // trust the backend even if the list is empty.
  const approved = window.cc.data.extractList(
    approvedRes, 'reviews:approved',
    (MOCK.reviews_history || []).filter(h => h.decision === 'approved'),
  );
  const rejected = window.cc.data.extractList(
    rejectedRes, 'reviews:rejected',
    (MOCK.reviews_history || []).filter(h => h.decision === 'rejected'),
  );
  const canDecide = role !== 'viewer';

  const handleNLIntent = (parsed, raw) => {
    // backend aggregation needed: POST /api/v1/reviews/parse-intent
    toast.toast({ title: t('reviews.nl.intent_applied'), desc: `${parsed.action} · ${parsed.target}${parsed.risk ? ' · risk=' + parsed.risk : ''}`, tone: 'success' });
  };

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-3 justify-between">
        <Tabs value={tab} onChange={setTab} items={[
          { value: 'pending',        label: t('tab_pending'),                    count: pending.length },
          { value: 'approved',       label: t('approved_tab'),                   count: approved.length },
          { value: 'rejected',       label: t('rejected_tab'),                   count: rejected.length },
          { value: 'audit_agent',    label: t('reviews.audit_agent_tab') },
          { value: 'policy_browser', label: t('reviews.policy_browser_tab') },
        ]} />
        <div className="text-[11px] text-fg-3 mono flex items-center gap-2">
          <span className="w-1.5 h-1.5 rounded-full bg-emerald-500 pulse-dot" />
          SSE · live
        </div>
      </div>

      {/* Migration banner — approval moved to primary Chat page (Sprint 12 L2) */}
      {tab === 'pending' && (
        <Card className="p-3 bg-accent-soft border border-[var(--accent)]/30">
          <div className="flex items-start gap-3">
            <I.Info size={16} className="text-[var(--accent)] shrink-0 mt-0.5" />
            <div className="text-[12px]">
              <strong className="text-fg">{t('reviews.moved_to_chat_title')}</strong>
              <div className="text-fg-3 mt-1">{t('reviews.moved_to_chat_body')}</div>
              <button
                className="mt-2 text-[var(--accent)] hover:underline text-[12px] font-medium transition-colors"
                onClick={() => window.dispatchEvent(new CustomEvent('cyberclaw:nav', { detail: { page: 'chat' } }))}
              >
                {t('reviews.open_chat')} →
              </button>
            </div>
          </div>
        </Card>
      )}

      {tab === 'pending' && (
        pending.length ? (
          <div className="space-y-3">
            {pending.map(r => (
              <Card key={r.review_id} className={`overflow-hidden ${r.risk_level === 'critical' ? 'border-rose-600/40' : r.risk_level === 'high' ? 'border-orange-500/40' : ''}`}>
                <div className={`px-4 py-2 border-b border-border flex items-center justify-between ${r.risk_level === 'critical' ? 'bg-rose-600/5' : r.risk_level === 'high' ? 'bg-orange-500/5' : 'bg-bg-3'}`}>
                  <div className="flex items-center gap-2.5 flex-wrap">
                    <RiskBadge level={r.risk_level} />
                    {r.target?.type && <TargetBadge type={r.target.type} lang={lang} />}
                    <span className="mono text-[12px] text-fg">{r.review_id}</span>
                    <span className="text-fg-4">→</span>
                    <span className="mono text-[12px] text-fg-2">{r.capability_id}</span>
                    <span className="text-fg-4">·</span>
                    <span className="text-[12px] text-fg-3">by <span className="mono text-fg-2">{r.actor}</span></span>
                  </div>
                  <span className="text-[11px] mono text-fg-3">{relTime(r.requested_at)}</span>
                </div>

                <div className="grid grid-cols-[1fr_380px] gap-0">
                  <div className="p-4 border-r border-border">
                    <div className="text-[11px] text-fg-3 uppercase tracking-wider mb-1.5">{t('reason_for_review')}</div>
                    <div className="text-[13px] text-fg-2 leading-relaxed">{r.reason_for_review}</div>

                    <div className="mt-4 flex items-center gap-2">
                      {canDecide ? (
                        <>
                          <Button variant="success" size="md" onClick={async () => {
                            try {
                              await window.cc.api.reviews.approve(r.review_id);
                              window.cc.data.invalidate('reviews:pending');
                              window.cc.data.invalidate('reviews:approved');
                              window.cc.data.invalidate('dashboard');
                              toast.toast({ title: t('approved_toast'), desc: `${r.review_id} → ${r.capability_id}`, tone: 'success' });
                            } catch (err) {
                              toast.toast({ title: t('approved_toast'), desc: `(offline) ${r.review_id}`, tone: 'error' });
                            }
                          }}>
                            <I.Check size={14} /> {t('approve')}
                          </Button>
                          <Button variant="danger" size="md" onClick={() => setRejectOf(r)}>
                            <I.Close size={14} /> {t('reject')}
                          </Button>
                          <Button variant="ghost" size="md" onClick={() => r.execution_id && onOpenTrace({ execution_id: r.execution_id, agent: r.actor, capability: r.capability_id, status: 'pending_review', risk_level: r.risk_level, started_at: r.requested_at, duration: '—' })}>
                            <I.Activity size={13} /> {t('view_trace')}
                          </Button>
                        </>
                      ) : <div className="text-[12px] text-fg-3 italic">{t('viewer_restricted')}</div>}
                    </div>
                  </div>

                  <div className="p-4 bg-bg-3/40">
                    <div className="text-[11px] text-fg-3 uppercase tracking-wider mb-1.5">{t('proposed_input')}</div>
                    <JsonViewer value={r.proposed_input} />
                  </div>
                </div>
              </Card>
            ))}
          </div>
        ) : (
          <EmptyState icon={I.CheckCircle} title={t('all_caught_up')} subtitle={t('no_pending_sub')} />
        )
      )}

      {tab === 'approved' && (
        <Card className="overflow-hidden">
          <div className="grid grid-cols-[130px_1fr_100px_140px_110px_110px] gap-3 px-4 py-2 text-[10px] uppercase tracking-wider text-fg-4 mono border-b border-border">
            <div>review_id</div><div>capability</div><div>risk</div><div>actor</div><div>by</div><div className="text-right">decided</div>
          </div>
          {approved.map(h => (
            <div key={h.review_id} className="grid grid-cols-[130px_1fr_100px_140px_110px_110px] gap-3 px-4 row-pad border-b border-border/60 last:border-b-0 text-[12px] items-center">
              <div className="mono text-fg-3 truncate">{h.review_id}</div>
              <div className="mono text-fg truncate">{h.capability_id}</div>
              <div><RiskBadge level={h.risk_level} /></div>
              <div className="mono text-fg-2 truncate">{h.actor}</div>
              <div className="mono text-fg-3">{h.decided_by}</div>
              <div className="mono text-fg-3 text-right">{relTime(h.decided_at)}</div>
            </div>
          ))}
          {!approved.length && <EmptyState icon={I.Check} title={t('no_approvals')} />}
        </Card>
      )}

      {tab === 'rejected' && (
        <Card className="overflow-hidden">
          {rejected.length ? rejected.map(h => (
            <div key={h.review_id} className="px-4 py-3 border-b border-border/60 last:border-b-0">
              <div className="flex items-center gap-2 flex-wrap">
                <span className="mono text-[12px] text-fg">{h.review_id}</span>
                <RiskBadge level={h.risk_level} />
                <span className="mono text-[12px] text-fg-2">{h.capability_id}</span>
                <span className="text-fg-4">·</span>
                <span className="text-[12px] text-fg-3">by <span className="mono text-fg-2">{h.actor}</span></span>
                <span className="ml-auto mono text-[11px] text-fg-4">{relTime(h.decided_at)} · {h.decided_by}</span>
              </div>
              {h.reason && <div className="mt-1.5 text-[12px] text-rose-300 bg-rose-500/5 border border-rose-500/15 rounded-md px-2.5 py-1.5 mono">{h.reason}</div>}
            </div>
          )) : <EmptyState icon={I.Close} title={t('no_rejections')} />}
        </Card>
      )}

      {tab === 'audit_agent' && <AuditAgentTab lang={lang} />}

      {tab === 'policy_browser' && <PolicyBrowserTab lang={lang} />}

      <RejectDialog open={!!rejectOf} review={rejectOf} lang={lang} onClose={() => setRejectOf(null)}
        onReject={async (reason) => {
          const rid = rejectOf.review_id;
          setRejectOf(null);
          try {
            await window.cc.api.reviews.reject(rid, reason);
            window.cc.data.invalidate('reviews:pending');
            window.cc.data.invalidate('reviews:rejected');
            window.cc.data.invalidate('dashboard');
            toast.toast({ title: t('rejected_toast'), desc: `${rid} — ${t('actor_notified')}`, tone: 'error' });
          } catch (err) {
            toast.toast({ title: t('rejected_toast'), desc: `(offline) ${rid}`, tone: 'error' });
          }
        }}
      />
    </div>
  );
};

// ---- Capabilities (3-tab layout: Capabilities | Connectors | Tools) ----
const EFFECT_TONE = { Read: 'slate', Write: 'amber', Execute: 'rose' };

const CapabilitiesListPane = ({ lang }) => {
  const t = tFor(lang);
  const [open, setOpen] = useState({});
  const [q, setQ] = useState('');
  const [selected, setSelected] = useState(null);
  const toast = useToast();
  const capsRes = window.cc.data.useCapabilities();
  const { loading, error } = capsRes;
  // Backend returns a FLAT list of capabilities tagged with connector_id.
  // The UI expects a list of connectors each with a `capabilities` sub-array,
  // so we group client-side. MOCK.capabilities is already pre-grouped.
  const flatOrGrouped = window.cc.data.extractList(capsRes, 'capabilities', MOCK.capabilities);
  const caps = (() => {
    if (!Array.isArray(flatOrGrouped) || flatOrGrouped.length === 0) return flatOrGrouped;
    // Detect shape: grouped entries have `capabilities` array; flat have `id` + `connector_id`.
    if (flatOrGrouped[0] && Array.isArray(flatOrGrouped[0].capabilities)) return flatOrGrouped;
    // Flat → group by connector_id.
    const groups = {};
    flatOrGrouped.forEach(cap => {
      const cid = cap.connector_id || 'unknown';
      if (!groups[cid]) groups[cid] = { connector_id: cid, name: cid, capabilities: [] };
      groups[cid].capabilities.push({
        id: cap.id,
        name: cap.title || cap.name || cap.id,
        description: cap.description || '',
        risk_level: (cap.risk_level || 'low').toString().toLowerCase(),
        effects: (cap.effects || []).map(e => (typeof e === 'string' ? e.toLowerCase() : e)),
      });
    });
    return Object.values(groups);
  })();
  useEffect(() => {
    const init = {};
    caps.forEach((c, i) => init[c.connector_id] = i < 2);
    setOpen(init);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [caps.length]);
  const filtered = caps
    .map(c => ({ ...c, capabilities: (c.capabilities || []).filter(x => !q || (x.id || '').includes(q) || (x.name || '').toLowerCase().includes(q.toLowerCase())) }))
    .filter(c => c.capabilities.length);
  return (
    <div className="space-y-4">
      <PageToolbar
        left={
          <div className="relative">
            <I.Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-fg-4" />
            <Input value={q} onChange={(e) => setQ(e.target.value)} className="pl-7 w-64" placeholder={t('search_capabilities')} />
          </div>
        }
        right={<div className="text-[11px] mono text-fg-3">{caps.length} {t('connectors')} · {caps.reduce((s, c) => s + ((c.capabilities || []).length), 0)} {t('capabilities_count')}</div>}
      />
      {error && !caps.length && <ErrorBanner message={error} onRetry={() => window.cc.data.invalidate('capabilities')} />}
      {loading && !caps.length && <SkeletonRows rows={4} cols={2} />}
      <div className="space-y-3">
        {filtered.map(c => (
          <Card key={c.connector_id} className="overflow-hidden">
            <button onClick={() => setOpen(o => ({ ...o, [c.connector_id]: !o[c.connector_id] }))} className="w-full flex items-center justify-between px-4 py-3 hover:bg-[var(--hover)]">
              <div className="flex items-center gap-3">
                <I.ChevronRight size={14} className={`text-fg-4 transition-transform ${open[c.connector_id] ? 'rotate-90' : ''}`} />
                <div className="h-7 w-7 rounded-md bg-bg-3 border border-border flex items-center justify-center mono text-[11px] text-fg-2 uppercase">{c.name.slice(0, 2)}</div>
                <div className="text-left">
                  <div className="text-[13px] font-medium">{c.name}</div>
                  <div className="text-[11px] text-fg-3 mono">{c.connector_id} · {c.capabilities.length} capabilities</div>
                </div>
              </div>
              <Badge tone="emerald">{t('connected')}</Badge>
            </button>
            {open[c.connector_id] && (
              <div className="border-t border-border">
                {c.capabilities.map(cap => (
                  <button key={cap.id} onClick={() => setSelected({ ...cap, connector: c })} className="w-full text-left grid grid-cols-[1fr_110px_200px_90px] gap-3 px-4 py-3 border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)] text-[12px] items-center">
                    <div>
                      <div className="mono text-fg">{cap.id}</div>
                      <div className="text-[11.5px] text-fg-3 mt-0.5">{cap.description}</div>
                    </div>
                    <div><RiskBadge level={cap.risk_level} /></div>
                    <div className="flex gap-1 flex-wrap">
                      {(cap.effects || []).map(e => <Badge key={e} tone={EFFECT_TONE[e]}>{e}</Badge>)}
                    </div>
                    <div className="text-right">
                      <Button variant="outline" size="xs"><I.Play size={10} /> {t('test')}</Button>
                    </div>
                  </button>
                ))}
              </div>
            )}
          </Card>
        ))}
      </div>

      {selected && <CapabilityTestSheet cap={selected} lang={lang} open={!!selected} onClose={() => setSelected(null)} onTest={(input) => { toast.toast({ title: t('capability_executed'), desc: `${selected.id} → 200 OK`, tone: 'success' }); setSelected(null); }} />}
    </div>
  );
};

const ConnectorsPane = ({ lang }) => {
  const t = tFor(lang);
  const [selected, setSelected] = useState(null);
  const connRes = window.cc.data.useConnectors();
  const { loading, error } = connRes;
  const connectors = window.cc.data.extractList(connRes, 'connectors', []);
  return (
    <div className="space-y-4">
      {error && !connectors.length && <ErrorBanner message={error} onRetry={() => window.cc.data.invalidate('connectors')} />}
      <Card className="overflow-hidden">
        <div className="grid grid-cols-[160px_1fr_110px_110px_110px_100px] gap-3 px-4 py-2 text-[10px] uppercase tracking-wider text-fg-4 mono border-b border-border">
          <div>{t('col_connector_id')}</div>
          <div>{t('col_name')}</div>
          <div>{t('col_runtime')}</div>
          <div>{t('capabilities_count')}</div>
          <div>{t('col_risk')}</div>
          <div>{t('col_status')}</div>
        </div>
        {loading && !connectors.length ? <SkeletonRows rows={5} cols={6} /> : connectors.map(c => (
          <div key={c.connector_id}
            onClick={() => setSelected(c)}
            className="grid grid-cols-[160px_1fr_110px_110px_110px_100px] gap-3 px-4 row-pad border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)] text-[12px] items-center cursor-pointer">
            <div className="mono text-fg truncate">{c.connector_id}</div>
            <div className="text-fg-2 truncate">
              <div className="font-medium">{c.name}</div>
              {c.description && <div className="text-[11px] text-fg-3 truncate">{c.description}</div>}
            </div>
            <div><Badge tone="violet">{c.runtime || '—'}</Badge></div>
            <div className="mono text-fg-3">{c.capability_count != null ? c.capability_count : '—'}</div>
            <div>{c.risk_level ? <RiskBadge level={c.risk_level} /> : <span className="text-fg-4 mono text-[11px]">—</span>}</div>
            <div>{c.status ? <StatusBadge status={c.status} /> : <span className="text-fg-4 mono text-[11px]">—</span>}</div>
          </div>
        ))}
        {!loading && !connectors.length && <EmptyState icon={I.Plug} title="no connectors" />}
      </Card>
      <Sheet open={!!selected} onClose={() => setSelected(null)} title={selected?.name || ''} subtitle={selected?.connector_id} width={620}>
        {selected && (
          <div className="p-5 space-y-4">
            <div className="flex items-center gap-2 flex-wrap">
              {selected.runtime && <Badge tone="violet">runtime: {selected.runtime}</Badge>}
              {selected.risk_level && <RiskBadge level={selected.risk_level} />}
              {selected.status && <StatusBadge status={selected.status} />}
            </div>
            {selected.description && <div className="text-[13px] text-fg-2">{selected.description}</div>}
            <div className="grid grid-cols-2 gap-3">
              <InfoRow label="connector_id" value={selected.connector_id} mono />
              <InfoRow label="name" value={selected.name} />
              <InfoRow label="runtime" value={selected.runtime || '—'} mono />
              <InfoRow label="capability_count" value={selected.capability_count != null ? selected.capability_count : '—'} mono />
            </div>
            <div>
              <div className="text-[11px] text-fg-3 uppercase tracking-wider mb-1.5">Raw</div>
              <JsonViewer value={selected} />
            </div>
          </div>
        )}
      </Sheet>
    </div>
  );
};

const ToolsPane = ({ lang }) => {
  const t = tFor(lang);
  const [q, setQ] = useState('');
  const [selected, setSelected] = useState(null);
  const toolsRes = window.cc.data.useTools();
  const { loading, error } = toolsRes;
  const tools = window.cc.data.extractList(toolsRes, 'tools', []);
  const filtered = tools.filter(x => !q ||
    (x.tool_id || '').toLowerCase().includes(q.toLowerCase()) ||
    (x.name || '').toLowerCase().includes(q.toLowerCase()) ||
    (x.connector_id || '').toLowerCase().includes(q.toLowerCase()));
  return (
    <div className="space-y-4">
      <PageToolbar
        left={
          <div className="relative">
            <I.Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-fg-4" />
            <Input value={q} onChange={(e) => setQ(e.target.value)} className="pl-7 w-64" placeholder={t('search_capabilities')} />
          </div>
        }
        right={<div className="text-[11px] mono text-fg-3">{tools.length} {t('tools')}</div>}
      />
      {error && !tools.length && <ErrorBanner message={error} onRetry={() => window.cc.data.invalidate('tools')} />}
      <Card className="overflow-hidden">
        <div className="grid grid-cols-[200px_1fr_140px_110px_200px] gap-3 px-4 py-2 text-[10px] uppercase tracking-wider text-fg-4 mono border-b border-border">
          <div>{t('col_tool_id')}</div>
          <div>{t('col_name')}</div>
          <div>{t('col_connector_id')}</div>
          <div>{t('col_risk_level')}</div>
          <div>{t('col_effects')}</div>
        </div>
        {loading && !tools.length ? <SkeletonRows rows={6} cols={5} /> : filtered.map(x => (
          <div key={x.tool_id}
            onClick={() => setSelected(x)}
            className="grid grid-cols-[200px_1fr_140px_110px_200px] gap-3 px-4 row-pad border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)] text-[12px] items-center cursor-pointer">
            <div className="mono text-fg truncate">{x.tool_id}</div>
            <div className="text-fg-2 truncate">
              <div className="font-medium">{x.name}</div>
              {x.description && <div className="text-[11px] text-fg-3 truncate">{x.description}</div>}
            </div>
            <div className="mono text-fg-3 truncate">{x.connector_id || '—'}</div>
            <div>{x.risk_level ? <RiskBadge level={x.risk_level} /> : <span className="text-fg-4 mono text-[11px]">—</span>}</div>
            <div className="flex gap-1 flex-wrap">
              {(x.effects || []).map(e => <Badge key={e} tone={EFFECT_TONE[e] || 'slate'}>{e}</Badge>)}
            </div>
          </div>
        ))}
        {!loading && !filtered.length && <EmptyState icon={I.Plug} title="no tools" />}
      </Card>
      <Sheet open={!!selected} onClose={() => setSelected(null)} title={selected?.name || ''} subtitle={selected?.tool_id} width={760}>
        {selected && (
          <div className="p-5 space-y-4">
            <div className="flex items-center gap-2 flex-wrap">
              {selected.connector_id && <Badge tone="slate">connector: {selected.connector_id}</Badge>}
              {selected.risk_level && <RiskBadge level={selected.risk_level} />}
              {(selected.effects || []).map(e => <Badge key={e} tone={EFFECT_TONE[e] || 'slate'}>{e}</Badge>)}
            </div>
            {selected.description && <div className="text-[13px] text-fg-2">{selected.description}</div>}
            {selected.input_schema && (
              <div>
                <div className="text-[11px] text-fg-3 uppercase tracking-wider mb-1.5">{t('input_schema')}</div>
                <JsonViewer value={selected.input_schema} />
              </div>
            )}
            {selected.output_schema && (
              <div>
                <div className="text-[11px] text-fg-3 uppercase tracking-wider mb-1.5">output_schema</div>
                <JsonViewer value={selected.output_schema} />
              </div>
            )}
          </div>
        )}
      </Sheet>
    </div>
  );
};

// ---- F3: Capability Discover for Goal panel ----
const CapDiscoverPane = ({ lang }) => {
  const t = tFor(lang || 'en');
  const toast = window.cc.ui && window.cc.ui.useToast ? window.cc.ui.useToast() : { show: () => {} };
  const [form, setForm] = useState({ deliverable_kind: '', search_terms: '', include_remote: false });
  const [result, setResult] = useState(null);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState(null);

  const set = (k, v) => setForm(f => ({ ...f, [k]: v }));

  const submit = async () => {
    setLoading(true); setErr(null); setResult(null);
    try {
      const terms = form.search_terms.split(',').map(s => s.trim()).filter(Boolean);
      const res = await window.cc.api.capabilities.discoverForGoal({
        deliverable_kind: form.deliverable_kind.trim() || undefined,
        search_terms: terms.length > 0 ? terms : undefined,
        include_remote: form.include_remote,
      });
      setResult(res);
    } catch (e) {
      setErr(e && e.message ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  const { Card, CardHeader, Button, Badge, Input, ErrorBanner, EmptyState } = window.cc.ui || window;

  const renderList = (items, label, tone) => {
    if (!items || items.length === 0) return null;
    return (
      <div className="space-y-1.5">
        <div className="text-[11px] uppercase tracking-wider text-fg-3 mb-2">{label}</div>
        {items.map((item, i) => {
          const name = item.name || item.capability_id || item.id || (typeof item === 'string' ? item : JSON.stringify(item));
          const pending = item.pending || item.not_installed;
          return (
            <div key={i} className="flex items-center gap-2 py-1.5 px-3 rounded-md bg-[var(--bg-3)] border border-border/50">
              <span className="mono text-[12px] text-fg flex-1">{name}</span>
              {pending && <Badge tone="amber">{t('cap.discover.pending_hint')}</Badge>}
              {!pending && <Badge tone={tone}>{label}</Badge>}
            </div>
          );
        })}
      </div>
    );
  };

  return (
    <div className="space-y-4">
      <Card>
        <CardHeader title={t('cap.discover.title')} subtitle="POST /api/v1/capabilities/discover_for_goal" />
        <div className="p-4 space-y-3">
          {err && <ErrorBanner message={err} />}
          <div>
            <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider mono">{t('cap.discover.deliverable')}</label>
            <Input
              value={form.deliverable_kind}
              onChange={e => set('deliverable_kind', e.target.value)}
              placeholder={t('cap.discover.deliverable_placeholder')}
              className="mono"
            />
          </div>
          <div>
            <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider mono">{t('cap.discover.search_terms')}</label>
            <Input
              value={form.search_terms}
              onChange={e => set('search_terms', e.target.value)}
              placeholder={t('cap.discover.search_terms_placeholder')}
              className="mono"
            />
          </div>
          <div className="flex items-center gap-2">
            <input
              type="checkbox"
              id="cap-include-remote"
              checked={form.include_remote}
              onChange={e => set('include_remote', e.target.checked)}
              className="rounded"
            />
            <label htmlFor="cap-include-remote" className="text-[12px] text-fg-2 cursor-pointer">{t('cap.discover.include_remote')}</label>
          </div>
          <div>
            <Button onClick={submit} disabled={loading || (!form.deliverable_kind.trim() && !form.search_terms.trim())}>
              <I.Discover size={13} />
              {loading ? t('cap.discover.submitting') : t('cap.discover.submit')}
            </Button>
          </div>
        </div>
      </Card>

      {result && (
        <Card>
          <CardHeader title={t('cap.discover.result')} />
          <div className="p-4 space-y-4">
            {renderList(result.native, t('cap.discover.native'), 'emerald')}
            {renderList(result.installed_skills, t('cap.discover.installed_skills'), 'accent')}
            {renderList(result.cmd_runtime, t('cap.discover.cmd_runtime'), 'cyan')}
            {!result.native?.length && !result.installed_skills?.length && !result.cmd_runtime?.length && (
              <EmptyState icon={I.Discover} title={t('cap.discover.empty')} />
            )}
            <details className="mt-2">
              <summary className="text-[11px] text-fg-4 cursor-pointer hover:text-fg-3 mono">raw json</summary>
              <pre className="mt-2 p-3 bg-bg-3 rounded text-[11px] mono text-fg-2 overflow-auto max-h-64">{JSON.stringify(result, null, 2)}</pre>
            </details>
          </div>
        </Card>
      )}
    </div>
  );
};

const CapabilitiesPage = ({ lang }) => {
  const t = tFor(lang);
  const [tab, setTab] = useState('capabilities');
  return (
    <div className="space-y-4">
      <Tabs value={tab} onChange={setTab} items={[
        { value: 'capabilities', label: t('capabilities_tab_capabilities') },
        { value: 'connectors',   label: t('capabilities_tab_connectors') },
        { value: 'tools',        label: t('capabilities_tab_tools') },
        { value: 'discover',     label: t('cap.discover.title') },
      ]} />
      {tab === 'capabilities' && <CapabilitiesListPane lang={lang} />}
      {tab === 'connectors'   && <ConnectorsPane lang={lang} />}
      {tab === 'tools'        && <ToolsPane lang={lang} />}
      {tab === 'discover'     && <CapDiscoverPane lang={lang} />}
    </div>
  );
};

// ---- Audit ----
const AUDIT_KIND_TONE = {
  Auth: 'accent', auth: 'accent',
  Mutation: 'amber', mutation: 'amber',
  Config: 'violet', config: 'violet',
  Security: 'rose', security: 'rose',
};
function auditKindTone(kind) {
  return AUDIT_KIND_TONE[kind] || 'slate';
}
function auditResultIsSuccess(result) {
  if (result === 'success' || result === 'Success') return true;
  if (result && typeof result === 'object' && (result.Success !== undefined || result.success !== undefined)) return true;
  return false;
}
function auditResultText(result) {
  if (!result) return '—';
  if (typeof result === 'string') return result;
  if (typeof result === 'object') {
    const key = Object.keys(result)[0];
    return key || JSON.stringify(result);
  }
  return String(result);
}

// ---- C4: Runtime Security Timeline ----
// Mock data — TODO: wire GET /api/v1/security/runtime/timeline?hours=24
const MOCK_RUNTIME_EVENTS = [
  { id: 're1', rule: 'mkfs.*', severity: 'critical', action: 'denied', actor: 'agent.ops.deployer', trace_id: 'tr_a1b2', ts: Date.now() - 3600000 * 2.1, offset_h: 2.1 },
  { id: 're2', rule: 'rm -rf /', severity: 'critical', action: 'denied', actor: 'agent.ops.cleaner', trace_id: 'tr_c3d4', ts: Date.now() - 3600000 * 5.4, offset_h: 5.4 },
  { id: 're3', rule: 'wallet.transfer', severity: 'high', action: 'ask', actor: 'agent.finance.treasury', trace_id: 'tr_e5f6', ts: Date.now() - 3600000 * 8.7, offset_h: 8.7 },
  { id: 're4', rule: 'sudo *', severity: 'high', action: 'ask', actor: 'agent.ops.deployer', trace_id: 'tr_g7h8', ts: Date.now() - 3600000 * 11.2, offset_h: 11.2 },
  { id: 're5', rule: 'db.drop_table', severity: 'critical', action: 'denied', actor: 'agent.db.migrator', trace_id: 'tr_i9j0', ts: Date.now() - 3600000 * 14.5, offset_h: 14.5 },
  { id: 're6', rule: 'git.force_push', severity: 'medium', action: 'ask', actor: 'agent.dev.coder', trace_id: 'tr_k1l2', ts: Date.now() - 3600000 * 17.0, offset_h: 17.0 },
  { id: 're7', rule: 'stripe.create_refund', severity: 'medium', action: 'ask', actor: 'agent.finance.support', trace_id: 'tr_m3n4', ts: Date.now() - 3600000 * 20.3, offset_h: 20.3 },
  { id: 're8', rule: 'k8s.rollout', severity: 'low', action: 'passed', actor: 'agent.ops.deployer', trace_id: 'tr_o5p6', ts: Date.now() - 3600000 * 22.8, offset_h: 22.8 },
];

const RULE_TRACKS = ['mkfs.*', 'rm -rf /', 'wallet.transfer', 'sudo *', 'db.drop_table', 'git.force_push', 'stripe.create_refund', 'k8s.rollout'];

function severityBarColor(severity, action) {
  if (action === 'denied' || severity === 'critical') return 'bg-rose-600';
  if (action === 'ask' || severity === 'high') return 'bg-amber-500';
  if (severity === 'medium') return 'bg-amber-400/70';
  return 'bg-sky-500/60';
}
function severityBadgeTone(severity) {
  if (severity === 'critical') return 'rose';
  if (severity === 'high') return 'amber';
  if (severity === 'medium') return 'amber';
  return 'slate';
}

const RuntimeSecurityTimeline = ({ lang }) => {
  const t = tFor(lang || 'en');
  const [selectedEvent, setSelectedEvent] = useState(null);
  const [ruleDetailOpen, setRuleDetailOpen] = useState(false);
  // Sprint 18 W2 — wired to GET /api/v1/runtime/timeline.
  const eventsRes = window.cc.data.useRuntimeTimeline();
  const events = eventsRes.data?.events ?? (eventsRes.error ? MOCK_RUNTIME_EVENTS : []);
  const ticks = ['00:00', '06:00', '12:00', '18:00', t('audit.runtime.now') || 'now'];
  const tickOffsets = [0, 25, 50, 75, 100];

  // KPI counts
  const denied = events.filter(e => e.action === 'denied').length;
  const asks = events.filter(e => e.action === 'ask').length;
  const passed = events.filter(e => e.action === 'passed').length;

  return (
    <div className="space-y-4">
      {/* KPI row */}
      <div className="grid grid-cols-4 gap-3">
        <StatCard label={t('audit.runtime.kpi_denied') || 'Denied (24h)'} value={denied} tone="rose" />
        <StatCard label={t('audit.runtime.kpi_ask') || 'Ask / Sensitive'} value={asks} tone="amber" />
        <StatCard label={t('audit.runtime.kpi_passed') || 'Passed'} value={passed} tone="emerald" />
        <StatCard label={t('audit.runtime.kpi_rules') || 'Active rules'} value={RULE_TRACKS.length} tone="slate" />
      </div>

      {/* Legend */}
      <div className="flex items-center gap-4 text-[11px] mono text-fg-3">
        <span className="flex items-center gap-1.5"><span className="w-3 h-2 rounded-sm bg-rose-600 inline-block" /> {t('audit.runtime.denied') || 'denied / critical'}</span>
        <span className="flex items-center gap-1.5"><span className="w-3 h-2 rounded-sm bg-amber-500 inline-block" /> {t('audit.runtime.ask') || 'ask / sensitive'}</span>
        <span className="flex items-center gap-1.5"><span className="w-3 h-2 rounded-sm bg-sky-500/60 inline-block" /> {t('audit.runtime.passed') || 'passed'}</span>
      </div>

      {/* Timeline grid: 6 cols — rule label + 4 hour-segment cells + hit count */}
      <Card className="overflow-hidden">
        {/* Time axis header */}
        <div className="grid grid-cols-[160px_1fr_1fr_1fr_1fr_50px] gap-0 border-b border-border px-4 py-2">
          <div className="text-[10px] mono text-fg-4 uppercase tracking-wider">{t('audit.runtime.col_rule') || 'rule'}</div>
          {ticks.slice(0, 4).map((tick, i) => (
            <div key={tick} className="text-[10px] mono text-fg-4 text-center">{tick}</div>
          ))}
          <div className="text-[10px] mono text-fg-4 text-right">{t('audit.runtime.col_hits') || 'hits'}</div>
        </div>

        {/* One row per rule track */}
        {RULE_TRACKS.map(rule => {
          const ruleEvents = events.filter(e => e.rule === rule);
          const hitCount = ruleEvents.length;
          // Each 6-hour segment: 0-6h, 6-12h, 12-18h, 18-24h ago
          const segments = [
            ruleEvents.filter(e => e.offset_h >= 18 && e.offset_h < 24),
            ruleEvents.filter(e => e.offset_h >= 12 && e.offset_h < 18),
            ruleEvents.filter(e => e.offset_h >= 6 && e.offset_h < 12),
            ruleEvents.filter(e => e.offset_h >= 0 && e.offset_h < 6),
          ];
          return (
            <div key={rule} className="grid grid-cols-[160px_1fr_1fr_1fr_1fr_50px] gap-0 border-b border-border/60 last:border-b-0 items-center min-h-[40px]">
              <div className="px-4 py-2 mono text-[11px] text-fg truncate" title={rule}>{rule}</div>
              {segments.map((segEvents, si) => (
                <div key={si} className="px-1 py-2 flex items-center gap-1 flex-wrap min-h-[40px]">
                  {segEvents.length === 0 ? (
                    <div className="w-full h-1.5 rounded-full bg-bg-3" />
                  ) : segEvents.map(ev => (
                    <button
                      key={ev.id}
                      title={`${ev.actor} · ${ev.action} · ${new Date(ev.ts).toLocaleTimeString()}`}
                      onClick={() => { setSelectedEvent(ev); setRuleDetailOpen(true); }}
                      className={`h-5 rounded-sm cursor-pointer transition-opacity hover:opacity-80 focus:outline-none focus:ring-1 focus:ring-offset-1 focus:ring-[var(--accent)] ${severityBarColor(ev.severity, ev.action)}`}
                      style={{ width: `${Math.max(18, 100 / Math.max(segEvents.length, 1))}%` }}
                      aria-label={`${rule} hit by ${ev.actor}`}
                    />
                  ))}
                </div>
              ))}
              <div className="px-3 py-2 text-right mono text-[11px] text-fg-3">{hitCount || '—'}</div>
            </div>
          );
        })}
      </Card>

      {/* Rule detail sheet */}
      <Sheet open={ruleDetailOpen} onClose={() => { setRuleDetailOpen(false); setSelectedEvent(null); }}
        title={selectedEvent ? selectedEvent.rule : ''}
        subtitle={selectedEvent ? `trace: ${selectedEvent.trace_id}` : ''}
        width={560}>
        {selectedEvent && (
          <div className="p-5 space-y-4">
            <div className="flex items-center gap-2 flex-wrap">
              <Badge tone={severityBadgeTone(selectedEvent.severity)}>{selectedEvent.severity}</Badge>
              <Badge tone={selectedEvent.action === 'denied' ? 'rose' : selectedEvent.action === 'ask' ? 'amber' : 'emerald'}>{selectedEvent.action}</Badge>
              <Badge tone="slate">actor: {selectedEvent.actor}</Badge>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <InfoRow label={t('audit.runtime.col_rule') || 'rule'} value={selectedEvent.rule} mono />
              <InfoRow label="trace_id" value={selectedEvent.trace_id} mono />
              <InfoRow label="actor" value={selectedEvent.actor} mono />
              <InfoRow label="time" value={new Date(selectedEvent.ts).toLocaleString()} mono />
            </div>
            <div className="bg-bg-3 border border-border rounded-md p-3 mono text-[11px] text-fg-3">
              {t('audit.runtime.todo_endpoint') || '// TODO: GET /api/v1/security/runtime/timeline?hours=24'}
            </div>
          </div>
        )}
      </Sheet>
    </div>
  );
};

// ---- C4: Injection Hits ----
// Mock data — TODO: wire GET /api/v1/security/injection/hits?since=
const MOCK_INJECTION_HITS = [
  { id: 'ih1', severity: 'CRIT', pattern: 'ignore (previous|all) instructions', actor: 'agent.support.responder', trace_id: 'tr_inj_001', rule_id: 'sanitizer.prompt_override', ts: Date.now() - 1800000 },
  { id: 'ih2', severity: 'HIGH', pattern: 'SECRET_KEY=sk-...', actor: 'agent.dev.coder', trace_id: 'tr_inj_002', rule_id: 'sanitizer.credential_leak', ts: Date.now() - 3600000 * 3 },
  { id: 'ih3', severity: 'HIGH', pattern: 'ANTHROPIC_API_KEY=...', actor: 'agent.research.analyst', trace_id: 'tr_inj_003', rule_id: 'sanitizer.credential_leak', ts: Date.now() - 3600000 * 6 },
  { id: 'ih4', severity: 'MED', pattern: '[\u202a-\u202e]', actor: 'agent.ops.deployer', trace_id: 'tr_inj_004', rule_id: 'sanitizer.invisible_unicode', ts: Date.now() - 3600000 * 9 },
  { id: 'ih5', severity: 'CRIT', pattern: 'disregard your system prompt', actor: 'agent.finance.treasury', trace_id: 'tr_inj_005', rule_id: 'sanitizer.prompt_override', ts: Date.now() - 3600000 * 12 },
  { id: 'ih6', severity: 'HIGH', pattern: 'print(os.environ)', actor: 'agent.dev.coder', trace_id: 'tr_inj_006', rule_id: 'sanitizer.env_probe', ts: Date.now() - 3600000 * 18 },
];

const INJECTION_SEVERITY_TONE = { CRIT: 'rose', HIGH: 'amber', MED: 'slate' };

const InjectionHitsPane = ({ lang }) => {
  const t = tFor(lang || 'en');
  const [selected, setSelected] = useState(null);
  // Sprint 18 W2 — wired to GET /api/v1/security/injection/hits.
  const hitsRes = window.cc.data.useInjectionHits();
  const hits = hitsRes.data?.hits ?? (hitsRes.error ? MOCK_INJECTION_HITS : []);
  const critCount = hits.filter(h => h.severity === 'CRIT').length;
  const highCount = hits.filter(h => h.severity === 'HIGH').length;
  const medCount = hits.filter(h => h.severity === 'MED').length;

  return (
    <div className="space-y-4">
      {/* KPI row */}
      <div className="grid grid-cols-3 gap-3">
        <StatCard label={t('audit.injection.kpi_crit') || 'CRIT — prompt override'} value={critCount} tone="rose" />
        <StatCard label={t('audit.injection.kpi_high') || 'HIGH — credential / probe'} value={highCount} tone="amber" />
        <StatCard label={t('audit.injection.kpi_med') || 'MED — encoding abuse'} value={medCount} tone="slate" />
      </div>

      <Card className="overflow-hidden">
        <div className="flex items-center justify-between px-4 py-2 border-b border-border">
          <div className="grid grid-cols-[70px_1fr_160px_140px_100px] gap-3 flex-1 text-[10px] mono uppercase tracking-wider text-fg-4">
            <div>{t('audit.injection.col_severity') || 'severity'}</div>
            <div>{t('audit.injection.col_pattern') || 'hit pattern'}</div>
            <div>{t('audit.injection.col_actor') || 'actor'}</div>
            <div>{t('audit.injection.col_rule') || 'rule_id'}</div>
            <div className="text-right">{t('audit.injection.col_time') || 'time'}</div>
          </div>
          <Button variant="outline" size="xs" className="ml-3 shrink-0">
            <I.Download size={11} /> {t('audit.injection.export') || 'Export'}
          </Button>
        </div>
        {hits.map(h => (
          <button
            key={h.id}
            onClick={() => setSelected(selected?.id === h.id ? null : h)}
            className="w-full grid grid-cols-[70px_1fr_160px_140px_100px] gap-3 px-4 row-pad border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)] text-[12px] items-center text-left"
          >
            <div><Badge tone={INJECTION_SEVERITY_TONE[h.severity] || 'slate'}>{h.severity}</Badge></div>
            <div className="mono text-fg truncate text-[11px]" title={h.pattern}>{h.pattern}</div>
            <div className="mono text-fg-2 truncate text-[11px]">{h.actor}</div>
            <div className="mono text-fg-3 truncate text-[11px]">{h.rule_id}</div>
            <div className="mono text-fg-3 text-right text-[11px]">{relTime(new Date(h.ts).toISOString())}</div>
          </button>
        ))}
        {!hits.length && <EmptyState icon={I.Shield} title={t('audit.injection.empty') || 'No injection hits detected'} />}
      </Card>

      {/* Detail expanded */}
      {selected && (
        <Card className="p-4 space-y-3 border-l-2 border-rose-600">
          <div className="flex items-center gap-2 flex-wrap">
            <Badge tone={INJECTION_SEVERITY_TONE[selected.severity] || 'slate'}>{selected.severity}</Badge>
            <span className="mono text-[11px] text-fg-3">{selected.rule_id}</span>
            <span className="mono text-[11px] text-accent">{selected.trace_id}</span>
          </div>
          <div className="mono text-[12px] bg-bg-3 border border-border rounded-md px-3 py-2 text-rose-400 break-all">{selected.pattern}</div>
          <div className="grid grid-cols-2 gap-3">
            <InfoRow label="actor" value={selected.actor} mono />
            <InfoRow label="time" value={new Date(selected.ts).toLocaleString()} mono />
          </div>
          <div className="text-[11px] text-fg-4 mono">
            {t('audit.injection.todo_endpoint') || '// TODO: GET /api/v1/security/injection/hits?since='}
          </div>
        </Card>
      )}
    </div>
  );
};

// ---- C4: Permission Rules (read-only view) ----
// TODO: wire GET /api/v1/security/permission/rules (31 rules: 24 tool + 7 dangerous)
const MOCK_PERMISSION_RULES = [
  { id: 'pr1', source: 'DangerousCapabilityFilter', pattern: 'mkfs.*', action: 'DENY', severity: 'critical', enabled: true, reason: 'Filesystem formatting is always destructive' },
  { id: 'pr2', source: 'DangerousCapabilityFilter', pattern: 'rm -rf /', action: 'DENY', severity: 'critical', enabled: true, reason: 'Recursive root deletion' },
  { id: 'pr3', source: 'DangerousCapabilityFilter', pattern: 'wallet.transfer', action: 'ASK', severity: 'high', enabled: true, reason: 'Financial transfer requires approval' },
  { id: 'pr4', source: 'DangerousCapabilityFilter', pattern: 'db.drop_table', action: 'DENY', severity: 'critical', enabled: true, reason: 'Irreversible schema mutation' },
  { id: 'pr5', source: 'DangerousCapabilityFilter', pattern: 'sudo *', action: 'ASK', severity: 'high', enabled: true, reason: 'Privilege escalation' },
  { id: 'pr6', source: 'DangerousCapabilityFilter', pattern: 'git.force_push', action: 'ASK', severity: 'medium', enabled: true, reason: 'Rewrites history' },
  { id: 'pr7', source: 'DangerousCapabilityFilter', pattern: 'k8s.delete_namespace', action: 'DENY', severity: 'critical', enabled: true, reason: 'Namespace deletion is irreversible' },
  { id: 'pr8', source: 'ToolPermissionMatcher', pattern: 'github.merge_pr', action: 'ASK', severity: 'medium', enabled: true, reason: 'Merging PRs requires review' },
  { id: 'pr9', source: 'ToolPermissionMatcher', pattern: 'slack.post_channel', action: 'ALLOW', severity: 'low', enabled: true, reason: 'Low-risk communication' },
  { id: 'pr10', source: 'ToolPermissionMatcher', pattern: 'stripe.create_refund', action: 'ASK', severity: 'high', enabled: true, reason: 'Financial mutation' },
  { id: 'pr11', source: 'ToolPermissionMatcher', pattern: 'k8s.rollout', action: 'ASK', severity: 'medium', enabled: true, reason: 'Deployment change' },
  { id: 'pr12', source: 'ToolPermissionMatcher', pattern: 'read.*', action: 'ALLOW', severity: 'low', enabled: true, reason: 'Read-only operations always allowed' },
];

const ACTION_BADGE_TONE = { DENY: 'rose', ASK: 'amber', ALLOW: 'emerald' };

const PermissionRulesPane = ({ lang }) => {
  const t = tFor(lang || 'en');
  const [filterSource, setFilterSource] = useState('all');
  const toast = useToast();
  // Sprint 18 W2 — wired to GET /api/v1/security/permission/rules.
  const rulesRes = window.cc.data.usePermissionRules();
  const rules = rulesRes.data?.rules ?? (rulesRes.error ? MOCK_PERMISSION_RULES : []);
  const filtered = filterSource === 'all' ? rules : rules.filter(r => r.source === filterSource);
  const dangerCount = rules.filter(r => r.source === 'DangerousCapabilityFilter').length;
  const toolCount = rules.filter(r => r.source === 'ToolPermissionMatcher').length;

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-3 justify-between">
        <div className="flex items-center gap-2">
          <Select value={filterSource} onChange={setFilterSource} className="w-64" options={[
            { value: 'all', label: t('audit.rules.all_sources') || `All sources (${rules.length})` },
            { value: 'DangerousCapabilityFilter', label: `DangerousCapabilityFilter (${dangerCount})` },
            { value: 'ToolPermissionMatcher', label: `ToolPermissionMatcher (${toolCount})` },
          ]} />
        </div>
        <div className="text-[11px] text-fg-3 mono">
          {t('audit.rules.readonly_hint') || 'Read-only — edits route through /reviews approval chain'}
        </div>
      </div>

      <Card className="overflow-hidden">
        <div className="grid grid-cols-[90px_180px_1fr_90px_70px_200px] gap-3 px-4 py-2 text-[10px] mono uppercase tracking-wider text-fg-4 border-b border-border">
          <div>{t('audit.rules.col_action') || 'action'}</div>
          <div>{t('audit.rules.col_source') || 'source'}</div>
          <div>{t('audit.rules.col_pattern') || 'pattern'}</div>
          <div>{t('audit.rules.col_severity') || 'severity'}</div>
          <div>{t('audit.rules.col_enabled') || 'on'}</div>
          <div>{t('audit.rules.col_reason') || 'reason'}</div>
        </div>
        {filtered.map(r => (
          <div key={r.id} className="grid grid-cols-[90px_180px_1fr_90px_70px_200px] gap-3 px-4 row-pad border-b border-border/60 last:border-b-0 text-[12px] items-center">
            <div><Badge tone={ACTION_BADGE_TONE[r.action] || 'slate'}>{r.action}</Badge></div>
            <div className="mono text-fg-3 truncate text-[10px]">{r.source}</div>
            <div className="mono text-fg font-medium truncate">{r.pattern}</div>
            <div><RiskBadge level={r.severity} /></div>
            <div>
              <Switch
                checked={r.enabled}
                onChange={() => {
                  toast.toast({
                    title: t('audit.rules.edit_needs_review') || 'Rule edit requires approval',
                    desc: `${r.pattern} → /reviews`,
                    tone: 'error',
                  });
                }}
              />
            </div>
            <div className="text-[11px] text-fg-3 truncate" title={r.reason}>{r.reason}</div>
          </div>
        ))}
        {!filtered.length && <EmptyState icon={I.Shield} title={t('audit.rules.empty') || 'No rules'} />}
      </Card>
      <div className="text-[11px] text-fg-4 mono px-1">
        {t('audit.rules.todo_endpoint') || '// TODO: GET /api/v1/security/permission/rules — PATCH /api/v1/security/permission/rules/{id} (enters /reviews flow)'}
      </div>
    </div>
  );
};

// ---- C5: Policy Rules (S29: read-only RuleBasedPolicyEngine declarative rules) ----
const POLICY_KIND_TONE = { deny: 'rose', allow: 'emerald' };

const PolicyRulesPane = ({ lang }) => {
  const t = tFor(lang || 'en');
  const res = window.cc.data.usePolicyRules();
  const data = res?.data || { engine: 'unknown', rules: [] };
  const engineLabel = data.engine === 'rule_based'
    ? (t('audit.policy.engine_rule_based') || 'RuleBasedPolicyEngine (S27)')
    : data.engine === 'default'
      ? (t('audit.policy.engine_default') || 'DefaultPolicyEngine (risk-only)')
      : (t('audit.policy.engine_unknown') || 'Unknown engine');

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-3 justify-between">
        <div className="flex items-center gap-2">
          <Badge tone={data.engine === 'rule_based' ? 'sky' : 'slate'}>{engineLabel}</Badge>
          <span className="text-[11px] text-fg-3 mono">
            {(t('audit.policy.rule_count') || 'rules') + ': ' + (data.rules?.length ?? 0)}
          </span>
        </div>
        <div className="text-[11px] text-fg-3 mono">
          {t('audit.policy.readonly_hint') || 'Read-only — edit YAML at $CYBERCLAW_POLICY_RULES_PATH (S28 hot-reload)'}
        </div>
      </div>

      {data.engine === 'default' && (
        <Card className="px-4 py-3 text-[12px] text-fg-3">
          {t('audit.policy.empty_default') ||
            'No declarative rules configured. The platform is using risk-based DefaultPolicyEngine. Set CYBERCLAW_POLICY_RULES_PATH to a yaml file to enable RuleBasedPolicyEngine.'}
        </Card>
      )}

      {data.engine === 'rule_based' && (
        <Card className="overflow-hidden">
          <div className="grid grid-cols-[80px_180px_220px_70px_1fr] gap-3 px-4 py-2 text-[10px] mono uppercase tracking-wider text-fg-4 border-b border-border">
            <div>{t('audit.policy.col_kind') || 'kind'}</div>
            <div>{t('audit.policy.col_agent') || 'agent_id'}</div>
            <div>{t('audit.policy.col_capability') || 'capability_id'}</div>
            <div>{t('audit.policy.col_priority') || 'pri'}</div>
            <div>{t('audit.policy.col_reason') || 'reason'}</div>
          </div>
          {(data.rules || []).map((r, i) => (
            <div key={i} className="grid grid-cols-[80px_180px_220px_70px_1fr] gap-3 px-4 row-pad border-b border-border/60 last:border-b-0 text-[12px] items-center">
              <div><Badge tone={POLICY_KIND_TONE[r.kind] || 'slate'}>{r.kind}</Badge></div>
              <div className="mono text-fg-2 truncate">{r.agent_id || <span className="text-fg-4">*</span>}</div>
              <div className="mono text-fg-2 truncate">{r.capability_id || <span className="text-fg-4">*</span>}</div>
              <div className="mono text-fg-3">{r.priority}</div>
              <div className="text-[11.5px] text-fg-3 truncate" title={r.reason || ''}>{r.reason || '—'}</div>
            </div>
          ))}
          {!(data.rules?.length) && <EmptyState icon={I.Shield} title={t('audit.policy.empty_rules') || 'YAML configured but no rules'} />}
        </Card>
      )}
    </div>
  );
};

// ---- AuditPage: 5 tabs (Audit Log + Runtime Security + Injection Hits + Permission Rules + Policy Rules) ----
const AuditPage = ({ lang }) => {
  const t = tFor(lang || 'en');
  const [auditTab, setAuditTab] = useState('log');
  const [filters, setFilters] = useState({ limit: 100, kind: 'all', action_prefix: '' });
  // Only send non-empty params to the backend so `kind=all` isn't treated as a
  // filter and an empty action_prefix isn't appended.
  const query = {};
  if (filters.limit) query.limit = filters.limit;
  if (filters.kind && filters.kind !== 'all') query.kind = filters.kind;
  if (filters.action_prefix) query.action_prefix = filters.action_prefix;
  const res = window.cc.data.useAudit(query);
  const { loading, error } = res;
  const entries = (res.data && Array.isArray(res.data.entries))
    ? res.data.entries
    : (Array.isArray(res.data) ? res.data : []);

  return (
    <div className="space-y-4">
      <Tabs value={auditTab} onChange={setAuditTab} items={[
        { value: 'log',     label: t('audit.tab_log') || 'Audit Log' },
        { value: 'runtime', label: t('audit.tab_runtime') || 'Runtime Security' },
        { value: 'injection', label: t('audit.tab_injection') || 'Injection Hits' },
        { value: 'rules',   label: t('audit.tab_rules') || 'Permission Rules' },
        { value: 'policy',  label: t('audit.tab_policy') || 'Policy Rules' },
      ]} />

      {auditTab === 'log' && (
        <div className="space-y-4">
          <PageToolbar
            left={
              <>
                <Select value={filters.kind} onChange={(v) => setFilters({ ...filters, kind: v })} className="w-44"
                  options={[
                    { value: 'all', label: t('audit_all_kinds') },
                    { value: 'Auth', label: t('audit_kind_auth') },
                    { value: 'Mutation', label: t('audit_kind_mutation') },
                    { value: 'Config', label: t('audit_kind_config') },
                    { value: 'Security', label: t('audit_kind_security') },
                  ]} />
                <Input
                  value={filters.action_prefix}
                  onChange={(e) => setFilters({ ...filters, action_prefix: e.target.value })}
                  placeholder={t('audit_filter_action')}
                  className="w-64 mono"
                />
              </>
            }
            right={<Button variant="outline" size="sm" onClick={() => window.cc.data.invalidate('audit:' + JSON.stringify(query))}><I.Refresh size={12} /> {t('refresh')}</Button>}
          />
          {error && !entries.length && <ErrorBanner message={error} onRetry={() => window.cc.data.invalidate('audit:' + JSON.stringify(query))} />}
          <Card className="overflow-hidden">
            <div className="grid grid-cols-[170px_160px_110px_1fr_200px_100px] gap-3 px-4 py-2 text-[10px] uppercase tracking-wider text-fg-4 mono border-b border-border">
              <div>{t('audit_col_ts')}</div>
              <div>{t('audit_col_actor')}</div>
              <div>{t('audit_col_kind')}</div>
              <div>{t('audit_col_action')}</div>
              <div>{t('audit_col_target')}</div>
              <div>{t('audit_col_result')}</div>
            </div>
            {loading && !entries.length ? <SkeletonRows rows={8} cols={6} /> : entries.map((e, i) => {
              const tsIso = e.ts ? (() => {
                try {
                  const d = new Date(e.ts);
                  if (Number.isNaN(d.getTime())) return String(e.ts);
                  return d.toISOString().slice(0, 19).replace('T', ' ');
                } catch { return String(e.ts); }
              })() : '—';
              return (
                <div key={(e.ts || i) + ':' + (e.action || i)} className="grid grid-cols-[170px_160px_110px_1fr_200px_100px] gap-3 px-4 row-pad border-b border-border/60 last:border-b-0 text-[12px] items-center">
                  <div className="mono text-fg-3 text-[11.5px]">{tsIso}</div>
                  <div className="mono text-fg-2 truncate">{e.actor || '—'}</div>
                  <div><Badge tone={auditKindTone(e.kind)}>{e.kind || '—'}</Badge></div>
                  <div className="mono text-fg truncate">{e.action || '—'}</div>
                  <div className="mono text-fg-3 truncate">{e.target || '—'}</div>
                  <div>
                    {auditResultIsSuccess(e.result)
                      ? <Badge tone="emerald">success</Badge>
                      : <Badge tone="rose">{auditResultText(e.result)}</Badge>}
                  </div>
                </div>
              );
            })}
            {!loading && !entries.length && <EmptyState icon={I.Shield} title={t('audit_empty')} />}
          </Card>
        </div>
      )}

      {auditTab === 'runtime' && <RuntimeSecurityTimeline lang={lang} />}
      {auditTab === 'injection' && <InjectionHitsPane lang={lang} />}
      {auditTab === 'rules' && <PermissionRulesPane lang={lang} />}
      {auditTab === 'policy' && <PolicyRulesPane lang={lang} />}
    </div>
  );
};

// AutoField (simple JSON-schema-ish)
const AutoField = ({ name, type, value, onChange, required, enumVals }) => {
  if (type === 'boolean') {
    return (
      <div className="flex items-center justify-between bg-bg-3 border border-border rounded-md px-3 py-2">
        <div className="mono text-[12px] text-fg">{name}{required && <span className="text-rose-400">*</span>}</div>
        <Switch checked={!!value} onChange={onChange} />
      </div>
    );
  }
  if (enumVals) {
    return (
      <div>
        <label className="mono text-[11px] text-fg-3 mb-1.5 block">{name}{required && <span className="text-rose-400">*</span>}</label>
        <Select value={value || ''} onChange={onChange} options={enumVals.map(v => ({ value: v, label: v }))} />
      </div>
    );
  }
  return (
    <div>
      <label className="mono text-[11px] text-fg-3 mb-1.5 block">{name}{required && <span className="text-rose-400">*</span>} <span className="text-fg-4">({type})</span></label>
      <Input value={value || ''} onChange={(e) => onChange(type === 'number' ? Number(e.target.value) : e.target.value)} className="mono" type={type === 'number' ? 'number' : 'text'} />
    </div>
  );
};

const CapabilityTestSheet = ({ cap, open, onClose, onTest, lang }) => {
  const t = tFor(lang || 'en');
  // Synthesize a schema from capability id
  const schema = useMemo(() => schemaFor(cap.id), [cap.id]);
  const [form, setForm] = useState({});
  const [response, setResponse] = useState(null);
  const [testing, setTesting] = useState(false);
  useEffect(() => { if (open) { setForm({}); setResponse(null); } }, [open, cap.id]);
  const run = async () => {
    setTesting(true);
    const started = Date.now();
    try {
      const res = await window.cc.api.capabilities.test(cap.connector && cap.connector.connector_id, cap.id, form);
      setResponse({ ok: true, status: 200, duration_ms: Date.now() - started, data: res });
    } catch (err) {
      setResponse({ ok: false, status: 500, duration_ms: Date.now() - started, error: err && err.message ? err.message : String(err), data: { ...form, executed_at: new Date().toISOString(), audit_id: 'au_' + Math.random().toString(36).slice(2, 8) } });
    } finally {
      setTesting(false);
    }
  };
  return (
    <Sheet open={open} onClose={onClose} title={cap.id} subtitle={cap.connector.name} width={720}>
      <div className="p-5 space-y-4">
        <div className="flex items-center gap-2 flex-wrap">
          <RiskBadge level={cap.risk_level} />
          {cap.effects.map(e => <Badge key={e} tone={EFFECT_TONE[e]}>{e}</Badge>)}
        </div>
        <div className="text-[13px] text-fg-2">{cap.description}</div>

        <div>
          <div className="text-[11px] text-fg-3 uppercase tracking-wider mb-1.5">{t('input_schema')}</div>
          <JsonViewer value={schema.input} />
        </div>

        <div>
          <div className="text-[11px] text-fg-3 uppercase tracking-wider mb-1.5">{t('test_form')}</div>
          <Card className="p-3 space-y-2.5">
            {Object.entries(schema.input.properties || {}).map(([k, v]) => (
              <AutoField key={k} name={k} type={v.type} value={form[k]}
                required={(schema.input.required || []).includes(k)}
                enumVals={v.enum}
                onChange={(nv) => setForm(f => ({ ...f, [k]: nv }))}
              />
            ))}
            <div className="pt-1 flex justify-end">
              <Button onClick={run} disabled={testing}>
                {testing ? <><I.Loader size={12} className="animate-spin" /> {t('running_btn')}</> : <><I.Play size={12} /> POST /capabilities/test</>}
              </Button>
            </div>
          </Card>
        </div>

        {response && (
          <div>
            <div className="text-[11px] text-fg-3 uppercase tracking-wider mb-1.5">{t('response')} · {response.status} · {response.duration_ms}ms</div>
            <JsonViewer value={response} />
          </div>
        )}
      </div>
    </Sheet>
  );
};
function schemaFor(id) {
  if (id.startsWith('k8s.rollout')) return { input: { type: 'object', required: ['cluster', 'namespace', 'deployment', 'image'], properties: { cluster: { type: 'string' }, namespace: { type: 'string' }, deployment: { type: 'string' }, image: { type: 'string' }, strategy: { type: 'string', enum: ['rolling', 'recreate'] }, max_surge: { type: 'string' } } } };
  if (id.startsWith('stripe.create_refund')) return { input: { type: 'object', required: ['charge_id', 'amount_cents'], properties: { charge_id: { type: 'string' }, amount_cents: { type: 'number' }, currency: { type: 'string', enum: ['usd', 'eur', 'gbp'] }, reason: { type: 'string', enum: ['duplicate', 'fraudulent', 'requested_by_customer'] } } } };
  if (id.startsWith('github.merge_pr')) return { input: { type: 'object', required: ['repo', 'pr_number'], properties: { repo: { type: 'string' }, pr_number: { type: 'number' }, merge_method: { type: 'string', enum: ['merge', 'squash', 'rebase'] }, commit_title: { type: 'string' } } } };
  if (id.startsWith('slack.post')) return { input: { type: 'object', required: ['channel', 'text'], properties: { channel: { type: 'string' }, text: { type: 'string' }, thread_ts: { type: 'string' }, as_user: { type: 'boolean' } } } };
  return { input: { type: 'object', required: ['query'], properties: { query: { type: 'string' }, limit: { type: 'number' } } } };
}

// ---- Channels ----
const CHANNEL_TONES = { discord: 'violet', slack: 'accent', telegram: 'cyan', whatsapp: 'emerald', signal: 'slate', gchat: 'emerald', imessage: 'accent', webhook: 'amber' };
const ChannelsPage = ({ lang, role }) => {
  const t = tFor(lang);
  const chanRes = window.cc.data.useChannels();
  const { loading, error } = chanRes;
  // Backend now returns only configured channels. MOCK fallback is preserved
  // for offline/dev but we don't fabricate the 8-platform static list.
  const channels = window.cc.data.extractList(chanRes, 'channels', MOCK.channels);
  const [configOf, setConfigOf] = useState(null);
  const toast = useToast();
  const copy = (text, label) => {
    try { navigator.clipboard.writeText(text); } catch {}
    toast.toast({ title: t('copied'), desc: label || text, tone: 'success' });
  };
  return (
    <div className="space-y-4">
      <PageToolbar
        left={<div className="text-[11px] mono text-fg-3">{t('enabled_count_tmpl', { n: channels.filter(c => c.enabled).length, total: channels.length })}</div>}
        right={<Button variant="outline" size="sm" onClick={() => window.cc.data.invalidate('channels')}><I.Refresh size={12} /> {t('refresh')}</Button>}
      />
      {error && !channels.length && <ErrorBanner message={error} onRetry={() => window.cc.data.invalidate('channels')} />}
      {loading && !channels.length && <SkeletonRows rows={3} cols={4} />}
      {!loading && !channels.length && (
        <EmptyState
          icon={I.Radio}
          title={t('channels_empty_title')}
          subtitle={t('channels_empty_body')}
        />
      )}
      <div className="grid grid-cols-4 gap-3">
        {channels.map(ch => (
          <Card key={ch.id} className="p-4">
            <div className="flex items-start justify-between">
              <div className="flex items-center gap-2.5">
                <div className={`h-9 w-9 rounded-md flex items-center justify-center mono text-[11px] font-semibold uppercase ${ch.enabled ? 'bg-accent-soft text-accent border border-[var(--accent)]/20' : 'bg-bg-3 border border-border text-fg-4'}`}>
                  {ch.name.slice(0, 2)}
                </div>
                <div>
                  <div className="text-[13px] font-medium text-fg">{ch.name}</div>
                  <div className="text-[11px] text-fg-3 mono">{ch.id}</div>
                </div>
              </div>
              <Switch checked={ch.enabled} onChange={async (v) => {
                if (role === 'viewer') { toast.toast({ title: t('permission_denied'), desc: t('viewer_cannot_toggle'), tone: 'error' }); return; }
                try {
                  await window.cc.api.channels.update(ch.id, { ...ch, enabled: v });
                  window.cc.data.invalidate('channels');
                } catch (err) {
                  toast.toast({ title: t('permission_denied'), desc: `(offline) ${ch.id}`, tone: 'error' });
                }
              }} />
            </div>
            {ch.webhook_url && (
              <div className="mt-3 flex items-center gap-2 bg-bg-3 border border-border rounded-md px-2 py-1.5">
                <span className="text-[10px] mono text-fg-4 shrink-0 uppercase tracking-wider">{t('channels_webhook_url')}</span>
                <span className="mono text-[11px] text-fg-2 truncate flex-1" title={ch.webhook_url}>{ch.webhook_url}</span>
                <Button variant="ghost" size="xs" onClick={() => copy(ch.webhook_url, ch.id)}><I.Copy size={10} /></Button>
              </div>
            )}
            <div className="mt-3 pt-3 border-t border-border flex items-center justify-between">
              <StatusBadge status={ch.status} />
              <span className="text-[11px] mono text-fg-3">{ch.last_event}</span>
            </div>
            <div className="mt-2 flex justify-end">
              <Button variant="ghost" size="xs" onClick={() => setConfigOf(ch)}><I.Settings size={11} /> {t('configure')}</Button>
            </div>
          </Card>
        ))}
      </div>
      <Dialog open={!!configOf} onClose={() => setConfigOf(null)} title={configOf ? `${t('configure_title')} · ${configOf.name}` : ''} subtitle={configOf && `PUT /channels/${configOf.id}`} width={500}
        footer={<><Button variant="ghost" onClick={() => setConfigOf(null)}>{t('cancel')}</Button><Button onClick={async () => {
          const payload = { ...configOf };
          const id = configOf.id;
          setConfigOf(null);
          try {
            await window.cc.api.channels.update(id, payload);
            window.cc.data.invalidate('channels');
            toast.toast({ title: t('channel_saved'), desc: id, tone: 'success' });
          } catch (err) {
            toast.toast({ title: t('channel_saved'), desc: `(offline) ${id}`, tone: 'error' });
          }
        }}>{t('save')}</Button></>}>
        {configOf && (
          <div className="space-y-3">
            <div className="flex items-center justify-between bg-bg-3 border border-border rounded-md px-3 py-2">
              <div className="text-[12px] text-fg">enabled</div>
              <Switch checked={configOf.enabled} onChange={(v) => setConfigOf({ ...configOf, enabled: v })} />
            </div>
            <div>
              <label className="mono text-[11px] text-fg-3 mb-1.5 block">webhook_secret</label>
              <Input defaultValue="whsec_•••••••••••••" className="mono" />
            </div>
            {Object.entries(configOf.config || {}).map(([k, v]) => (
              <div key={k}>
                <label className="mono text-[11px] text-fg-3 mb-1.5 block">{k}</label>
                <Input defaultValue={v} className="mono" />
              </div>
            ))}
          </div>
        )}
      </Dialog>
    </div>
  );
};

// ---- Nodes ----
const NodesPage = ({ lang }) => {
  const t = tFor(lang);
  const [selected, setSelected] = useState(null);
  const nodesRes = window.cc.data.useNodes();
  const { loading, error, data } = nodesRes;
  const execRes = window.cc.data.useExecutions({ status: 'running' });
  const runningExecs = window.cc.data.extractList(
    execRes, 'executions:{"status":"running"}',
    MOCK.executions.filter(e => e.status === 'running'),
  );
  const nodes = window.cc.data.extractList(nodesRes, 'nodes', MOCK.nodes);
  const online = nodes.filter(n => n.status !== 'offline').length;
  const leader = nodes.find(n => n.status === 'leader');
  return (
    <div className="space-y-4">
      <Card className="p-4 flex items-center gap-6">
        <div>
          <div className="text-[11px] uppercase tracking-wider text-fg-3">{t('nodes_label')}</div>
          <div className="text-[22px] font-semibold mono tabular-nums">{online}/{nodes.length}</div>
        </div>
        <div className="h-10 w-px bg-border" />
        <div>
          <div className="text-[11px] uppercase tracking-wider text-fg-3">{t('leader')}</div>
          <div className="text-[14px] mono text-fg">{leader ? leader.node_id : '—'}</div>
        </div>
        <div className="h-10 w-px bg-border" />
        <div>
          <div className="text-[11px] uppercase tracking-wider text-fg-3">{t('active_exec')}</div>
          <div className="text-[14px] mono text-fg">{nodes.reduce((s, n) => s + (n.active_execs || 0), 0)}</div>
        </div>
        <div className="flex-1" />
        <div className="flex items-center gap-2 text-[11px] mono text-fg-3"><span className="w-1.5 h-1.5 rounded-full bg-emerald-500 pulse-dot" /> {t('gossip_healthy')}</div>
      </Card>
      <Card className="overflow-hidden">
        <div className="grid grid-cols-[150px_110px_120px_120px_100px_1fr_100px] gap-3 px-4 py-2 text-[10px] uppercase tracking-wider text-fg-4 mono border-b border-border">
          <div>node_id</div><div>role</div><div>status</div><div>heartbeat</div><div>version</div><div>utilisation</div><div className="text-right">actions</div>
        </div>
        {loading && !nodes.length ? <SkeletonRows rows={5} cols={7} /> : nodes.map(n => (
          <div key={n.node_id} className="grid grid-cols-[150px_110px_120px_120px_100px_1fr_100px] gap-3 px-4 row-pad border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)] text-[12px] items-center">
            <div className="mono text-fg truncate">{n.node_id}</div>
            <div className="mono text-fg-2">{n.role}</div>
            <div><StatusBadge status={n.status} /></div>
            <div className="mono text-fg-3">{n.last_heartbeat}</div>
            <div className="mono text-fg-3">{n.version}</div>
            <div className="flex items-center gap-3 text-[11px]">
              <div className="flex items-center gap-1.5 w-28"><span className="text-fg-4 mono w-6">cpu</span><MiniBar v={n.cpu} /><span className="mono text-fg-3 w-8 text-right">{n.cpu}%</span></div>
              <div className="flex items-center gap-1.5 w-28"><span className="text-fg-4 mono w-6">mem</span><MiniBar v={n.mem} /><span className="mono text-fg-3 w-8 text-right">{n.mem}%</span></div>
              <div className="mono text-fg-3">{n.active_execs} execs</div>
            </div>
            <div className="text-right">
              <Button variant="ghost" size="xs" onClick={() => setSelected(n)}><I.Eye size={11} /> {t('details')}</Button>
            </div>
          </div>
        ))}
      </Card>
      <Sheet open={!!selected} onClose={() => setSelected(null)} title={selected?.node_id || ''} subtitle={selected?.region} width={600}>
        {selected && (
          <div className="p-5 space-y-4">
            <div className="flex items-center gap-2 flex-wrap">
              <StatusBadge status={selected.status} />
              <Badge>role: {selected.role}</Badge>
              <Badge>region: {selected.region}</Badge>
              <Badge>v{selected.version}</Badge>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <InfoRow label="heartbeat" value={selected.last_heartbeat} mono />
              <InfoRow label="active_execs" value={selected.active_execs} mono />
              <InfoRow label="cpu" value={selected.cpu + '%'} mono />
              <InfoRow label="mem" value={selected.mem + '%'} mono />
            </div>
            <div>
              <div className="text-[11px] text-fg-3 mb-1.5 uppercase tracking-wider">{t('active_exec_on_node')}</div>
              <div className="border border-border rounded-md overflow-hidden">
                {runningExecs.filter(e => !e.node_id || e.node_id === selected.node_id).slice(0, selected.active_execs || 1).map(e => (
                  <div key={e.execution_id} className="grid grid-cols-[1fr_100px_90px] gap-2 px-3 py-2 border-b border-border/60 last:border-b-0 text-[12px] items-center">
                    <div className="mono text-fg-3 truncate">{e.execution_id} · {e.agent}</div>
                    <StatusBadge status={e.status} />
                    <div className="mono text-fg-3 text-right">{e.duration}</div>
                  </div>
                ))}
                {!selected.active_execs && <div className="px-3 py-4 text-[12px] text-fg-3 text-center">{t('no_active_exec')}</div>}
              </div>
            </div>
          </div>
        )}
      </Sheet>
    </div>
  );
};
const MiniBar = ({ v }) => {
  const tone = v > 80 ? 'bg-rose-500' : v > 60 ? 'bg-amber-400' : 'bg-emerald-500';
  return (
    <div className="flex-1 h-1.5 bg-[var(--bg-3)] rounded-full overflow-hidden border border-border">
      <div className={`h-full ${tone}`} style={{ width: v + '%' }} />
    </div>
  );
};

// ---- Settings ----
const SettingsPage = ({ lang, role }) => {
  const t = tFor(lang);
  const [tab, setTab] = useState('config');
  const cfgRes = window.cc.data.useSettings();
  const envRes = window.cc.data.useEnv();
  const aboutRes = window.cc.data.useAbout();
  const policiesRes = window.cc.data.usePolicies();
  const toast = useToast();
  // Backend GET /settings/config returns { path, content }. Accept either
  // a raw string (legacy) or {content|config_toml} shapes. Only fall back
  // to MOCK.config_toml when the fetch ERRORED — on success we show what
  // the backend returned even if empty.
  const cfgText = cfgRes.data
    ? (typeof cfgRes.data === 'string' ? cfgRes.data : (cfgRes.data.content || cfgRes.data.config_toml || ''))
    : (cfgRes.error ? MOCK.config_toml : '');
  const envSeed = window.cc.data.extractList(envRes, 'env', MOCK.env);
  // `about` is whatever the backend returns on success. Only fall back
  // to MOCK fields when the fetch errored.
  const about = (aboutRes.data && typeof aboutRes.data === 'object')
    ? aboutRes.data
    : (aboutRes.error ? { version: MOCK.version, commit: MOCK.commit, build_time: MOCK.build_time, uptime_secs: MOCK.uptime_secs } : {});
  const policies = (policiesRes.data && typeof policiesRes.data === 'object')
    ? policiesRes.data
    : (policiesRes.error ? MOCK.policies : { dangerous: [], tools: [] });
  const dangerousRules = Array.isArray(policies.dangerous) ? policies.dangerous : [];
  const toolRules = Array.isArray(policies.tools) ? policies.tools : [];
  const [env, setEnv] = useState([]);
  React.useEffect(() => {
    setEnv(envSeed.map(e => ({ ...e, revealed: false })));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [envSeed.length, envRes.data]);
  return (
    <div className="space-y-4">
      <Tabs value={tab} onChange={setTab} items={[
        { value: 'config', label: t('tab_config') }, { value: 'env', label: t('tab_env') },
        { value: 'llm', label: t('tab_llm') }, { value: 'governance', label: t('tab_governance') },
        { value: 'about', label: t('tab_about') },
      ]} />
      {tab === 'config' && (
        <Card className="overflow-hidden">
          <CardHeader title="config.toml" subtitle={<span className="mono">PUT /settings/config</span>} right={<Button size="sm" onClick={async () => {
            try {
              await window.cc.api.settings.updateConfig({ content: cfgText });
              window.cc.data.invalidate('settings');
              toast.toast({ title: t('save'), desc: 'config.toml', tone: 'success' });
            } catch (err) {
              toast.toast({ title: t('save'), desc: '(offline)', tone: 'error' });
            }
          }}><I.Check size={12} /> {t('save')}</Button>} />
          <Textarea rows={22} key={cfgText} defaultValue={cfgText} className="rounded-none border-x-0 border-b-0" />
        </Card>
      )}
      {tab === 'env' && (
        <Card className="overflow-hidden">
          <CardHeader title={t('env_vars')} subtitle={<span className="mono">GET /settings/env</span>} />
          <div className="border-t border-border">
            {env.map((e, i) => (
              <div key={e.key} className="grid grid-cols-[260px_1fr_120px] gap-3 px-4 py-2.5 border-b border-border/60 last:border-b-0 items-center">
                <div className="mono text-[12px] text-fg">{e.key}</div>
                <div className="mono text-[12px] text-fg-2 truncate">
                  {e.secret && !e.revealed ? '••••••••••••' : e.value}
                </div>
                <div className="flex justify-end gap-1">
                  {e.secret && (
                    <Button variant="ghost" size="xs" onClick={() => setEnv(es => es.map((x, ix) => ix === i ? { ...x, revealed: !x.revealed } : x))}>
                      {e.revealed ? <I.EyeOff size={11} /> : <I.Eye size={11} />}
                    </Button>
                  )}
                  <Button variant="ghost" size="xs" onClick={() => { try { navigator.clipboard.writeText(e.value); } catch {}; toast.toast({ title: t('copied'), desc: e.key, tone: 'success' }); }}><I.Copy size={11} /></Button>
                </div>
              </div>
            ))}
          </div>
        </Card>
      )}
      {tab === 'llm' && (
        <Card className="p-5 space-y-3 max-w-2xl">
          <div><label className="mono text-[11px] text-fg-3 mb-1.5 block">provider</label><Select value="anthropic" onChange={() => {}} options={[{ value: 'anthropic', label: 'anthropic' }, { value: 'openai', label: 'openai' }, { value: 'local', label: 'local (ollama)' }]} /></div>
          <div><label className="mono text-[11px] text-fg-3 mb-1.5 block">model</label><Input defaultValue="claude-sonnet-4-5" className="mono" /></div>
          <div><label className="mono text-[11px] text-fg-3 mb-1.5 block">base_url</label><Input defaultValue="https://api.anthropic.com" className="mono" /></div>
          <div className="grid grid-cols-2 gap-3">
            <div><label className="mono text-[11px] text-fg-3 mb-1.5 block">max_tokens</label><Input defaultValue="4096" className="mono" /></div>
            <div><label className="mono text-[11px] text-fg-3 mb-1.5 block">temperature</label><Input defaultValue="0.2" className="mono" /></div>
          </div>
          <div className="pt-2 flex justify-end"><Button size="sm"><I.Check size={12} /> {t('save')}</Button></div>
        </Card>
      )}
      {tab === 'governance' && (
        <div className="space-y-4">
          {policiesRes.error && !dangerousRules.length && !toolRules.length && (
            <ErrorBanner message={policiesRes.error} onRetry={() => window.cc.data.invalidate('policies')} />
          )}
          <Card className="overflow-hidden">
            <CardHeader title="DangerousCapabilityFilter" subtitle={<span className="mono">GET /api/v1/settings/policies · {dangerousRules.length} rules</span>} />
            <div className="border-t border-border">
              <div className="grid grid-cols-[80px_1fr_160px_100px] gap-3 px-4 py-2 text-[10px] uppercase tracking-wider text-fg-4 mono border-b border-border">
                <div>rule</div><div>pattern</div><div>action</div><div>risk</div>
              </div>
              {dangerousRules.length ? dangerousRules.map(p => (
                <div key={p.rule} className="grid grid-cols-[80px_1fr_160px_100px] gap-3 px-4 py-2.5 border-b border-border/60 last:border-b-0 items-center text-[12px]" title={p.reason || ''}>
                  <div className="mono text-fg">{p.rule}</div>
                  <div className="mono text-fg-2 truncate">{p.pattern}</div>
                  <div className="mono text-fg-3">{p.action}</div>
                  <div><RiskBadge level={p.risk} /></div>
                </div>
              )) : <div className="px-4 py-4 text-[12px] text-fg-3 text-center">{t('no_rules') !== 'no_rules' ? t('no_rules') : 'no rules configured'}</div>}
            </div>
          </Card>
          <Card className="overflow-hidden">
            <CardHeader title="ToolPermissionMatcher" subtitle={<span className="mono">GET /api/v1/settings/policies · {toolRules.length} rules</span>} />
            <div className="border-t border-border">
              {toolRules.length ? (
                // Backend shape: { tool, argument, action, reason }
                // Legacy MOCK shape: { actor, allow[], deny[] }
                toolRules.map((p, i) => p.tool ? (
                  <div key={i} className="grid grid-cols-[160px_1fr_110px] gap-3 px-4 py-2.5 border-b border-border/60 last:border-b-0 items-center text-[12px]" title={p.reason || ''}>
                    <div className="mono text-fg">{p.tool}</div>
                    <div className="mono text-fg-2 truncate">{p.argument || '*'}</div>
                    <div>
                      <Badge tone={p.action === 'allow' ? 'emerald' : p.action === 'deny' ? 'rose' : 'amber'}>{p.action}</Badge>
                    </div>
                  </div>
                ) : (
                  <div key={i} className="px-4 py-3 border-b border-border/60 last:border-b-0 space-y-1">
                    <div className="mono text-[12px] text-fg">{p.actor}</div>
                    <div className="flex flex-wrap gap-1">
                      {(p.allow || []).map(a => <Badge key={a} tone="emerald">allow: {a}</Badge>)}
                      {(p.deny || []).map(a => <Badge key={a} tone="rose">deny: {a}</Badge>)}
                    </div>
                  </div>
                ))
              ) : <div className="px-4 py-4 text-[12px] text-fg-3 text-center">{t('no_rules') !== 'no_rules' ? t('no_rules') : 'no rules configured'}</div>}
            </div>
          </Card>
        </div>
      )}
      {tab === 'about' && (
        <Card className="p-6 max-w-2xl">
          <div className="flex items-center gap-3 mb-5">
            <div className="h-12 w-12 rounded-lg bg-accent-soft text-accent flex items-center justify-center"><I.Claw size={24} /></div>
            <div>
              <div className="text-[18px] font-semibold">{t('about_title')}</div>
              <div className="text-[12px] text-fg-3 mono">{t('app_subtitle')}</div>
            </div>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <InfoRow label="version" value={about.version ? 'v' + about.version : '—'} mono />
            <InfoRow label="commit" value={about.commit || '—'} mono />
            <InfoRow label="build_time" value={formatBuildTime(about.build_time)} mono />
            <InfoRow label="uptime" value={formatUptime(about.uptime_secs)} mono />
            <InfoRow label="node_id" value={about.node_id || '—'} mono />
            <InfoRow label="role" value={role || '—'} />
          </div>
        </Card>
      )}
    </div>
  );
};

Object.assign(window, { ReviewsPage, CapabilitiesPage, AuditPage, ChannelsPage, NodesPage, SettingsPage });
