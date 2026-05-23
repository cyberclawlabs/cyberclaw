//! Mixture of Agents (MoA) — runs the same prompt through N proposer LLMs
//! in parallel, then routes all proposals through an aggregator model that
//! synthesizes a final answer.
//!
//! Mirrors Hermes v0.12 `tools/mixture_of_agents_tool.py`. Cuts down hallucination
//! and bias by averaging across heterogenous models (e.g. Claude + GPT-4 +
//! local Llama). The aggregator sees all proposals and the original question;
//! its prompt asks for a synthesis that acknowledges agreements + disagreements.
//!
//! # Why N=3 default
//!
//! - 1 proposer = no diversity benefit
//! - 2 proposers = aggregator forced to pick one without tie-break signal
//! - 3+ = aggregator can spot consensus vs outlier, weighted synthesis
//!
//! # Cost / latency
//!
//! Latency = max(proposer_latency) + aggregator_latency. Cost ≈ N+1 × single-call.
//! Use only for high-stakes decisions where the cost is justified.

use crate::client::LlmClient;
use crate::error::{LlmError, LlmResult};
use crate::types::{ChatRequest, ChatResponse, Message, Role};
use futures::future::join_all;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{info, warn};

/// Default aggregator system prompt. Asks the aggregator to synthesize a
/// final answer that acknowledges agreements + disagreements among proposers.
const DEFAULT_AGGREGATOR_SYSTEM: &str = "\
You are an expert aggregator. You will be given a user question followed by N \
candidate answers from different AI models (labeled 'Proposal 1', 'Proposal 2', etc.). \
Your job is to synthesize a single high-quality answer that:\n\
\n\
1. Identifies the points where proposals agree (likely correct).\n\
2. Identifies the points where they disagree (caller should evaluate).\n\
3. Produces a final answer that is at least as accurate as the best proposal,\n\
   while flagging any disagreement so the caller knows where to verify.\n\
\n\
Do not pick one proposal verbatim — synthesize. Do not invent claims that \
no proposal made. If proposals fundamentally disagree, say so explicitly.";

/// One proposer's contribution.
#[derive(Debug, Clone)]
pub struct Proposal {
    /// Model name (from `LlmClient::provider` + request.model).
    pub model: String,
    /// Proposer text answer (extracted from `choices[0].message.content`).
    pub content: String,
    /// True if this proposer succeeded; false if it timed out or errored.
    pub success: bool,
    /// Error message when `success == false`.
    pub error: Option<String>,
}

/// Final MoA result. The `aggregated` field is what callers usually want.
/// Individual proposals are kept for audit + debugging.
#[derive(Debug, Clone)]
pub struct MoAResult {
    /// Aggregator's synthesized final answer.
    pub aggregated: ChatResponse,
    /// Individual proposer responses (in input order).
    pub proposals: Vec<Proposal>,
    /// Aggregator model identifier.
    pub aggregator_model: String,
}

/// Configurable MoA orchestrator.
pub struct MixtureOfAgents {
    proposers: Vec<Arc<dyn LlmClient>>,
    aggregator: Arc<dyn LlmClient>,
    aggregator_model: String,
    proposer_timeout: Duration,
    aggregator_system_prompt: String,
}

impl MixtureOfAgents {
    /// Build with explicit proposer + aggregator clients. `aggregator_model`
    /// is the model name passed to the aggregator's `chat_completion`.
    pub fn new(
        proposers: Vec<Arc<dyn LlmClient>>,
        aggregator: Arc<dyn LlmClient>,
        aggregator_model: impl Into<String>,
    ) -> Self {
        Self {
            proposers,
            aggregator,
            aggregator_model: aggregator_model.into(),
            proposer_timeout: Duration::from_secs(60),
            aggregator_system_prompt: DEFAULT_AGGREGATOR_SYSTEM.to_string(),
        }
    }

    pub fn with_proposer_timeout(mut self, t: Duration) -> Self {
        self.proposer_timeout = t;
        self
    }

    pub fn with_aggregator_system(mut self, s: impl Into<String>) -> Self {
        self.aggregator_system_prompt = s.into();
        self
    }

    /// Run the original `request` through every proposer in parallel, collect
    /// results (failures included), then send them to the aggregator. The
    /// returned [`MoAResult`] has both the synthesized answer and individual
    /// proposals.
    pub async fn complete(&self, request: ChatRequest) -> LlmResult<MoAResult> {
        if self.proposers.is_empty() {
            return Err(LlmError::InvalidResponse(
                "MixtureOfAgents requires at least one proposer".to_string(),
            ));
        }
        // Fan out.
        let futures = self.proposers.iter().map(|client| {
            let req = request.clone();
            let timeout_dur = self.proposer_timeout;
            let provider = client.provider().to_string();
            let model = req.model.clone();
            let client = client.clone();
            async move {
                let r = timeout(timeout_dur, client.chat_completion(req)).await;
                match r {
                    Ok(Ok(resp)) => {
                        let content = resp
                            .choices
                            .first()
                            .map(|c| c.message.content.clone())
                            .unwrap_or_default();
                        Proposal {
                            model: format!("{}/{}", provider, model),
                            content,
                            success: true,
                            error: None,
                        }
                    }
                    Ok(Err(e)) => Proposal {
                        model: format!("{}/{}", provider, model),
                        content: String::new(),
                        success: false,
                        error: Some(e.to_string()),
                    },
                    Err(_) => Proposal {
                        model: format!("{}/{}", provider, model),
                        content: String::new(),
                        success: false,
                        error: Some(format!("timeout after {:?}", timeout_dur)),
                    },
                }
            }
        });
        let proposals: Vec<Proposal> = join_all(futures).await;

        let success_count = proposals.iter().filter(|p| p.success).count();
        info!(
            total = proposals.len(),
            success = success_count,
            "MoA proposers complete"
        );
        if success_count == 0 {
            return Err(LlmError::Internal(format!(
                "All {} proposers failed; cannot aggregate",
                proposals.len()
            )));
        }
        if success_count < proposals.len() {
            warn!(
                "MoA: {}/{} proposers failed; aggregating partial results",
                proposals.len() - success_count,
                proposals.len()
            );
        }

        // Build aggregator request.
        let original_user = extract_last_user_text(&request.messages).unwrap_or_default();
        let mut aggregator_user_body = format!("User question:\n{}\n\n", original_user);
        for (i, p) in proposals.iter().enumerate() {
            if p.success {
                aggregator_user_body.push_str(&format!(
                    "--- Proposal {} (from {}) ---\n{}\n\n",
                    i + 1,
                    p.model,
                    p.content
                ));
            } else {
                aggregator_user_body.push_str(&format!(
                    "--- Proposal {} (from {}, FAILED) ---\n[no answer: {}]\n\n",
                    i + 1,
                    p.model,
                    p.error.as_deref().unwrap_or("unknown")
                ));
            }
        }
        aggregator_user_body.push_str("Synthesize the final answer per the system instructions.");

        let agg_request = ChatRequest {
            model: self.aggregator_model.clone(),
            messages: vec![
                Message::system(&self.aggregator_system_prompt),
                Message::user(&aggregator_user_body),
            ],
            temperature: Some(0.3),
            ..Default::default()
        };
        let aggregated = self.aggregator.chat_completion(agg_request).await?;

        Ok(MoAResult {
            aggregated,
            proposals,
            aggregator_model: self.aggregator_model.clone(),
        })
    }
}

fn extract_last_user_text(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, Role::User))
        .map(|m| m.content.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Choice, Usage};
    use async_trait::async_trait;

    /// Mock LlmClient that returns a fixed response after an optional delay.
    struct MockClient {
        provider: &'static str,
        reply: String,
        delay: Duration,
        fail: bool,
    }
    #[async_trait]
    impl LlmClient for MockClient {
        async fn chat_completion(&self, request: ChatRequest) -> LlmResult<ChatResponse> {
            if self.delay > Duration::ZERO {
                tokio::time::sleep(self.delay).await;
            }
            if self.fail {
                return Err(LlmError::Internal("mock fail".to_string()));
            }
            Ok(ChatResponse {
                id: "id".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: request.model,
                choices: vec![Choice {
                    index: 0,
                    message: Message::assistant(&self.reply),
                    finish_reason: Some("stop".to_string()),
                }],
                usage: Some(Usage {
                    prompt_tokens: 10,
                    completion_tokens: 10,
                    total_tokens: 20,
                    ..Default::default()
                }),
                rate_limit: None,
            })
        }
        async fn chat_completion_stream(
            &self,
            _request: ChatRequest,
        ) -> LlmResult<
            Box<dyn futures::Stream<Item = LlmResult<crate::types::ChatChunk>> + Send + Unpin>,
        > {
            unimplemented!("not used in tests")
        }
        fn provider(&self) -> &str {
            self.provider
        }
        async fn validate_connection(&self) -> LlmResult<()> {
            Ok(())
        }
    }
    fn mock(provider: &'static str, reply: &str) -> Arc<dyn LlmClient> {
        Arc::new(MockClient {
            provider,
            reply: reply.to_string(),
            delay: Duration::ZERO,
            fail: false,
        })
    }

    #[tokio::test]
    async fn complete_aggregates_three_proposals() {
        let p1 = mock("claude", "The answer is 42.");
        let p2 = mock("gpt", "Forty-two.");
        let p3 = mock("local", "42 (most agree).");
        let agg = mock("aggregator", "Synthesized: 42, all proposals agree.");
        let moa = MixtureOfAgents::new(vec![p1, p2, p3], agg, "aggregator-model");
        let req = ChatRequest {
            model: "x".to_string(),
            messages: vec![Message::user("What is 6 * 7?")],
            ..Default::default()
        };
        let result = moa.complete(req).await.unwrap();
        assert_eq!(result.proposals.len(), 3);
        assert!(result.proposals.iter().all(|p| p.success));
        assert!(result
            .aggregated
            .choices
            .first()
            .map(|c| c.message.content.as_str())
            .unwrap()
            .contains("Synthesized"));
    }

    #[tokio::test]
    async fn proposer_timeout_marks_failed_but_continues() {
        let slow = Arc::new(MockClient {
            provider: "slow",
            reply: "late".to_string(),
            delay: Duration::from_secs(5),
            fail: false,
        });
        let fast = mock("fast", "quick");
        let agg = mock("agg", "synth");
        let moa = MixtureOfAgents::new(vec![slow, fast], agg, "m")
            .with_proposer_timeout(Duration::from_millis(200));
        let req = ChatRequest {
            model: "x".to_string(),
            messages: vec![Message::user("q")],
            ..Default::default()
        };
        let result = moa.complete(req).await.unwrap();
        assert_eq!(result.proposals.len(), 2);
        let failed = result.proposals.iter().find(|p| !p.success).unwrap();
        assert!(failed.error.as_ref().unwrap().contains("timeout"));
        let success = result.proposals.iter().find(|p| p.success).unwrap();
        assert_eq!(success.content, "quick");
    }

    #[tokio::test]
    async fn all_proposers_fail_returns_error() {
        let p1 = Arc::new(MockClient {
            provider: "p1",
            reply: "".to_string(),
            delay: Duration::ZERO,
            fail: true,
        });
        let p2 = Arc::new(MockClient {
            provider: "p2",
            reply: "".to_string(),
            delay: Duration::ZERO,
            fail: true,
        });
        let agg = mock("agg", "won't be called");
        let moa = MixtureOfAgents::new(vec![p1, p2], agg, "m");
        let req = ChatRequest {
            model: "x".to_string(),
            messages: vec![Message::user("q")],
            ..Default::default()
        };
        let err = moa.complete(req).await.unwrap_err();
        assert!(err.to_string().contains("All 2 proposers failed"));
    }

    #[tokio::test]
    async fn empty_proposers_errors() {
        let moa = MixtureOfAgents::new(vec![], mock("agg", ""), "m");
        let err = moa.complete(ChatRequest::default()).await.unwrap_err();
        assert!(err.to_string().contains("at least one proposer"));
    }
}
