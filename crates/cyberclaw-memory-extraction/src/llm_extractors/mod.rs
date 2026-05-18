//! LLM-based implementations of extraction traits.
//! Uses cyberclaw-llm's LlmClient for all LLM calls.

use cyberclaw_llm::{
    client::LlmClient,
    types::{ChatRequest, Message},
};

/// Maximum byte length of a stored summary (S18 NFR: ≤ 2 KB).
pub const MAX_SUMMARY_BYTES: usize = 2048;

/// A simplified conversation turn for summarization input.
#[derive(Debug, Clone)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
}

/// Summarize a conversation in 3-5 sentences using LLM extraction.
///
/// Falls back gracefully: callers should catch `Err` and use string-concat fallback.
/// Applies a 30-second timeout on the LLM call.
///
/// # Arguments
/// * `messages` - conversation turns (role + content)
/// * `llm` - LLM client to use for the extraction call
///
/// # Returns
/// A 3-5 sentence summary string (≤ 2 KB), or `anyhow::Error` on failure.
pub async fn summarize_conversation(
    messages: &[ConversationMessage],
    llm: &dyn LlmClient,
) -> anyhow::Result<String> {
    let conversation_text = messages
        .iter()
        .map(|m| format!("{}: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "Summarize the following conversation in 3-5 concise sentences. \
         Focus on: (1) key decisions made, (2) information revealed that \
         is important for future reference, (3) any open questions or \
         next steps. Be specific, avoid meta-commentary.\n\n\
         Conversation:\n{}",
        conversation_text
    );

    let request = ChatRequest {
        model: String::new(), // use LLM client default
        messages: vec![Message::user(prompt)],
        max_tokens: Some(300),
        ..Default::default()
    };

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        llm.chat_completion(request),
    )
    .await
    .map_err(|_| anyhow::anyhow!("LLM summary call timed out after 30s"))?
    .map_err(|e| anyhow::anyhow!("LLM summary call failed: {}", e))?;

    let summary = response
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .unwrap_or_default();

    let summary = summary.trim().to_string();
    if summary.is_empty() {
        anyhow::bail!("LLM returned empty summary");
    }

    // Truncate to 2KB (S18 NFR)
    if summary.len() > MAX_SUMMARY_BYTES {
        let truncated = summary
            .char_indices()
            .take_while(|(i, _)| *i < MAX_SUMMARY_BYTES - 15)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(MAX_SUMMARY_BYTES - 15);
        Ok(format!("{}... [truncated]", &summary[..truncated]))
    } else {
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use cyberclaw_llm::{
        error::{LlmError, LlmResult},
        types::{ChatResponse, Choice, Message as LlmMessage, Role},
    };
    use futures::stream::Stream;

    struct MockLlmClient {
        response: String,
    }

    #[async_trait]
    impl LlmClient for MockLlmClient {
        async fn chat_completion(&self, _req: ChatRequest) -> LlmResult<ChatResponse> {
            Ok(ChatResponse {
                id: "mock-id".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: "mock".to_string(),
                choices: vec![Choice {
                    index: 0,
                    message: LlmMessage {
                        role: Role::Assistant,
                        content: self.response.clone(),
                        tool_calls: None,
                        tool_call_id: None,
                        name: None,
                        cache_control: None,
                    },
                    finish_reason: Some("stop".to_string()),
                }],
                usage: None,
            })
        }

        async fn chat_completion_stream(
            &self,
            _req: ChatRequest,
        ) -> LlmResult<
            Box<dyn Stream<Item = LlmResult<cyberclaw_llm::types::ChatChunk>> + Send + Unpin>,
        > {
            unimplemented!("not needed for tests")
        }

        fn provider(&self) -> &str {
            "mock"
        }

        async fn validate_connection(&self) -> LlmResult<()> {
            Ok(())
        }
    }

    struct ErrorLlmClient;

    #[async_trait]
    impl LlmClient for ErrorLlmClient {
        async fn chat_completion(&self, _req: ChatRequest) -> LlmResult<ChatResponse> {
            Err(LlmError::Internal("mock error".to_string()))
        }

        async fn chat_completion_stream(
            &self,
            _req: ChatRequest,
        ) -> LlmResult<
            Box<dyn Stream<Item = LlmResult<cyberclaw_llm::types::ChatChunk>> + Send + Unpin>,
        > {
            unimplemented!()
        }

        fn provider(&self) -> &str {
            "mock-error"
        }

        async fn validate_connection(&self) -> LlmResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_summarize_conversation_returns_llm_response() {
        let llm = MockLlmClient {
            response: "TEST SUMMARY".to_string(),
        };
        let messages = vec![
            ConversationMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            },
            ConversationMessage {
                role: "assistant".to_string(),
                content: "Hi there".to_string(),
            },
        ];
        let result = summarize_conversation(&messages, &llm).await.unwrap();
        assert_eq!(result, "TEST SUMMARY");
    }

    #[tokio::test]
    async fn test_summarize_conversation_error_propagates() {
        let llm = ErrorLlmClient;
        let messages = vec![ConversationMessage {
            role: "user".to_string(),
            content: "test".to_string(),
        }];
        let result = summarize_conversation(&messages, &llm).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_summarize_truncates_long_response() {
        let long_response = "A".repeat(3000);
        let llm = MockLlmClient {
            response: long_response,
        };
        let messages = vec![ConversationMessage {
            role: "user".to_string(),
            content: "test".to_string(),
        }];
        let result = summarize_conversation(&messages, &llm).await.unwrap();
        assert!(result.len() <= MAX_SUMMARY_BYTES);
        assert!(result.ends_with("... [truncated]"));
    }
}
