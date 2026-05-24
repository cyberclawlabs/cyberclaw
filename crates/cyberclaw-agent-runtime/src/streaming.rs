//! Streaming response types for the CyberClaw agent runtime.
//!
//! Provides typed streaming events, a sink trait for consumers,
//! a channel-based implementation, and an adapter from LLM chunks.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use cyberclaw_llm::types::ChatChunk;

// ---------------------------------------------------------------------------
// StreamError
// ---------------------------------------------------------------------------

/// Errors that can occur during stream operations.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// The underlying channel has been closed.
    #[error("stream channel closed")]
    ChannelClosed,

    /// A send or receive operation timed out.
    #[error("stream operation timed out")]
    Timeout,

    /// Failed to serialize or deserialize a stream event.
    #[error("serialization error: {0}")]
    SerializationError(String),
}

// ---------------------------------------------------------------------------
// StreamSummary
// ---------------------------------------------------------------------------

/// Summary emitted when the stream completes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StreamSummary {
    /// Total agentic-loop iterations executed.
    pub total_iterations: usize,
    /// Total tokens consumed (prompt + completion).
    pub total_tokens: usize,
    /// Wall-clock duration of the entire stream in milliseconds.
    pub total_duration_ms: u64,
    /// Number of tool calls made across all iterations.
    pub tool_calls_count: usize,
}

// ---------------------------------------------------------------------------
// StreamEvent
// ---------------------------------------------------------------------------

/// A typed event emitted on the streaming channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// A text content delta.
    Text { content: String, index: usize },
    /// A new tool call has started.
    ToolCallStart {
        id: String,
        name: String,
        index: usize,
    },
    /// Incremental arguments for an in-progress tool call.
    ToolCallDelta { id: String, arguments_delta: String },
    /// A tool call has completed.
    ToolCallEnd { id: String },
    /// An agentic-loop iteration is starting.
    IterationStart {
        iteration: usize,
        budget_remaining: usize,
    },
    /// An agentic-loop iteration has ended.
    IterationEnd { iteration: usize, duration_ms: u64 },
    /// An error occurred during streaming.
    Error { message: String, recoverable: bool },
    /// The stream is complete.
    Done { summary: StreamSummary },
    /// The loop governor promoted the token budget to a higher tier.
    ///
    /// BUG-R8-01: emitted when `try_upgrade()` fires inside `pre_iteration`.
    /// Callers may surface this as an SSE frame so operators can observe
    /// dynamic budget promotions in real time.
    BudgetUpgraded {
        /// Profile name before upgrade (e.g. `"L1"`).
        from_profile: String,
        /// Profile name after upgrade (e.g. `"L2"`).
        to_profile: String,
        /// New token ceiling after the upgrade.
        new_token_ceiling: u64,
    },
}

// ---------------------------------------------------------------------------
// StreamSink trait
// ---------------------------------------------------------------------------

/// Consumer of [`StreamEvent`]s.
#[async_trait]
pub trait StreamSink: Send + Sync {
    /// Send a single event to the consumer.
    async fn send(&self, event: StreamEvent) -> Result<(), StreamError>;

    /// Signal that no more events will be sent.
    async fn close(&self) -> Result<(), StreamError>;
}

// ---------------------------------------------------------------------------
// StreamReceiver
// ---------------------------------------------------------------------------

/// Receiver half of a [`ChannelStreamSink`].
pub struct StreamReceiver {
    inner: mpsc::Receiver<StreamEvent>,
}

impl StreamReceiver {
    /// Receive the next event, returning `None` when the channel closes.
    pub async fn recv(&mut self) -> Option<StreamEvent> {
        self.inner.recv().await
    }
}

// ---------------------------------------------------------------------------
// ChannelStreamSink
// ---------------------------------------------------------------------------

/// A [`StreamSink`] backed by a `tokio::sync::mpsc` channel.
///
/// Calling [`close`](StreamSink::close) explicitly drops the internal sender,
/// which causes the receiver to observe channel closure immediately rather than
/// waiting for the `ChannelStreamSink` value itself to be dropped.
pub struct ChannelStreamSink {
    tx: std::sync::Mutex<Option<mpsc::Sender<StreamEvent>>>,
}

impl ChannelStreamSink {
    /// Create a new sink/receiver pair with the given channel capacity.
    pub fn new(capacity: usize) -> (Self, StreamReceiver) {
        let (tx, rx) = mpsc::channel(capacity);
        (
            Self {
                tx: std::sync::Mutex::new(Some(tx)),
            },
            StreamReceiver { inner: rx },
        )
    }

    /// Returns `true` if [`close`](StreamSink::close) has already been called.
    pub fn is_closed(&self) -> bool {
        self.tx.lock().expect("sink lock poisoned").is_none()
    }
}

#[async_trait]
impl StreamSink for ChannelStreamSink {
    async fn send(&self, event: StreamEvent) -> Result<(), StreamError> {
        let tx = {
            let guard = self.tx.lock().expect("sink lock poisoned");
            guard.as_ref().cloned().ok_or(StreamError::ChannelClosed)?
        };
        tx.send(event).await.map_err(|_| StreamError::ChannelClosed)
    }

    async fn close(&self) -> Result<(), StreamError> {
        let mut guard = self.tx.lock().expect("sink lock poisoned");
        // Drop the sender to close the channel immediately.
        let _ = guard.take();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// StreamAdapter
// ---------------------------------------------------------------------------

/// Converts LLM [`ChatChunk`]s into [`StreamEvent`]s.
pub struct StreamAdapter;

impl StreamAdapter {
    /// Convert a single [`ChatChunk`] into zero or more [`StreamEvent`]s.
    pub fn from_chunk(chunk: &ChatChunk, iteration: usize) -> Vec<StreamEvent> {
        let mut events = Vec::new();

        for choice in &chunk.choices {
            // Text content delta
            if let Some(ref content) = choice.delta.content {
                if !content.is_empty() {
                    events.push(StreamEvent::Text {
                        content: content.clone(),
                        index: iteration,
                    });
                }
            }

            // Tool call deltas
            if let Some(ref tool_calls) = choice.delta.tool_calls {
                for tc in tool_calls {
                    let has_name = !tc.function.name.is_empty();
                    let has_args = !tc.function.arguments.is_empty();

                    if has_name {
                        events.push(StreamEvent::ToolCallStart {
                            id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            index: iteration,
                        });
                    }

                    if has_args {
                        events.push(StreamEvent::ToolCallDelta {
                            id: tc.id.clone(),
                            arguments_delta: tc.function.arguments.clone(),
                        });
                    }
                }
            }

            // finish_reason signals tool call end for any active tool calls
            if choice.finish_reason.as_deref() == Some("tool_calls") {
                if let Some(ref tool_calls) = choice.delta.tool_calls {
                    for tc in tool_calls {
                        events.push(StreamEvent::ToolCallEnd { id: tc.id.clone() });
                    }
                }
            }
        }

        events
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_llm::types::{ChunkChoice, Delta, FunctionCall, ToolCall};

    // -- helpers --

    fn make_text_chunk(content: &str) -> ChatChunk {
        ChatChunk {
            id: "chunk-1".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "test".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: None,
                    content: Some(content.into()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
        }
    }

    fn make_tool_call_chunk(id: &str, name: &str, args: &str) -> ChatChunk {
        ChatChunk {
            id: "chunk-2".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "test".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: None,
                    content: None,
                    tool_calls: Some(vec![ToolCall {
                        id: id.into(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: name.into(),
                            arguments: args.into(),
                        },
                    }]),
                },
                finish_reason: None,
            }],
        }
    }

    fn make_tool_call_end_chunk(id: &str) -> ChatChunk {
        ChatChunk {
            id: "chunk-3".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "test".into(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    role: None,
                    content: None,
                    tool_calls: Some(vec![ToolCall {
                        id: id.into(),
                        call_type: "function".into(),
                        function: FunctionCall {
                            name: String::new(),
                            arguments: String::new(),
                        },
                    }]),
                },
                finish_reason: Some("tool_calls".into()),
            }],
        }
    }

    // -- StreamEvent serialization tests --

    #[test]
    fn test_stream_event_text_serialization() {
        let event = StreamEvent::Text {
            content: "hello".into(),
            index: 0,
        };
        let json = serde_json::to_string(&event).unwrap();
        let roundtrip: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, roundtrip);
    }

    #[test]
    fn test_stream_event_tool_call_start_serialization() {
        let event = StreamEvent::ToolCallStart {
            id: "tc-1".into(),
            name: "read_file".into(),
            index: 1,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"tool_call_start\""));
        let roundtrip: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, roundtrip);
    }

    #[test]
    fn test_stream_event_done_serialization() {
        let event = StreamEvent::Done {
            summary: StreamSummary {
                total_iterations: 3,
                total_tokens: 1500,
                total_duration_ms: 4200,
                tool_calls_count: 5,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        let roundtrip: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, roundtrip);
    }

    #[test]
    fn test_stream_event_error_serialization() {
        let event = StreamEvent::Error {
            message: "something broke".into(),
            recoverable: true,
        };
        let json = serde_json::to_string(&event).unwrap();
        let roundtrip: StreamEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, roundtrip);
    }

    // -- StreamSummary --

    #[test]
    fn test_stream_summary_creation() {
        let summary = StreamSummary {
            total_iterations: 2,
            total_tokens: 500,
            total_duration_ms: 1000,
            tool_calls_count: 3,
        };
        assert_eq!(summary.total_iterations, 2);
        assert_eq!(summary.total_tokens, 500);
        assert_eq!(summary.total_duration_ms, 1000);
        assert_eq!(summary.tool_calls_count, 3);
    }

    // -- ChannelStreamSink --

    #[tokio::test]
    async fn test_channel_stream_sink_send_receive() {
        let (sink, mut receiver) = ChannelStreamSink::new(8);
        let event = StreamEvent::Text {
            content: "hi".into(),
            index: 0,
        };
        sink.send(event.clone()).await.unwrap();
        let received = receiver.recv().await.unwrap();
        assert_eq!(received, event);
    }

    #[tokio::test]
    async fn test_channel_stream_sink_close_behaviour() {
        let (sink, mut receiver) = ChannelStreamSink::new(4);
        assert!(!sink.is_closed());
        sink.close().await.unwrap();
        assert!(sink.is_closed());
        // After close(), the receiver should immediately get None (channel closed).
        let result = receiver.recv().await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_channel_send_after_close_returns_error() {
        let (sink, _receiver) = ChannelStreamSink::new(4);
        sink.close().await.unwrap();
        let result = sink
            .send(StreamEvent::Text {
                content: "after close".into(),
                index: 0,
            })
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StreamError::ChannelClosed));
    }

    #[tokio::test]
    async fn test_channel_close_is_idempotent() {
        let (sink, _receiver) = ChannelStreamSink::new(4);
        sink.close().await.unwrap();
        // Second close should also succeed without panic.
        sink.close().await.unwrap();
        assert!(sink.is_closed());
    }

    #[tokio::test]
    async fn test_channel_send_after_receiver_dropped() {
        let (sink, receiver) = ChannelStreamSink::new(4);
        drop(receiver);
        let result = sink
            .send(StreamEvent::Text {
                content: "orphan".into(),
                index: 0,
            })
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StreamError::ChannelClosed));
    }

    // -- StreamAdapter --

    #[test]
    fn test_adapter_text_conversion() {
        let chunk = make_text_chunk("Hello world");
        let events = StreamAdapter::from_chunk(&chunk, 0);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            StreamEvent::Text {
                content: "Hello world".into(),
                index: 0,
            }
        );
    }

    #[test]
    fn test_adapter_tool_call_start_and_delta() {
        let chunk = make_tool_call_chunk("tc-1", "read_file", "{\"path\":");
        let events = StreamAdapter::from_chunk(&chunk, 1);
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], StreamEvent::ToolCallStart { id, name, index }
            if id == "tc-1" && name == "read_file" && *index == 1)
        );
        assert!(
            matches!(&events[1], StreamEvent::ToolCallDelta { id, arguments_delta }
            if id == "tc-1" && arguments_delta == "{\"path\":")
        );
    }

    #[test]
    fn test_adapter_tool_call_end() {
        let chunk = make_tool_call_end_chunk("tc-1");
        let events = StreamAdapter::from_chunk(&chunk, 2);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], StreamEvent::ToolCallEnd { id: "tc-1".into() });
    }

    #[test]
    fn test_adapter_empty_content_skipped() {
        let chunk = make_text_chunk("");
        let events = StreamAdapter::from_chunk(&chunk, 0);
        assert!(events.is_empty());
    }

    // -- Error formatting --

    #[test]
    fn test_stream_error_display() {
        let e = StreamError::ChannelClosed;
        assert_eq!(e.to_string(), "stream channel closed");

        let e = StreamError::Timeout;
        assert_eq!(e.to_string(), "stream operation timed out");

        let e = StreamError::SerializationError("bad json".into());
        assert_eq!(e.to_string(), "serialization error: bad json");
    }
}
