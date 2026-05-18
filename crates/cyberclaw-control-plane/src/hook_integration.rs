//! # Hook Integration
//!
//! Integrates the existing [`HookDispatcher`] into the [`MiddlewarePipeline`] and provides
//! a [`PluginHookLoader`] that discovers hook declarations from Platform Plugin manifests.
//!
//! ## Key types
//!
//! - [`IntegratedHookMiddleware`] -- a [`Middleware`] implementation (priority 30) that
//!   dispatches `PreExecution`, `PostExecution`, and `OnError` hooks around the downstream
//!   middleware chain.
//! - [`HookDeclaration`] -- a parsed hook entry from a plugin manifest.
//! - [`HookPhase`] -- the execution-chain phase where a hook fires.
//! - [`HookFailurePolicy`] -- what to do when a hook handler fails.
//! - [`HookRegistry`] -- an indexed registry for fast lookup by phase.
//! - [`PluginHookLoader`] -- scans plugin directories for `manifest.yaml` and returns
//!   [`HookDeclaration`]s.

use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::hook_dispatcher::{DispatchResult, HookContext, HookDispatcher, HookPoint};
use crate::middleware_pipeline::{
    Middleware, MiddlewareError, MiddlewareNext, MiddlewareRequest, MiddlewareResponse,
};

// ─── HookPhase ──────────────────────────────────────────────────────────────

/// Execution-chain phase where a hook fires.
///
/// This mirrors [`HookPoint`] but is the external/manifest-facing enum used by
/// plugin authors.  Conversion to the internal [`HookPoint`] is provided via
/// `Into<HookPoint>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPhase {
    /// Before the core capability execution.
    PreExecution,
    /// After the core capability execution completes successfully.
    PostExecution,
    /// When an error occurs in the execution chain.
    OnError,
    /// When an approval/review decision is required.
    OnApproval,
}

impl From<&HookPhase> for HookPoint {
    fn from(phase: &HookPhase) -> Self {
        match phase {
            HookPhase::PreExecution => HookPoint::PreExecution,
            HookPhase::PostExecution => HookPoint::PostExecution,
            HookPhase::OnError => HookPoint::OnError,
            HookPhase::OnApproval => HookPoint::PreReview,
        }
    }
}

// ─── HookFailurePolicy ─────────────────────────────────────────────────────

/// Policy governing what happens when a hook handler fails.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HookFailurePolicy {
    /// Log the failure and continue execution.
    #[default]
    Continue,
    /// Abort the entire execution chain.
    Abort,
    /// Retry the hook up to `max_attempts` times before aborting.
    Retry {
        /// Maximum number of retry attempts.
        max_attempts: u32,
    },
}

/// Convert our public [`HookFailurePolicy`] into the dispatcher-internal
/// [`crate::hook_dispatcher::FailurePolicy`].
impl From<&HookFailurePolicy> for crate::hook_dispatcher::FailurePolicy {
    fn from(policy: &HookFailurePolicy) -> Self {
        match policy {
            HookFailurePolicy::Continue => crate::hook_dispatcher::FailurePolicy::Ignore,
            HookFailurePolicy::Abort => crate::hook_dispatcher::FailurePolicy::Abort,
            HookFailurePolicy::Retry { max_attempts } => {
                crate::hook_dispatcher::FailurePolicy::Retry {
                    max_attempts: *max_attempts,
                }
            }
        }
    }
}

// ─── HookDeclaration ────────────────────────────────────────────────────────

/// A hook declaration parsed from a Platform Plugin manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct HookDeclaration {
    /// Name of the plugin that owns this hook.
    pub plugin_name: String,
    /// The execution phase this hook targets.
    pub hook_phase: HookPhase,
    /// Path to the handler script or binary (relative to plugin root).
    pub handler_path: String,
    /// What to do when the hook handler fails.
    #[serde(default)]
    pub failure_policy: HookFailurePolicy,
    /// Priority within a phase (lower = earlier).  Defaults to 100.
    #[serde(default = "default_priority")]
    pub priority: u32,
    /// Absolute path to the plugin root directory (set by `PluginHookLoader`).
    /// `ScriptHookHandler` joins this with `handler_path` to locate the script.
    /// `None` for declarations created without going through the loader (tests).
    #[serde(default)]
    pub plugin_root: Option<PathBuf>,
}

fn default_priority() -> u32 {
    100
}

// ─── HookRegistry ───────────────────────────────────────────────────────────

/// Registry that indexes [`HookDeclaration`]s by phase and keeps them
/// sorted by priority within each phase.
#[derive(Debug, Clone, Default)]
pub struct HookRegistry {
    entries: HashMap<HookPhase, Vec<HookDeclaration>>,
}

impl HookRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a hook declaration.  The declaration is inserted into the
    /// phase bucket and the bucket is re-sorted by priority.
    pub fn register(&mut self, decl: HookDeclaration) {
        let bucket = self.entries.entry(decl.hook_phase.clone()).or_default();
        bucket.push(decl);
        bucket.sort_by_key(|d| d.priority);
    }

    /// Remove all hooks owned by the given plugin name.
    pub fn unregister(&mut self, plugin_name: &str) {
        for bucket in self.entries.values_mut() {
            bucket.retain(|d| d.plugin_name != plugin_name);
        }
    }

    /// List all hooks registered for a given phase, sorted by priority.
    pub fn hooks_for_phase(&self, phase: &HookPhase) -> &[HookDeclaration] {
        self.entries.get(phase).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Total number of registered hooks across all phases.
    pub fn len(&self) -> usize {
        self.entries.values().map(|v| v.len()).sum()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ─── PluginHookLoader ───────────────────────────────────────────────────────

/// Manifest YAML top-level structure (only the `hooks` field is extracted).
#[derive(Debug, Deserialize)]
struct PluginManifest {
    /// Plugin name.
    #[serde(default)]
    name: String,
    /// Hook declarations (optional; plugins without hooks are valid).
    #[serde(default)]
    hooks: Vec<ManifestHookEntry>,
}

/// A single hook entry inside a manifest's `hooks` array.
#[derive(Debug, Deserialize)]
struct ManifestHookEntry {
    phase: HookPhase,
    handler: String,
    #[serde(default)]
    failure_policy: HookFailurePolicy,
    #[serde(default = "default_priority")]
    priority: u32,
}

/// Scans a directory tree for Platform Plugin `manifest.yaml` files and
/// extracts [`HookDeclaration`]s from them.
#[derive(Debug, Clone)]
pub struct PluginHookLoader {
    /// Root directory to scan (e.g. `ecosystem/platform-plugins/`).
    root: PathBuf,
}

impl PluginHookLoader {
    /// Create a loader rooted at the given path.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Create a loader with the default ecosystem path.
    pub fn default_path() -> Self {
        Self::new("ecosystem/platform-plugins")
    }

    /// Scan the root directory for `manifest.yaml` files and return all
    /// declared hooks.
    ///
    /// Errors in individual manifests are logged and skipped; a partial
    /// result is always returned.
    pub fn load(&self) -> Vec<HookDeclaration> {
        let mut declarations = Vec::new();

        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(err) => {
                warn!(
                    path = %self.root.display(),
                    error = %err,
                    "PluginHookLoader: cannot read plugin root directory"
                );
                return declarations;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let manifest_path = path.join("manifest.yaml");
            if !manifest_path.exists() {
                debug!(
                    plugin_dir = %path.display(),
                    "PluginHookLoader: no manifest.yaml, skipping"
                );
                continue;
            }

            match self.parse_manifest(&manifest_path) {
                Ok(decls) => {
                    info!(
                        manifest = %manifest_path.display(),
                        hooks = decls.len(),
                        "PluginHookLoader: loaded hook declarations"
                    );
                    declarations.extend(decls);
                }
                Err(err) => {
                    error!(
                        manifest = %manifest_path.display(),
                        error = %err,
                        "PluginHookLoader: failed to parse manifest"
                    );
                }
            }
        }

        declarations
    }

    /// Parse a single manifest file and return hook declarations.
    fn parse_manifest(&self, path: &Path) -> anyhow::Result<Vec<HookDeclaration>> {
        let content = std::fs::read_to_string(path)?;
        let manifest: PluginManifest = serde_yaml::from_str(&content)?;

        let plugin_name = if manifest.name.is_empty() {
            // Derive name from the parent directory.
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string()
        } else {
            manifest.name
        };

        let plugin_root = path.parent().map(|p| p.to_path_buf());

        let decls = manifest
            .hooks
            .into_iter()
            .map(|h| HookDeclaration {
                plugin_name: plugin_name.clone(),
                hook_phase: h.phase,
                handler_path: h.handler,
                failure_policy: h.failure_policy,
                priority: h.priority,
                plugin_root: plugin_root.clone(),
            })
            .collect();

        Ok(decls)
    }
}

// ─── IntegratedHookMiddleware ───────────────────────────────────────────────

/// Middleware that integrates the [`HookDispatcher`] into the
/// [`MiddlewarePipeline`].
///
/// Execution flow:
/// 1. Dispatch `PreExecution` hooks.
/// 2. If hooks return `Continue`, call `next.run()`.
/// 3. On success, dispatch `PostExecution` hooks.
/// 4. On error, dispatch `OnError` hooks before propagating.
///
/// Hook failures are handled according to each hook's [`HookFailurePolicy`]
/// (translated to the dispatcher's internal [`FailurePolicy`]).
pub struct IntegratedHookMiddleware {
    dispatcher: Arc<HookDispatcher>,
}

impl IntegratedHookMiddleware {
    /// Create a new middleware wrapping the given dispatcher.
    pub fn new(dispatcher: Arc<HookDispatcher>) -> Self {
        Self { dispatcher }
    }

    /// Build a [`HookContext`] from a [`MiddlewareRequest`].
    fn build_context(request: &MiddlewareRequest) -> HookContext {
        let execution_id = request
            .metadata
            .get("execution_id")
            .cloned()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut ctx = HookContext::new(&execution_id, &request.capability_id, 0);
        ctx.metadata = request.metadata.clone();
        ctx
    }

    /// Translate a [`DispatchResult`] into a possible middleware error.
    fn dispatch_result_to_error(
        result: &DispatchResult,
        phase_label: &str,
    ) -> Option<MiddlewareError> {
        match result {
            DispatchResult::Continue | DispatchResult::Skip => None,
            DispatchResult::Abort(reason) => Some(MiddlewareError::Rejected {
                middleware: "hook".to_string(),
                reason: format!("{phase_label} hook aborted: {reason}"),
            }),
            DispatchResult::Retry => Some(MiddlewareError::Rejected {
                middleware: "hook".to_string(),
                reason: format!(
                    "{phase_label} hook requested retry (unsupported at middleware level)"
                ),
            }),
        }
    }
}

#[async_trait]
impl Middleware for IntegratedHookMiddleware {
    fn name(&self) -> &str {
        "hook"
    }

    fn priority(&self) -> u32 {
        30
    }

    async fn process(
        &self,
        request: MiddlewareRequest,
        next: &dyn MiddlewareNext,
    ) -> Result<MiddlewareResponse, MiddlewareError> {
        let ctx = Self::build_context(&request);

        // ── Phase 1: PreExecution hooks ─────────────────────────────────
        let pre_result = self
            .dispatcher
            .dispatch(&HookPoint::PreExecution, &ctx)
            .await;

        debug!(
            execution_id = %ctx.execution_id,
            capability_id = %ctx.capability_id,
            result = ?pre_result,
            "IntegratedHookMiddleware: PreExecution dispatch complete"
        );

        if let Some(err) = Self::dispatch_result_to_error(&pre_result, "PreExecution") {
            // Dispatch OnError hooks before returning the error.
            let error_ctx = ctx.clone().with_error(err.to_string());
            let _ = self
                .dispatcher
                .dispatch(&HookPoint::OnError, &error_ctx)
                .await;
            return Err(err);
        }

        // ── Phase 2: Execute downstream ─────────────────────────────────
        let downstream_result = next.run(request).await;

        match downstream_result {
            Ok(response) => {
                // ── Phase 3: PostExecution hooks ────────────────────────
                let post_result = self
                    .dispatcher
                    .dispatch(&HookPoint::PostExecution, &ctx)
                    .await;

                debug!(
                    execution_id = %ctx.execution_id,
                    capability_id = %ctx.capability_id,
                    result = ?post_result,
                    "IntegratedHookMiddleware: PostExecution dispatch complete"
                );

                if let Some(err) = Self::dispatch_result_to_error(&post_result, "PostExecution") {
                    return Err(err);
                }

                Ok(response)
            }
            Err(downstream_err) => {
                // ── Phase 4: OnError hooks ─────────────────────────────
                let error_ctx = ctx.clone().with_error(downstream_err.to_string());
                let on_error_result = self
                    .dispatcher
                    .dispatch(&HookPoint::OnError, &error_ctx)
                    .await;

                debug!(
                    execution_id = %ctx.execution_id,
                    capability_id = %ctx.capability_id,
                    result = ?on_error_result,
                    "IntegratedHookMiddleware: OnError dispatch complete"
                );

                // OnError hooks cannot suppress the original error, but an
                // Abort result replaces the original error message.
                if let DispatchResult::Abort(reason) = on_error_result {
                    return Err(MiddlewareError::Rejected {
                        middleware: "hook".to_string(),
                        reason: format!("OnError hook aborted: {reason}"),
                    });
                }

                Err(downstream_err)
            }
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook_dispatcher::{HookHandler, HookResult};
    use crate::middleware_pipeline::MiddlewarePipeline;
    use cyberclaw_core::identity::{ActorRef, ActorType};
    use cyberclaw_core::ids::ActorId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── Helpers ─────────────────────────────────────────────────────────

    fn test_request() -> MiddlewareRequest {
        MiddlewareRequest {
            capability_id: "cap-test".into(),
            connector_id: "conn-test".into(),
            input: serde_json::json!({"test": true}),
            actor: ActorRef {
                id: ActorId::new(),
                actor_type: ActorType::System,
                tenant_id: None,
                home_node_id: None,
                display_name: "test-actor".to_string(),
            },
            metadata: HashMap::new(),
            risk_level: None,
            effects: vec![],
        }
    }

    /// Handler that always continues.
    struct ContinueHandler;

    #[async_trait]
    impl HookHandler for ContinueHandler {
        async fn handle(&self, _point: &HookPoint, _ctx: &HookContext) -> HookResult {
            HookResult::Continue
        }
    }

    /// Handler that always aborts with a reason.
    struct AbortHandler(String);

    #[async_trait]
    impl HookHandler for AbortHandler {
        async fn handle(&self, _point: &HookPoint, _ctx: &HookContext) -> HookResult {
            HookResult::Abort(self.0.clone())
        }
    }

    /// Handler that counts invocations.
    struct CountHandler(Arc<AtomicUsize>);

    #[async_trait]
    impl HookHandler for CountHandler {
        async fn handle(&self, _point: &HookPoint, _ctx: &HookContext) -> HookResult {
            self.0.fetch_add(1, Ordering::SeqCst);
            HookResult::Continue
        }
    }

    /// A middleware that always fails, placed *after* the hook middleware.
    struct FailingMiddleware;

    #[async_trait]
    impl Middleware for FailingMiddleware {
        fn name(&self) -> &str {
            "failing"
        }
        fn priority(&self) -> u32 {
            40
        }
        async fn process(
            &self,
            _request: MiddlewareRequest,
            _next: &dyn MiddlewareNext,
        ) -> Result<MiddlewareResponse, MiddlewareError> {
            Err(MiddlewareError::Internal {
                middleware: "failing".into(),
                source: anyhow::anyhow!("deliberate failure"),
            })
        }
    }

    // ── HookRegistry tests ──────────────────────────────────────────────

    #[test]
    fn registry_register_and_lookup() {
        let mut registry = HookRegistry::new();
        assert!(registry.is_empty());

        registry.register(HookDeclaration {
            plugin_name: "plugin-a".into(),
            hook_phase: HookPhase::PreExecution,
            handler_path: "scripts/pre.sh".into(),
            failure_policy: HookFailurePolicy::Continue,
            priority: 10,
            plugin_root: None,
        });
        registry.register(HookDeclaration {
            plugin_name: "plugin-b".into(),
            hook_phase: HookPhase::PreExecution,
            handler_path: "scripts/pre2.sh".into(),
            failure_policy: HookFailurePolicy::Abort,
            priority: 5,
            plugin_root: None,
        });
        registry.register(HookDeclaration {
            plugin_name: "plugin-a".into(),
            hook_phase: HookPhase::PostExecution,
            handler_path: "scripts/post.sh".into(),
            failure_policy: HookFailurePolicy::Continue,
            priority: 20,
            plugin_root: None,
        });

        assert_eq!(registry.len(), 3);

        let pre_hooks = registry.hooks_for_phase(&HookPhase::PreExecution);
        assert_eq!(pre_hooks.len(), 2);
        // Should be sorted by priority: plugin-b (5) before plugin-a (10).
        assert_eq!(pre_hooks[0].plugin_name, "plugin-b");
        assert_eq!(pre_hooks[1].plugin_name, "plugin-a");

        let post_hooks = registry.hooks_for_phase(&HookPhase::PostExecution);
        assert_eq!(post_hooks.len(), 1);

        let error_hooks = registry.hooks_for_phase(&HookPhase::OnError);
        assert_eq!(error_hooks.len(), 0);
    }

    #[test]
    fn registry_unregister_removes_by_plugin() {
        let mut registry = HookRegistry::new();
        registry.register(HookDeclaration {
            plugin_name: "plugin-a".into(),
            hook_phase: HookPhase::PreExecution,
            handler_path: "a.sh".into(),
            failure_policy: HookFailurePolicy::Continue,
            priority: 10,
            plugin_root: None,
        });
        registry.register(HookDeclaration {
            plugin_name: "plugin-b".into(),
            hook_phase: HookPhase::PreExecution,
            handler_path: "b.sh".into(),
            failure_policy: HookFailurePolicy::Continue,
            priority: 20,
            plugin_root: None,
        });

        assert_eq!(registry.len(), 2);
        registry.unregister("plugin-a");
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.hooks_for_phase(&HookPhase::PreExecution)[0].plugin_name,
            "plugin-b"
        );
    }

    // ── IntegratedHookMiddleware tests ───────────────────────────────────

    #[tokio::test]
    async fn pre_and_post_hooks_dispatched_on_success() {
        let dispatcher = Arc::new(HookDispatcher::new());
        let pre_count = Arc::new(AtomicUsize::new(0));
        let post_count = Arc::new(AtomicUsize::new(0));

        dispatcher
            .register(
                HookPoint::PreExecution,
                Arc::new(CountHandler(pre_count.clone())),
                crate::hook_dispatcher::FailurePolicy::Ignore,
            )
            .await;
        dispatcher
            .register(
                HookPoint::PostExecution,
                Arc::new(CountHandler(post_count.clone())),
                crate::hook_dispatcher::FailurePolicy::Ignore,
            )
            .await;

        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(IntegratedHookMiddleware::new(dispatcher)));

        let resp = pipeline.execute(test_request()).await;
        assert!(resp.is_ok());
        assert_eq!(
            pre_count.load(Ordering::SeqCst),
            1,
            "PreExecution hook should fire"
        );
        assert_eq!(
            post_count.load(Ordering::SeqCst),
            1,
            "PostExecution hook should fire"
        );
    }

    #[tokio::test]
    async fn pre_hook_abort_prevents_execution() {
        let dispatcher = Arc::new(HookDispatcher::new());
        dispatcher
            .register(
                HookPoint::PreExecution,
                Arc::new(AbortHandler("blocked".into())),
                crate::hook_dispatcher::FailurePolicy::Abort,
            )
            .await;

        let post_count = Arc::new(AtomicUsize::new(0));
        dispatcher
            .register(
                HookPoint::PostExecution,
                Arc::new(CountHandler(post_count.clone())),
                crate::hook_dispatcher::FailurePolicy::Ignore,
            )
            .await;

        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(IntegratedHookMiddleware::new(dispatcher)));

        let result = pipeline.execute(test_request()).await;
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("blocked"),
            "Error should contain abort reason"
        );

        assert_eq!(
            post_count.load(Ordering::SeqCst),
            0,
            "PostExecution hook must not fire after PreExecution abort"
        );
    }

    #[tokio::test]
    async fn on_error_hooks_fire_on_downstream_failure() {
        let dispatcher = Arc::new(HookDispatcher::new());
        let error_count = Arc::new(AtomicUsize::new(0));

        dispatcher
            .register(
                HookPoint::PreExecution,
                Arc::new(ContinueHandler),
                crate::hook_dispatcher::FailurePolicy::Ignore,
            )
            .await;
        dispatcher
            .register(
                HookPoint::OnError,
                Arc::new(CountHandler(error_count.clone())),
                crate::hook_dispatcher::FailurePolicy::Ignore,
            )
            .await;

        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(IntegratedHookMiddleware::new(dispatcher)));
        pipeline.add(Arc::new(FailingMiddleware));

        let result = pipeline.execute(test_request()).await;
        assert!(result.is_err());
        assert_eq!(
            error_count.load(Ordering::SeqCst),
            1,
            "OnError hook should fire on downstream error"
        );
    }

    #[tokio::test]
    async fn on_error_hook_abort_replaces_original_error() {
        let dispatcher = Arc::new(HookDispatcher::new());
        dispatcher
            .register(
                HookPoint::OnError,
                Arc::new(AbortHandler("error-hook-override".into())),
                crate::hook_dispatcher::FailurePolicy::Abort,
            )
            .await;

        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(IntegratedHookMiddleware::new(dispatcher)));
        pipeline.add(Arc::new(FailingMiddleware));

        let result = pipeline.execute(test_request()).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("error-hook-override"),
            "OnError abort reason should replace original error, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn no_hooks_passes_through() {
        let dispatcher = Arc::new(HookDispatcher::new());
        let mut pipeline = MiddlewarePipeline::new();
        pipeline.add(Arc::new(IntegratedHookMiddleware::new(dispatcher)));

        let req = test_request();
        let expected = req.input.clone();
        let resp = pipeline.execute(req).await.unwrap();
        assert_eq!(resp.output, expected);
    }

    #[tokio::test]
    async fn middleware_has_correct_name_and_priority() {
        let dispatcher = Arc::new(HookDispatcher::new());
        let mw = IntegratedHookMiddleware::new(dispatcher);
        assert_eq!(mw.name(), "hook");
        assert_eq!(mw.priority(), 30);
    }

    // ── HookFailurePolicy tests ─────────────────────────────────────────

    #[test]
    fn failure_policy_default_is_continue() {
        let policy = HookFailurePolicy::default();
        assert_eq!(policy, HookFailurePolicy::Continue);
    }

    #[test]
    fn failure_policy_conversions() {
        use crate::hook_dispatcher::FailurePolicy;

        let cont: FailurePolicy = (&HookFailurePolicy::Continue).into();
        assert!(matches!(cont, FailurePolicy::Ignore));

        let abort: FailurePolicy = (&HookFailurePolicy::Abort).into();
        assert!(matches!(abort, FailurePolicy::Abort));

        let retry: FailurePolicy = (&HookFailurePolicy::Retry { max_attempts: 3 }).into();
        assert!(matches!(retry, FailurePolicy::Retry { max_attempts: 3 }));
    }

    // ── PluginHookLoader tests ──────────────────────────────────────────

    #[test]
    fn loader_returns_empty_for_nonexistent_dir() {
        let loader = PluginHookLoader::new("/nonexistent/path/to/plugins");
        let decls = loader.load();
        assert!(decls.is_empty());
    }

    #[test]
    fn loader_parses_manifest_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("my-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();

        let manifest = r#"
name: my-plugin
hooks:
  - phase: pre_execution
    handler: scripts/pre.sh
    failure_policy:
      type: continue
    priority: 10
  - phase: post_execution
    handler: scripts/post.sh
    failure_policy:
      type: retry
      max_attempts: 3
    priority: 20
  - phase: on_error
    handler: scripts/error.sh
    priority: 50
"#;
        std::fs::write(plugin_dir.join("manifest.yaml"), manifest).unwrap();

        let loader = PluginHookLoader::new(dir.path());
        let decls = loader.load();
        assert_eq!(decls.len(), 3);

        assert_eq!(decls[0].plugin_name, "my-plugin");
        assert_eq!(decls[0].hook_phase, HookPhase::PreExecution);
        assert_eq!(decls[0].handler_path, "scripts/pre.sh");
        assert_eq!(decls[0].failure_policy, HookFailurePolicy::Continue);
        assert_eq!(decls[0].priority, 10);

        assert_eq!(decls[1].hook_phase, HookPhase::PostExecution);
        assert_eq!(
            decls[1].failure_policy,
            HookFailurePolicy::Retry { max_attempts: 3 }
        );

        assert_eq!(decls[2].hook_phase, HookPhase::OnError);
        // Default failure policy
        assert_eq!(decls[2].failure_policy, HookFailurePolicy::Continue);
    }

    #[test]
    fn loader_derives_plugin_name_from_directory() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("derived-name");
        std::fs::create_dir_all(&plugin_dir).unwrap();

        let manifest = r#"
hooks:
  - phase: pre_execution
    handler: run.sh
"#;
        std::fs::write(plugin_dir.join("manifest.yaml"), manifest).unwrap();

        let loader = PluginHookLoader::new(dir.path());
        let decls = loader.load();
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].plugin_name, "derived-name");
    }

    #[test]
    fn loader_skips_invalid_manifest() {
        let dir = tempfile::tempdir().unwrap();

        // Valid plugin
        let good = dir.path().join("good-plugin");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(
            good.join("manifest.yaml"),
            "name: good\nhooks:\n  - phase: pre_execution\n    handler: run.sh\n",
        )
        .unwrap();

        // Invalid plugin (bad YAML)
        let bad = dir.path().join("bad-plugin");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("manifest.yaml"), "{{{{invalid yaml").unwrap();

        let loader = PluginHookLoader::new(dir.path());
        let decls = loader.load();
        // Only the good plugin's hook should be loaded.
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].plugin_name, "good");
    }

    // ── HookPhase serde tests ───────────────────────────────────────────

    #[test]
    fn hook_phase_deserializes_from_snake_case() {
        let phases: Vec<HookPhase> =
            serde_yaml::from_str("[pre_execution, post_execution, on_error, on_approval]").unwrap();
        assert_eq!(
            phases,
            vec![
                HookPhase::PreExecution,
                HookPhase::PostExecution,
                HookPhase::OnError,
                HookPhase::OnApproval,
            ]
        );
    }

    #[test]
    fn hook_phase_converts_to_hook_point() {
        assert_eq!(
            HookPoint::from(&HookPhase::PreExecution),
            HookPoint::PreExecution
        );
        assert_eq!(
            HookPoint::from(&HookPhase::PostExecution),
            HookPoint::PostExecution
        );
        assert_eq!(HookPoint::from(&HookPhase::OnError), HookPoint::OnError);
        assert_eq!(
            HookPoint::from(&HookPhase::OnApproval),
            HookPoint::PreReview
        );
    }
}
