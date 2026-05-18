// API client — all backend fetches funnel through here.
// Reads JWT from sessionStorage, auto-injects Authorization header,
// normalizes error handling, supports SSE subscription.
//
// Exposed as `window.cc.api` (no ES modules in this codebase).

const API_BASE = ''; // same-origin
// Backend mounts resource routes under /api/v1/*. Admin routes stay at /admin/*.
const V1 = '/api/v1';

function getJwt() {
  try {
    return sessionStorage.getItem('cyberclaw.admin.jwt') || '';
  } catch {
    return '';
  }
}

async function apiFetch(path, opts) {
  opts = opts || {};
  const jwt = getJwt();
  const headers = Object.assign(
    { 'content-type': 'application/json' },
    jwt ? { authorization: 'Bearer ' + jwt } : {},
    opts.headers || {}
  );
  const r = await fetch(API_BASE + path, Object.assign({}, opts, { headers }));
  if (r.status === 401) {
    // Auto-logout on token expiry
    try { sessionStorage.removeItem('cyberclaw.admin.jwt'); } catch {}
    try { sessionStorage.removeItem('cyberclaw.admin.op'); } catch {}
    try { window.dispatchEvent(new CustomEvent('cyberclaw:auth-expired')); } catch {}
    throw new Error('auth expired');
  }
  if (!r.ok) throw new Error(path + ': HTTP ' + r.status);
  const ct = r.headers.get('content-type') || '';
  return ct.includes('application/json') ? r.json() : r.text();
}

const api = {
  // Auth
  login: (user_id, password) =>
    apiFetch('/admin/login', {
      method: 'POST',
      body: JSON.stringify({ user_id, password }),
    }),
  me: () => apiFetch('/admin/me'),

  // Onboarding (Sprint 8 L4)
  onboarding: {
    status: () => apiFetch('/admin/onboarding/status'),
    complete: () => apiFetch('/admin/onboarding/complete', { method: 'POST' }),
  },
  logout: () => apiFetch('/admin/logout', { method: 'POST' }),

  // Dashboard
  dashboard: () => apiFetch('/admin/dashboard'),

  // Agents
  agents: {
    list: () => apiFetch(V1 + '/agents'),
    // POST /api/v1/agents — scaffold a new AdminAgent entry.
    // Backend accepts {name, display_name?, description?}; the wizard sends
    // a richer payload (role_template, default_skills, approval_policy, ...)
    // which the backend currently ignores but we pass through so the
    // Sprint 12 handler can adopt extra fields without a frontend change.
    create: (payload) =>
      apiFetch(V1 + '/agents', {
        method: 'POST',
        body: JSON.stringify(payload || {}),
      }),
    invoke: (agent_id, input) =>
      apiFetch(V1 + '/agents/' + encodeURIComponent(agent_id) + '/invoke', {
        method: 'POST',
        body: JSON.stringify({ input }),
      }),
    detail: (id) => apiFetch(V1 + '/agents/' + encodeURIComponent(id) + '/status'),
    // F5 — delegate subtask to a child agent
    delegate: (req) =>
      apiFetch(V1 + '/agents/delegate', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
  },

  // Skills
  skills: {
    list: () => apiFetch(V1 + '/skills'),
    install: (name, source) =>
      apiFetch(V1 + '/skills/install', {
        method: 'POST',
        body: JSON.stringify({ name, source }),
      }),
    create: ({ name, description, methodology, trigger_examples }) =>
      apiFetch(V1 + '/skills/create', {
        method: 'POST',
        body: JSON.stringify({
          name,
          description,
          methodology: methodology || null,
          trigger_examples: trigger_examples || [],
        }),
      }),
    content: (id) => apiFetch(V1 + '/skills/' + encodeURIComponent(id) + '/content'),
    uninstall: (id) =>
      apiFetch(V1 + '/skills/' + encodeURIComponent(id), { method: 'DELETE' }),
    // Sprint 18 W2 — replaces MOCK_POISON_VERDICTS in pages_b.jsx.
    poisonVerdicts: () => apiFetch(V1 + '/skills/poison-verdicts'),
  },

  // Tasks
  tasks: {
    list: (status, limit) => {
      if (limit == null) limit = 50;
      return apiFetch(
        V1 + '/tasks?status=' +
          encodeURIComponent(status || 'all') +
          '&limit=' +
          limit
      );
    },
    // POST /api/v1/tasks — backend CreateTaskRequest expects
    // {title, description|summary, priority, labels|tags, ...} at the
    // top level, NOT nested inside `inputs`. The `inputs` object is
    // reserved for task-kind-specific payload (TaskInput.payload).
    //
    // The admin SPA passes (task_type, description, extras) where
    // `extras` is {title, priority, tags, assigned_agent}; we flatten
    // those to top-level fields here.
    create: (task_type, description, extras) => {
      const e = extras || {};
      const body = {
        task_type,
        description,
        title: e.title,
        priority: e.priority,
        tags: e.tags,
        assigned_agent: e.assigned_agent,
      };
      // Drop undefined fields so serde defaults apply.
      Object.keys(body).forEach((k) => {
        if (body[k] === undefined || body[k] === null) delete body[k];
      });
      return apiFetch(V1 + '/tasks', {
        method: 'POST',
        body: JSON.stringify(body),
      });
    },
    cancel: (id) =>
      apiFetch(V1 + '/tasks/' + encodeURIComponent(id), { method: 'DELETE' }),
    output: (id) => apiFetch(V1 + '/tasks/' + encodeURIComponent(id)),
  },

  // Executions
  executions: {
    list: (params) => {
      const q = new URLSearchParams(params || {}).toString();
      return apiFetch(V1 + '/executions' + (q ? '?' + q : ''));
    },
    trace: (id) => apiFetch(V1 + '/executions/' + encodeURIComponent(id) + '/trace'),
    cancel: (id) =>
      apiFetch(V1 + '/executions/' + encodeURIComponent(id) + '/cancel', {
        method: 'POST',
      }),
  },

  // Reviews
  reviews: {
    list: (status) =>
      apiFetch(V1 + '/reviews?status=' + encodeURIComponent(status || 'pending')),
    approve: (id) =>
      apiFetch(V1 + '/reviews/' + encodeURIComponent(id) + '/approve', {
        method: 'POST',
      }),
    reject: (id, reason) =>
      apiFetch(V1 + '/reviews/' + encodeURIComponent(id) + '/reject', {
        method: 'POST',
        body: JSON.stringify({ reason }),
      }),
  },

  // Capabilities
  capabilities: {
    discover: () => apiFetch(V1 + '/capabilities/discover'),
    test: (connector_id, capability_id, input) =>
      apiFetch(V1 + '/capabilities/test', {
        method: 'POST',
        body: JSON.stringify({ connector_id, capability_id, input }),
      }),
    // F3 — discover capabilities for a goal
    discoverForGoal: (req) =>
      apiFetch(V1 + '/capabilities/discover_for_goal', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
  },

  // Channels
  channels: {
    list: () => apiFetch(V1 + '/channels'),
    update: (id, config) =>
      apiFetch(V1 + '/channels/' + encodeURIComponent(id), {
        method: 'PUT',
        body: JSON.stringify(config),
      }),
  },

  // Cluster / Nodes
  cluster: {
    nodes: () => apiFetch(V1 + '/cluster/nodes'),
    node: (id) => apiFetch(V1 + '/cluster/nodes/' + encodeURIComponent(id)),
    // F4 — brain cluster management
    state: () => apiFetch(V1 + '/cluster/state'),
    registerBrain: (req) =>
      apiFetch(V1 + '/cluster/brain/register', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
    heartbeat: (brain_id) =>
      apiFetch(V1 + '/cluster/heartbeat/' + encodeURIComponent(brain_id), { method: 'POST' }),
    assignSession: (req) =>
      apiFetch(V1 + '/cluster/sessions/assign', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
  },

  // Settings
  settings: {
    config: () => apiFetch(V1 + '/settings/config'),
    updateConfig: (config) =>
      apiFetch(V1 + '/settings/config', {
        method: 'PUT',
        body: JSON.stringify(config),
      }),
    env: () => apiFetch(V1 + '/settings/env'),
    updateEnv: (key, value) =>
      apiFetch(V1 + '/settings/env', {
        method: 'PUT',
        body: JSON.stringify({ key, value }),
      }),
    about: () => apiFetch(V1 + '/settings/about'),
    policies: () => apiFetch(V1 + '/settings/policies'),
  },

  // Audit (read-only operation log)
  audit: {
    logs: (params) => {
      const q = new URLSearchParams(params || {}).toString();
      return apiFetch(V1 + '/audit/logs' + (q ? '?' + q : ''));
    },
    // Sprint 11 W2 — replaces MOCK_AUDIT_AGENT_PROFILE in pages_c.jsx.
    agentProfile: () => apiFetch(V1 + '/reviews/audit-agent/profile'),
    agentAccuracyCurve: (days) =>
      apiFetch(V1 + '/reviews/audit-agent/accuracy-curve?days=' + (days || 30)),
  },

  // Security observability (Sprint 18 W2 — replaces MOCK_INJECTION_HITS,
  // MOCK_PERMISSION_RULES in pages_c.jsx).
  security: {
    permissionRules: () => apiFetch(V1 + '/security/permission/rules'),
    injectionHits: () => apiFetch(V1 + '/security/injection/hits'),
  },

  // Runtime timeline (Sprint 18 W2 — replaces MOCK_RUNTIME_EVENTS).
  runtime: {
    timeline: () => apiFetch(V1 + '/runtime/timeline'),
  },

  // Governance (S29 read-only declarative policy rules from RuleBasedPolicyEngine)
  governance: {
    policyRules: () => apiFetch(V1 + '/governance/policy-rules'),
  },

  // Workbench (Sprint 18 W2 — replaces MOCK_RESPONSES in pages_workbench.jsx).
  workbench: {
    chat: (mode, prompt) =>
      apiFetch(V1 + '/workbench/chat', {
        method: 'POST',
        body: JSON.stringify({ mode, prompt }),
      }),
  },

  // Connectors (discover registered connectors)
  connectors: {
    list: () => apiFetch(V1 + '/connectors'),
  },

  // Tools (aggregated tool descriptors across connectors)
  tools: {
    list: () => apiFetch(V1 + '/tools'),
    // F7 — deferred/active tool state management
    state: () => apiFetch(V1 + '/tools/state'),
    promote: (name) =>
      apiFetch(V1 + '/tools/promote/' + encodeURIComponent(name), { method: 'POST' }),
    demote: (name) =>
      apiFetch(V1 + '/tools/demote/' + encodeURIComponent(name), { method: 'POST' }),
  },

  // Memory Console (S17 Lane B) — admin-only
  memory: {
    list: (params) => {
      const q = new URLSearchParams(params || {}).toString();
      return apiFetch(V1 + '/memory' + (q ? '?' + q : ''));
    },
    get: (id) => apiFetch(V1 + '/memory/' + encodeURIComponent(id)),
    edit: (id, content) =>
      apiFetch(V1 + '/memory/' + encodeURIComponent(id) + '/edit', {
        method: 'POST',
        body: JSON.stringify({ content }),
      }),
    trace: (id) => apiFetch(V1 + '/memory/' + encodeURIComponent(id) + '/trace'),
    delete: (id) =>
      apiFetch(V1 + '/memory/' + encodeURIComponent(id), { method: 'DELETE' }),
    create: (req) =>
      apiFetch(V1 + '/memory', {
        method: 'POST',
        body: JSON.stringify(req),
      }),
    search: (params) => {
      const q = new URLSearchParams(params || {}).toString();
      return apiFetch(V1 + '/memory/search' + (q ? '?' + q : ''));
    },
  },

  // Handoffs (Sprint 21 T13) — read-only audit + accept/reject endpoints
  handoff: {
    list: (params) => {
      const q = new URLSearchParams(params || {}).toString();
      return apiFetch(V1 + '/chat/handoff' + (q ? '?' + q : ''));
    },
    get: (id) => apiFetch(V1 + '/chat/handoff/' + encodeURIComponent(id)),
    accept: (id) =>
      apiFetch(V1 + '/chat/handoff/' + encodeURIComponent(id) + '/accept', {
        method: 'POST',
      }),
    reject: (id, reason) =>
      apiFetch(V1 + '/chat/handoff/' + encodeURIComponent(id) + '/reject', {
        method: 'POST',
        body: JSON.stringify({ reason: reason || '' }),
      }),
  },

  // BT-37 — admin MCP server hot-reload (list/register/unregister).
  mcp: {
    list: () => apiFetch(V1 + '/admin/mcp/servers'),
    register: (req) =>
      apiFetch(V1 + '/admin/mcp/servers', {
        method: 'POST',
        body: JSON.stringify(req),
      }),
    unregister: (name) =>
      apiFetch(V1 + '/admin/mcp/servers/' + encodeURIComponent(name), {
        method: 'DELETE',
      }),
    // F6 — bridge a MCP tool to a cyberclaw capability
    bridgeTool: (req) =>
      apiFetch(V1 + '/mcp/bridge_tool', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
  },

  // BT-40 — workflow chain convenience endpoint.
  workflows: {
    chain: (req) =>
      apiFetch(V1 + '/admin/workflows/chain', {
        method: 'POST',
        body: JSON.stringify(req),
      }),
    status: (instanceId) =>
      apiFetch(V1 + '/workflows/' + encodeURIComponent(instanceId)),
  },

  // R-07 — Kanban dispatcher (4-column board).
  // GET    /api/v1/admin/kanban/tasks        → { tasks: [...] }
  // POST   /api/v1/admin/kanban/tasks        ← { title, body?, priority?, due_at? } → { id }
  // PUT    /api/v1/admin/kanban/tasks/:id    ← { ... } → { ok }
  // DELETE /api/v1/admin/kanban/tasks/:id    → { ok }
  kanban: {
    listTasks: () => apiFetch(V1 + '/admin/kanban/tasks'),
    createTask: (req) =>
      apiFetch(V1 + '/admin/kanban/tasks', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
    updateTask: (id, patch) =>
      apiFetch(V1 + '/admin/kanban/tasks/' + encodeURIComponent(id), {
        method: 'PUT',
        body: JSON.stringify(patch || {}),
      }),
    deleteTask: (id) =>
      apiFetch(V1 + '/admin/kanban/tasks/' + encodeURIComponent(id), {
        method: 'DELETE',
      }),
  },

  // R-01 — Browser CDP console.
  // GET  /api/v1/admin/browser/status                     → { ws_url, debug_url, targets:[...] }
  // POST /api/v1/admin/browser/{navigate|click|fill|...}  → action result
  browser: {
    status: () => apiFetch(V1 + '/admin/browser/status'),
    navigate: (req) =>
      apiFetch(V1 + '/admin/browser/navigate', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
    click: (req) =>
      apiFetch(V1 + '/admin/browser/click', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
    fill: (req) =>
      apiFetch(V1 + '/admin/browser/fill', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
    evaluate: (req) =>
      apiFetch(V1 + '/admin/browser/evaluate', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
    screenshot: (req) =>
      apiFetch(V1 + '/admin/browser/screenshot', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
    dialog: (req) =>
      apiFetch(V1 + '/admin/browser/dialog', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
  },

  // R-02/03/04/05 — Multimodal control plane.
  // POST /api/v1/admin/multimodal/vision/analyze        ← { image_b64?|image_url?, prompt, provider? }
  // POST /api/v1/admin/multimodal/image/generate        ← { prompt, size, provider? }
  // POST /api/v1/admin/multimodal/audio/transcribe      ← { audio_b64, mime, language? }
  // POST /api/v1/admin/multimodal/audio/synthesize      ← { text, voice, provider? }
  multimodal: {
    vision: (req) =>
      apiFetch(V1 + '/admin/multimodal/vision/analyze', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
    imageGenerate: (req) =>
      apiFetch(V1 + '/admin/multimodal/image/generate', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
    // Backend takes JSON { audio_b64, mime, language? } — base64 the file
    // here so we stay on the standard apiFetch JSON path (no axum_multipart
    // dep on the server).
    audioTranscribe: async (file, extra) => {
      const audio_b64 = await new Promise((resolve, reject) => {
        const r = new FileReader();
        r.onload = () => {
          const s = String(r.result || '');
          const idx = s.indexOf(',');
          resolve(idx >= 0 ? s.slice(idx + 1) : s);
        };
        r.onerror = () => reject(r.error);
        r.readAsDataURL(file);
      });
      const body = { audio_b64, mime: file.type || 'audio/mpeg' };
      if (extra && typeof extra === 'object') Object.assign(body, extra);
      return apiFetch(V1 + '/admin/multimodal/audio/transcribe', {
        method: 'POST',
        body: JSON.stringify(body),
      });
    },
    audioSynthesize: (req) =>
      apiFetch(V1 + '/admin/multimodal/audio/synthesize', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
  },

  // R-06 — IM platforms (Discord/Slack/Telegram/...).
  // GET  /api/v1/admin/im-platforms
  // PUT  /api/v1/admin/im-platforms/:name/config  ← { kind, signing_key, bot_token_ref, app_id, ... }
  // POST /api/v1/admin/im-platforms/:name/test    ← { channel_id?, text }
  imPlatforms: {
    list: () => apiFetch(V1 + '/admin/im-platforms'),
    configure: (name, cfg) =>
      apiFetch(V1 + '/admin/im-platforms/' + encodeURIComponent(name) + '/config', {
        method: 'PUT',
        body: JSON.stringify(cfg || {}),
      }),
    test: (name, req) =>
      apiFetch(V1 + '/admin/im-platforms/' + encodeURIComponent(name) + '/test', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
  },

  // Autonomous Curator + Skill Usage.
  // GET  /api/v1/admin/curator/status        → { last_run, next_run, total_runs, recent_verdicts:[...] }
  // POST /api/v1/admin/curator/run-now       → { ok, run_id }
  // GET  /api/v1/admin/curator/skill-usage   → { records:[...] }
  curator: {
    status: () => apiFetch(V1 + '/admin/curator/status'),
    runNow: () =>
      apiFetch(V1 + '/admin/curator/run-now', { method: 'POST' }),
    skillUsage: () => apiFetch(V1 + '/admin/curator/skill-usage'),
  },

  // Capability gap queue (W4 D-3/D-4 + W2 A-4) — operator-facing failure cluster monitor.
  // Backend: apps/cyberclaw-server/src/api/admin/capability_requests.rs
  //   GET    /api/v1/admin/capability-requests?status=&limit=  → { entries:[...], total_returned }
  //   PUT    /api/v1/admin/capability-requests/:id/status      ← { status, notes? } → { id, status, updated }
  //   POST   /api/v1/admin/capability-requests/run-feedback    → { aggregated, written_to_queue }
  // failure_reason ∈ { not_found | governance_denied | execution_error }
  // status         ∈ { pending  | implemented      | rejected         }
  capabilityRequests: {
    list: (params) => {
      const q = new URLSearchParams(params || {}).toString();
      return apiFetch(V1 + '/admin/capability-requests' + (q ? '?' + q : ''));
    },
    updateStatus: (id, body) =>
      apiFetch(V1 + '/admin/capability-requests/' + encodeURIComponent(id) + '/status', {
        method: 'PUT',
        body: JSON.stringify(body || {}),
      }),
    runFeedback: () =>
      apiFetch(V1 + '/admin/capability-requests/run-feedback', { method: 'POST' }),
  },

  // Mixture-of-Agents (MoA) config + test harness.
  // GET  /api/v1/admin/moa/config            → { enabled, proposers:[{model,provider,weight}], aggregator:{...} }
  // PUT  /api/v1/admin/moa/config            ← same shape
  // POST /api/v1/admin/moa/test              ← { prompt } → { proposers:[{output,latency_ms}], aggregated, latency_ms }
  moa: {
    getConfig: () => apiFetch(V1 + '/admin/moa/config'),
    putConfig: (cfg) =>
      apiFetch(V1 + '/admin/moa/config', {
        method: 'PUT',
        body: JSON.stringify(cfg || {}),
      }),
    test: (req) =>
      apiFetch(V1 + '/admin/moa/test', {
        method: 'POST',
        body: JSON.stringify(req || {}),
      }),
  },

  // Chat conversations (S14-3b)
  chat: {
    conversations: {
      list: () => apiFetch(V1 + '/chat/conversations'),
      get: (id) => apiFetch(V1 + '/chat/conversations/' + encodeURIComponent(id)),
      create: (payload) => apiFetch(V1 + '/chat/conversations', { method: 'POST', body: JSON.stringify(payload || {}) }),
      rename: (id, title) => apiFetch(V1 + '/chat/conversations/' + encodeURIComponent(id), { method: 'PATCH', body: JSON.stringify({ title }) }),
      delete: (id) => apiFetch(V1 + '/chat/conversations/' + encodeURIComponent(id), { method: 'DELETE' }),
      appendMessage: (id, msg) => apiFetch(V1 + '/chat/conversations/' + encodeURIComponent(id) + '/messages', { method: 'POST', body: JSON.stringify(msg) }),
    },
  },
};

// Chat Completions SSE subscription (S14-4, extended S15-T12)
//
// EventSource can't POST a body, so we use fetch + ReadableStream to
// consume `data: ...` frames emitted by `/api/v1/chat/completions` when
// the backend sees `stream: true` or `Accept: text/event-stream`.
//
// New signature (S15-T12):
//   subscribeChatCompletions(body, handlers)
//   handlers: {
//     onDelta(content)            — token text delta (legacy S14-4 compat)
//     onClarify(clarifyPayload)   — NEW: {id, conversation_id, questions[], source, expires_at}
//     onClarifyResolved(id, ans)  — NEW: called when clarify resolved server-side
//     onEnd()                     — stream completed normally
//     onError(err)                — network/abort/parse error
//   }
//
// Legacy signature (S14-4, still works):
//   subscribeChatCompletions(body, onDelta, onEnd, onError)
//
// Returns an `AbortController` — call `.abort()` to cancel mid-stream.
// If the backend returns a non-SSE response (406, JSON body, etc.) the
// caller is expected to fall back to the regular non-stream path.
function subscribeChatCompletions(body, onDeltaOrHandlers, onEnd, onError) {
  // Polymorphic signature detection (S15-T12 backward compat)
  let handlers;
  if (onDeltaOrHandlers && typeof onDeltaOrHandlers === 'object' && !Array.isArray(onDeltaOrHandlers)) {
    handlers = onDeltaOrHandlers;
  } else {
    handlers = {
      onDelta: onDeltaOrHandlers,
      onEnd: onEnd,
      onError: onError,
    };
  }

  const controller = new AbortController();
  const jwt = getJwt();

  (async () => {
    let response;
    try {
      response = await fetch(API_BASE + V1 + '/chat/completions', {
        method: 'POST',
        signal: controller.signal,
        headers: {
          'content-type': 'application/json',
          accept: 'text/event-stream',
          ...(jwt ? { authorization: 'Bearer ' + jwt } : {}),
        },
        body: JSON.stringify(Object.assign({}, body || {}, { stream: true })),
      });
    } catch (err) {
      if (err && err.name === 'AbortError') {
        handlers.onError && handlers.onError({ aborted: true });
      } else {
        handlers.onError && handlers.onError(err);
      }
      return;
    }

    if (response.status === 401) {
      try { sessionStorage.removeItem('cyberclaw.admin.jwt'); } catch {}
      try { window.dispatchEvent(new CustomEvent('cyberclaw:auth-expired')); } catch {}
      handlers.onError && handlers.onError(new Error('auth expired'));
      return;
    }

    const ct = (response.headers.get('content-type') || '').toLowerCase();
    if (!response.ok || !ct.includes('text/event-stream')) {
      // Not SSE — signal fallback. Include body text where possible.
      let text = '';
      try { text = await response.text(); } catch {}
      handlers.onError && handlers.onError({ notSse: true, status: response.status, body: text });
      return;
    }

    const reader = response.body.getReader();
    const decoder = new TextDecoder('utf-8');
    let buf = '';
    let done = false;

    try {
      while (!done) {
        const { value, done: finished } = await reader.read();
        if (finished) break;
        buf += decoder.decode(value, { stream: true });
        // Split on double newline — SSE frame delimiter.
        let idx;
        while ((idx = buf.indexOf('\n\n')) !== -1) {
          const frame = buf.slice(0, idx);
          buf = buf.slice(idx + 2);
          // Each frame is one or more `field: value` lines. We only care
          // about `data:` lines.
          const lines = frame.split('\n');
          const dataLines = [];
          for (const ln of lines) {
            if (ln.startsWith('data:')) {
              dataLines.push(ln.slice(5).trimStart());
            }
          }
          if (dataLines.length === 0) continue;
          const payload = dataLines.join('\n');
          if (payload === '[DONE]') {
            done = true;
            break;
          }
          try {
            const obj = JSON.parse(payload);
            // S15-T12: route by payload.type
            if (obj.type === 'clarify') {
              handlers.onClarify && handlers.onClarify(obj.clarify);
            } else if (obj.type === 'clarify_resolved') {
              handlers.onClarifyResolved && handlers.onClarifyResolved(obj.clarify_id, obj.answer);
            } else {
              // Legacy token frame: {choices:[{delta:{content:...}}]}
              const delta = obj && obj.choices && obj.choices[0] && obj.choices[0].delta;
              const text = delta && typeof delta.content === 'string' ? delta.content : '';
              if (text) handlers.onDelta && handlers.onDelta(text);
            }
          } catch {
            // Malformed frame — ignore and keep going.
          }
        }
      }
      handlers.onEnd && handlers.onEnd();
    } catch (err) {
      if (err && err.name === 'AbortError') {
        handlers.onError && handlers.onError({ aborted: true });
      } else {
        handlers.onError && handlers.onError(err);
      }
    } finally {
      try { reader.releaseLock(); } catch {}
    }
  })();

  return controller;
}

// S16-T6: Compress a conversation's context messages
// POST /api/v1/chat/conversations/<id>/compress
async function compressConversation(conversationId) {
  const jwt = getJwt();
  const res = await fetch(
    API_BASE + V1 + '/chat/conversations/' + encodeURIComponent(conversationId) + '/compress',
    {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        ...(jwt ? { authorization: 'Bearer ' + jwt } : {}),
      },
      body: '{}',
    }
  );
  if (res.status === 401) {
    try { sessionStorage.removeItem('cyberclaw.admin.jwt'); } catch {}
    try { window.dispatchEvent(new CustomEvent('cyberclaw:auth-expired')); } catch {}
    throw new Error('auth expired');
  }
  if (!res.ok) {
    let body = {};
    try { body = await res.json(); } catch {}
    throw new Error((body.error && body.error.message) || 'Compression failed (HTTP ' + res.status + ')');
  }
  return res.json();
}

// S15-T15: Fetch all clarify requests for admin audit (GET /api/v1/chat/clarify/all?since=<iso8601>)
async function fetchAllClarifications(sinceIso8601) {
  const params = sinceIso8601 ? '?since=' + encodeURIComponent(sinceIso8601) : '';
  const res = await apiFetch(V1 + '/chat/clarify/all' + params);
  return Array.isArray(res) ? res : [];
}

// S15-T12: Fetch pending clarify requests for a conversation (GET /api/v1/chat/clarify/pending)
async function fetchPendingClarifies(conversationId) {
  try {
    const result = await apiFetch(
      V1 + '/chat/clarify/pending?conversation_id=' + encodeURIComponent(conversationId)
    );
    return Array.isArray(result) ? result : [];
  } catch {
    return [];
  }
}

// S15-T12: Submit answer to a clarify request (POST /api/v1/chat/clarify/:id/respond)
async function submitClarifyResponse(clarifyId, answer) {
  return apiFetch(
    V1 + '/chat/clarify/' + encodeURIComponent(clarifyId) + '/respond',
    {
      method: 'POST',
      body: JSON.stringify({ answer }),
    }
  );
}

// SSE subscription (JWT via query param since EventSource can't set headers)
function subscribeEvents(onEvent) {
  const jwt = getJwt();
  let es;
  try {
    es = new EventSource('/admin/events?token=' + encodeURIComponent(jwt));
  } catch (e) {
    return function noop() {};
  }
  es.onmessage = (m) => {
    try {
      onEvent(JSON.parse(m.data));
    } catch {
      /* ignore */
    }
  };
  es.onerror = () => {
    try { es.close(); } catch {}
  };
  return () => {
    try { es.close(); } catch {}
  };
}

// Expose globally (no ES modules in this codebase)
window.cc = window.cc || {};
window.cc.api = api;
window.cc.apiFetch = apiFetch;
window.cc.subscribeEvents = subscribeEvents;
window.cc.subscribeChatCompletions = subscribeChatCompletions;
// S15-T12: clarify API helpers
window.cc.api.chat.fetchPendingClarifies = fetchPendingClarifies;
window.cc.api.chat.submitClarifyResponse = submitClarifyResponse;
// S15-T15: admin clarify audit
window.cc.api.chat.fetchAllClarifications = fetchAllClarifications;
// S16-T6: compress conversation
window.cc.api.chat.compress = compressConversation;
