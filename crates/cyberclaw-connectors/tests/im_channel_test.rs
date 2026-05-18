//! IM Channel + Voice Processing 集成测试
//!
//! 覆盖 M0 里程碑的 18+ 测试用例

use cyberclaw_connectors::im_channel::{
    AudioFormat, ImChannelConfig, ImChannelConnector, ImMessage, ImMessageType, ImPlatformAdapter,
    ReplyMode, SessionBinding,
};
use cyberclaw_connectors::voice_processing::{
    ClassificationContext, IntentClassifier, RuleBasedClassifier, UserIntent, VoiceSafeSummarizer,
    VoiceSummary,
};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Mock adapter for tests
// ---------------------------------------------------------------------------

struct TestImAdapter;

#[async_trait::async_trait]
impl ImPlatformAdapter for TestImAdapter {
    fn platform_id(&self) -> &str {
        "test"
    }

    fn supports_voice(&self) -> bool {
        true
    }

    async fn normalize_inbound(&self, raw: serde_json::Value) -> anyhow::Result<ImMessage> {
        let id = raw
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("test-id")
            .to_string();
        let text = raw
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let voice = raw
            .get("voice_media_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let msg_type = if voice.is_some() {
            ImMessageType::Voice
        } else if text.as_ref().map(|t| t.starts_with('/')).unwrap_or(false) {
            ImMessageType::Command
        } else {
            ImMessageType::Text
        };

        Ok(ImMessage {
            id,
            platform: "test".to_string(),
            chat_id: raw
                .get("chat_id")
                .and_then(|v| v.as_str())
                .unwrap_or("chat-1")
                .to_string(),
            sender: raw
                .get("sender")
                .and_then(|v| v.as_str())
                .unwrap_or("user-1")
                .to_string(),
            message_type: msg_type,
            text_content: text,
            voice_media_id: voice,
            voice_duration_secs: None,
            timestamp: chrono::Utc::now(),
            metadata: HashMap::new(),
            raw,
        })
    }

    async fn send_text(&self, _chat_id: &str, _text: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn send_voice(
        &self,
        _chat_id: &str,
        _audio: &[u8],
        _format: AudioFormat,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn download_voice(&self, _media_id: &str) -> anyhow::Result<Vec<u8>> {
        Ok(vec![0xAA; 64])
    }

    fn validate_signature(&self, _payload: &[u8], _signature: &str) -> bool {
        true
    }
}

fn make_connector() -> ImChannelConnector {
    ImChannelConnector::new(ImChannelConfig::default())
}

// ===========================================================================
// ImMessage 标准化 (4 tests)
// ===========================================================================

#[tokio::test]
async fn test_normalize_text_message() {
    let adapter = TestImAdapter;
    let msg = adapter
        .normalize_inbound(serde_json::json!({
            "id": "msg-001",
            "text": "hello world",
            "chat_id": "chat-1",
            "sender": "alice"
        }))
        .await
        .unwrap();
    assert_eq!(msg.id, "msg-001");
    assert_eq!(msg.message_type, ImMessageType::Text);
    assert_eq!(msg.text_content.as_deref(), Some("hello world"));
    assert_eq!(msg.sender, "alice");
}

#[tokio::test]
async fn test_normalize_voice_message() {
    let adapter = TestImAdapter;
    let msg = adapter
        .normalize_inbound(serde_json::json!({
            "id": "msg-002",
            "voice_media_id": "file_key_123",
            "chat_id": "chat-1"
        }))
        .await
        .unwrap();
    assert_eq!(msg.message_type, ImMessageType::Voice);
    assert_eq!(msg.voice_media_id.as_deref(), Some("file_key_123"));
    assert!(msg.text_content.is_none());
}

#[tokio::test]
async fn test_normalize_command_message() {
    let adapter = TestImAdapter;
    let msg = adapter
        .normalize_inbound(serde_json::json!({
            "id": "msg-003",
            "text": "/status",
            "chat_id": "chat-1"
        }))
        .await
        .unwrap();
    assert_eq!(msg.message_type, ImMessageType::Command);
    assert_eq!(msg.text_content.as_deref(), Some("/status"));
}

#[tokio::test]
async fn test_normalize_missing_fields() {
    let adapter = TestImAdapter;
    let msg = adapter
        .normalize_inbound(serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(msg.id, "test-id");
    assert_eq!(msg.chat_id, "chat-1");
    assert_eq!(msg.sender, "user-1");
    assert_eq!(msg.message_type, ImMessageType::Text);
    assert!(msg.text_content.is_none());
}

// ===========================================================================
// SessionBinding (4 tests)
// ===========================================================================

#[tokio::test]
async fn test_session_bind() {
    let connector = make_connector();
    let now = chrono::Utc::now();
    let binding = SessionBinding {
        chat_id: "chat-bind-1".to_string(),
        platform: "test".to_string(),
        session_id: "sess-1".to_string(),
        execution_id: None,
        external_session_id: None,
        reply_mode: ReplyMode::FollowUser,
        created_at: now,
        last_activity: now,
    };
    connector.bind_session(binding).await.unwrap();
    let found = connector.lookup_binding("chat-bind-1").await;
    assert!(found.is_some());
    let b = found.unwrap();
    assert_eq!(b.session_id, "sess-1");
    assert_eq!(b.reply_mode, ReplyMode::FollowUser);
}

#[tokio::test]
async fn test_session_rebind() {
    let connector = make_connector();
    let now = chrono::Utc::now();
    // 第一次绑定
    connector
        .bind_session(SessionBinding {
            chat_id: "chat-rebind".to_string(),
            platform: "test".to_string(),
            session_id: "sess-old".to_string(),
            execution_id: None,
            external_session_id: None,
            reply_mode: ReplyMode::TextOnly,
            created_at: now,
            last_activity: now,
        })
        .await
        .unwrap();
    // 覆盖绑定
    connector
        .bind_session(SessionBinding {
            chat_id: "chat-rebind".to_string(),
            platform: "test".to_string(),
            session_id: "sess-new".to_string(),
            execution_id: None,
            external_session_id: None,
            reply_mode: ReplyMode::VoiceOnly,
            created_at: now,
            last_activity: now,
        })
        .await
        .unwrap();
    let found = connector.lookup_binding("chat-rebind").await.unwrap();
    assert_eq!(found.session_id, "sess-new");
    assert_eq!(found.reply_mode, ReplyMode::VoiceOnly);
}

#[tokio::test]
async fn test_session_lookup_nonexistent() {
    let connector = make_connector();
    let found = connector.lookup_binding("no-such-chat").await;
    assert!(found.is_none());
}

#[tokio::test]
async fn test_session_expiry() {
    let config = ImChannelConfig {
        session_timeout_secs: 0, // 立即过期
        ..Default::default()
    };
    let connector = ImChannelConnector::new(config);
    let past = chrono::Utc::now() - chrono::Duration::seconds(10);
    connector
        .bind_session(SessionBinding {
            chat_id: "chat-expired".to_string(),
            platform: "test".to_string(),
            session_id: "sess-exp".to_string(),
            execution_id: None,
            external_session_id: None,
            reply_mode: ReplyMode::TextOnly,
            created_at: past,
            last_activity: past,
        })
        .await
        .unwrap();
    let found = connector.lookup_binding("chat-expired").await;
    assert!(found.is_none(), "过期的 session 应返回 None");
}

// ===========================================================================
// ReplyMode 行为 (4 tests)
// ===========================================================================

#[test]
fn test_reply_mode_text_only() {
    let mode = ReplyMode::TextOnly;
    assert_eq!(mode, ReplyMode::TextOnly);
    // 序列化/反序列化
    let json = serde_json::to_string(&mode).unwrap();
    let deserialized: ReplyMode = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, ReplyMode::TextOnly);
}

#[test]
fn test_reply_mode_voice_only() {
    let mode = ReplyMode::VoiceOnly;
    let json = serde_json::to_string(&mode).unwrap();
    let deserialized: ReplyMode = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, ReplyMode::VoiceOnly);
}

#[test]
fn test_reply_mode_follow_user() {
    let mode = ReplyMode::FollowUser;
    let json = serde_json::to_string(&mode).unwrap();
    let deserialized: ReplyMode = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, ReplyMode::FollowUser);
}

#[test]
fn test_reply_mode_driving_mode() {
    let mode = ReplyMode::DrivingMode;
    let json = serde_json::to_string(&mode).unwrap();
    let deserialized: ReplyMode = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, ReplyMode::DrivingMode);
}

// ===========================================================================
// UserIntent 分类 (4 tests)
// ===========================================================================

#[tokio::test]
async fn test_intent_execute_capability() {
    let classifier = RuleBasedClassifier::with_defaults();
    let ctx = ClassificationContext::default();
    let intent = classifier.classify("跑一下测试", &ctx).await.unwrap();
    match intent {
        UserIntent::ExecuteCapability { description } => {
            assert!(description.contains("测试"));
        }
        other => panic!("Expected ExecuteCapability, got {:?}", other),
    }
}

#[tokio::test]
async fn test_intent_invoke_external_agent() {
    let classifier = RuleBasedClassifier::with_defaults();
    let ctx = ClassificationContext::default();
    let intent = classifier
        .classify("用 claude code 把接口改成异步", &ctx)
        .await
        .unwrap();
    match intent {
        UserIntent::InvokeExternalAgent { runtime, task } => {
            assert_eq!(runtime.as_deref(), Some("claude_code"));
            assert!(task.contains("接口"));
        }
        other => panic!("Expected InvokeExternalAgent, got {:?}", other),
    }
}

#[tokio::test]
async fn test_intent_approve() {
    let classifier = RuleBasedClassifier::with_defaults();
    let ctx = ClassificationContext {
        has_pending_confirmation: true,
        ..Default::default()
    };
    let intent = classifier.classify("确认", &ctx).await.unwrap();
    assert_eq!(intent, UserIntent::Approve);
}

#[tokio::test]
async fn test_intent_unclassified() {
    let classifier = RuleBasedClassifier::with_defaults();
    let ctx = ClassificationContext::default();
    let intent = classifier.classify("今天天气真好啊", &ctx).await.unwrap();
    match intent {
        UserIntent::Unclassified { raw_text } => {
            assert!(raw_text.contains("天气"));
        }
        other => panic!("Expected Unclassified, got {:?}", other),
    }
}

// ===========================================================================
// VoiceSafeSummarizer (2 tests)
// ===========================================================================

#[test]
fn test_summarizer_brief_status() {
    let summarizer = VoiceSafeSummarizer::default();
    let summary = VoiceSummary::BriefStatus {
        message: "正在执行中".to_string(),
    };
    let text = summarizer.render(&summary);
    assert_eq!(text, "正在执行中");
}

#[test]
fn test_summarizer_completion_summary() {
    let summarizer = VoiceSafeSummarizer::default();
    let summary = VoiceSummary::CompletionSummary {
        message: "已完成，修改了 3 个文件，测试通过".to_string(),
        detail_count: 5,
    };
    let text = summarizer.render(&summary);
    assert!(text.contains("已完成"));
    assert!(text.contains("5 项细节"));
}

// ===========================================================================
// Review Fix: FIFO 去重淘汰测试
// ===========================================================================

#[tokio::test]
async fn test_dedup_fifo_eviction() {
    let config = ImChannelConfig {
        dedup_window_size: 3,
        ..Default::default()
    };
    let connector = ImChannelConnector::new(config);

    // 填满窗口: msg-1, msg-2, msg-3
    assert!(!connector.is_duplicate("msg-1").await);
    assert!(!connector.is_duplicate("msg-2").await);
    assert!(!connector.is_duplicate("msg-3").await);

    // 已存在的应返回 true
    assert!(connector.is_duplicate("msg-1").await);

    // 插入 msg-4，应导致 msg-1 被 FIFO 淘汰（而非清空全部）
    assert!(!connector.is_duplicate("msg-4").await);

    // msg-1 已被淘汰，不再被视为重复
    assert!(!connector.is_duplicate("msg-1").await);

    // msg-2 此时也应已被淘汰（因为 msg-4 和 msg-1 的再次插入挤掉了 msg-2 和 msg-3）
    // 当前窗口应为: msg-4, msg-1, (msg-2 被淘汰)
    assert!(!connector.is_duplicate("msg-2").await);
}
