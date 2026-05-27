//! EmergenceKit — single factory for the chat-time emergence stack.
//!
//! Bundles the three "涌现支柱" components every cb chat handler should
//! get for free:
//!
//! 1. **MemoryIntegration** — load/inject working memory each turn.
//! 2. **SessionSearchInjector** — optional passive cross-session recall
//!    (default off; IRON LAW 8 drives LLM-pulled recall instead).
//! 3. **ChatVerifier** — cheap per-turn heuristic verification.
//!
//! Before this module each handler wired these one at a time, producing
//! fragmentation: `chat_handler.rs` had memory but no verifier,
//! `chat.rs` had neither, etc. Now any handler doing
//! `EmergenceKit::for_chat(...)` gets the full triple, and adding the
//! 7th pillar in the future means changing this factory in one place.
//!
//! See `docs/research/agent-emergence-design-2026-05-27.md` for the
//! design rationale.

use std::sync::Arc;

use cyberclaw_core::ids::SessionId;
use cyberclaw_core::memory::WorkingMemory;

use crate::chat_verification_gate::{
    ChatTurnContext, ChatVerifier, ChatVerifyVerdict, HeuristicChatVerifier,
};
use crate::memory_integration::MemoryIntegration;
use crate::session_search_injector::SessionSearchInjector;

// ---------------------------------------------------------------------------
// Factory profile
// ---------------------------------------------------------------------------

/// Which chat path is constructing the kit. Lets the factory pick
/// sensible defaults without callers re-declaring them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmergenceProfile {
    /// Normal chat path (covers `/v1/agent/chat/completions`,
    /// `/v2/agent/chat`, `chat_conversations`, and `agents.rs`).
    /// Full triple wired; passive session_search off by default.
    Chat,
    /// `/v1/chat/completions` OpenAI-compat path — wired identically to
    /// `Chat` today, exists for future divergence (e.g. if we want to
    /// disable verifier here for strict OpenAI parity).
    OpenAiCompat,
    /// Autopilot / persistent loop — memory + session_search wired, but
    /// verifier is left to the control-plane `EvidenceBasedVerificationGate`.
    /// Calling `verify_turn` on this profile is a no-op (always Pass).
    Autopilot,
}

// ---------------------------------------------------------------------------
// EmergenceKit
// ---------------------------------------------------------------------------

/// Bundle of emergence components for a single chat session.
///
/// Cheap to clone: all heavyweight pieces live behind `Arc`.
#[derive(Clone)]
pub struct EmergenceKit {
    profile: EmergenceProfile,
    memory: Option<Arc<std::sync::Mutex<MemoryIntegration>>>,
    session_search: SessionSearchInjector,
    verifier: Option<Arc<dyn ChatVerifier>>,
}

impl EmergenceKit {
    /// Build the standard kit for a chat-style endpoint. Wires
    /// `MemoryIntegration` with default debounce and the heuristic
    /// verifier; session_search injector starts disabled so callers can
    /// opt in via `with_session_search_provider`.
    pub fn for_chat(
        profile: EmergenceProfile,
        working_memory: Arc<dyn WorkingMemory>,
        session_id: SessionId,
    ) -> Self {
        let memory = match profile {
            EmergenceProfile::Chat
            | EmergenceProfile::OpenAiCompat
            | EmergenceProfile::Autopilot => Some(Arc::new(std::sync::Mutex::new(
                MemoryIntegration::with_defaults(working_memory, session_id),
            ))),
        };
        let verifier: Option<Arc<dyn ChatVerifier>> = match profile {
            EmergenceProfile::Chat | EmergenceProfile::OpenAiCompat => {
                Some(Arc::new(HeuristicChatVerifier))
            }
            // Autopilot: verifier is centrally enforced by the
            // EvidenceBasedVerificationGate in control-plane. Returning
            // None here keeps responsibility singular.
            EmergenceProfile::Autopilot => None,
        };
        Self {
            profile,
            memory,
            session_search: SessionSearchInjector::disabled(),
            verifier,
        }
    }

    /// Replace the session-search injector with one that has a provider
    /// attached. Callers wire this when they want passive top-k
    /// pre-injection (operator opt-in).
    pub fn with_session_search(mut self, injector: SessionSearchInjector) -> Self {
        self.session_search = injector;
        self
    }

    /// Replace the per-turn verifier. Use sparingly — the default
    /// `HeuristicChatVerifier` is calibrated to catch v1.5–v1.7 known
    /// failure modes without LLM calls.
    pub fn with_verifier(mut self, verifier: Arc<dyn ChatVerifier>) -> Self {
        self.verifier = Some(verifier);
        self
    }

    pub fn profile(&self) -> EmergenceProfile {
        self.profile
    }

    pub fn session_search_enabled(&self) -> bool {
        self.session_search.is_enabled()
    }

    pub fn has_verifier(&self) -> bool {
        self.verifier.is_some()
    }

    // -----------------------------------------------------------------
    // Integration points the handlers call
    // -----------------------------------------------------------------

    /// Inject all available emergence context onto a system prompt
    /// in-place. Currently this is just the optional
    /// `SessionSearchInjector`; `MemoryIntegration` is intentionally
    /// kept handler-driven because it owns its own load/flush lifecycle.
    pub fn inject_system_prompt(
        &self,
        system_prompt: &mut String,
        agent_id: Option<&str>,
        current_session: Option<&SessionId>,
        query: &str,
    ) {
        self.session_search
            .inject_context(system_prompt, agent_id, current_session, query);
    }

    /// Verify a single chat turn. Returns `Pass` when no verifier is
    /// configured (autopilot profile).
    pub fn verify_turn(&self, ctx: &ChatTurnContext<'_>) -> ChatVerifyVerdict {
        match &self.verifier {
            Some(v) => v.verify(ctx),
            None => ChatVerifyVerdict::Pass,
        }
    }

    /// Access the memory integration for handler-side load/flush. Returns
    /// `None` only if the kit was constructed without memory (currently
    /// no profile does that, but the field is `Option` for future
    /// flexibility).
    pub fn memory(&self) -> Option<Arc<std::sync::Mutex<MemoryIntegration>>> {
        self.memory.clone()
    }
}

// ---------------------------------------------------------------------------
// Stateless observability helper — handlers call this to fire the verifier
// without owning a kit. Zero-allocation in the Pass path.
// ---------------------------------------------------------------------------

/// One-shot per-turn verification + structured log emission. Handlers add
/// a single call after producing the assistant response; this does NOT
/// mutate or block the response (observability only, no overwrite).
///
/// `span_label` should identify the handler (e.g. `"chat_handler"` /
/// `"agent_chat_completions_v2"`) so log lines are filterable.
pub fn observe_chat_turn(profile: EmergenceProfile, ctx: &ChatTurnContext<'_>, span_label: &str) {
    let verifier: Option<HeuristicChatVerifier> = match profile {
        EmergenceProfile::Chat | EmergenceProfile::OpenAiCompat => Some(HeuristicChatVerifier),
        EmergenceProfile::Autopilot => None,
    };
    let Some(v) = verifier else {
        return;
    };
    match v.verify(ctx) {
        ChatVerifyVerdict::Pass => {}
        ChatVerifyVerdict::Warn { reasons } => {
            tracing::warn!(
                target: "cyberclaw_agent_runtime::emergence_kit",
                handler = span_label,
                reasons = ?reasons,
                "emergence verifier WARN"
            );
        }
        ChatVerifyVerdict::Fail { reasons } => {
            tracing::error!(
                target: "cyberclaw_agent_runtime::emergence_kit",
                handler = span_label,
                reasons = ?reasons,
                "emergence verifier FAIL (response not overwritten — observability only in v1)"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::memory::{InMemoryWorkingMemory, WorkingMemoryConfig};

    fn wm() -> Arc<dyn WorkingMemory> {
        Arc::new(InMemoryWorkingMemory::new(WorkingMemoryConfig::default()))
    }

    #[test]
    fn for_chat_wires_memory_and_verifier() {
        let kit = EmergenceKit::for_chat(
            EmergenceProfile::Chat,
            wm(),
            SessionId::new(),
        );
        assert!(kit.memory().is_some());
        assert!(kit.has_verifier());
        assert!(!kit.session_search_enabled());
        assert_eq!(kit.profile(), EmergenceProfile::Chat);
    }

    #[test]
    fn autopilot_profile_omits_verifier() {
        let kit = EmergenceKit::for_chat(
            EmergenceProfile::Autopilot,
            wm(),
            SessionId::new(),
        );
        assert!(kit.memory().is_some());
        assert!(!kit.has_verifier());
        // verify_turn on no-verifier kit is a pass-through
        let ctx = ChatTurnContext {
            assistant_text: "anything",
            tool_intent_pending: false,
            user_prompt: "hi",
            iteration: Some(1),
        };
        assert_eq!(kit.verify_turn(&ctx), ChatVerifyVerdict::Pass);
    }

    #[test]
    fn verify_turn_catches_fabrication_in_chat_profile() {
        let kit = EmergenceKit::for_chat(
            EmergenceProfile::Chat,
            wm(),
            SessionId::new(),
        );
        let ctx = ChatTurnContext {
            assistant_text: "Result: \"tool_result\": {\"hits\":[]}",
            tool_intent_pending: true,
            user_prompt: "search",
            iteration: Some(2),
        };
        assert!(kit.verify_turn(&ctx).is_fail());
    }

    #[test]
    fn inject_system_prompt_is_noop_when_session_search_disabled() {
        let kit = EmergenceKit::for_chat(
            EmergenceProfile::Chat,
            wm(),
            SessionId::new(),
        );
        let mut prompt = String::from("system prompt body");
        kit.inject_system_prompt(&mut prompt, None, None, "anything");
        assert_eq!(prompt, "system prompt body");
    }
}
