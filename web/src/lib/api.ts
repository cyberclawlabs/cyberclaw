// 极简 API 客户端：只覆盖 v2 skeleton 立刻需要的接口（login / me / about）。
// 全栈迁完后会从老 web/src/api.jsx 引迁完整接口面。
//
// JWT 存 sessionStorage，与老 stack 同 key 兼容（'cyberclaw.admin.jwt'），
// 这样开发期同一浏览器登录一次两边都通。

const JWT_KEY = "cyberclaw.admin.jwt";
const OP_KEY = "cyberclaw.admin.op";

export type ApiError = { status: number; body: string };

export type Operator = {
  user_id: string;
  display_name: string;
  role: "admin" | "operator" | "viewer";
  created_at?: string;
  last_login?: string;
  onboarded_at?: string | null;
  intent_auto_route?: boolean;
};

export type AboutInfo = {
  version: string;
  commit: string;
  build_time: string;
  uptime_secs: number;
  node_id?: string;
};

function getJwt(): string | null {
  try {
    return sessionStorage.getItem(JWT_KEY);
  } catch {
    return null;
  }
}

async function apiFetch<T>(path: string, init: RequestInit = {}): Promise<T> {
  const jwt = getJwt();
  const headers = new Headers(init.headers);
  if (jwt) headers.set("Authorization", `Bearer ${jwt}`);
  if (init.body && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }
  const res = await fetch(path, { ...init, headers });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw { status: res.status, body } satisfies ApiError;
  }
  // 2026-05-17: DELETE endpoints commonly return 204 No Content (empty body).
  // Always calling .json() on empty body throws "Unexpected end of JSON input",
  // which optimistic-update sites then catch and treat as failure — leaving
  // the UI showing stale rows after a successful server-side delete.
  // (Bug surfaced via ProfilesPage delete during 4th-round e2e sweep.)
  if (res.status === 204 || res.headers.get("content-length") === "0") {
    return undefined as T;
  }
  // Some 200 responses may also have empty body (e.g. ack-only endpoints).
  // text-first read prevents JSON parse crash on truly-empty bodies.
  const text = await res.text();
  if (text === "") {
    return undefined as T;
  }
  return JSON.parse(text) as T;
}

export async function login(user_id: string, password = ""): Promise<Operator> {
  const data = await apiFetch<{ jwt: string; user: Operator }>("/admin/login", {
    method: "POST",
    body: JSON.stringify({ user_id, password }),
  });
  sessionStorage.setItem(JWT_KEY, data.jwt);
  sessionStorage.setItem(OP_KEY, JSON.stringify(data.user));
  return data.user;
}

export function logout(): void {
  sessionStorage.removeItem(JWT_KEY);
  sessionStorage.removeItem(OP_KEY);
}

export function currentOperator(): Operator | null {
  try {
    const raw = sessionStorage.getItem(OP_KEY);
    return raw ? (JSON.parse(raw) as Operator) : null;
  } catch {
    return null;
  }
}

export async function fetchAbout(): Promise<AboutInfo> {
  return apiFetch<AboutInfo>("/api/v1/settings/about");
}

export async function fetchMe(): Promise<{ user: Operator }> {
  return apiFetch<{ user: Operator }>("/admin/me");
}

// Audit log（cyberclaw 已有 endpoint），LogsPage 直接消费。
export type AuditEntry = {
  ts: string;
  actor: string;
  kind: string;
  target: string | null;
  action: string;
  ip: string | null;
  user_agent: string | null;
  detail: Record<string, unknown>;
  result: "success" | { failure: { reason: string } };
};
export type LogsResponse = {
  entries: AuditEntry[];
  total_returned: number;
};
export type LogsQuery = {
  limit?: number;
  actor?: string;
  kind?: string;
  action_prefix?: string;
  since?: string;
};

export async function fetchAuditLogs(q: LogsQuery = {}): Promise<LogsResponse> {
  const params = new URLSearchParams();
  if (q.limit !== undefined) params.set("limit", String(q.limit));
  if (q.actor) params.set("actor", q.actor);
  if (q.kind) params.set("kind", q.kind);
  if (q.action_prefix) params.set("action_prefix", q.action_prefix);
  if (q.since) params.set("since", q.since);
  const qs = params.toString();
  return apiFetch<LogsResponse>(
    `/api/v1/audit/logs${qs ? `?${qs}` : ""}`,
  );
}

/// Open an SSE connection to the audit log live-tail endpoint.
/// EventSource cannot set the Authorization header, so the JWT is passed
/// as a `?token=` query parameter — matching the pattern used by /admin/events.
export function streamAuditLogs(q: LogsQuery = {}): EventSource {
  const params = new URLSearchParams();
  if (q.kind) params.set("kind", q.kind);
  if (q.actor) params.set("actor", q.actor);
  const jwt = getJwt();
  if (jwt) params.set("token", jwt);
  return new EventSource(`/api/v1/audit/logs/stream?${params.toString()}`);
}

// Agents
export type Agent = {
  agent_id: string;
  name: string;
  description?: string;
  trust_level?: "high" | "medium" | "low";
  status?: "active" | "idle" | "paused";
  source?: "builtin" | "user" | "hub";
  tools?: number;
  skills?: number;
  executions_24h?: number;
};
export async function fetchAgents(): Promise<Agent[]> {
  const data = await apiFetch<Agent[] | { agents: Agent[] }>("/api/v1/agents");
  return Array.isArray(data) ? data : data.agents ?? [];
}

// Skills
export type Skill = {
  skill_id: string;
  name: string;
  category?: string;
  source?: "builtin" | "hub" | "local";
  description?: string;
  installed_at?: string | number;
  enabled?: boolean;
  // CyberClaw enrichment fields
  version?: string;
  tags?: string[];
  author?: string;
  readme_excerpt?: string;
  trust_level?: "high" | "medium" | "low";
  size_bytes?: number;
  file_count?: number;
  has_scripts?: boolean;
  // i18n + provenance — populated when SKILL.md frontmatter declares them.
  description_zh?: string;
  origin?: string;
  imported_at_origin?: string;
};
export async function fetchSkills(): Promise<Skill[]> {
  const data = await apiFetch<Skill[] | { skills: Skill[] }>("/api/v1/skills");
  return Array.isArray(data) ? data : data.skills ?? [];
}

// Memory
export type MemoryRecord = {
  id: string;
  agent_id: string;
  level: "L0" | "L1" | "L2";
  created_at: string;
  updated_at: string;
  preview: string;
  size_bytes: number;
};
export type MemoryResponse = { records: MemoryRecord[] };
export async function fetchMemory(level?: "L0" | "L1" | "L2", limit = 50): Promise<MemoryRecord[]> {
  const params = new URLSearchParams({ limit: String(limit) });
  if (level) params.set("level", level);
  const data = await apiFetch<MemoryResponse>(`/api/v1/memory?${params}`);
  return data.records ?? [];
}
export async function editMemory(id: string, content: string): Promise<MemoryRecord> {
  return apiFetch(`/api/v1/memory/${encodeURIComponent(id)}/edit`, {
    method: "POST",
    body: JSON.stringify({ content }),
  });
}
export async function deleteMemory(id: string): Promise<void> {
  await apiFetch(`/api/v1/memory/${encodeURIComponent(id)}`, { method: "DELETE" });
}

// Memory search (GET /api/v1/memory/search?q=<query>)
export type MemorySearchResult = {
  id: string;
  agent_id: string;
  level: "L0" | "L1" | "L2";
  preview: string;
  size_bytes: number;
  created_at: string;
  updated_at: string;
  score?: number;
};
export type MemorySearchResponse = { results: MemorySearchResult[] };
export async function searchMemory(q: string): Promise<MemorySearchResult[]> {
  const params = new URLSearchParams({ q });
  const data = await apiFetch<MemorySearchResponse | MemorySearchResult[]>(
    `/api/v1/memory/search?${params}`,
  );
  return Array.isArray(data) ? data : (data as MemorySearchResponse).results ?? [];
}

// Agent notes (GET /api/v1/memory/agent_notes)
export type AgentNote = {
  id?: string;
  agent_id: string;
  content: string;
  created_at?: string;
  ts?: string;
};
export type AgentNotesResponse = { notes: AgentNote[] };
export async function fetchAgentNotes(): Promise<AgentNote[]> {
  const data = await apiFetch<AgentNote[] | AgentNotesResponse>(
    "/api/v1/memory/agent_notes",
  );
  return Array.isArray(data) ? data : (data as AgentNotesResponse).notes ?? [];
}

// Tasks
export type Task = {
  id: string;
  agent_id?: string;
  title?: string;
  description?: string;
  status: "pending" | "running" | "done" | "failed" | string;
  created_at?: string;
  updated_at?: string;
};
export async function fetchTasks(status = "all", limit = 50): Promise<Task[]> {
  const params = new URLSearchParams({ status, limit: String(limit) });
  const data = await apiFetch<Task[] | { tasks: Task[] }>(`/api/v1/tasks?${params}`);
  return Array.isArray(data) ? data : data.tasks ?? [];
}

// Reviews
export type Review = {
  id: string;
  agent_id?: string;
  actor?: string;
  risk?: string;
  status: "pending" | "approved" | "rejected" | string;
  decision?: string;
  created_at?: string;
  updated_at?: string;
  summary?: string;
};
export async function fetchReviews(status?: string, limit = 50): Promise<Review[]> {
  const params = new URLSearchParams({ limit: String(limit) });
  if (status) params.set("status", status);
  const data = await apiFetch<Review[] | { reviews: Review[] }>(`/api/v1/reviews?${params}`);
  return Array.isArray(data) ? data : data.reviews ?? [];
}

// Executions
export type Execution = {
  exec_id: string;
  agent_id?: string;
  capability?: string;
  status: "running" | "done" | "failed" | "pending_review" | "cancelled" | string;
  risk?: string;
  started_at?: string;
  duration_secs?: number;
};
export async function cancelExecution(id: string): Promise<void> {
  await apiFetch(`/api/v1/executions/${encodeURIComponent(id)}/cancel`, { method: "POST" });
}
export async function resumeExecution(id: string): Promise<Execution> {
  return apiFetch(`/api/v1/executions/${encodeURIComponent(id)}/resume`, { method: "POST" });
}

export async function fetchExecutions(status = "all", risk = "all", limit = 50): Promise<Execution[]> {
  const params = new URLSearchParams({ limit: String(limit) });
  if (status !== "all") params.set("status", status);
  if (risk !== "all") params.set("risk", risk);
  const data = await apiFetch<Execution[] | { executions: Execution[] }>(`/api/v1/executions?${params}`);
  return Array.isArray(data) ? data : (data as { executions?: Execution[] }).executions ?? [];
}

// Capabilities
export type Capability = {
  id: string;
  title?: string;
  connector_id?: string;
  risk_level?: string;
  description?: string;
};
export async function fetchCapabilities(limit = 50): Promise<Capability[]> {
  const params = new URLSearchParams({ limit: String(limit) });
  const data = await apiFetch<Capability[] | { capabilities: Capability[] }>(`/api/v1/capabilities?${params}`);
  return Array.isArray(data) ? data : (data as { capabilities?: Capability[] }).capabilities ?? [];
}

// Channels
export type Channel = {
  id: string;
  name?: string;
  status?: string;
  enabled: boolean;
  secret_configured?: boolean;
  webhook_url?: string;
  config?: Record<string, unknown>;
};
export async function fetchChannels(): Promise<Channel[]> {
  const data = await apiFetch<Channel[] | { channels: Channel[] }>("/api/v1/channels");
  return Array.isArray(data) ? data : (data as { channels?: Channel[] }).channels ?? [];
}

// Clarifications (endpoint 404 — built with empty-state fallback)
export type Clarification = {
  id: string;
  conversation_id?: string;
  question: string;
  status: "pending" | "answered" | string;
  created_at?: string;
};
export async function fetchClarifications(): Promise<Clarification[]> {
  const data = await apiFetch<Clarification[] | { clarifications: Clarification[] }>(
    "/api/v1/clarifications",
  );
  return Array.isArray(data) ? data : (data as { clarifications?: Clarification[] }).clarifications ?? [];
}

// Handoffs — audit trail (GET /api/v1/chat/handoff?limit=N)
export type Handoff = {
  handoff_id: string;
  from_agent?: string;
  to_agent?: string;
  status?: "pending" | "accepted" | "rejected" | string;
  reason?: string;
  created_at?: string;
  // audit trail fields (from /api/v1/chat/handoff)
  ts?: string;
  handoff_kind?: string;
  message_excerpt?: string;
  briefing_text?: string;
  from_agent_id?: string;
  to_agent_id?: string;
  initiated_at?: string;
  decided_at?: string;
};
export async function fetchHandoffs(limit = 50): Promise<Handoff[]> {
  const params = new URLSearchParams({ limit: String(limit) });
  const data = await apiFetch<Handoff[] | { handoffs: Handoff[] }>(
    `/api/v1/chat/handoff?${params}`,
  );
  return Array.isArray(data) ? data : (data as { handoffs?: Handoff[] }).handoffs ?? [];
}

// Nodes (endpoint /api/v1/cluster/nodes — live)
export type ClusterNode = {
  node_id: string;
  role?: string;
  status?: string;
  last_heartbeat?: string;
  version?: string;
  cpu?: number;
  mem?: number;
  active_execs?: number;
  region?: string;
  addr?: string;
};
export async function fetchNodes(): Promise<ClusterNode[]> {
  const data = await apiFetch<ClusterNode[] | { nodes: ClusterNode[] }>(
    "/api/v1/cluster/nodes",
  );
  return Array.isArray(data) ? data : (data as { nodes?: ClusterNode[] }).nodes ?? [];
}

// Tools
export type Tool = {
  tool_id: string;
  name: string;
  connector_id?: string;
  description?: string;
  risk_level?: string;
  effects?: string[];
};
export async function fetchTools(): Promise<Tool[]> {
  const data = await apiFetch<Tool[] | { tools: Tool[] }>("/api/v1/tools");
  return Array.isArray(data) ? data : (data as { tools?: Tool[] }).tools ?? [];
}

// Onboarding (wrappers used by OnboardingBanner — centralizes auth header
// + error handling instead of three raw fetch() calls in the component).
export type OnboardingStatus = {
  needs_onboarding: boolean;
  onboarded_at?: string | null;
};

export async function fetchOnboardingStatus(): Promise<OnboardingStatus> {
  return apiFetch<OnboardingStatus>("/admin/onboarding/status");
}

export async function postOnboardingLlmConfig(args: {
  provider: string;
  api_key: string;
  display_name?: string | null;
}): Promise<void> {
  await apiFetch<unknown>("/admin/onboarding/llm-config", {
    method: "POST",
    body: JSON.stringify(args),
  });
}

export async function postOnboardingComplete(): Promise<void> {
  // Server-side handler is idempotent — multiple POSTs from the same actor
  // are safe (sets onboarded_at on first call, then no-ops).
  await apiFetch<unknown>("/admin/onboarding/complete", {
    method: "POST",
    body: "{}",
  });
}

// Uploads — multipart POST + list/delete wrappers (server enforces sanitize,
// 25 MB cap, magic-byte mime sniff, path-traversal guard).
export type UploadListEntry = {
  id: string;
  stored_filename: string;
  path: string;
  size_bytes: number;
  created_at?: string;
};
type UploadListResponse = { uploads: UploadListEntry[]; total: number };

export async function fetchUploads(): Promise<UploadListEntry[]> {
  const data = await apiFetch<UploadListResponse>("/api/v1/uploads");
  return data.uploads ?? [];
}

/// Server-canonical upload response. `path` is what to reference from chat
/// (the agent's cmd_run sandbox mounts the same workspace).
export type UploadResponse = {
  id: string;
  path: string;
  size_bytes: number;
  mime: string;
  original_filename: string | null;
};

/// POST multipart/form-data with the file under field name "file".
/// apiFetch is bypassed because the JWT header is still needed but the
/// JSON content-type wrapper would break the multipart boundary.
export async function uploadFile(file: File): Promise<UploadResponse> {
  const jwt = sessionStorage.getItem("cyberclaw.admin.jwt") || "";
  const fd = new FormData();
  fd.append("file", file);
  const res = await fetch("/api/v1/uploads", {
    method: "POST",
    headers: jwt ? { Authorization: `Bearer ${jwt}` } : {},
    body: fd,
  });
  if (!res.ok) {
    const text = await res.text().catch(() => "");
    throw new Error(`upload failed (${res.status}): ${text}`);
  }
  // P1 fix: validate response shape — a server returning a 200 with a
  // malformed body would otherwise leave `path` undefined and the
  // composer would insert the literal string "undefined".
  const body = (await res.json()) as unknown;
  if (
    !body ||
    typeof body !== "object" ||
    typeof (body as { path?: unknown }).path !== "string" ||
    typeof (body as { id?: unknown }).id !== "string"
  ) {
    throw new Error("upload succeeded but response shape unexpected");
  }
  return body as UploadResponse;
}

export async function deleteUpload(id: string): Promise<void> {
  await apiFetch<unknown>(`/api/v1/uploads/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
}

// Platform Plugins
export type Plugin = {
  name: string;
  version: string;
  label?: string;
  description?: string;
  icon?: string;
  tab_path?: string;
  enabled: boolean;
  hidden?: boolean;
};
export type PluginsResponse = { plugins: Plugin[]; total: number };
export async function fetchPlugins(): Promise<Plugin[]> {
  const data = await apiFetch<PluginsResponse>("/api/v1/plugins");
  return data.plugins ?? [];
}

/// Trigger a server-side rescan of `~/.cyberclaw/plugins/*/plugin.json`.
/// Returns the fresh list (same shape as `fetchPlugins`).
export async function rescanPlugins(): Promise<Plugin[]> {
  const data = await apiFetch<PluginsResponse>("/api/v1/plugins/rescan", {
    method: "POST",
  });
  return data.plugins ?? [];
}

/// Execute a registered Platform Plugin. The plugin id is the manifest `name`.
/// Returns the plugin's raw JSON output. 404 when the plugin id is not loaded.
export async function invokePlugin(
  id: string,
  input: unknown,
): Promise<unknown> {
  return apiFetch<unknown>(
    `/api/v1/plugins/${encodeURIComponent(id)}/invoke`,
    {
      method: "POST",
      body: JSON.stringify(input ?? {}),
    },
  );
}

// Browser CDP status (admin)
export type BrowserTarget = {
  id: string;
  url: string;
  title: string;
  type: string;
};
export type BrowserStatusResponse = {
  enabled: boolean;
  ws_url: string;
  debug_url: string;
  attached: boolean;
  targets: BrowserTarget[];
};
export async function fetchBrowserStatus(): Promise<BrowserStatusResponse> {
  return apiFetch<BrowserStatusResponse>("/api/v1/admin/browser/status");
}

// MCP Servers (admin)
export type McpServer = {
  name: string;
  connector_id: string;
  capabilities?: number;
};
export async function fetchMcpServers(): Promise<McpServer[]> {
  const data = await apiFetch<McpServer[] | { servers: McpServer[] }>(
    "/api/v1/admin/mcp/servers",
  );
  return Array.isArray(data) ? data : (data as { servers?: McpServer[] }).servers ?? [];
}

// IM Platforms (admin)
export type ImPlatform = {
  name: string;
  kind: string;
  configured: boolean;
  status?: string;
  last_error?: string;
  config?: Record<string, unknown>;
};
export async function fetchImPlatforms(): Promise<ImPlatform[]> {
  const data = await apiFetch<ImPlatform[] | { platforms: ImPlatform[] }>(
    "/api/v1/admin/im-platforms",
  );
  return Array.isArray(data) ? data : (data as { platforms?: ImPlatform[] }).platforms ?? [];
}

// MoA Config (admin)
export type MoaProposer = {
  model: string;
  provider?: string;
  weight?: number;
};
export type MoaConfig = {
  enabled: boolean;
  proposers: MoaProposer[];
  aggregator?: { model: string; provider?: string };
};
export async function fetchMoaConfig(): Promise<MoaConfig> {
  return apiFetch<MoaConfig>("/api/v1/admin/moa/config");
}
export async function updateMoaConfig(config: MoaConfig): Promise<void> {
  await apiFetch("/api/v1/admin/moa/config", {
    method: "PUT",
    body: JSON.stringify(config),
  });
}

// Sessions aggregate view
export type SessionSummary = {
  id: string;
  title: string;
  owner_user_id: string;
  message_count: number;
  model: string | null;
  estimated_tokens: number;
  created_at: string;
  updated_at: string;
  last_activity: string;
  active_agent_id: string | null;
};
export async function fetchSessions(limit = 20): Promise<SessionSummary[]> {
  const data = await apiFetch<SessionSummary[] | { sessions: SessionSummary[] }>(
    `/api/v1/sessions?limit=${limit}`,
  );
  return Array.isArray(data) ? data : (data as { sessions?: SessionSummary[] }).sessions ?? [];
}

// Chat conversations + completions (SSE)
export type ChatMessage = { role: "user" | "assistant" | "system"; content: string };
export type Conversation = {
  id: string;
  title: string;
  message_count?: number;
  messages?: ChatMessage[];
  updated_at?: string;
};
export async function fetchConversations(): Promise<Conversation[]> {
  const data = await apiFetch<Conversation[] | { conversations: Conversation[] }>(
    "/api/v1/chat/conversations",
  );
  return Array.isArray(data) ? data : (data as { conversations?: Conversation[] }).conversations ?? [];
}
export async function fetchConversation(id: string): Promise<Conversation> {
  return apiFetch<Conversation>(
    `/api/v1/chat/conversations/${encodeURIComponent(id)}`,
  );
}
export async function createConversation(title: string): Promise<Conversation> {
  return apiFetch<Conversation>("/api/v1/chat/conversations", {
    method: "POST",
    body: JSON.stringify({ title }),
  });
}
export async function deleteConversation(id: string): Promise<void> {
  await apiFetch<unknown>(
    `/api/v1/chat/conversations/${encodeURIComponent(id)}`,
    { method: "DELETE" },
  );
}
// Persist a single message into a conversation. /v1/chat/completions does NOT
// auto-persist user/assistant turns; ChatPage calls this after each send so
// reload preserves history.
export async function appendMessage(id: string, msg: ChatMessage): Promise<void> {
  await apiFetch<unknown>(
    `/api/v1/chat/conversations/${encodeURIComponent(id)}/messages`,
    { method: "POST", body: JSON.stringify(msg) },
  );
}

export type CompressResult = { original_count: number; compressed_count: number };
export async function compressConversation(id: string): Promise<CompressResult> {
  return apiFetch<CompressResult>(
    `/api/v1/chat/conversations/${encodeURIComponent(id)}/compress`,
    { method: "POST" },
  );
}

// SSE streaming chat completion. onToken called per delta;
// 错误（HTTP 非 2xx）throw ApiError；流中断 → 调用方 catch 处理。
export async function streamChatCompletion(args: {
  conversationId: string;
  model: string;
  messages: ChatMessage[];
  onToken: (chunk: string) => void;
  signal?: AbortSignal;
}): Promise<void> {
  const { conversationId, model, messages, onToken, signal } = args;
  const jwt = getJwt();
  const res = await fetch("/v1/chat/completions", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "text/event-stream",
      ...(jwt ? { Authorization: `Bearer ${jwt}` } : {}),
    },
    body: JSON.stringify({
      model,
      messages,
      stream: true,
      conversation_id: conversationId,
    }),
    signal,
  });
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw { status: res.status, body } satisfies ApiError;
  }
  if (!res.body) throw { status: 0, body: "no response body" } satisfies ApiError;
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buf = "";
  while (true) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });
    let idx;
    while ((idx = buf.indexOf("\n\n")) !== -1) {
      const event = buf.slice(0, idx);
      buf = buf.slice(idx + 2);
      for (const line of event.split("\n")) {
        if (!line.startsWith("data: ")) continue;
        const data = line.slice(6).trim();
        if (data === "[DONE]") return;
        try {
          const j = JSON.parse(data);
          const c = j?.choices?.[0]?.delta?.content;
          if (typeof c === "string") onToken(c);
        } catch {
          // 非 JSON / clarify 等其它帧跳过
        }
      }
    }
  }
}

export const isAuthenticated = (): boolean => !!getJwt();

// Usage statistics
export type ModelUsageSnapshot = {
  requests: number;
  input_tokens: number;
  output_tokens: number;
  first_token_avg_ms: number;
  last_used_at: string | null;
};

export type UsageInfo = {
  by_model: Record<string, ModelUsageSnapshot>;
  total_requests: number;
  total_input_tokens: number;
  total_output_tokens: number;
  total_sessions: number;
  estimated_cost_usd: number;
  window_secs: number;
};

export async function fetchUsage(): Promise<UsageInfo> {
  return apiFetch<UsageInfo>("/api/v1/usage");
}

// LLM model catalog (persisted ~/.cyberclaw/models.json)

export type ModelEntry = {
  id: string;
  label?: string | null;
  provider?: string | null;
  role?: string | null;
};

export type ModelsCatalog = {
  current_default: string;
  models: ModelEntry[];
};

export async function fetchModelsCatalog(): Promise<ModelsCatalog> {
  return apiFetch<ModelsCatalog>("/api/v1/admin/llm/models");
}

export async function deleteModelFromCatalog(id: string): Promise<ModelsCatalog> {
  return apiFetch<ModelsCatalog>(`/api/v1/admin/llm/models/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
}

export async function setDefaultModel(id: string): Promise<ModelsCatalog> {
  return apiFetch<ModelsCatalog>("/api/v1/admin/llm/models/default", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ id }),
  });
}

export async function addModelToCatalog(entry: ModelEntry): Promise<ModelsCatalog> {
  return apiFetch<ModelsCatalog>("/api/v1/admin/llm/models", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(entry),
  });
}

// Cron Jobs
export type CronJob = {
  id: string;
  name: string;
  schedule: string;        // "0 */5 * * * *" 6-field cron expression (seconds required)
  action_type: string;     // "echo" | "chat" | "webhook"
  action_payload: string;
  enabled: boolean;
  last_run_at?: string;
  next_run_at?: string;
  last_status?: "ok" | "failed" | "running" | string;
};

export async function fetchCronJobs(): Promise<CronJob[]> {
  const data = await apiFetch<CronJob[] | { jobs: CronJob[] }>("/api/v1/cron");
  return Array.isArray(data) ? data : (data as { jobs?: CronJob[] }).jobs ?? [];
}

export async function createCronJob(body: {
  name: string;
  schedule: string;
  action_type?: string;
  action_payload?: string;
  enabled?: boolean;
}): Promise<CronJob> {
  return apiFetch<CronJob>("/api/v1/cron", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export async function updateCronJob(
  id: string,
  body: Partial<Pick<CronJob, "name" | "schedule" | "action_type" | "action_payload" | "enabled">>,
): Promise<CronJob> {
  return apiFetch<CronJob>(`/api/v1/cron/${encodeURIComponent(id)}`, {
    method: "PATCH",
    body: JSON.stringify(body),
  });
}

export async function deleteCronJob(id: string): Promise<void> {
  await apiFetch<unknown>(`/api/v1/cron/${encodeURIComponent(id)}`, { method: "DELETE" });
}

export async function runCronJobNow(id: string): Promise<CronJob> {
  return apiFetch<CronJob>(`/api/v1/cron/${encodeURIComponent(id)}/run`, { method: "POST" });
}

// Settings config — server stores raw TOML; GET returns the file path + the
// raw TOML body (NOT parsed JSON). PUT expects { content: <raw toml> }.
export type SettingsConfigResponse = { path: string; content: string; redacted?: boolean };
export async function fetchSettingsConfig(): Promise<SettingsConfigResponse> {
  return apiFetch<SettingsConfigResponse>("/api/v1/settings/config");
}
export async function updateSettingsConfig(content: string): Promise<void> {
  await apiFetch<unknown>("/api/v1/settings/config", {
    method: "PUT",
    body: JSON.stringify({ content }),
  });
}

// Settings env — GET returns array of { key, value, secret }
export type EnvVar = { key: string; value: string; secret: boolean };
export async function fetchSettingsEnv(): Promise<Record<string, string>> {
  const data = await apiFetch<EnvVar[] | Record<string, string>>("/api/v1/settings/env");
  if (Array.isArray(data)) {
    return Object.fromEntries(data.map((e) => [e.key, e.value]));
  }
  return data as Record<string, string>;
}
export async function updateSettingsEnv(env: Record<string, string>): Promise<void> {
  await apiFetch<unknown>("/api/v1/settings/env", {
    method: "PUT",
    body: JSON.stringify(env),
  });
}

// Agents — mutations
export async function createAgent(payload: {
  name: string;
  description?: string;
  trust_level?: string;
  tools?: string[];
  skills?: string[];
}): Promise<Agent> {
  return apiFetch<Agent>("/api/v1/agents", {
    method: "POST",
    body: JSON.stringify(payload),
  });
}

/// PATCH /api/v1/agents/:id — partial update.
/// Backend allows updating description, trust_level, status only.
/// Name + skills + tools are immutable after creation by design.
export async function patchAgent(
  id: string,
  body: { description?: string; trust_level?: string; status?: string },
): Promise<Agent> {
  return apiFetch<Agent>(`/api/v1/agents/${encodeURIComponent(id)}`, {
    method: "PATCH",
    body: JSON.stringify(body),
  });
}

// Skills — toggle enable/disable
export async function toggleSkill(skill_id: string, enabled: boolean): Promise<Skill> {
  return apiFetch(`/api/v1/skills/${encodeURIComponent(skill_id)}/toggle`, {
    method: "POST",
    body: JSON.stringify({ enabled }),
  });
}

// Skills — mutations
export async function installSkill(source: string, name: string): Promise<Skill> {
  return apiFetch<Skill>("/api/v1/skills/install", {
    method: "POST",
    body: JSON.stringify({ source, name }),
  });
}
export async function installSkillRemote(url: string): Promise<Skill> {
  return apiFetch<Skill>("/api/v1/skills/install-remote", {
    method: "POST",
    body: JSON.stringify({ url }),
  });
}
export async function createSkill(payload: {
  name: string;
  category?: string;
  description?: string;
  content?: string;
}): Promise<Skill> {
  return apiFetch<Skill>("/api/v1/skills/create", {
    method: "POST",
    body: JSON.stringify(payload),
  });
}
export async function uninstallSkill(skill_id: string): Promise<void> {
  await apiFetch<unknown>(`/api/v1/skills/${encodeURIComponent(skill_id)}`, {
    method: "DELETE",
  });
}

// Channels — mutations
export async function updateChannel(id: string, body: Partial<Channel>): Promise<void> {
  await apiFetch<unknown>(`/api/v1/channels/${encodeURIComponent(id)}`, {
    method: "PUT",
    body: JSON.stringify(body),
  });
}

// IM Platforms — mutations
export async function putImPlatformConfig(
  name: string,
  config: Record<string, unknown>,
): Promise<void> {
  await apiFetch<unknown>(`/api/v1/admin/im-platforms/${encodeURIComponent(name)}`, {
    method: "PUT",
    body: JSON.stringify(config),
  });
}
export async function testImPlatform(
  name: string,
): Promise<{ ok: boolean; detail?: string }> {
  return apiFetch<{ ok: boolean; detail?: string }>(
    `/api/v1/admin/im-platforms/${encodeURIComponent(name)}/test`,
    { method: "POST" },
  );
}

// Profiles — user-level personality / SOUL preference layer.
export type Profile = {
  id: string;
  name: string;
  system_prompt: string;
  default_agent_id?: string;
  default_model?: string;
  temperature?: number;
  max_tokens?: number;
  created_at: string;
  updated_at: string;
  owner_user_id: string;
};
export async function fetchProfiles(): Promise<Profile[]> {
  const data = await apiFetch<{ profiles: Profile[] }>("/api/v1/profiles");
  return data.profiles ?? [];
}
export async function createProfile(p: Partial<Profile>): Promise<Profile> {
  return apiFetch<Profile>("/api/v1/profiles", {
    method: "POST",
    body: JSON.stringify(p),
  });
}
export async function updateProfile(id: string, p: Partial<Profile>): Promise<Profile> {
  return apiFetch<Profile>(`/api/v1/profiles/${encodeURIComponent(id)}`, {
    method: "PATCH",
    body: JSON.stringify(p),
  });
}
export async function deleteProfile(id: string): Promise<void> {
  await apiFetch(`/api/v1/profiles/${encodeURIComponent(id)}`, { method: "DELETE" });
}

// Security — permission rules (DangerousCapabilityFilter + ToolPermissionMatcher)
export type PermissionRule = {
  id: string;
  source: "dcf" | "tpm" | string;
  pattern: string;
  action: "allow" | "ask" | "deny" | "warn" | string;
  severity: "Info" | "Low" | "Medium" | "High" | "Critical" | string;
  enabled: boolean;
};
export async function fetchPermissionRules(): Promise<PermissionRule[]> {
  const data = await apiFetch<{ rules: PermissionRule[] }>(
    "/api/v1/security/permission/rules",
  );
  return data.rules ?? [];
}
export async function togglePermissionRule(
  id: string,
  enabled: boolean,
): Promise<{ proposal_id: string; status: string }> {
  return apiFetch(`/api/v1/security/permission/rules/${encodeURIComponent(id)}`, {
    method: "PATCH",
    body: JSON.stringify({ enabled }),
  });
}

// Security — injection hits
export type InjectionHit = {
  id: string;
  severity: string;
  pattern: string;
  actor: string;
  trace_id: string;
  rule_id: string;
  ts: number;
};
export async function fetchInjectionHits(): Promise<InjectionHit[]> {
  const data = await apiFetch<{ hits: InjectionHit[] }>(
    "/api/v1/security/injection/hits",
  );
  return data.hits ?? [];
}

// Capability Requests (admin gap queue)
export type CapabilityRequestRow = {
  id: string;
  tool_name: string;
  failure_reason?: string;
  attempted_capability_id?: string;
  attempted_connector_id?: string;
  count?: number;
  status: "pending" | "implemented" | "rejected" | string;
  notes?: string;
  requested_at?: string;
  last_seen_at?: string;
  agent_id?: string;
};
export type CapabilityRequestsResponse = {
  entries: CapabilityRequestRow[];
  total_returned: number;
};
export async function fetchCapabilityRequests(
  status?: string,
  limit = 200,
): Promise<CapabilityRequestsResponse> {
  const params = new URLSearchParams({ limit: String(limit) });
  if (status && status !== "all") params.set("status", status);
  return apiFetch<CapabilityRequestsResponse>(
    `/api/v1/admin/capability-requests?${params}`,
  );
}

// Skill Usage Telemetry (admin curator)
export type SkillUsageRow = {
  skill: string;
  origin: string;
  use_count: number;
  last_used_at: number; // unix timestamp (seconds)
  days_idle: number;
};
export type SkillUsageResponse = { records: SkillUsageRow[] };
export async function fetchSkillUsage(): Promise<SkillUsageResponse> {
  return apiFetch<SkillUsageResponse>("/api/v1/admin/curator/skill-usage");
}

// ---- Audit Agent (Reviews governance analytics) ----

export type LearnedPattern = {
  pattern: string;
  count: number;
  last_seen: string;
};

export type AuditAgentProfile = {
  agent_id?: string;
  model_id?: string;
  accuracy: number;           // 0.0 – 1.0
  total_decisions: number;
  auto_approved: number;
  manual_override: number;
  pending_labels?: number;
  learned_patterns: LearnedPattern[];
};

export type AccuracyCurvePoint = {
  date: string;               // ISO date "YYYY-MM-DD"
  human_agreement?: number;   // 0.0 – 1.0
  false_positive_rate?: number;
  false_negative_rate?: number;
  accuracy?: number;          // single-series fallback
};

export type AccuracyCurveResponse = {
  points: AccuracyCurvePoint[];
  days?: number;
};

export async function fetchAuditAgentProfile(): Promise<AuditAgentProfile> {
  return apiFetch<AuditAgentProfile>("/api/v1/reviews/audit-agent/profile");
}

export async function fetchAuditAgentAccuracyCurve(
  days = 7,
): Promise<AccuracyCurveResponse> {
  return apiFetch<AccuracyCurveResponse>(
    `/api/v1/reviews/audit-agent/accuracy-curve?days=${days}`,
  );
}

// ---- Intent Parsing (dev tool) ----

export type IntentParseResult = {
  intent_class: string;       // e.g. "CreateSkill" | "Execute" | "Query" | "Approval"
  confidence: number;         // 0.0 – 1.0
  entities: Record<string, string | number | boolean>;
  recommended_route?: string;
};

export async function parseIntent(message: string): Promise<IntentParseResult> {
  return apiFetch<IntentParseResult>("/api/v1/reviews/parse-intent", {
    method: "POST",
    body: JSON.stringify({ message }),
  });
}

// Governance — policy rules
export type PolicyRule = {
  kind: "allow" | "deny" | string;
  agent_id: string | null;
  capability_id: string | null;
  priority: number;
  reason: string | null;
};
export type PolicyRulesResponse = {
  engine: "rule_based" | "default" | string;
  rules: PolicyRule[];
};
export async function fetchPolicyRules(): Promise<PolicyRulesResponse> {
  return apiFetch<PolicyRulesResponse>("/api/v1/governance/policy-rules");
}

// Curator status
export type CuratorVerdictRow = {
  skill: string;
  action: string;
  reason: string;
  decided_at: number; // unix seconds
};
export type CuratorStatusResponse = {
  enabled: boolean;
  last_run_at: number | null;
  next_run_at: number | null;
  total_runs: number;
  recent_verdicts: CuratorVerdictRow[];
};
export async function fetchCuratorStatus(): Promise<CuratorStatusResponse> {
  return apiFetch<CuratorStatusResponse>("/api/v1/admin/curator/status");
}

export type CuratorRunNowResponse = { run_id: string; started_at: number };
export async function runCuratorNow(): Promise<CuratorRunNowResponse> {
  return apiFetch<CuratorRunNowResponse>("/api/v1/admin/curator/run-now", {
    method: "POST",
  });
}

// Daily digest
export type DigestKpi = { value: number; delta: string; sub: string };
export type DigestStats = {
  executions: DigestKpi;
  approvals: DigestKpi;
  learning_entries: DigestKpi;
  incidents: DigestKpi;
};
export type DigestFeedEntry = {
  ts: string;
  avatar: string;
  name: string;
  body: string;
  tags: string[];
};
export type DigestFeedBucket = { bucket: string; entries: DigestFeedEntry[] };
export type DigestHighlight = { icon: string; label: string; tone: string };
export type DailyDigestResponse = {
  date: string;
  stats: DigestStats;
  highlights: DigestHighlight[];
  recommendations: Record<string, unknown>[];
  feed: DigestFeedBucket[];
};
export async function fetchDailyDigest(date?: string): Promise<DailyDigestResponse> {
  const params = date ? `?date=${encodeURIComponent(date)}` : "";
  return apiFetch<DailyDigestResponse>(`/api/v1/learning/daily-digest${params}`);
}

// Org memory
export type OrgMemoryEntry = {
  id: string;
  scope: Record<string, unknown>;
  kind: string;
  content: string;
  rules: unknown[];
  provenance: Record<string, unknown>;
  created_at: string;
  ttl_secs?: number | null;
};
export type OrgMemoryListResponse = { entries: OrgMemoryEntry[] };
export async function fetchOrgMemory(limit = 100): Promise<OrgMemoryEntry[]> {
  const data = await apiFetch<OrgMemoryListResponse>(
    `/api/v1/learning/org-memory?limit=${limit}`,
  );
  return data.entries ?? [];
}

// Evolution timeline
export type EvolutionCycle = {
  id: string;
  started_at: string;
  ended_at?: string | null;
  outcome: string; // "converged" | "max_iterations" | "failed"
  gene_changed?: string | null;
  meta: Record<string, unknown>;
};
export type EvolutionTimelineResponse = { cycles: EvolutionCycle[] };
export async function fetchEvolutionTimeline(days = 30): Promise<EvolutionCycle[]> {
  const data = await apiFetch<EvolutionTimelineResponse>(
    `/api/v1/learning/evolution/timeline?days=${days}`,
  );
  return data.cycles ?? [];
}

// ─── Trace / Provenance / ExecutionTree ──────────────────────────────────────

// Runtime timeline event (from /api/v1/runtime/timeline).
export type RuntimeEvent = {
  id: string;
  rule: string;
  severity: string;
  action: string;
  actor: string;
  trace_id: string;
  ts: number; // unix ms
  offset_h?: number;
};
export type RuntimeTimelineResponse = { events: RuntimeEvent[] };

// Fetch runtime timeline events, optionally filtered by trace_id.
// Falls back to audit log aggregation if the endpoint is unavailable.
export async function fetchTraceTimeline(
  trace_id?: string,
  limit = 50,
): Promise<RuntimeEvent[]> {
  try {
    const params = new URLSearchParams({ limit: String(limit) });
    if (trace_id) params.set("trace_id", trace_id);
    const data = await apiFetch<RuntimeTimelineResponse>(
      `/api/v1/runtime/timeline?${params}`,
    );
    const events = data.events ?? [];
    if (trace_id) {
      return events.filter((e) => e.trace_id === trace_id);
    }
    return events;
  } catch {
    // Fallback: pull audit logs and reshape into RuntimeEvent shape
    const logs = await fetchAuditLogs({ limit });
    const entries = (logs.entries ?? []) as AuditEntry[];
    return entries
      .filter(
        (e) =>
          !trace_id ||
          (e.detail?.trace_id as string | undefined) === trace_id,
      )
      .map((e, i) => ({
        id: `audit-${i}`,
        rule: e.action,
        severity: "low",
        action: typeof e.result === "string" ? e.result : "failure",
        actor: e.actor,
        trace_id: (e.detail?.trace_id as string | undefined) ?? "",
        ts: new Date(e.ts).getTime(),
      }));
  }
}

// Fetch parent+children execution tree rooted at exec_id.
export async function fetchExecutionTree(exec_id?: string): Promise<Execution[]> {
  const all = await fetchExecutions("all", "all", 100);
  if (!exec_id) return all;
  const result: Execution[] = [];
  const queue = [exec_id];
  const seen = new Set<string>();
  while (queue.length > 0) {
    const cur = queue.shift()!;
    if (seen.has(cur)) continue;
    seen.add(cur);
    const node = all.find((e) => e.exec_id === cur);
    if (node) {
      result.push(node);
      all
        .filter(
          (e) => (e as Execution & { parent_id?: string }).parent_id === cur,
        )
        .forEach((child) => queue.push(child.exec_id));
    }
  }
  return result.length > 0 ? result : all;
}

// Fetch recent trace_id list from timeline (used for selector dropdown).
export async function fetchRecentTraceIds(limit = 20): Promise<string[]> {
  try {
    const data = await apiFetch<RuntimeTimelineResponse>(
      `/api/v1/runtime/timeline?limit=${limit * 3}`,
    );
    const seen = new Set<string>();
    const ids: string[] = [];
    for (const ev of data.events ?? []) {
      if (ev.trace_id && !seen.has(ev.trace_id)) {
        seen.add(ev.trace_id);
        ids.push(ev.trace_id);
        if (ids.length >= limit) break;
      }
    }
    return ids;
  } catch {
    return [];
  }
}
