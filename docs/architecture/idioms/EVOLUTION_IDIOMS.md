# Evolution-Layer Idioms — Sprint 4+ PR-Gate Reference

> Required reading before authoring any new component in `cyberclaw-control-plane`,
> `cyberclaw-connectors`, or `cyberclaw-agent-runtime` during Sprint 4 and beyond.
> Every PR checklist includes "idiom compliance — verified" as a mandatory box.

---

## 0. Why this doc exists

Sprint 1–3 closed a working self-evolution loop across 13 commits (SkillArchive,
MutationEngine, FitnessEvaluator, StagedVerificationGate, EvolutionOrchestrator,
PersistentLoop, SandboxConnector). Four structural patterns emerged consistently
across these components that keep the five-object model intact and execution
testable without connectors. Regressing any one of them pollutes the architecture
fast — a single concrete connector import in a new orchestrator can break the
test-isolation boundary; a new "Wizard object" sitting beside Agent breaks the
object taxonomy. This doc makes the four patterns PR-checkable with concrete
line references so reviewers have ground truth, not memory.

---

## 1. Idiom: Injected Trait Dispatcher

**Rule**: Long-running orchestration components MUST accept I/O as a trait object,
never as a concrete connector import.

**Example** — `EvolutionDispatcher` trait
(`crates/cyberclaw-control-plane/src/evolution_orchestrator.rs`, lines 53–78):

```rust
/// The orchestrator's dispatch interface. Concrete implementations wire this
/// to the connector runtime; tests provide a mock. The orchestrator never
/// calls outside this trait for I/O — every LLM call, every variant
/// execution, every smoke-test run flows through it.
#[async_trait]
pub trait EvolutionDispatcher: Send + Sync {
    /// Execute a mutation plan. Implementations are expected to dispatch
    /// `plan.target_capability` through a connector and return the resulting
    /// diff / new skill text as a [`MutationResult`].
    async fn execute_mutation(&self, plan: &MutationPlan) -> anyhow::Result<MutationResult>;

    /// Execute a single evaluation case against a candidate variant's skill text.
    async fn execute_case(
        &self,
        variant: &SkillVariant,
        skill_text: &str,
        case: &EvaluationCase,
    ) -> anyhow::Result<CaseOutcome>;

    /// Run the smoke regression suite for [`StagedVerificationGate`].
    async fn run_smoke_regression(
        &self,
        variant: &SkillVariant,
        skill_text: &str,
    ) -> anyhow::Result<RegressionResult>;
}
```

The orchestrator's `step()` signature receives `dispatcher: &dyn EvolutionDispatcher`
— never `Arc<SandboxConnector>` or any concrete type.

**Why**:
- Unit-testable with `MockDispatcher` (no real connectors needed; see test module
  in the same file, lines 893–950).
- Preserves the "Connector is the only code-level capability interface" invariant
  from `CLAUDE.md` §3 without coupling the orchestrator to any specific connector.
- Allows the host binary to swap the real connector implementation without
  touching control-plane logic.

**PR check**:
- [ ] New component accepts `&dyn XxxDispatcher` (or equivalent trait), not `&ConnectorRegistry` or any concrete connector type
- [ ] Unit tests use a mock dispatcher struct; zero `cyberclaw-connectors` imports in test modules
- [ ] Grep in the new component file: zero hits on `use cyberclaw_connectors::`

**Antipattern** (rejected at review):

```rust
// DO NOT DO THIS
pub struct BadOrchestrator {
    connector: Arc<SandboxConnector>,  // concrete type — breaks test isolation
}
```

---

## 2. Idiom: Event Sink Trait

**Rule**: Every long-running component emits lifecycle events through an
`Option<Arc<dyn XxxEventSink>>` injected at construction. Execution proceeds
whether or not a sink is attached.

**Example** — `EvolutionEvent`, `EvolutionEventSink`, `NoopEventSink`, `VecEventSink`
(`crates/cyberclaw-control-plane/src/evolution_orchestrator.rs`, lines 271–376):

```rust
/// Structured events emitted at every state transition inside `step()`.
#[derive(Debug, Clone)]
pub enum EvolutionEvent {
    StepStarted { iteration: u32, skill_id: SkillId },
    ParentSelected { variant_id: VariantId, score: f32 },
    MutationCompleted { parent_id: VariantId, child_text_len: usize },
    EvaluationCompleted { variant_id: VariantId, case_count: usize, all_passed: bool },
    FitnessScored { variant_id: VariantId, breakdown: FitnessBreakdown },
    VariantArchived { variant_id: VariantId, composite_score: f32 },
    VariantRejected { reason: StepRejection, breakdown: Option<FitnessBreakdown> },
    StepFailed { error: String },
}

/// Optional subscriber for [`EvolutionEvent`].
#[async_trait]
pub trait EvolutionEventSink: Send + Sync {
    /// Record one event. Implementations should not panic or block on failure.
    async fn record(&self, event: EvolutionEvent);
}

/// Default sink: discards every event.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopEventSink;

#[async_trait]
impl EvolutionEventSink for NoopEventSink {
    async fn record(&self, _event: EvolutionEvent) {
        // intentionally empty
    }
}
```

The orchestrator field: `event_sink: Option<Arc<dyn EvolutionEventSink>>` (line 391).
Emission is a no-op when `None`: `if let Some(sink) = self.event_sink.as_ref() { sink.record(event).await; }` (lines 428–432).

The `VecEventSink` (lines 332–376) captures events into a `Mutex<Vec<_>>` for
test assertions. Tests assert on the exact event sequence (e.g., line 1633:
`assert_eq!(events.len(), 6)`).

**Why**:
- Observability is optional at the architectural level; the component works
  correctly with `NoopEventSink`.
- `trace_timeline` / `trace_summary` tools can subscribe without modifying the
  component under observation.
- `VecEventSink` makes event-sequence assertions deterministic in unit tests.

**PR check**:
- [ ] New long-running component defines `XxxEvent` enum + `XxxEventSink` trait
- [ ] Component holds `Option<Arc<dyn XxxEventSink>>`, defaulting to `None`
- [ ] `NoopXxxEventSink` provided and used as the zero-overhead default
- [ ] At least one test using `VecXxxEventSink` asserts on the event sequence

**Applies to**: Orchestrators, loops, pipelines, wizard sessions, any component
whose internal state transitions are observable to external systems.

---

## 3. Idiom: Declarative Plan-In + Events-Out

**Rule**: Long-running components take a **plan** as input (immutable description
of work) and emit **events** as output (observable state changes). They do NOT
accept mutable callback functions for business logic.

**Example** — `ExecutionPlan`, `Story`, `StoryState`
(`crates/cyberclaw-control-plane/src/persistent_execution.rs`, lines 119–277):

```rust
/// The lifecycle state of a story.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoryState {
    Pending,
    InProgress,
    Passed,
    Failed,
    Blocked,
}

/// A discrete unit of work with testable acceptance criteria.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Story {
    pub id: String,
    pub description: String,
    pub criteria: Vec<AcceptanceCriterion>,
    pub state: StoryState,
    pub priority: u32,
    pub attempt_count: u32,
    pub max_attempts: u32,
    pub depends_on: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A structured execution plan composed of ordered stories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub goal: String,
    pub stories: Vec<Story>,
    pub created_at: DateTime<Utc>,
    pub metadata: HashMap<String, String>,
}
```

**Example** — `EvolutionConfig` consumed by `run()`
(`crates/cyberclaw-control-plane/src/evolution_orchestrator.rs`, lines 131–163):

```rust
/// Configuration knobs for one orchestrator instance.
#[derive(Debug, Clone)]
pub struct EvolutionConfig {
    pub selection_method: SelectionMethod,
    pub min_score_to_archive: f32,
    pub max_iterations: u32,
    pub default_track: CaseTrack,
    pub max_wall_time: Option<Duration>,
    pub max_cost_usd: Option<f32>,
}
```

The `run()` method signature (`evolution_orchestrator.rs`, line 795) takes
`&EvolutionConfig` embedded in `self` and emits `Vec<StepOutcome>` — a plain
data sequence. No callbacks, no closures for business logic.

**Why**:
- Plans are `Serialize + Deserialize` — they are persistable, replayable, and
  diff-able across runs.
- Business logic lives in the plan structure (`Story` criteria, `StoryState`
  transitions), not scattered across closures passed at call sites.
- Enables durable execution (resume from a serialized plan), dry-run validation,
  and plan diffing between evolution generations.

**PR check**:
- [ ] New component accepts a `#[derive(Serialize, Deserialize)]` Plan struct
- [ ] Internal state transitions are modeled as events emitted outward, not
      as mutations via injected callbacks
- [ ] Plan validation is a pure function `(plan: &Plan) -> Result<(), ValidationError>`

**Applies to**: `EvolutionOrchestrator`, `PersistentLoop`, and in Sprint 4:
`WizardEngine` (takes a `WizardPlan`), any new mutation or integration pipeline.

---

## 4. Idiom: Facades From Owner Crate

**Rule**: Each Connector owns its `CapabilityFacade` declarations. The central
`BuiltinToolRegistry` imports and composes them; it does NOT declare facades
for other crates' capabilities directly.

**Example** — `default_facades()` in `BuiltinToolRegistry`
(`crates/cyberclaw-agent-runtime/src/builtin_tools.rs`, lines 239–413):

The function currently returns exactly **10 entries** (verified in test at line
438–443): `file_read`, `file_write`, `file_list`, `file_search`, `bash`,
`web_fetch`, `web_search`, `mcp_call`, `memory_read`, `memory_write`.

All 10 map to connector IDs `"local"` or `"internal"` — string references, not
direct Rust imports from `cyberclaw_connectors`. The `SandboxConnector` facade
is NOT in this list; it is registered separately by the host binary at startup.

```rust
// Correct pattern — connector referenced by string ID, no crate import:
make_facade(
    "bash",
    "Execute a shell command ...",
    serde_json::json!({ ... }),
    "local",           // connector_id as string
    "bash",            // capability_id as string
    RiskLevel::High,
),
```

**Why**:
- Prevents a `cyberclaw-agent-runtime` → `cyberclaw-connectors` dependency cycle.
- Each Connector crate is self-documenting: its facade list lives next to its
  implementation.
- Adding a new Connector requires one crate change (export `capability_facades()`),
  and one host-level registration call — not a change to `builtin_tools.rs`.

**Status (2026-05-06)**: ✅ **Idiom fully enforced**. After F8 (3 phases) + F12 (4 phases):

- `CapabilityFacade` + `ToolsetCategory` live in `cyberclaw-core::facade` (not `cyberclaw-agent-runtime`) — connectors can return real types, no mirror needed (commit `6dd9c26`).
- 6 + 7 = **13 connector modules** export `pub fn capability_facades() -> Vec<(CapabilityFacade, ToolsetCategory)>` (commits `ee2a343` + `29190a2`).
- `BuiltinToolRegistry::default_facades` shrank from 22 to **3** (only `chat_handler`-intercepted tools: `skill_create` / `skill_search` / `delegate_to_sub_agent`) (commit `29190a2`).
- Host binary aggregates connector facades at startup via `apps/cyberclaw-server/src/state.rs::AppState::new` (commits `e132005` + `29190a2`).
- `connector_drift.rs` is **production fail-loud** — drift > 0 in production aborts startup (commit `8fc9032`). Dev keeps warn for iteration.
- 4 levels of `FacadeExposure` route facades into LLM-default / LLM-advanced / Internal / AdminOnly tiers (commits `5770f31` + `ab18f6b`).

Pre-§4-enforcement reality (recovered 2026-05-06):
- 6 mirror types (`CmdFacadeDescriptor` / `FsFacadeSpec` / etc.) — **all deleted**
- `default_facades` had 22 hardcoded entries (>15 limit) — **shrunk to 3**
- 6 `capability_facades()` had 0 production callers — **all 13 now wired**
- `connector_drift.rs` only logged warn — **now fatal in production**

**PR check (CI-enforceable, post-2026-05-06)**:
- [ ] New Connector module exports `pub fn capability_facades() -> Vec<(CapabilityFacade, ToolsetCategory)>` returning real `cyberclaw_core::facade::CapabilityFacade`
- [ ] `BuiltinToolRegistry::default_facades` stays at or under **3** entries (skill_create / skill_search / delegate_to_sub_agent only — anything new with a connector owner goes through that connector's `capability_facades()`)
- [ ] Facade composition for new connectors happens in `cyberclaw-server` or
      `cyberclaw-control-plane/registry`, never inside `cyberclaw-agent-runtime`
- [ ] `FacadeExposure` set explicitly when the facade should not be `LlmDefault` (e.g. high-blast variants → `LlmAdvanced`; admin-only operations → `Internal`)
- [ ] `connector_drift::audit` returns 0 warnings on `cargo run -p cyberclaw-server` (production builds will refuse to start otherwise)

**Antipattern** (rejected at review):

```rust
// DO NOT: import connector crate directly to bloat default_facades
use cyberclaw_connectors::sandbox::SandboxConnector;

fn default_facades() -> Vec<(CapabilityFacade, ToolsetCategory)> {
    vec![
        // ... existing 10 entries ...
        // WRONG: direct cross-crate call that creates a dep cycle
        (cyberclaw_connectors::sandbox::facade(), ToolsetCategory::Terminal),
    ]
}
```

---

## 5. Idiom: No Covert Sixth Object

**Rule**: CyberClaw has exactly five first-class platform objects: `Agent`,
`Skill`, `Connector`, `Capability`, `Platform Plugin`. New code MUST NOT
introduce a sixth alongside these.

**Common smuggling attempts** (all rejected at review):

| Proposal | What it actually is | Correct placement |
|---|---|---|
| `WizardEngine` as a new object | Control-plane component like `EvolutionOrchestrator` | `crates/cyberclaw-control-plane/src/` |
| `Planner` as a new object | An `Agent` with a planning Skill | `ecosystem/agents/` |
| `Dispatcher` as a new object | A trait on top of `Connector→Capability` | Inline trait in the owning crate |
| `ecosystem/wizards/` directory | Would imply a sixth object class | Use `ecosystem/skills/` or `ecosystem/agents/` |
| `PluginKind::Wizard` variant | Breaks five-object taxonomy | Not valid; use `PlatformPlugin` |

The `CLAUDE.md` §2 definition is canonical: these five objects and no others.
Any new abstraction must map to one of: "control-plane component", "trait
implementation", or one of the five named objects.

**PR check**:
- [ ] No new top-level `ecosystem/` subdirectory added
- [ ] No new `PluginKind::` variant introduced
- [ ] Any new named abstraction is classified in the PR description as one of:
      control-plane component / trait / Agent / Skill / Connector / Capability /
      Platform Plugin

---

## 6. Idiom: Skill is Methodology, Not Executor

**Rule**: Skills (`SKILL.md` bundles under `ecosystem/skills/`) are methodology,
prompts, and reference material. They do NOT contain execution code that invokes
LLMs or spawns processes. All execution flows through `Connector→Capability`.

**Correct Skill bundle layout**:

```
ecosystem/skills/my-skill/
  SKILL.md            # YAML frontmatter + methodology body (required)
  references/         # Reference docs, papers, examples (optional)
  assets/             # Static assets: diagrams, templates (optional)
  scripts/            # Pure data/validation scripts only — see constraint below
```

A `scripts/` directory is permitted only for pure validation or data manipulation
(e.g., schema validation, fixture generation). Scripts must not shell out to LLM
APIs, call `claude`, `openai`, or any AI SDK, or spawn external processes that
perform the Skill's core work.

Execution references in `SKILL.md` must point to CyberClaw runtimes
(`PersistentLoop`, `AutopilotRuntime`, `SubAgentOrchestrator`), not to Claude
Code's `Task()` primitive or any external agent framework directly.

**Why**:
- A Skill that directly invokes an LLM bypasses governance, audit, and the
  `Connector→Capability` execution chain entirely.
- Methodology in `SKILL.md` is readable, diffable, and evolvable by the
  `EvolutionOrchestrator`; embedded execution code is not.
- Skills that are pure methodology can be safely shared, versioned, and
  composited by the platform without security review of executable payloads.

**PR check**:
- [ ] Skill bundle contains no `.py`/`.sh`/`.js` files that call LLM APIs or
      spawn agent processes
- [ ] If `scripts/` exists, each script is pure validation or data manipulation
- [ ] `SKILL.md` execution references point to CyberClaw runtimes, not to
      `Task(subagent_type=...)` or equivalent external primitives

---

## 7. Combined PR Checklist (Copy-Paste)

```markdown
## Idiom Compliance

- [ ] Injected trait dispatcher (§1): [link to `XxxDispatcher` trait def in PR]
- [ ] Event sink trait (§2): [link to `XxxEvent` enum + `XxxEventSink` trait]
- [ ] Declarative Plan-in + Events-out (§3): [link to Plan struct with `Serialize`]
- [ ] Facades from owner crate (§4): [link to `capability_facades()` export, or N/A]
- [ ] No new sixth object (§5): [confirmation — new abstraction classified as: ___]
- [ ] Skill is methodology only (§6): [confirmation or N/A if no Skill bundle added]

grep verification (run before submitting):
- [ ] `use cyberclaw_connectors::` in `cyberclaw-agent-runtime` test modules: **zero hits**
- [ ] `Task(subagent_type=` anywhere in `ecosystem/`: **zero hits**
- [ ] `println!` in new orchestrator/engine files: **zero hits** (use `tracing::info!`)
- [ ] New `ecosystem/` top-level subdirectory: **none added**
- [ ] `PluginKind::` new variant: **none added**
- [ ] `default_facades()` entry count ≤ 15: **current count: ___**
```

---

## 8. References

- Sprint 1 blueprint: `docs/implementation/2026-04-18-hyperagents-evolution-sprint.md`
- Sprint 4 kickoff: `docs/implementation/2026-04-18-sprint4-integration-kickoff.md`
- CyberClaw object model: `CLAUDE.md` §2–3
- Architecture overview: `docs/architecture/overview/ARCHITECTURE_V2.0.md`

Canonical idiom sources (read before modifying an idiom rule):

| Idiom | Canonical file | Key lines |
|---|---|---|
| Injected Trait Dispatcher | `crates/cyberclaw-control-plane/src/evolution_orchestrator.rs` | 53–78 (trait), 491–499 (usage) |
| Event Sink Trait | `crates/cyberclaw-control-plane/src/evolution_orchestrator.rs` | 271–376 |
| Declarative Plan-In + Events-Out | `crates/cyberclaw-control-plane/src/persistent_execution.rs` | 119–277, 525–560 |
| Facades From Owner Crate | `crates/cyberclaw-agent-runtime/src/builtin_tools.rs` | 239–413 |
| Sandbox Connector (reference impl) | `crates/cyberclaw-connectors/src/sandbox/mod.rs` | 1–81 |
