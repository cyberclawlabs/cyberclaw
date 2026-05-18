// User menu sheets: Profile, API tokens, Docs

// ---- Profile ----
const ProfileSheet = ({ open, onClose, operator, lang }) => {
  const t = tFor(lang);
  const sessions = [
    { id: 's-01', device: 'MacBook Pro · Chrome 130', ip: '10.14.8.22', loc: 'Shanghai, CN', current: true,  last: '2s ago' },
    { id: 's-02', device: 'iPhone 15 · Safari',      ip: '172.19.4.9',  loc: 'Shanghai, CN', current: false, last: '14m ago' },
    { id: 's-03', device: 'CLI · claw-cli/0.1.0',    ip: '10.2.3.17',   loc: 'node-ctl-01',  current: false, last: '3h ago' },
  ];
  const activity = [
    { t: '12:04:18', kind: 'exec.approve',  target: 'exec_8a3f',   ok: true },
    { t: '11:58:02', kind: 'agent.update',  target: 'agent.triage', ok: true },
    { t: '11:42:51', kind: 'skill.publish', target: 'skill.refund@1.4.0', ok: true },
    { t: '10:21:09', kind: 'token.rotate',  target: 'tok_live_prod', ok: true },
    { t: '09:04:33', kind: 'login',         target: '—', ok: true },
  ];
  return (
    <Sheet open={open} onClose={onClose}
      title={t('profile')}
      subtitle={operator.user_id + ' · ' + operator.role}>
      <div className="p-5 space-y-5">
        {/* header */}
        <div className="flex items-center gap-4">
          <div className="h-14 w-14 rounded-lg bg-accent-soft text-accent flex items-center justify-center mono font-semibold text-[18px] border border-[var(--border)]">
            {operator.avatar_initials || 'AC'}
          </div>
          <div className="min-w-0 flex-1">
            <div className="text-[15px] font-semibold">{operator.display_name}</div>
            <div className="text-[11px] mono text-fg-3 mt-0.5">{operator.user_id}@cyberclaw</div>
            <div className="flex items-center gap-1.5 mt-1.5">
              <Badge tone="indigo">{operator.role}</Badge>
              <Badge tone="emerald"><span className="w-1 h-1 rounded-full bg-emerald-400 mr-1 inline-block align-middle" />mfa on</Badge>
              <Badge tone="slate">sso · okta</Badge>
            </div>
          </div>
        </div>

        {/* stats */}
        <div className="grid grid-cols-4 gap-2">
          {[
            { k: 'approvals', v: '142', s: '30d' },
            { k: 'executions', v: '2.3k', s: '30d' },
            { k: 'agents', v: '6', s: 'owned' },
            { k: 'tokens', v: '4', s: 'active' },
          ].map(s => (
            <div key={s.k} className="rounded-md border border-border bg-bg-3 px-2.5 py-2">
              <div className="text-[10px] uppercase tracking-wider text-fg-4">{s.k}</div>
              <div className="mt-0.5 mono text-[16px] font-semibold tabular-nums">{s.v}</div>
              <div className="text-[10px] text-fg-4 mono">{s.s}</div>
            </div>
          ))}
        </div>

        {/* details */}
        <div>
          <SectionTitle>{t('profile_details') || 'Details'}</SectionTitle>
          <div className="mt-2 rounded-md border border-border divide-y divide-[var(--border)] bg-bg-2">
            <KV k="user id"       v={<span className="mono">{operator.user_id}</span>} />
            <KV k="email"         v={<span className="mono">ada.chen@cyberclaw.io</span>} />
            <KV k="role"          v={<Badge tone="indigo">{operator.role}</Badge>} />
            <KV k="team"          v={<span>platform · operators</span>} />
            <KV k="timezone"      v={<span className="mono">Asia/Shanghai (UTC+8)</span>} />
            <KV k="created"       v={<span className="mono">2024-11-02</span>} />
            <KV k="last login"    v={<span className="mono">today 09:04:33</span>} />
          </div>
        </div>

        {/* sessions */}
        <div>
          <SectionTitle right={<button className="text-[11px] text-fg-3 hover:text-rose-400 mono">revoke all</button>}>
            {t('profile_sessions') || 'Active sessions'}
          </SectionTitle>
          <div className="mt-2 rounded-md border border-border divide-y divide-[var(--border)]">
            {sessions.map(s => (
              <div key={s.id} className="px-3 py-2 flex items-center gap-3">
                <div className="h-7 w-7 rounded bg-bg-3 border border-border flex items-center justify-center text-fg-3">
                  <I.Terminal size={13} />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="text-[12px] text-fg truncate">{s.device}</div>
                  <div className="text-[10px] mono text-fg-4 truncate">{s.ip} · {s.loc} · {s.last}</div>
                </div>
                {s.current
                  ? <Badge tone="emerald">current</Badge>
                  : <button className="text-[11px] text-fg-3 hover:text-rose-400 mono">revoke</button>}
              </div>
            ))}
          </div>
        </div>

        {/* activity */}
        <div>
          <SectionTitle>{t('profile_activity') || 'Recent activity'}</SectionTitle>
          <div className="mt-2 rounded-md border border-border bg-[var(--bg)] overflow-hidden">
            <div className="mono text-[11px]">
              {activity.map((a, i) => (
                <div key={i} className="px-3 py-1.5 flex items-center gap-3 border-b border-border last:border-b-0">
                  <span className="text-fg-4 w-[68px]">{a.t}</span>
                  <span className={a.ok ? 'text-emerald-400' : 'text-rose-400'}>●</span>
                  <span className="text-fg-2 w-[140px]">{a.kind}</span>
                  <span className="text-fg-3 truncate">{a.target}</span>
                </div>
              ))}
            </div>
          </div>
        </div>
      </div>
    </Sheet>
  );
};

// ---- API tokens ----
const INITIAL_TOKENS = [
  { id: 'tok_live_prod',     name: 'prod · controller',  prefix: 'ccv1_live_xK8Q', scopes: ['read:*', 'write:exec', 'approve'], created: '2025-01-14', last_used: '2s ago',  expires: '2026-01-14' },
  { id: 'tok_ci_deploy',     name: 'ci · deploy bot',    prefix: 'ccv1_live_9Hm3', scopes: ['write:skill', 'write:agent'],       created: '2024-12-02', last_used: '4m ago',  expires: 'never' },
  { id: 'tok_read_grafana',  name: 'grafana · read',     prefix: 'ccv1_live_Pq21', scopes: ['read:metrics'],                     created: '2024-10-09', last_used: '1h ago',  expires: '2025-10-09' },
  { id: 'tok_dev_ada',       name: 'ada · local cli',    prefix: 'ccv1_dev__Wt7y', scopes: ['read:*', 'write:*'],                created: '2024-11-03', last_used: '22h ago', expires: '2025-11-03' },
];
const ALL_SCOPES = ['read:*', 'write:exec', 'approve', 'write:skill', 'write:agent', 'read:metrics', 'admin'];

const TokensSheet = ({ open, onClose, lang }) => {
  const t = tFor(lang);
  const toast = useContext(ToastCtx);
  const [tokens, setTokens] = useState(INITIAL_TOKENS);
  const [creating, setCreating] = useState(false);
  const [newToken, setNewToken] = useState({ name: '', scopes: ['read:*'], expires: '90d' });
  const [created, setCreated] = useState(null); // { id, secret }
  const [reveal, setReveal] = useState(false);

  const create = () => {
    if (!newToken.name.trim()) return;
    const id = 'tok_' + Math.random().toString(36).slice(2, 8);
    const secret = 'ccv1_live_' + Array.from({ length: 32 }, () =>
      'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789'[Math.floor(Math.random() * 62)]).join('');
    const tok = {
      id, name: newToken.name.trim(), prefix: secret.slice(0, 16),
      scopes: newToken.scopes.length ? newToken.scopes : ['read:*'],
      created: new Date().toISOString().slice(0, 10),
      last_used: 'never', expires: newToken.expires === 'never' ? 'never' :
        new Date(Date.now() + parseInt(newToken.expires) * 864e5).toISOString().slice(0, 10),
    };
    setTokens([tok, ...tokens]);
    setCreated({ id, secret });
    setCreating(false);
    setReveal(true);
    setNewToken({ name: '', scopes: ['read:*'], expires: '90d' });
  };

  const revoke = (id) => {
    setTokens(ts => ts.filter(t => t.id !== id));
    toast && toast.toast && toast.toast({ title: 'Token revoked', tone: 'rose' });
  };

  const copy = async (s) => {
    try { await navigator.clipboard.writeText(s); toast && toast.toast && toast.toast({ title: 'Copied' }); } catch {}
  };

  return (
    <Sheet open={open} onClose={onClose}
      title={t('api_tokens')}
      subtitle="cyberclaw api · v1"
      right={!creating && !created && (
        <Button onClick={() => setCreating(true)} variant="primary" icon={<I.Plus size={12} />}>
          {t('tokens_new') || 'New token'}
        </Button>
      )}>
      <div className="p-5 space-y-4">

        {/* created secret banner */}
        {created && (
          <div className="rounded-md border border-amber-500/40 bg-amber-500/5 p-3">
            <div className="flex items-start gap-2">
              <I.Shield size={14} className="text-amber-400 mt-0.5" />
              <div className="min-w-0 flex-1">
                <div className="text-[12px] font-medium text-fg">Save this token now — it won't be shown again.</div>
                <div className="text-[11px] text-fg-3 mt-0.5">Store it in your secret manager. You can revoke it at any time.</div>
                <div className="mt-2 flex items-center gap-1.5 rounded-md border border-border bg-[var(--bg)] px-2.5 py-2">
                  <span className="mono text-[12px] text-fg flex-1 truncate">
                    {reveal ? created.secret : '•'.repeat(48)}
                  </span>
                  <button onClick={() => setReveal(r => !r)} className="p-1 text-fg-3 hover:text-fg" title={reveal ? 'hide' : 'show'}>
                    {reveal ? <I.EyeOff size={13} /> : <I.Eye size={13} />}
                  </button>
                  <button onClick={() => copy(created.secret)} className="p-1 text-fg-3 hover:text-fg" title="copy">
                    <I.Copy size={13} />
                  </button>
                </div>
                <div className="mt-2 flex justify-end">
                  <button onClick={() => setCreated(null)} className="text-[11px] text-fg-3 hover:text-fg mono">dismiss</button>
                </div>
              </div>
            </div>
          </div>
        )}

        {/* create form */}
        {creating && (
          <div className="rounded-md border border-border bg-bg-3 p-4 space-y-3">
            <div className="flex items-center justify-between">
              <div className="text-[13px] font-semibold">{t('tokens_new') || 'New token'}</div>
              <button onClick={() => setCreating(false)} className="text-fg-3 hover:text-fg"><I.Close size={13} /></button>
            </div>
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-4 mb-1">name</div>
              <input autoFocus value={newToken.name} onChange={e => setNewToken({ ...newToken, name: e.target.value })}
                placeholder="e.g. ci · deploy bot"
                className="w-full h-8 rounded-md bg-bg-2 border border-border px-2.5 text-[12px] mono focus:outline-none focus:border-accent" />
            </div>
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-4 mb-1.5">scopes</div>
              <div className="flex flex-wrap gap-1.5">
                {ALL_SCOPES.map(sc => {
                  const on = newToken.scopes.includes(sc);
                  return (
                    <button key={sc} onClick={() => setNewToken(n => ({
                      ...n, scopes: on ? n.scopes.filter(s => s !== sc) : [...n.scopes, sc]
                    }))}
                      className={`px-2 h-6 rounded mono text-[11px] border ${on
                        ? 'bg-accent-soft text-accent border-[color:var(--accent-ring)]'
                        : 'bg-bg-2 text-fg-3 border-border hover:text-fg'}`}>
                      {sc}
                    </button>
                  );
                })}
              </div>
            </div>
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-4 mb-1.5">expires</div>
              <div className="inline-flex rounded-md border border-border bg-bg-2 p-0.5">
                {['30d', '90d', '1y', 'never'].map(e => (
                  <button key={e} onClick={() => setNewToken(n => ({ ...n, expires: e }))}
                    className={`px-2.5 h-6 mono text-[11px] rounded-[4px] ${newToken.expires === e ? 'bg-bg-3 text-fg border border-border' : 'text-fg-3 hover:text-fg'}`}>
                    {e}
                  </button>
                ))}
              </div>
            </div>
            <div className="flex justify-end gap-2 pt-1">
              <Button onClick={() => setCreating(false)}>{t('cancel')}</Button>
              <Button onClick={create} variant="primary" icon={<I.Key size={12} />}>create token</Button>
            </div>
          </div>
        )}

        {/* token list */}
        <div>
          <SectionTitle right={<span className="mono text-[10px] text-fg-4">{tokens.length} active</span>}>
            {t('tokens_list') || 'Your tokens'}
          </SectionTitle>
          <div className="mt-2 rounded-md border border-border divide-y divide-[var(--border)] overflow-hidden">
            {tokens.length === 0 && (
              <div className="px-3 py-8 text-center text-[12px] text-fg-4">No tokens. Create one to get started.</div>
            )}
            {tokens.map(tok => (
              <div key={tok.id} className="px-3 py-2.5 flex items-start gap-3">
                <div className="h-7 w-7 rounded bg-bg-3 border border-border flex items-center justify-center text-fg-3 mt-0.5">
                  <I.Key size={12} />
                </div>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="text-[12px] font-medium text-fg truncate">{tok.name}</span>
                    <span className="mono text-[10px] text-fg-4">{tok.id}</span>
                  </div>
                  <div className="mono text-[11px] text-fg-3 mt-0.5 truncate">
                    {tok.prefix}<span className="text-fg-4">••••••••••••••••</span>
                  </div>
                  <div className="mt-1.5 flex flex-wrap items-center gap-1">
                    {tok.scopes.map(s => (
                      <span key={s} className="mono text-[10px] px-1.5 py-0.5 rounded border border-border bg-bg-3 text-fg-3">{s}</span>
                    ))}
                  </div>
                  <div className="mt-1 mono text-[10px] text-fg-4 flex items-center gap-3">
                    <span>created {tok.created}</span>
                    <span className="text-fg-5">·</span>
                    <span>last used {tok.last_used}</span>
                    <span className="text-fg-5">·</span>
                    <span>expires {tok.expires}</span>
                  </div>
                </div>
                <div className="flex items-center gap-1 shrink-0">
                  <button onClick={() => copy(tok.prefix + '…')} title="copy prefix"
                    className="p-1.5 rounded text-fg-3 hover:text-fg hover:bg-[var(--hover)]">
                    <I.Copy size={12} />
                  </button>
                  <button onClick={() => revoke(tok.id)} title="revoke"
                    className="p-1.5 rounded text-fg-3 hover:text-rose-400 hover:bg-[var(--hover)]">
                    <I.Trash size={12} />
                  </button>
                </div>
              </div>
            ))}
          </div>
        </div>

        <div className="rounded-md border border-border bg-bg-3 px-3 py-2 text-[11px] text-fg-3 flex items-center gap-2">
          <I.Shield size={12} className="text-fg-4" />
          <span>Tokens inherit your role. Scope them narrowly and rotate quarterly.</span>
        </div>
      </div>
    </Sheet>
  );
};

// ---- Docs ----
const DOC_SECTIONS = [
  { id: 'overview', label: 'Overview', icon: I.Book },
  { id: 'auth',     label: 'Authentication', icon: I.Shield },
  { id: 'agents',   label: 'Agents', icon: I.Bot },
  { id: 'skills',   label: 'Skills', icon: I.Brain },
  { id: 'exec',     label: 'Executions', icon: I.Activity },
  { id: 'webhooks', label: 'Webhooks', icon: I.Radio },
  { id: 'errors',   label: 'Errors', icon: I.XCircle },
  { id: 'sdks',     label: 'SDKs', icon: I.Code },
];

const DocsSheet = ({ open, onClose, lang }) => {
  const t = tFor(lang);
  const [section, setSection] = useState('overview');
  const toast = useContext(ToastCtx);
  const copy = async (s) => {
    try { await navigator.clipboard.writeText(s); toast && toast.toast && toast.toast({ title: 'Copied' }); } catch {}
  };
  return (
    <Sheet open={open} onClose={onClose} width={820}
      title={t('docs')}
      subtitle="cyberclaw.dev/docs · v1"
      right={
        <a href="#" onClick={(e) => e.preventDefault()}
           className="inline-flex items-center gap-1 text-[11px] text-fg-3 hover:text-fg mono">
          <I.ArrowUpRight size={11} /> open in new tab
        </a>
      }>
      <div className="flex h-full min-h-0">
        {/* sidebar */}
        <div className="w-[180px] shrink-0 border-r border-border p-2 overflow-auto">
          <div className="px-2 py-1 text-[10px] uppercase tracking-wider text-fg-4">Reference</div>
          {DOC_SECTIONS.map(s => {
            const Ic = s.icon;
            const on = section === s.id;
            return (
              <button key={s.id} onClick={() => setSection(s.id)}
                className={`w-full flex items-center gap-2 px-2 h-7 rounded-md text-[12px] mb-0.5 ${on
                  ? 'bg-[var(--bg-3)] text-fg border border-border'
                  : 'text-fg-3 hover:text-fg hover:bg-[var(--hover)] border border-transparent'}`}>
                <Ic size={12} className={on ? 'text-accent' : 'text-fg-4'} /> {s.label}
              </button>
            );
          })}
          <div className="mt-3 px-2 py-1 text-[10px] uppercase tracking-wider text-fg-4">Links</div>
          {[
            { k: 'changelog', v: 'v0.1.0 · 7a3f4c9' },
            { k: 'status',    v: 'status.cyberclaw.io' },
            { k: 'discord',   v: 'join · 2.1k online' },
          ].map(l => (
            <div key={l.k} className="px-2 py-1">
              <div className="text-[11px] text-fg-2">{l.k}</div>
              <div className="text-[10px] mono text-fg-4">{l.v}</div>
            </div>
          ))}
        </div>

        {/* body */}
        <div className="flex-1 overflow-auto">
          <div className="p-6 max-w-[600px]">
            {section === 'overview' && (
              <>
                <DocTitle eyebrow="REFERENCE" title="Overview">
                  The CyberClaw API lets you orchestrate autonomous agents, publish skills, and observe executions from any service.
                </DocTitle>
                <DocEndpoint method="GET" path="/v1/status" />
                <DocCode onCopy={copy} code={`curl https://api.cyberclaw.io/v1/status \\
  -H "Authorization: Bearer $CCV1_TOKEN"`} />
                <DocH2>Base URL</DocH2>
                <DocP>All requests use HTTPS. Requests are region-pinned by your controller's home region.</DocP>
                <div className="mono text-[12px] rounded-md border border-border bg-[var(--bg)] px-3 py-2 text-fg-2">https://api.cyberclaw.io/v1</div>
                <DocH2>Rate limits</DocH2>
                <DocTable rows={[
                  ['tier', 'rpm', 'burst'],
                  ['dev',  '60',  '120'],
                  ['pro',  '600', '1200'],
                  ['ent',  'custom', 'custom'],
                ]} />
              </>
            )}

            {section === 'auth' && (
              <>
                <DocTitle eyebrow="REFERENCE" title="Authentication">
                  Bearer tokens. Create them under <span className="mono text-fg">/settings › api tokens</span> or via the menu above.
                </DocTitle>
                <DocEndpoint method="POST" path="/v1/tokens" />
                <DocCode onCopy={copy} code={`# every request carries a bearer token
curl https://api.cyberclaw.io/v1/agents \\
  -H "Authorization: Bearer ccv1_live_…"`} />
                <DocH2>Scopes</DocH2>
                <DocList items={[
                  ['read:*',        'read any resource in your workspace'],
                  ['write:exec',    'trigger and cancel executions'],
                  ['approve',       'approve queued review items'],
                  ['write:skill',   'publish and retire skills'],
                  ['write:agent',   'create and update agents'],
                  ['admin',         'full control — use sparingly'],
                ]} />
                <DocH2>Errors</DocH2>
                <DocP><span className="mono text-fg">401 invalid_token</span> · <span className="mono text-fg">403 insufficient_scope</span></DocP>
              </>
            )}

            {section === 'agents' && (
              <>
                <DocTitle eyebrow="REFERENCE" title="Agents">
                  An agent is a named policy bundle: model, tools, guardrails, and a routing key.
                </DocTitle>
                <DocEndpoint method="GET"  path="/v1/agents" />
                <DocEndpoint method="POST" path="/v1/agents" />
                <DocEndpoint method="PATCH" path="/v1/agents/{agent_id}" />
                <DocCode onCopy={copy} code={`curl -X POST https://api.cyberclaw.io/v1/agents \\
  -H "Authorization: Bearer $CCV1_TOKEN" \\
  -H "Content-Type: application/json" \\
  -d '{
    "name": "triage",
    "model": "claude-sonnet-4-5",
    "skills": ["lookup.order", "refund.issue"],
    "guardrails": ["pii.mask", "spend.cap:100"]
  }'`} />
              </>
            )}

            {section === 'skills' && (
              <>
                <DocTitle eyebrow="REFERENCE" title="Skills">
                  A skill is a versioned tool an agent can call. Skills declare their schema, side-effects, and review policy.
                </DocTitle>
                <DocEndpoint method="POST" path="/v1/skills" />
                <DocCode onCopy={copy} code={`{
  "name": "refund.issue",
  "version": "1.4.0",
  "side_effects": ["external"],
  "requires_review": true,
  "schema": { "type": "object", "properties": {
    "order_id": { "type": "string" },
    "amount":   { "type": "number" }
  }, "required": ["order_id"] }
}`} />
              </>
            )}

            {section === 'exec' && (
              <>
                <DocTitle eyebrow="REFERENCE" title="Executions">
                  Triggers an agent run. Stream tokens and tool calls over SSE.
                </DocTitle>
                <DocEndpoint method="POST" path="/v1/executions" />
                <DocCode onCopy={copy} code={`curl -N -X POST https://api.cyberclaw.io/v1/executions \\
  -H "Authorization: Bearer $CCV1_TOKEN" \\
  -H "Accept: text/event-stream" \\
  -d '{ "agent": "triage", "input": "refund #4821" }'`} />
                <DocH2>Event types</DocH2>
                <DocList items={[
                  ['thought',     'agent reasoning token'],
                  ['tool.call',   'skill invocation'],
                  ['tool.result', 'skill result'],
                  ['review',      'execution paused for human approval'],
                  ['done',        'final response'],
                ]} />
              </>
            )}

            {section === 'webhooks' && (
              <>
                <DocTitle eyebrow="REFERENCE" title="Webhooks">
                  Subscribe to lifecycle events. Payloads are signed with HMAC-SHA256.
                </DocTitle>
                <DocEndpoint method="POST" path="/v1/webhooks" />
                <DocCode onCopy={copy} code={`X-CyberClaw-Signature: t=1735776000,v1=abc123…

{ "event": "execution.completed",
  "data":  { "execution_id": "exec_8a3f", "status": "success" } }`} />
              </>
            )}

            {section === 'errors' && (
              <>
                <DocTitle eyebrow="REFERENCE" title="Errors">
                  Problem+JSON responses with a stable <span className="mono text-fg">code</span>.
                </DocTitle>
                <DocTable rows={[
                  ['code', 'http', 'meaning'],
                  ['invalid_token', '401', 'token missing or malformed'],
                  ['insufficient_scope', '403', 'token lacks required scope'],
                  ['rate_limited', '429', 'slow down — see Retry-After'],
                  ['guardrail_blocked', '451', 'a guardrail rejected the call'],
                  ['node_unavailable', '503', 'controller failover in progress'],
                ]} />
              </>
            )}

            {section === 'sdks' && (
              <>
                <DocTitle eyebrow="REFERENCE" title="SDKs">Official clients, all v0.1.x.</DocTitle>
                <DocList items={[
                  ['typescript', 'npm i @cyberclaw/sdk'],
                  ['python',     'pip install cyberclaw'],
                  ['go',         'go get github.com/cyberclaw/go'],
                  ['cli',        'brew install cyberclaw/tap/claw'],
                ]} />
                <DocCode onCopy={copy} code={`import { CyberClaw } from '@cyberclaw/sdk';
const cc = new CyberClaw({ token: process.env.CCV1_TOKEN });
const run = await cc.executions.create({ agent: 'triage', input: 'refund #4821' });
for await (const ev of run) console.log(ev);`} />
              </>
            )}
          </div>
        </div>
      </div>
    </Sheet>
  );
};

// ---- Shared bits ----
const SectionTitle = ({ children, right }) => (
  <div className="flex items-center justify-between">
    <div className="text-[11px] uppercase tracking-wider text-fg-3">{children}</div>
    {right}
  </div>
);
const KV = ({ k, v }) => (
  <div className="px-3 py-1.5 flex items-center gap-3">
    <div className="w-[88px] text-[11px] uppercase tracking-wider text-fg-4 mono">{k}</div>
    <div className="text-[12px] text-fg-2 flex-1 min-w-0 truncate">{v}</div>
  </div>
);
const DocTitle = ({ eyebrow, title, children }) => (
  <div className="mb-5">
    <div className="text-[10px] mono uppercase tracking-[0.18em] text-accent">{eyebrow}</div>
    <div className="text-[22px] font-semibold tracking-tight mt-1">{title}</div>
    {children && <div className="text-[13px] text-fg-3 mt-2 leading-relaxed">{children}</div>}
  </div>
);
const DocH2 = ({ children }) => (
  <div className="text-[13px] font-semibold text-fg mt-5 mb-2">{children}</div>
);
const DocP = ({ children }) => (
  <div className="text-[12px] text-fg-2 leading-relaxed mb-2">{children}</div>
);
const DocEndpoint = ({ method, path }) => {
  const tone = { GET: 'emerald', POST: 'indigo', PATCH: 'amber', DELETE: 'rose' }[method] || 'slate';
  return (
    <div className="flex items-center gap-2 mb-2 rounded-md border border-border bg-[var(--bg)] px-3 py-1.5">
      <Badge tone={tone}>{method}</Badge>
      <span className="mono text-[12px] text-fg">{path}</span>
    </div>
  );
};
const DocCode = ({ code, onCopy }) => (
  <div className="relative rounded-md border border-border bg-[var(--bg)] overflow-hidden mb-3">
    <button onClick={() => onCopy && onCopy(code)}
      className="absolute top-1.5 right-1.5 p-1.5 rounded text-fg-4 hover:text-fg hover:bg-[var(--hover)]">
      <I.Copy size={11} />
    </button>
    <pre className="mono text-[11.5px] leading-[1.6] text-fg-2 p-3 whitespace-pre-wrap">{code}</pre>
  </div>
);
const DocTable = ({ rows }) => (
  <div className="rounded-md border border-border overflow-hidden mb-3 mono text-[11px]">
    {rows.map((r, i) => (
      <div key={i} className={`grid grid-cols-3 gap-2 px-3 py-1.5 ${i === 0 ? 'bg-bg-3 text-fg-3 uppercase tracking-wider' : 'text-fg-2 border-t border-border'}`}>
        {r.map((c, j) => <span key={j} className="truncate">{c}</span>)}
      </div>
    ))}
  </div>
);
const DocList = ({ items }) => (
  <div className="rounded-md border border-border divide-y divide-[var(--border)] mb-3">
    {items.map(([k, v], i) => (
      <div key={i} className="px-3 py-1.5 flex items-baseline gap-3">
        <span className="mono text-[11px] text-fg w-[140px] shrink-0">{k}</span>
        <span className="text-[12px] text-fg-3">{v}</span>
      </div>
    ))}
  </div>
);

Object.assign(window, { ProfileSheet, TokensSheet, DocsSheet });
