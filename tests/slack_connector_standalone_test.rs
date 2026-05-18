/// 独立的 Slack Connector 编译测试
///
/// 此文件用于验证 Slack Connector 的代码是否正确编译，
/// 不依赖其他可能有问题的 connectors。

#[cfg(test)]
mod slack_standalone_tests {
    use cyberclaw_connectors::slack_connector::*;
    use cyberclaw_core::prelude::*;
    use serde_json::json;

    #[test]
    fn test_slack_connector_creation() {
        let connector = SlackConnector::new("xoxb-test-token".to_string());
        assert_eq!(connector.id().as_str(), "slack-connector");
    }

    #[test]
    fn test_slack_connector_runtime() {
        let connector = SlackConnector::new("xoxb-test-token".to_string());
        assert_eq!(connector.runtime(), ConnectorRuntime::Native);
    }

    #[test]
    fn test_slack_capabilities_count() {
        let connector = SlackConnector::new("xoxb-test-token".to_string());
        assert_eq!(connector.capabilities().len(), 4);
    }

    #[test]
    fn test_send_message_input() {
        let input = SendMessageInput {
            channel: "#general".to_string(),
            text: Some("Hello".to_string()),
            template: None,
            data: json!({}),
            blocks: None,
            thread_ts: None,
        };

        let json_str = serde_json::to_string(&input).unwrap();
        let _: SendMessageInput = serde_json::from_str(&json_str).unwrap();
    }

    #[test]
    fn test_create_channel_input() {
        let input = CreateChannelInput {
            name: "test-channel".to_string(),
            is_private: false,
            description: Some("Test".to_string()),
        };

        let json_str = serde_json::to_string(&input).unwrap();
        let _: CreateChannelInput = serde_json::from_str(&json_str).unwrap();
    }

    #[test]
    fn test_upload_file_input() {
        use base64::Engine;
        let content = base64::engine::general_purpose::STANDARD.encode(b"test");
        let input = UploadFileInput {
            channel: "#general".to_string(),
            content,
            filename: "test.txt".to_string(),
            filetype: Some("text/plain".to_string()),
            initial_comment: None,
        };

        let json_str = serde_json::to_string(&input).unwrap();
        let _: UploadFileInput = serde_json::from_str(&json_str).unwrap();
    }

    #[test]
    fn test_react_emoji_input() {
        let input = ReactEmojiInput {
            channel: "C12345".to_string(),
            timestamp: "1234567890.123456".to_string(),
            emoji: "thumbsup".to_string(),
        };

        let json_str = serde_json::to_string(&input).unwrap();
        let _: ReactEmojiInput = serde_json::from_str(&json_str).unwrap();
    }

    #[test]
    fn test_capability_contracts() {
        let connector = SlackConnector::new("xoxb-test-token".to_string());
        let caps = connector.capabilities();

        assert_eq!(caps[0].id, "slack.send_message");
        assert_eq!(caps[0].title, "Send Slack Message");
        assert_eq!(caps[0].risk, RiskLevel::Low);

        assert_eq!(caps[1].id, "slack.create_channel");
        assert_eq!(caps[1].title, "Create Slack Channel");
        assert_eq!(caps[1].risk, RiskLevel::High);

        assert_eq!(caps[2].id, "slack.upload_file");
        assert_eq!(caps[2].title, "Upload File to Slack");
        assert_eq!(caps[2].risk, RiskLevel::Medium);

        assert_eq!(caps[3].id, "slack.react_emoji");
        assert_eq!(caps[3].title, "React with Emoji");
        assert_eq!(caps[3].risk, RiskLevel::Low);
    }

    #[tokio::test]
    async fn test_template_registration() {
        let connector = SlackConnector::new("xoxb-test-token".to_string());
        let result = connector
            .register_template(
                "test".to_string(),
                "Hello {{name}}!".to_string(),
            )
            .await;
        assert!(result.is_ok());
    }
}
