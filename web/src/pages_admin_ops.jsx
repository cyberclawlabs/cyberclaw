// pages_admin_ops.jsx — Admin Operations panel (Hermes BT-37 + BT-40 + BT-06)
//
// Three sub-tabs:
//   - MCP Servers      — list / register / unregister (BT-37 hot-reload)
//   - Workflow Chains  — A → B → C builder (BT-40)
//   - Search Provider  — read-only snapshot of current provider config (BT-06)
//
// All sections funnel through window.cc.api.{mcp,workflows} (added in api.jsx).
// Component exports as window.AdminOpsPage; registered in app.jsx pages map.

const { useEffect, useState } = React;

// ---- shared helpers (Card / Button / Input). Reused from ui.jsx exports.
// `ui.jsx` writes via `Object.assign(window, ...)` so the actual binding
// is on `window`, not `window.cc.ui`. Match the convention used by every
// other page module: `|| window` fallback so the destructure can never
// throw on a fresh page load.
const { Card, CardHeader, Button, Input, Tabs, useToast, I } =
  window.cc?.ui || window;

// ---- MCP Servers panel ----------------------------------------------------

const McpServersPanel = () => {
  const toast = useToast();
  const [rows, setRows] = useState([]);
  const [loading, setLoading] = useState(false);
  const [name, setName] = useState('');
  const [url, setUrl] = useState('');
  const [busy, setBusy] = useState(false);

  const reload = async () => {
    setLoading(true);
    try {
      const r = await window.cc.api.mcp.list();
      setRows(Array.isArray(r) ? r : []);
    } catch (e) {
      toast.toast({ title: 'MCP list failed', desc: String(e), tone: 'error' });
      setRows([]);
    } finally {
      setLoading(false);
    }
  };
  useEffect(() => { reload(); }, []);

  const onRegister = async () => {
    if (!name.trim() || !url.trim()) {
      toast.toast({ title: 'Need name + URL', tone: 'error' });
      return;
    }
    setBusy(true);
    try {
      const resp = await window.cc.api.mcp.register({
        name: name.trim(),
        transport: { type: 'http', url: url.trim(), headers: {} },
        timeout_secs: 30,
      });
      toast.toast({
        title: '✓ Registered',
        desc: `${resp.connector_id} (${resp.capabilities} caps)`,
        tone: 'success',
      });
      setName(''); setUrl('');
      reload();
    } catch (e) {
      toast.toast({ title: 'Register failed', desc: String(e), tone: 'error' });
    } finally {
      setBusy(false);
    }
  };

  const onUnregister = async (n) => {
    if (!confirm(`Unregister MCP server "${n}"?`)) return;
    try {
      await window.cc.api.mcp.unregister(n);
      toast.toast({ title: '✓ Unregistered', desc: n, tone: 'success' });
      reload();
    } catch (e) {
      toast.toast({ title: 'Unregister failed', desc: String(e), tone: 'error' });
    }
  };

  return (
    <div className="space-y-4">
      <Card className="overflow-hidden">
        <CardHeader
          title="Register new MCP server"
          subtitle={<span className="mono">POST /api/v1/admin/mcp/servers</span>}
          right={<Button size="sm" disabled={busy} onClick={onRegister}>
            <I.Plus size={12} /> Register
          </Button>}
        />
        <div className="grid grid-cols-2 gap-3 p-4">
          <Input value={name} onChange={(e) => setName(e.target.value)}
            placeholder="logical name (e.g. echo-server)" />
          <Input value={url} onChange={(e) => setUrl(e.target.value)}
            placeholder="http://localhost:9001/mcp" />
        </div>
      </Card>

      <Card className="overflow-hidden">
        <CardHeader
          title={`Registered MCP servers ${loading ? '· loading…' : `· ${rows.length}`}`}
          subtitle={<span className="mono">GET /api/v1/admin/mcp/servers</span>}
          right={<Button size="sm" onClick={reload}><I.Refresh size={12} /> Reload</Button>}
        />
        {rows.length === 0 ? (
          <div className="p-6 text-center text-fg-2 text-sm">
            No MCP servers registered. Use the form above to add one.
          </div>
        ) : (
          <table className="w-full text-sm">
            <thead className="border-b border-border bg-bg-2 text-fg-2 text-[12px]">
              <tr><th className="text-left p-2">name</th><th className="text-left p-2">connector_id</th><th className="text-left p-2">caps</th><th></th></tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.connector_id} className="border-b border-border/60 last:border-b-0">
                  <td className="p-2 mono">{r.name}</td>
                  <td className="p-2 mono text-fg-2">{r.connector_id}</td>
                  <td className="p-2">{r.capabilities}</td>
                  <td className="p-2 text-right">
                    <Button size="sm" variant="ghost" onClick={() => onUnregister(r.name)}>
                      Unregister
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
    </div>
  );
};

// ---- Workflow Chain panel -------------------------------------------------

const WorkflowChainPanel = () => {
  const toast = useToast();
  const [name, setName] = useState('');
  const [tasks, setTasks] = useState([{ id: '', name: '' }]);
  const [busy, setBusy] = useState(false);
  const [lastResult, setLastResult] = useState(null);

  const addTask = () => setTasks((s) => [...s, { id: '', name: '' }]);
  const removeTask = (i) => setTasks((s) => s.filter((_, j) => j !== i));
  const update = (i, key, val) =>
    setTasks((s) => s.map((t, j) => (j === i ? { ...t, [key]: val } : t)));

  const onSubmit = async () => {
    if (!name.trim()) {
      toast.toast({ title: 'Chain needs a name', tone: 'error' });
      return;
    }
    const cleaned = tasks
      .map((t) => ({ id: t.id.trim(), name: t.name.trim() }))
      .filter((t) => t.id && t.name);
    if (cleaned.length === 0) {
      toast.toast({ title: 'Need at least one task with id+name', tone: 'error' });
      return;
    }
    setBusy(true);
    try {
      const resp = await window.cc.api.workflows.chain({
        name: name.trim(),
        version: '1.0.0',
        tasks: cleaned,
      });
      setLastResult(resp);
      toast.toast({
        title: '✓ Chain registered',
        desc: `${cleaned.length} tasks, instance ${resp.instance_id}`,
        tone: 'success',
      });
    } catch (e) {
      toast.toast({ title: 'Chain failed', desc: String(e), tone: 'error' });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="space-y-4">
      <Card className="overflow-hidden">
        <CardHeader
          title="Build sequential workflow chain"
          subtitle={<span className="mono">POST /api/v1/admin/workflows/chain</span>}
        />
        <div className="p-4 space-y-3">
          <Input value={name} onChange={(e) => setName(e.target.value)}
            placeholder="chain name (e.g. nightly-build)" />

          {tasks.map((t, i) => (
            <div key={i} className="grid grid-cols-[120px_1fr_40px] gap-2 items-center">
              <Input value={t.id} onChange={(e) => update(i, 'id', e.target.value)}
                placeholder={`task${i + 1}`} />
              <Input value={t.name} onChange={(e) => update(i, 'name', e.target.value)}
                placeholder="Display name (e.g. Generate code)" />
              <Button size="sm" variant="ghost" disabled={tasks.length === 1}
                onClick={() => removeTask(i)}>×</Button>
            </div>
          ))}

          <div className="flex gap-2">
            <Button size="sm" variant="ghost" onClick={addTask}>
              <I.Plus size={12} /> Add task
            </Button>
            <Button size="sm" disabled={busy} onClick={onSubmit}>
              <I.Check size={12} /> Submit chain
            </Button>
          </div>

          <div className="text-[12px] text-fg-2 mono">
            Tasks chain in order: {tasks.filter((t) => t.id).map((t) => t.id).join(' → ') || '(empty)'}
          </div>
        </div>
      </Card>

      {lastResult && (
        <Card className="overflow-hidden">
          <CardHeader title="Last submission" />
          <pre className="p-4 text-[12px] mono text-fg-2 overflow-auto">
            {JSON.stringify(lastResult, null, 2)}
          </pre>
        </Card>
      )}
    </div>
  );
};

// ---- Search Provider panel ------------------------------------------------

const SearchProviderPanel = () => {
  return (
    <Card className="overflow-hidden">
      <CardHeader
        title="Search provider"
        subtitle={<span className="mono">env: WEB_SEARCH_PROVIDER + EXA_API_KEY</span>}
      />
      <div className="p-4 space-y-3 text-sm">
        <p className="text-fg-2">
          The web_search tool routes to one of: <code className="mono">duckduckgo</code> (default,
          limited), <code className="mono">brave</code>, <code className="mono">tavily</code>,{' '}
          <code className="mono">exa</code> (BT-06 — recommended for research queries).
        </p>
        <p className="text-fg-2">
          To enable Exa: set environment variables on the server and restart:
        </p>
        <pre className="bg-bg-2 p-3 rounded mono text-[12px] overflow-auto">
{`# server-side .env or k8s Secret
WEB_SEARCH_PROVIDER=exa
EXA_API_KEY=<your-exa-api-key>`}
        </pre>
        <p className="text-fg-2 text-[12px]">
          Provider selection happens at request time — no server restart needed if the env
          vars were already set when the server started. To change at runtime, redeploy
          with the new vars (k8s) or restart the local process.
        </p>
      </div>
    </Card>
  );
};

// ---- F6: MCP Bridge Tool panel ------------------------------------------------

const McpBridgePanel = ({ lang }) => {
  const t = tFor(lang || 'en');
  const [form, setForm] = useState({
    server_name: '',
    tool_name: '',
    description: '',
    input_schema: '{}',
  });
  const [result, setResult] = useState(null);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState(null);

  const set = (k, v) => setForm(f => ({ ...f, [k]: v }));

  const submit = async () => {
    let parsedSchema;
    try { parsedSchema = JSON.parse(form.input_schema || '{}'); } catch {
      setErr('input_schema is not valid JSON');
      return;
    }
    setLoading(true); setErr(null); setResult(null);
    try {
      const res = await window.cc.api.mcp.bridgeTool({
        server_name: form.server_name.trim(),
        tool_name: form.tool_name.trim(),
        description: form.description.trim() || undefined,
        input_schema: parsedSchema,
      });
      setResult(res);
    } catch (e) {
      setErr(e && e.message ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  const { Card, CardHeader, Button, Badge, Input, Textarea, ErrorBanner } = window.cc.ui || window;
  const RISK_TONE = { low: 'slate', medium: 'amber', high: 'orange', critical: 'rose' };

  return (
    <div className="space-y-4">
      <Card>
        <CardHeader title={t('mcp.bridge.title')} subtitle="POST /api/v1/mcp/bridge_tool" />
        <div className="p-4 space-y-3">
          {err && <ErrorBanner message={err} />}
          {[
            { key: 'server_name', label: t('mcp.bridge.server_name'), placeholder: 'my-mcp-server' },
            { key: 'tool_name',   label: t('mcp.bridge.tool_name'),   placeholder: 'my_tool' },
            { key: 'description', label: t('mcp.bridge.description'), placeholder: 'Describe what this tool does…' },
          ].map(f => (
            <div key={f.key}>
              <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider mono">{f.label}</label>
              <Input
                value={form[f.key]}
                onChange={e => set(f.key, e.target.value)}
                placeholder={f.placeholder}
                className="mono"
              />
            </div>
          ))}
          <div>
            <label className="text-[11px] text-fg-3 mb-1.5 block uppercase tracking-wider mono">{t('mcp.bridge.input_schema')}</label>
            <Textarea
              rows={6}
              value={form.input_schema}
              onChange={e => set('input_schema', e.target.value)}
              placeholder='{"type":"object","properties":{}}'
              className="mono"
            />
          </div>
          <div>
            <Button
              onClick={submit}
              disabled={loading || !form.server_name.trim() || !form.tool_name.trim()}
            >
              <I.Bridge size={13} />
              {loading ? t('mcp.bridge.submitting') : t('mcp.bridge.submit')}
            </Button>
          </div>
        </div>
      </Card>

      {result && (
        <Card>
          <CardHeader title={t('mcp.bridge.result')} />
          <div className="p-4 space-y-3">
            {[
              { label: t('mcp.bridge.namespaced_name'), value: result.namespaced_name || result.name || '—' },
              { label: t('mcp.bridge.capability_id'),   value: result.capability_id   || '—' },
            ].map(row => (
              <div key={row.label} className="flex items-start gap-3">
                <span className="text-[11px] text-fg-4 uppercase tracking-wider mono w-36 shrink-0 mt-0.5">{row.label}</span>
                <span className="mono text-[13px] text-fg">{row.value}</span>
              </div>
            ))}
            {(result.risk_level || result.risk) && (
              <div className="flex items-start gap-3">
                <span className="text-[11px] text-fg-4 uppercase tracking-wider mono w-36 shrink-0 mt-0.5">{t('mcp.bridge.risk_level')}</span>
                <Badge tone={RISK_TONE[(result.risk_level || result.risk || '').toLowerCase()] || 'slate'}>
                  {result.risk_level || result.risk}
                </Badge>
              </div>
            )}
            <details className="mt-2">
              <summary className="text-[11px] text-fg-4 cursor-pointer hover:text-fg-3 mono">raw json</summary>
              <pre className="mt-2 p-3 bg-bg-3 rounded text-[11px] mono text-fg-2 overflow-auto max-h-48">{JSON.stringify(result, null, 2)}</pre>
            </details>
          </div>
        </Card>
      )}
    </div>
  );
};

// ---- Combined page --------------------------------------------------------

const AdminOpsPage = ({ lang }) => {
  const [tab, setTab] = useState('mcp');
  const t = tFor(lang || 'en');
  return (
    <div className="space-y-4">
      <Tabs value={tab} onChange={setTab} items={[
        { value: 'mcp',    label: 'MCP Servers' },
        { value: 'bridge', label: t('mcp.bridge.title') },
        { value: 'chain',  label: 'Workflow Chain' },
        { value: 'search', label: 'Search Provider' },
      ]} />
      {tab === 'mcp'    && <McpServersPanel />}
      {tab === 'bridge' && <McpBridgePanel lang={lang} />}
      {tab === 'chain'  && <WorkflowChainPanel />}
      {tab === 'search' && <SearchProviderPanel />}
    </div>
  );
};

Object.assign(window, { AdminOpsPage });
