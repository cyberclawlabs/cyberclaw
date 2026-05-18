// pages_im_platforms.jsx — R-06 IM Platforms.
//
// Table: name | kind | configured | status | last_error
// Row click → Sheet config + Test action.
// SecretRef inputs (signing_key, bot_token_ref) are reference names, never plaintext.
//
// Backend contract:
//   GET  /api/v1/admin/im-platforms                       → [{ name, kind, configured, status, last_error?, config?:{...} }]
//   PUT  /api/v1/admin/im-platforms/:name/config          ← { kind, signing_key, bot_token_ref, app_id, ... }
//   POST /api/v1/admin/im-platforms/:name/test            ← { channel_id?, text } → { ok, message_id?, error? }

const { useEffect: ipU, useState: ipS, useMemo: ipM } = React;
const {
  Card: IpCard, CardHeader: IpCardHeader, Button: IpButton, Input: IpInput,
  Textarea: IpTextarea, Sheet: IpSheet, Select: IpSelect, JsonViewer: IpJson,
  EmptyState: IpEmpty, SkeletonRows: IpSkeletonRows, ErrorBanner: IpErr,
  Badge: IpBadge, StatusBadge: IpStatusBadge, useToast: ipUseToast, I: IpI,
} = window.cc.ui || window;

const KIND_TONE = {
  discord:   'violet',
  slack:     'amber',
  telegram:  'cyan',
  whatsapp:  'emerald',
  signal:    'slate',
  webhook:   'orange',
  imessage:  'slate',
  gchat:     'accent',
  feishu:    'cyan',
  dingtalk:  'amber',
  wechat:    'emerald',
};

const KIND_FIELDS = {
  discord: [
    { k: 'app_id',         label: 'App ID' },
    { k: 'guild_id',       label: 'Guild ID' },
    { k: 'bot_token_ref',  label: 'Bot token (SecretRef)', secret: true, placeholder: 'discord-bot-token' },
    { k: 'signing_key',    label: 'Signing key (SecretRef)', secret: true, placeholder: 'discord-signing-key' },
  ],
  slack: [
    { k: 'app_id',         label: 'App ID' },
    { k: 'team_id',        label: 'Team ID' },
    { k: 'bot_token_ref',  label: 'Bot token (SecretRef)', secret: true, placeholder: 'slack-bot-token' },
    { k: 'signing_key',    label: 'Signing secret (SecretRef)', secret: true, placeholder: 'slack-signing-secret' },
  ],
  telegram: [
    { k: 'bot_username',   label: 'Bot username' },
    { k: 'bot_token_ref',  label: 'Bot token (SecretRef)', secret: true, placeholder: 'telegram-bot-token' },
    { k: 'signing_key',    label: 'Webhook secret (SecretRef)', secret: true, placeholder: 'telegram-webhook-secret' },
  ],
  webhook: [
    { k: 'url',            label: 'Inbound URL' },
    { k: 'signing_key',    label: 'HMAC secret (SecretRef)', secret: true, placeholder: 'webhook-hmac' },
  ],
};
const DEFAULT_FIELDS = [
  { k: 'app_id',        label: 'App ID' },
  { k: 'bot_token_ref', label: 'Bot token (SecretRef)', secret: true, placeholder: 'platform-bot-token' },
  { k: 'signing_key',   label: 'Signing key (SecretRef)', secret: true, placeholder: 'platform-signing-key' },
];

// ---- Editor sheet --------------------------------------------------------

const IpEditor = ({ open, platform, onClose, onSaved, t }) => {
  const toast = ipUseToast();
  const [draft, setDraft] = ipS({});
  const [busy, setBusy] = ipS(false);
  const [testing, setTesting] = ipS(false);
  const [testInput, setTestInput] = ipS({ channel_id: '', text: 'Hello from CyberClaw admin console.' });
  const [testResult, setTestResult] = ipS(null);
  const [testError, setTestError] = ipS(null);
  ipU(() => {
    setDraft(platform && platform.config ? { ...platform.config } : {});
    setTestResult(null);
    setTestError(null);
  }, [platform]);
  if (!open || !platform) return null;
  const fields = KIND_FIELDS[platform.kind] || DEFAULT_FIELDS;

  const save = async () => {
    setBusy(true);
    try {
      await window.cc.api.imPlatforms.configure(platform.name, { kind: platform.kind, ...draft });
      toast && toast.toast({ title: t('im.toast.configured'), tone: 'success' });
      onSaved && onSaved();
    } catch (e) {
      toast && toast.toast({ title: t('im.toast.configure_failed'), desc: String(e), tone: 'error' });
    } finally { setBusy(false); }
  };

  const test = async () => {
    setTesting(true); setTestResult(null); setTestError(null);
    try {
      const resp = await window.cc.api.imPlatforms.test(platform.name, {
        channel_id: testInput.channel_id || undefined,
        text: testInput.text,
      });
      setTestResult(resp);
      if (resp && resp.ok === false) {
        toast && toast.toast({ title: t('im.toast.test_failed'), desc: resp.error || '', tone: 'error' });
      } else {
        toast && toast.toast({ title: t('im.toast.test_ok'), tone: 'success' });
      }
    } catch (e) {
      setTestError(e && e.message ? e.message : String(e));
      toast && toast.toast({ title: t('im.toast.test_failed'), desc: String(e), tone: 'error' });
    } finally { setTesting(false); }
  };

  return (
    <IpSheet
      open={open}
      onClose={onClose}
      title={platform.name}
      subtitle={platform.kind}
      width={680}
      right={<IpBadge tone={KIND_TONE[platform.kind] || 'slate'}>{platform.kind}</IpBadge>}
    >
      <div className="p-5 space-y-5">
        {/* Config */}
        <div>
          <div className="flex items-center gap-2 mb-3">
            <IpI.Settings size={13} className="text-fg-3" />
            <div className="text-[12px] font-medium text-fg uppercase tracking-wider">{t('im.section.config')}</div>
          </div>
          <div className="grid grid-cols-1 gap-3">
            {fields.map((f) => (
              <div key={f.k}>
                <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{f.label}</div>
                <IpInput
                  type={f.secret ? 'text' : 'text'}
                  value={draft[f.k] || ''}
                  onChange={(e) => setDraft({ ...draft, [f.k]: e.target.value })}
                  placeholder={f.placeholder || ''}
                />
                {f.secret && (
                  <div className="text-[10px] text-amber-400 mt-1 flex items-center gap-1">
                    <IpI.Shield size={10} />
                    {t('im.secret_ref_hint')}
                  </div>
                )}
              </div>
            ))}
          </div>
          <div className="flex justify-end mt-4">
            <IpButton onClick={save} disabled={busy}>
              {busy ? <><IpI.Loader size={12} className="animate-spin" /> {t('im.saving')}</>
                    : <><IpI.Save size={12} /> {t('im.save_config')}</>}
            </IpButton>
          </div>
        </div>

        {/* Test */}
        <div className="pt-5 border-t border-border">
          <div className="flex items-center gap-2 mb-3">
            <IpI.Send size={13} className="text-fg-3" />
            <div className="text-[12px] font-medium text-fg uppercase tracking-wider">{t('im.section.test')}</div>
          </div>
          <div className="space-y-3">
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('im.test.channel_id')}</div>
              <IpInput
                value={testInput.channel_id}
                onChange={(e) => setTestInput((s) => ({ ...s, channel_id: e.target.value }))}
                placeholder={t('im.test.channel_id_placeholder')}
              />
            </div>
            <div>
              <div className="text-[10px] uppercase tracking-wider text-fg-3 mb-1.5">{t('im.test.text')}</div>
              <IpTextarea
                rows={3}
                value={testInput.text}
                onChange={(e) => setTestInput((s) => ({ ...s, text: e.target.value }))}
              />
            </div>
            <IpButton variant="outline" onClick={test} disabled={testing}>
              {testing ? <><IpI.Loader size={12} className="animate-spin" /> {t('im.test.sending')}</>
                       : <><IpI.Send size={12} /> {t('im.test.send')}</>}
            </IpButton>
            {testError && <IpErr message={testError} />}
            {testResult && (
              <div className={`mt-2 border rounded-md p-3 text-[12px] ${testResult.ok === false ? 'border-rose-500/40 bg-rose-500/10' : 'border-emerald-500/40 bg-emerald-500/10'}`}>
                <div className="flex items-center gap-2 mb-2">
                  {testResult.ok === false
                    ? <><IpI.XCircle size={12} className="text-rose-400" /><span className="text-rose-300 font-medium">{t('im.test.result_failed')}</span></>
                    : <><IpI.CheckCircle size={12} className="text-emerald-400" /><span className="text-emerald-300 font-medium">{t('im.test.result_ok')}</span></>}
                </div>
                {testResult.message_id && (
                  <div className="mono text-[11px] text-fg-2">message_id: {testResult.message_id}</div>
                )}
                {testResult.error && (
                  <div className="text-[11px] text-rose-300 mt-1">{testResult.error}</div>
                )}
                <details className="mt-2 text-[10px] text-fg-3">
                  <summary className="cursor-pointer">{t('im.test.raw_response')}</summary>
                  <div className="mt-1.5"><IpJson value={testResult} /></div>
                </details>
              </div>
            )}
          </div>
        </div>
      </div>
    </IpSheet>
  );
};

// ---- Page ----------------------------------------------------------------

const ImPlatformsPage = ({ lang }) => {
  const t = tFor(lang || 'en');
  const [rows, setRows] = ipS([]);
  const [loading, setLoading] = ipS(true);
  const [error, setError] = ipS(null);
  const [editing, setEditing] = ipS(null);

  const reload = async () => {
    setLoading(true); setError(null);
    try {
      const r = await window.cc.api.imPlatforms.list();
      const list = Array.isArray(r) ? r : Array.isArray(r && r.platforms) ? r.platforms : [];
      setRows(list);
    } catch (e) {
      setError(e && e.message ? e.message : String(e));
      setRows([]);
    } finally { setLoading(false); }
  };
  ipU(() => { reload(); }, []);

  return (
    <div className="space-y-4">
      <IpCard className="overflow-hidden">
        <IpCardHeader
          title={t('im.title')}
          subtitle={<span className="mono">GET /api/v1/admin/im-platforms · {rows.length}</span>}
          right={<IpButton size="sm" variant="ghost" onClick={reload}>
            <IpI.Refresh size={12} /> {t('im.action.reload')}
          </IpButton>}
        />
        {error && <div className="px-4 pb-3"><IpErr message={error} onRetry={reload} /></div>}
        {loading ? (
          <IpSkeletonRows rows={5} cols={5} />
        ) : rows.length === 0 ? (
          <IpEmpty
            icon={IpI.Radio}
            title={t('im.empty.title')}
            subtitle={t('im.empty.subtitle')}
          />
        ) : (
          <table className="w-full text-sm">
            <thead className="border-b border-border bg-bg-3 text-fg-3 text-[11px]">
              <tr>
                <th className="text-left px-4 py-2 font-medium">{t('im.col.name')}</th>
                <th className="text-left px-4 py-2 font-medium">{t('im.col.kind')}</th>
                <th className="text-left px-4 py-2 font-medium">{t('im.col.configured')}</th>
                <th className="text-left px-4 py-2 font-medium">{t('im.col.status')}</th>
                <th className="text-left px-4 py-2 font-medium">{t('im.col.last_error')}</th>
                <th className="px-4 py-2"></th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.name} className="border-b border-border/60 last:border-b-0 hover:bg-[var(--hover)] cursor-pointer"
                    onClick={() => setEditing(r)}>
                  <td className="px-4 py-2.5 mono text-[12.5px] text-fg">{r.name}</td>
                  <td className="px-4 py-2.5">
                    <IpBadge tone={KIND_TONE[r.kind] || 'slate'}>{r.kind}</IpBadge>
                  </td>
                  <td className="px-4 py-2.5">
                    {r.configured
                      ? <span className="inline-flex items-center gap-1 text-[11px] text-emerald-400 mono"><IpI.CheckCircle size={11} /> yes</span>
                      : <span className="inline-flex items-center gap-1 text-[11px] text-fg-4 mono"><IpI.Circle size={11} /> not configured</span>}
                  </td>
                  <td className="px-4 py-2.5">
                    <IpStatusBadge status={r.status || 'idle'} />
                  </td>
                  <td className="px-4 py-2.5 mono text-[11px] text-rose-400 truncate max-w-[260px]" title={r.last_error || ''}>
                    {r.last_error || '—'}
                  </td>
                  <td className="px-4 py-2.5 text-right">
                    <IpButton size="xs" variant="ghost" onClick={(e) => { e.stopPropagation(); setEditing(r); }}>
                      <IpI.Edit size={11} /> {t('im.action.edit')}
                    </IpButton>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </IpCard>
      <IpEditor
        open={!!editing}
        platform={editing}
        onClose={() => setEditing(null)}
        onSaved={() => { setEditing(null); reload(); }}
        t={t}
      />
    </div>
  );
};

Object.assign(window, { ImPlatformsPage });
