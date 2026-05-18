// Runtime data layer for CyberClaw Admin Console.
//
// Pages consume `window.cc.data.useXxx()` hooks. Each hook wraps a live
// `cc.api.xxx()` call and falls back to the MOCK constant (below) when the
// backend is unreachable or returns an error. This is intentional: backend
// endpoints are still landing under Sprint 7; the MOCK fallback keeps the
// console usable during the transition.
//
// Mutations: callers invoke `cc.api.xxx.yyy(...)` directly and call
// `cc.data.invalidate('resource_key')` on success to trigger a refetch.

const MOCK = {
  operator: { user_id: 'op_ada', display_name: 'Ada Chen', role: 'admin', avatar_initials: 'AC' },
  version: '0.1.0',
  commit: '7a3f4c9',
  build_time: '2026-04-17T14:22:00Z',
  uptime_secs: 287421,

  // Status / dashboard
  dashboard: {
    health: 'healthy', // healthy | degraded | incident
    counts: {
      active_agents: 12,
      pending_reviews: 3,
      tasks_today: 187,
      system_health_score: 98,
    },
    trend: {
      tasks_today: '+12%',
      pending_reviews: '-2',
      active_agents: '+1',
      system_health_score: 'stable',
    },
  },

  agents: [
    { agent_id: 'ag_01H8X', name: 'research-analyst', description: 'Synthesizes web + doc research into structured briefs', trust_level: 'high', status: 'active', tools: 14, skills: 6, executions_24h: 42 },
    { agent_id: 'ag_01H9A', name: 'code-reviewer', description: 'Reviews PRs against house style; comments inline', trust_level: 'high', status: 'active', tools: 9, skills: 4, executions_24h: 31 },
    { agent_id: 'ag_01H9B', name: 'triage-bot', description: 'Classifies inbound tickets, routes to humans + sets SLA', trust_level: 'medium', status: 'active', tools: 7, skills: 3, executions_24h: 128 },
    { agent_id: 'ag_01HAC', name: 'release-captain', description: 'Orchestrates release checklist; requires approvals for production', trust_level: 'medium', status: 'idle', tools: 22, skills: 8, executions_24h: 3 },
    { agent_id: 'ag_01HBD', name: 'data-exfil-detector', description: 'Monitors connector calls for sensitive-data leakage', trust_level: 'high', status: 'active', tools: 4, skills: 2, executions_24h: 912 },
    { agent_id: 'ag_01HC3', name: 'onboarding-buddy', description: 'Guides new hires through week-1 tasks', trust_level: 'low', status: 'active', tools: 6, skills: 5, executions_24h: 7 },
    { agent_id: 'ag_01HD1', name: 'incident-commander', description: 'Declares + runs incident channels; pages on-call', trust_level: 'high', status: 'paused', tools: 18, skills: 7, executions_24h: 0 },
    { agent_id: 'ag_01HE7', name: 'finance-reconciler', description: 'Reconciles daily Stripe + ledger; flags anomalies', trust_level: 'medium', status: 'active', tools: 5, skills: 3, executions_24h: 24 },
  ],

  skills: [
    { skill_id: 'sk_pr_review', name: 'pull-request-review', category: 'Development', source: 'builtin', description: 'Walks a diff, posts contextual review comments', installed_at: '2026-02-12' },
    { skill_id: 'sk_unit_test', name: 'unit-test-authoring', category: 'Development', source: 'builtin', description: 'Generates unit tests from a function signature + behaviors', installed_at: '2026-02-12' },
    { skill_id: 'sk_migration', name: 'db-migration-writer', category: 'Development', source: 'hub', description: 'Drafts reversible DB migrations', installed_at: '2026-03-04' },
    { skill_id: 'sk_web_search', name: 'web-research-synthesizer', category: 'Research', source: 'builtin', description: 'Plans + executes multi-step research, cites sources', installed_at: '2026-02-12' },
    { skill_id: 'sk_lit_review', name: 'literature-review', category: 'Research', source: 'hub', description: 'Summarizes + clusters papers for systematic review', installed_at: '2026-03-18' },
    { skill_id: 'sk_calendar', name: 'calendar-scheduler', category: 'Productivity', source: 'builtin', description: 'Finds meeting slots across calendars with tz awareness', installed_at: '2026-02-12' },
    { skill_id: 'sk_inbox', name: 'inbox-triager', category: 'Productivity', source: 'builtin', description: 'Labels, snoozes, drafts replies for email triage', installed_at: '2026-02-12' },
    { skill_id: 'sk_copy', name: 'copywriting-assistant', category: 'Creative', source: 'hub', description: 'Drafts on-brand copy from briefs', installed_at: '2026-03-22' },
    { skill_id: 'sk_illu', name: 'illustration-director', category: 'Creative', source: 'local', description: 'Plans + orchestrates image gen pipelines', installed_at: '2026-04-01' },
    { skill_id: 'sk_orch', name: 'agent-orchestrator', category: 'Agents', source: 'builtin', description: 'Spawns + coordinates sub-agents for parallel subtasks', installed_at: '2026-02-12' },
    { skill_id: 'sk_mem', name: 'long-term-memory', category: 'Agents', source: 'builtin', description: 'Persists episodic memory across sessions', installed_at: '2026-02-12' },
    { skill_id: 'sk_misc', name: 'filesystem-toolkit', category: 'Other', source: 'local', description: 'Safe fs operations with path allow-listing', installed_at: '2026-04-10' },
  ],

  tasks: [
    { task_id: 't_01JX4A', description: 'Draft Q2 product brief from PRD + research notes', status: 'running', created_at: nowMinus(4 * 60), elapsed: '04:12', task_type: 'synthesis' },
    { task_id: 't_01JX4B', description: 'Triage 23 inbound security reports', status: 'running', created_at: nowMinus(11 * 60), elapsed: '11:38', task_type: 'triage' },
    { task_id: 't_01JX42', description: 'Reconcile Stripe payouts for 2026-04-17', status: 'done', created_at: nowMinus(3600), elapsed: '18:44', task_type: 'reconcile' },
    { task_id: 't_01JX3Z', description: 'Generate release notes for v0.2.0', status: 'done', created_at: nowMinus(5400), elapsed: '02:11', task_type: 'synthesis' },
    { task_id: 't_01JX3X', description: 'Review PR #4429 — retry backoff refactor', status: 'done', created_at: nowMinus(7200), elapsed: '06:02', task_type: 'code_review' },
    { task_id: 't_01JX3U', description: 'Schedule all-hands across 4 timezones', status: 'done', created_at: nowMinus(8400), elapsed: '01:03', task_type: 'scheduling' },
    { task_id: 't_01JX3S', description: 'Classify support ticket #19203', status: 'failed', created_at: nowMinus(9100), elapsed: '00:08', task_type: 'triage', error: 'connector "zendesk.read_ticket" returned 401' },
    { task_id: 't_01JX3Q', description: 'Produce weekly engineering digest', status: 'pending', created_at: nowMinus(200), elapsed: '—', task_type: 'synthesis' },
    { task_id: 't_01JX3M', description: 'Kick off incident IR-488', status: 'cancelled', created_at: nowMinus(13400), elapsed: '00:22', task_type: 'incident' },
  ],

  executions: [
    { execution_id: 'ex_01N9F2', agent: 'research-analyst', capability: 'web.search_and_read', status: 'running', risk_level: 'low', started_at: nowMinus(120), duration: '02:00' },
    { execution_id: 'ex_01N9F1', agent: 'code-reviewer', capability: 'github.post_review_comment', status: 'running', risk_level: 'medium', started_at: nowMinus(40), duration: '00:40' },
    { execution_id: 'ex_01N9F0', agent: 'release-captain', capability: 'k8s.rollout_deployment', status: 'pending_review', risk_level: 'critical', started_at: nowMinus(75), duration: '01:15' },
    { execution_id: 'ex_01N9EZ', agent: 'finance-reconciler', capability: 'stripe.list_payouts', status: 'done', risk_level: 'low', started_at: nowMinus(900), duration: '00:12' },
    { execution_id: 'ex_01N9EY', agent: 'triage-bot', capability: 'zendesk.read_ticket', status: 'done', risk_level: 'low', started_at: nowMinus(980), duration: '00:04' },
    { execution_id: 'ex_01N9EX', agent: 'data-exfil-detector', capability: 'sentinel.audit_read', status: 'done', risk_level: 'low', started_at: nowMinus(1120), duration: '00:02' },
    { execution_id: 'ex_01N9EW', agent: 'code-reviewer', capability: 'github.read_pr', status: 'done', risk_level: 'low', started_at: nowMinus(1340), duration: '00:08' },
    { execution_id: 'ex_01N9EV', agent: 'release-captain', capability: 'github.create_release', status: 'failed', risk_level: 'high', started_at: nowMinus(2400), duration: '00:31', error: 'Permission denied: actor missing `repo:release` scope' },
    { execution_id: 'ex_01N9EU', agent: 'onboarding-buddy', capability: 'slack.post_message', status: 'done', risk_level: 'medium', started_at: nowMinus(3100), duration: '00:06' },
    { execution_id: 'ex_01N9ET', agent: 'finance-reconciler', capability: 'stripe.list_charges', status: 'done', risk_level: 'low', started_at: nowMinus(3800), duration: '00:19' },
    { execution_id: 'ex_01N9ES', agent: 'incident-commander', capability: 'pagerduty.trigger_incident', status: 'cancelled', risk_level: 'high', started_at: nowMinus(5200), duration: '00:02' },
    { execution_id: 'ex_01N9ER', agent: 'research-analyst', capability: 'notion.create_page', status: 'done', risk_level: 'medium', started_at: nowMinus(6400), duration: '00:44' },
  ],

  reviews: [
    {
      review_id: 'rv_01Q7A1',
      risk_level: 'critical',
      capability_id: 'k8s.rollout_deployment',
      actor: 'release-captain',
      reason_for_review: 'Production rollout to `prod-us-east` cluster for service `payments-api` requires human approval (DangerousCapabilityFilter: rule #12).',
      proposed_input: {
        cluster: 'prod-us-east',
        namespace: 'payments',
        deployment: 'payments-api',
        image: 'registry.internal/payments-api:v0.2.0-rc3',
        strategy: 'rolling',
        max_surge: '25%',
        max_unavailable: 0,
      },
      requested_at: nowMinus(75),
      execution_id: 'ex_01N9F0',
      target: { type: 'execution', execution_id: 'ex_01N9F0' },
    },
    {
      review_id: 'rv_01Q7A0',
      risk_level: 'high',
      capability_id: 'stripe.create_refund',
      actor: 'finance-reconciler',
      reason_for_review: 'Refund amount ($4,280.00) exceeds auto-approve threshold of $1,000.',
      proposed_input: {
        charge_id: 'ch_3P9fxQ2eZvKYlo2C0yXj9Vqe',
        amount_cents: 428000,
        currency: 'usd',
        reason: 'duplicate',
        metadata: { ticket: 'ZDK-19087' },
      },
      requested_at: nowMinus(320),
      target: { type: 'execution', execution_id: 'ex_01N9F1' },
    },
    {
      review_id: 'rv_01Q79Z',
      risk_level: 'high',
      capability_id: 'agent.handoff',
      actor: 'pm-review-agent',
      reason_for_review: 'Agent A wants to transfer this conversation to backend-expert agent for schema design feedback. Briefing summarizes Q3 launch constraints and prior decisions (~1.4KB).',
      proposed_input: {
        from_agent_id: 'pm-review-agent',
        to_agent_id: 'backend-expert',
        conversation_id: 'conv_01Q73B',
        reason: 'Schema review needs deeper backend expertise',
      },
      requested_at: nowMinus(740),
      target: { type: 'handoff', handoff_id: 'ho_01Q79Z' },
    },
  ],

  reviews_history: [
    { review_id: 'rv_01Q78F', risk_level: 'high', capability_id: 'slack.admin_invite', actor: 'onboarding-buddy', decision: 'approved', decided_by: 'op_ada', decided_at: nowMinus(2 * 3600) },
    { review_id: 'rv_01Q784', risk_level: 'critical', capability_id: 'aws.iam_attach_policy', actor: 'release-captain', decision: 'rejected', decided_by: 'op_ada', decided_at: nowMinus(5 * 3600), reason: 'Grants * on * — too broad. Scope to payments role.' },
    { review_id: 'rv_01Q77Y', risk_level: 'medium', capability_id: 'notion.share_public', actor: 'research-analyst', decision: 'approved', decided_by: 'op_liu', decided_at: nowMinus(9 * 3600) },
  ],

  capabilities: [
    {
      connector_id: 'github',
      name: 'GitHub',
      icon: 'github',
      capabilities: [
        { id: 'github.read_pr', name: 'Read PR', risk_level: 'low', effects: ['Read'], description: 'Fetch PR metadata, files, reviews' },
        { id: 'github.post_review_comment', name: 'Post review comment', risk_level: 'medium', effects: ['Write'], description: 'Post an inline review comment on a PR' },
        { id: 'github.merge_pr', name: 'Merge PR', risk_level: 'high', effects: ['Write', 'Execute'], description: 'Merge a pull request' },
        { id: 'github.create_release', name: 'Create release', risk_level: 'high', effects: ['Write'], description: 'Create a tagged release' },
      ],
    },
    {
      connector_id: 'slack',
      name: 'Slack',
      capabilities: [
        { id: 'slack.post_message', name: 'Post message', risk_level: 'medium', effects: ['Write'], description: 'Post a message to a channel or DM' },
        { id: 'slack.read_channel', name: 'Read channel', risk_level: 'low', effects: ['Read'], description: 'Read channel messages' },
        { id: 'slack.admin_invite', name: 'Invite user (admin)', risk_level: 'high', effects: ['Write', 'Execute'], description: 'Invite a user to the workspace' },
      ],
    },
    {
      connector_id: 'stripe',
      name: 'Stripe',
      capabilities: [
        { id: 'stripe.list_payouts', name: 'List payouts', risk_level: 'low', effects: ['Read'], description: 'List recent payouts' },
        { id: 'stripe.list_charges', name: 'List charges', risk_level: 'low', effects: ['Read'], description: 'List charges for a customer' },
        { id: 'stripe.create_refund', name: 'Create refund', risk_level: 'high', effects: ['Write', 'Execute'], description: 'Refund a charge' },
      ],
    },
    {
      connector_id: 'k8s',
      name: 'Kubernetes',
      capabilities: [
        { id: 'k8s.list_pods', name: 'List pods', risk_level: 'low', effects: ['Read'], description: 'List pods in a namespace' },
        { id: 'k8s.rollout_deployment', name: 'Rollout deployment', risk_level: 'critical', effects: ['Write', 'Execute'], description: 'Apply a new deployment revision' },
        { id: 'k8s.delete_pod', name: 'Delete pod', risk_level: 'high', effects: ['Execute'], description: 'Delete a pod' },
      ],
    },
    {
      connector_id: 'notion',
      name: 'Notion',
      capabilities: [
        { id: 'notion.read_page', name: 'Read page', risk_level: 'low', effects: ['Read'], description: 'Read a Notion page' },
        { id: 'notion.create_page', name: 'Create page', risk_level: 'medium', effects: ['Write'], description: 'Create a Notion page' },
        { id: 'notion.share_public', name: 'Share publicly', risk_level: 'medium', effects: ['Write'], description: 'Make a page publicly accessible' },
      ],
    },
    {
      connector_id: 'aws',
      name: 'AWS',
      capabilities: [
        { id: 'aws.s3_list', name: 'List S3 objects', risk_level: 'low', effects: ['Read'], description: 'List objects in a bucket' },
        { id: 'aws.iam_attach_policy', name: 'Attach IAM policy', risk_level: 'critical', effects: ['Write', 'Execute'], description: 'Attach a managed policy to a role' },
      ],
    },
  ],

  channels: [
    { id: 'discord', name: 'Discord', enabled: true, status: 'connected', last_event: '32s ago', config: { guild_id: '912038471', bot_name: 'cyberclaw' } },
    { id: 'slack', name: 'Slack', enabled: true, status: 'connected', last_event: '4s ago', config: { workspace: 'cyberclaw', app_id: 'A05XR2' } },
    { id: 'telegram', name: 'Telegram', enabled: true, status: 'connected', last_event: '2m ago', config: { bot_username: 'cyberclaw_bot' } },
    { id: 'whatsapp', name: 'WhatsApp', enabled: false, status: 'disabled', last_event: '—', config: {} },
    { id: 'signal', name: 'Signal', enabled: false, status: 'disabled', last_event: '—', config: {} },
    { id: 'gchat', name: 'Google Chat', enabled: true, status: 'connected', last_event: '11m ago', config: { space: 'spaces/AAAA' } },
    { id: 'imessage', name: 'iMessage', enabled: false, status: 'disabled', last_event: '—', config: {} },
    { id: 'webhook', name: 'Webhook', enabled: true, status: 'connected', last_event: '1s ago', config: { url: 'https://hooks.cyberclaw.io/in/9f2a' } },
  ],

  nodes: [
    { node_id: 'node-ctl-01', role: 'controller', status: 'leader', last_heartbeat: '1s ago', version: '0.1.0', cpu: 18, mem: 42, active_execs: 3, region: 'us-east-1' },
    { node_id: 'node-ctl-02', role: 'controller', status: 'follower', last_heartbeat: '1s ago', version: '0.1.0', cpu: 9, mem: 36, active_execs: 0, region: 'us-west-2' },
    { node_id: 'node-wrk-01', role: 'worker', status: 'online', last_heartbeat: '2s ago', version: '0.1.0', cpu: 64, mem: 71, active_execs: 5, region: 'us-east-1' },
    { node_id: 'node-wrk-02', role: 'worker', status: 'online', last_heartbeat: '1s ago', version: '0.1.0', cpu: 47, mem: 58, active_execs: 4, region: 'us-east-1' },
    { node_id: 'node-wrk-03', role: 'worker', status: 'online', last_heartbeat: '3s ago', version: '0.1.0', cpu: 22, mem: 44, active_execs: 2, region: 'eu-west-1' },
    { node_id: 'node-wrk-04', role: 'worker', status: 'draining', last_heartbeat: '4s ago', version: '0.1.0', cpu: 11, mem: 28, active_execs: 1, region: 'eu-west-1' },
    { node_id: 'node-wrk-05', role: 'worker', status: 'offline', last_heartbeat: '2m 14s ago', version: '0.0.9', cpu: 0, mem: 0, active_execs: 0, region: 'ap-south-1' },
  ],

  // Trace for ex_01N9F0 (release-captain rollout)
  trace: {
    execution_id: 'ex_01N9F0',
    total_ms: 1820,
    spans: [
      {
        id: 's1', name: 'agent.invoke', kind: 'agent', start: 0, dur: 1820, status: 'ok',
        attrs: { agent: 'release-captain', model: 'claude-sonnet-4-5' },
        children: [
          { id: 's1-1', name: 'llm.plan', kind: 'llm', start: 10, dur: 412, status: 'ok', attrs: { tokens_in: 2140, tokens_out: 318 } },
          { id: 's1-2', name: 'skill.release_checklist', kind: 'skill', start: 430, dur: 186, status: 'ok', attrs: {},
            children: [
              { id: 's1-2-1', name: 'fs.read', kind: 'tool', start: 440, dur: 18, status: 'ok', attrs: { path: 'RELEASE.md' } },
              { id: 's1-2-2', name: 'fs.read', kind: 'tool', start: 462, dur: 12, status: 'ok', attrs: { path: 'CHANGELOG.md' } },
            ],
          },
          { id: 's1-3', name: 'capability.github.create_release', kind: 'capability', start: 624, dur: 310, status: 'ok', attrs: { repo: 'cyberclaw/app', tag: 'v0.2.0-rc3' } },
          { id: 's1-4', name: 'capability.k8s.rollout_deployment', kind: 'capability', start: 944, dur: 28, status: 'pending_review', attrs: { cluster: 'prod-us-east' },
            children: [
              { id: 's1-4-1', name: 'governance.dangerous_capability_filter', kind: 'policy', start: 948, dur: 20, status: 'halt', attrs: { rule: '#12', decision: 'require_review' } },
            ],
          },
          { id: 's1-5', name: 'review.wait', kind: 'wait', start: 980, dur: 840, status: 'pending', attrs: { review_id: 'rv_01Q7A1' } },
        ],
      },
    ],
  },

  // Event stream seed
  eventSeeds: [
    { kind: 'exec.started',     color: 'accent', text: 'execution ex_01N9F0 started — release-captain → k8s.rollout_deployment' },
    { kind: 'policy.halt',      color: 'rose',   text: '⚠ DangerousCapabilityFilter halted exec ex_01N9F0 (rule #12)' },
    { kind: 'review.requested', color: 'amber',  text: 'review rv_01Q7A1 requested (risk=critical)' },
    { kind: 'exec.done',        color: 'emerald',text: 'exec ex_01N9EZ finished in 120ms — stripe.list_payouts' },
    { kind: 'agent.active',     color: 'muted',  text: 'agent research-analyst → active (tools=14 skills=6)' },
    { kind: 'heartbeat',        color: 'muted',  text: 'heartbeat node-wrk-03 ok (cpu=22 mem=44)' },
    { kind: 'task.queued',      color: 'muted',  text: 'task t_01JX3Q queued (task_type=synthesis)' },
    { kind: 'connector.call',   color: 'muted',  text: 'github.read_pr ← code-reviewer (repo=cyberclaw/app pr=4429)' },
    { kind: 'exec.done',        color: 'emerald',text: 'exec ex_01N9EW finished in 80ms — github.read_pr' },
    { kind: 'channel.inbound',  color: 'accent', text: 'slack #support message routed → triage-bot' },
    { kind: 'exec.failed',      color: 'rose',   text: '✗ exec ex_01N9EV failed: Permission denied (repo:release)' },
    { kind: 'capability.discovered', color: 'muted', text: 'connector stripe registered 12 capabilities' },
  ],

  config_toml: `# cyberclaw config (sample, read-only preview)
[server]
bind = "0.0.0.0:4142"
admin_bind = "127.0.0.1:4143"

[security]
jwt_secret_env = "CYBERCLAW_JWT_SECRET"
session_ttl_secs = 86400

[llm]
provider = "anthropic"
model = "claude-sonnet-4-5"
base_url = "https://api.anthropic.com"
max_tokens = 4096
temperature = 0.2

[governance]
dangerous_capability_filter = "policies/dangerous.yaml"
tool_permission_matcher     = "policies/tools.yaml"

[cluster]
mode = "multi-node"
discovery = "gossip"
`,

  env: [
    { key: 'CYBERCLAW_JWT_SECRET', value: 'c9f3a24b7e', secret: true },
    { key: 'ANTHROPIC_API_KEY', value: 'sk-ant-a1b2c3d4', secret: true },
    { key: 'DATABASE_URL', value: 'postgres://claw:***@db.internal:5432/cyberclaw', secret: true },
    { key: 'LOG_LEVEL', value: 'info', secret: false },
    { key: 'METRICS_PORT', value: '9090', secret: false },
    { key: 'REGION', value: 'us-east-1', secret: false },
  ],

  policies: {
    dangerous: [
      { rule: '#01', pattern: 'aws.iam_*', action: 'require_review', risk: 'critical' },
      { rule: '#07', pattern: 'stripe.create_refund WHERE amount_cents > 100000', action: 'require_review', risk: 'high' },
      { rule: '#12', pattern: 'k8s.rollout_deployment WHERE cluster.startsWith("prod-")', action: 'require_review', risk: 'critical' },
      { rule: '#19', pattern: 'github.merge_pr WHERE paths MATCH "infra/**"', action: 'require_review', risk: 'medium' },
      { rule: '#24', pattern: 'slack.admin_*', action: 'require_review', risk: 'high' },
    ],
    tools: [
      { actor: 'trust_level=high',    allow: ['read.*', 'write.*', 'execute.safe.*'], deny: ['execute.destructive.*'] },
      { actor: 'trust_level=medium',  allow: ['read.*', 'write.safe.*'],              deny: ['execute.*'] },
      { actor: 'trust_level=low',     allow: ['read.safe.*'],                          deny: ['write.*', 'execute.*'] },
    ],
  },
};

function nowMinus(s) {
  return new Date(Date.now() - s * 1000).toISOString();
}

Object.assign(window, { MOCK });

// ============================================================================
// Runtime data layer — real API calls with MOCK fallback.
// ============================================================================

const _ccCache = new Map();
const _ccSubs = new Map();

function _ccInvalidate(key) {
  // Match exact key + glob prefixes like "executions:*"
  const keys = [];
  _ccCache.forEach((_, k) => {
    if (k === key) keys.push(k);
    else if (key.endsWith(':*') && k.startsWith(key.slice(0, -1))) keys.push(k);
  });
  keys.forEach((k) => _ccCache.delete(k));

  const subKeys = [];
  _ccSubs.forEach((_, k) => {
    if (k === key) subKeys.push(k);
    else if (key.endsWith(':*') && k.startsWith(key.slice(0, -1))) subKeys.push(k);
  });
  subKeys.forEach((k) => {
    (_ccSubs.get(k) || []).forEach((fn) => {
      try { fn(); } catch {}
    });
  });
}

function _ccSubscribe(key, fn) {
  if (!_ccSubs.has(key)) _ccSubs.set(key, []);
  _ccSubs.get(key).push(fn);
  return () => {
    const arr = _ccSubs.get(key) || [];
    _ccSubs.set(key, arr.filter((f) => f !== fn));
  };
}

// MOCK fallback lookup — matches the first segment of the cache key
// (e.g. "executions:{}" → MOCK.executions).
function _ccMockFallback(key) {
  const head = key.split(':')[0];
  return MOCK[head];
}

// Normalize a paginated/enveloped list response to a plain array.
//
// Backend conventions (Sprint 7):
//   - `/api/v1/executions` → { executions: [...], total, offset, limit }
//   - `/api/v1/reviews`    → { reviews: [...], total }
//   - `/api/v1/tasks`      → { tasks: [...], ... }  OR  [...]
//   - Many other list endpoints return a bare array.
//
// `key` is the cache key (e.g. "executions:{}"); we use its head segment
// to look up the envelope field. Pages call this to avoid re-implementing
// "array | {items} | {resource: []} | error-fallback MOCK" logic at every
// consumption site.
function _ccExtractList(result, key, mockFallback) {
  if (Array.isArray(result.data)) return result.data;
  if (result.data && typeof result.data === 'object') {
    const head = key.split(':')[0];
    if (Array.isArray(result.data[head])) return result.data[head];
    if (Array.isArray(result.data.items)) return result.data.items;
  }
  if (result.error && mockFallback) return mockFallback;
  return [];
}

// useResource — React integration point.
function _useResource(key, fetcher) {
  const [state, setState] = React.useState({
    loading: !_ccCache.has(key),
    error: null,
    data: _ccCache.has(key) ? _ccCache.get(key) : null,
  });

  React.useEffect(() => {
    let cancelled = false;

    const run = async () => {
      if (_ccCache.has(key)) {
        if (!cancelled) {
          setState({ loading: false, error: null, data: _ccCache.get(key) });
        }
        return;
      }
      if (!cancelled) setState((s) => ({ ...s, loading: true }));
      try {
        const data = await fetcher();
        _ccCache.set(key, data);
        if (!cancelled) setState({ loading: false, error: null, data });
      } catch (e) {
        const fallback = _ccMockFallback(key);
        if (!cancelled) {
          setState({
            loading: false,
            error: e && e.message ? e.message : String(e),
            data: fallback != null ? fallback : null,
          });
        }
      }
    };

    run();
    const unsub = _ccSubscribe(key, () => run());
    return () => {
      cancelled = true;
      unsub();
    };
  }, [key]);

  return state;
}

// Expose data layer globally.
window.cc = window.cc || {};
window.cc.data = {
  useAgents: () =>
    _useResource('agents', () => window.cc.api.agents.list()),
  useSkills: () =>
    _useResource('skills', () => window.cc.api.skills.list()),
  useTasks: (status) =>
    _useResource('tasks:' + (status || 'all'), () =>
      window.cc.api.tasks.list(status)
    ),
  useExecutions: (params) =>
    _useResource(
      'executions:' + JSON.stringify(params || {}),
      () => window.cc.api.executions.list(params)
    ),
  useReviews: (status) =>
    _useResource('reviews:' + (status || 'pending'), () =>
      window.cc.api.reviews.list(status)
    ),
  useCapabilities: () =>
    _useResource('capabilities', () => window.cc.api.capabilities.discover()),
  useChannels: () =>
    _useResource('channels', () => window.cc.api.channels.list()),
  useNodes: () =>
    _useResource('nodes', () => window.cc.api.cluster.nodes()),
  useDashboard: () =>
    _useResource('dashboard', () => window.cc.api.dashboard()),
  useSettings: () =>
    _useResource('settings', () => window.cc.api.settings.config()),
  useEnv: () =>
    _useResource('env', () => window.cc.api.settings.env()),
  useAbout: () =>
    _useResource('about', () => window.cc.api.settings.about()),
  usePolicies: () =>
    _useResource('policies', () => window.cc.api.settings.policies()),
  useAudit: (params) =>
    _useResource('audit:' + JSON.stringify(params || {}), () =>
      window.cc.api.audit.logs(params)
    ),
  useConnectors: () =>
    _useResource('connectors', () => window.cc.api.connectors.list()),
  useTools: () =>
    _useResource('tools', () => window.cc.api.tools.list()),
  // S29: declarative policy rules from RuleBasedPolicyEngine (read-only).
  usePolicyRules: () =>
    _useResource('policy-rules', () => window.cc.api.governance.policyRules()),
  // Sprint 11 W2 / Sprint 18 W2 — observability hooks that replace the
  // remaining hardcoded MOCK_* constants in pages_b.jsx + pages_c.jsx.
  useAuditAgentProfile: () =>
    _useResource('audit-agent:profile', () => window.cc.api.audit.agentProfile()),
  useAuditAgentAccuracyCurve: (days) =>
    _useResource(
      'audit-agent:accuracy:' + (days || 30),
      () => window.cc.api.audit.agentAccuracyCurve(days),
    ),
  usePermissionRules: () =>
    _useResource('permission-rules', () => window.cc.api.security.permissionRules()),
  useInjectionHits: () =>
    _useResource('injection-hits', () => window.cc.api.security.injectionHits()),
  useRuntimeTimeline: () =>
    _useResource('runtime-timeline', () => window.cc.api.runtime.timeline()),
  useSkillPoisonVerdicts: () =>
    _useResource('skill-poison-verdicts', () => window.cc.api.skills.poisonVerdicts()),
  invalidate: _ccInvalidate,
  extractList: _ccExtractList,
};
