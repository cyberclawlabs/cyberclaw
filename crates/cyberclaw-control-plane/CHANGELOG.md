# Changelog

- Status: Active
- Scope: Crate
- Owner: CyberClaw Control Plane Maintainers
- Last Updated: 2026-04-14

All notable changes to the `cyberclaw-control-plane` crate are documented in this file.

For repository-level changes, see [../../CHANGELOG.md](../../CHANGELOG.md).
For stage reports and reviews, see [../../docs/implementation/README.md](../../docs/implementation/README.md).

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this crate follows the repository versioning policy.

## [Unreleased]

### Added

#### Behavioral Engineering & Code Review Fixes (2026-04-15)

- `ExecutionMode` enum unified: removed duplicate in `execution_autopilot_types.rs`, re-export from `cyberclaw_core::execution::ExecutionMode` (Normal/Autopilot/Persistent)
- `submit()` now maps `request.execution_mode` to `Execution.execution_mode` (was hardcoded `Normal`)
- `#[cfg(test)]`/`#[cfg(not(test))]` testability gap documented with future migration path

### Fixed

#### CRITICAL: Autopilot detection via submit() (2026-04-15)

- `execution_service.rs:815`: `submit()` was hardcoding `ExecutionMode::Normal`, making autopilot detection dead code on the primary submit path. Now uses `request.execution_mode.unwrap_or_default()`
- `leak_detector.rs`: Performance threshold now uses `cfg!(debug_assertions)` — 300ms for debug, 100ms for release (was blanket 500ms)

#### Ralph Closed-Loop Engine (2026-04-14)

- `persistent_execution` module: `ExecutionPlan`, `Story`, `AcceptanceCriterion`, `PersistentLoop`, `ProgressJournal`, `VerificationVerdict` — story-driven persistent execution with dependency-aware scheduling, cross-iteration learning, retry/stuck detection (22 tests)
- `prd_generator` module: `StoryDraft`, `RuleBasedPrdGenerator`, `RefinementReport` — goal-to-stories decomposition with auto quality checks, Kahn's algorithm cycle detection (12 tests)
- `verification_gate` module: `CriteriaBasedGate`, `ReviewerTier`, `RegressionSpec` — evidence-based completion validation with auto tier selection (15 tests)

#### Previous (2026-04-11)

- `AutoModeGate` trait + `DefaultAutoModeGate` for Autopilot permission snapshot/restore (`auto_mode_gate.rs`) (2026-04-11)
- `CircuitBreaker` state machine (Closed→Open→HalfOpen) for consecutive failure detection (`circuit_breaker.rs`) (2026-04-11)
- `AgentTrustLevel`-based risk adjustment in `calculate_execution_risk_level` (2026-04-11)
- `duration_ms` field in `ExecutionResult` threaded through autopilot pipeline (2026-04-11)
- `strategy_variant` counter for `StuckResolution::ChangeStrategy` action reordering (2026-04-11)

### Fixed

- `GovernedLoopRuntime` integrated with `AutoModeGate` enter/exit and `CircuitBreaker` per-iteration check (2026-04-11)
- 4 Medium TODO items: state loading, analyze step, finalize_run cleanup, skill extraction (2026-04-11)
- `capability_id` in `record_step_results` populated with execution_id (2026-04-11)

### Changed

- **SharedStateStore trait migrated to async** - Converted `SharedStateStore` trait from synchronous methods with `block_on()` to native async trait using `#[async_trait]`, eliminating "Cannot start a runtime from within a runtime" errors (2026-03-28)
- Aligned crate-local documentation with the repository documentation system
- Simplified crate README to actual module boundaries and current validation commands
- Reduced crate changelog to crate-scoped changes instead of repository-wide status snapshots

### Fixed

- Fixed 28 test failures caused by nested tokio runtime errors in `SharedStateStore` implementation (2026-03-28)
- Fixed 40+ async method call sites across src and test files to properly await async trait methods (2026-03-28)

## [0.1.0-alpha]

### Added

- Manifest loading, ecosystem scanning, in-memory registry, and resolver foundations
- Control plane managers for task, case, review, subagent scheduling, automation, and orchestration
- Execution service and multi-node control-plane primitives including membership, placement, lease, event, artifact, and shared state components
- Integration and stress test suites for control-plane behavior

### Fixed

- Multiple security and reliability issues across loader, registry, resolver, execution, and concurrency-sensitive paths

### Notes

- Historical implementation details, milestone reports, and security-fix narratives are maintained in repository-level reports and fix records rather than repeated here
