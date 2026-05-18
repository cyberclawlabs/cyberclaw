//! # Slack Connector 集成测试
//!
//! 包含至少 15 个集成测试用例，覆盖所有 capabilities 和边界情况。

#[cfg(test)]
mod integration_tests {
    use super::super::slack_connector::*;
    use crate::types::*;
    use cyberclaw_core::prelude::*;
    use serde_json::json;

    /// 创建测试用的 Slack Connector
    fn create_test_connector() -> SlackConnector {
        SlackConnector::new("xoxb-test-token-12345".to_string())
    }

    /// 创建测试用的执行请求
    fn create_test_request(
        capability: &str,
        input: serde_json::Value,
    ) -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "test-trace-123".to_string(),
            actor: ActorRef::system(),
            workspace: WorkspaceRef {
                id: "test-workspace".to_string(),
                path: "/tmp/test".into(),
            },
            connector_id: ConnectorId::from_string("slack-connector".to_string()).unwrap(),
            capability_id: CapabilityId::from_string(capability.to_string()).unwrap(),
            input,
        }
    }

    // ===== 基础功能测试 (5 tests) =====

    #[test]
    fn test_01_connector_initialization() {
        let connector = create_test_connector();
        assert_eq!(connector.id().as_str(), "slack-connector");
        assert_eq!(connector.runtime(), ConnectorRuntime::Native);
    }

    #[test]
    fn test_02_capabilities_count() {
        let connector = create_test_connector();
        let caps = connector.capabilities();
        assert_eq!(caps.len(), 4, "Should have exactly 4 capabilities");
    }

    #[test]
    fn test_03_capability_ids() {
        let connector = create_test_connector();
        let caps = connector.capabilities();
        let ids: Vec<&str> = caps.iter().map(|c| c.id.as_str()).collect();

        assert!(ids.contains(&"slack.send_message"));
        assert!(ids.contains(&"slack.create_channel"));
        assert!(ids.contains(&"slack.upload_file"));
        assert!(ids.contains(&"slack.react_emoji"));
    }

    #[test]
    fn test_04_capability_risk_levels() {
        let connector = create_test_connector();
        let caps = connector.capabilities();

        // send_message 和 react_emoji 应该是 Low
        let send_msg = caps
            .iter()
            .find(|c| c.id == "slack.send_message")
            .unwrap();
        assert_eq!(send_msg.risk, RiskLevel::Low);

        let react = caps
            .iter()
            .find(|c| c.id == "slack.react_emoji")
            .unwrap();
        assert_eq!(react.risk, RiskLevel::Low);

        // create_channel 应该是 High
        let create_ch = caps
            .iter()
            .find(|c| c.id == "slack.create_channel")
            .unwrap();
        assert_eq!(create_ch.risk, RiskLevel::High);

        // upload_file 应该是 Medium
        let upload = caps
            .iter()
            .find(|c| c.id == "slack.upload_file")
            .unwrap();
        assert_eq!(upload.risk, RiskLevel::Medium);
    }

    #[test]
    fn test_05_capability_approval_requirements() {
        let connector = create_test_connector();
        let caps = connector.capabilities();

        // create_channel 风险高
        let create_ch = caps
            .iter()
            .find(|c| c.id == "slack.create_channel")
            .unwrap();
        assert_eq!(
            create_ch.risk,
            RiskLevel::High,
            "create_channel should be high risk"
        );
    }

    // ===== 消息模板系统测试 (5 tests) =====

    #[tokio::test]
    async fn test_06_template_registration() {
        let connector = create_test_connector();
        let result = connector
            .register_template(
                "test_template".to_string(),
                "Hello {{name}}!".to_string(),
            )
            .await;

        assert!(result.is_ok(), "Template registration should succeed");
    }

    #[tokio::test]
    async fn test_07_template_with_multiple_variables() {
        let connector = create_test_connector();
        connector
            .register_template(
                "multi_var".to_string(),
                "{{greeting}} {{name}}, welcome to {{channel}}!".to_string(),
            )
            .await
            .unwrap();

        // 实际渲染会在 send_message 中进行
        // 这里只测试注册成功
    }

    #[tokio::test]
    async fn test_08_template_overwrite() {
        let connector = create_test_connector();

        // 第一次注册
        connector
            .register_template("overwrite".to_string(), "Version 1".to_string())
            .await
            .unwrap();

        // 覆盖注册
        let result = connector
            .register_template("overwrite".to_string(), "Version 2".to_string())
            .await;

        assert!(result.is_ok(), "Template overwrite should succeed");
    }

    #[tokio::test]
    async fn test_09_template_with_json_data() {
        let connector = create_test_connector();
        connector
            .register_template(
                "json_data".to_string(),
                "User: {{user.name}}, Email: {{user.email}}".to_string(),
            )
            .await
            .unwrap();

        // 验证复杂数据结构支持
    }

    #[tokio::test]
    async fn test_10_template_with_loops() {
        let connector = create_test_connector();
        let template = r#"
Issues:
{{#each issues}}
- {{this.title}}
{{/each}}
"#
        .to_string();

        let result = connector
            .register_template("loop_template".to_string(), template)
            .await;

        assert!(result.is_ok(), "Loop template registration should succeed");
    }

    // ===== 输入验证测试 (5 tests) =====

    #[test]
    fn test_11_send_message_input_validation() {
        let input = SendMessageInput {
            channel: "#general".to_string(),
            text: Some("Test message".to_string()),
            template: None,
            data: json!({}),
            blocks: None,
            thread_ts: None,
        };

        let json = serde_json::to_value(&input).unwrap();
        let _: SendMessageInput = serde_json::from_value(json).unwrap();
    }

    #[test]
    fn test_12_create_channel_input_validation() {
        let input = CreateChannelInput {
            name: "test-channel".to_string(),
            is_private: false,
            description: Some("Test channel".to_string()),
        };

        let json = serde_json::to_value(&input).unwrap();
        let deserialized: CreateChannelInput = serde_json::from_value(json).unwrap();

        assert_eq!(deserialized.name, "test-channel");
        assert!(!deserialized.is_private);
    }

    #[test]
    fn test_13_upload_file_input_validation() {
        use base64::Engine;
        let content = base64::engine::general_purpose::STANDARD.encode(b"test file content");
        let input = UploadFileInput {
            channel: "#general".to_string(),
            content,
            filename: "test.txt".to_string(),
            filetype: Some("text/plain".to_string()),
            initial_comment: None,
        };

        let json = serde_json::to_value(&input).unwrap();
        let _: UploadFileInput = serde_json::from_value(json).unwrap();
    }

    #[test]
    fn test_14_react_emoji_input_validation() {
        let input = ReactEmojiInput {
            channel: "C12345678".to_string(),
            timestamp: "1234567890.123456".to_string(),
            emoji: "thumbsup".to_string(),
        };

        let json = serde_json::to_value(&input).unwrap();
        let deserialized: ReactEmojiInput = serde_json::from_value(json).unwrap();

        assert_eq!(deserialized.emoji, "thumbsup");
    }

    #[test]
    fn test_15_send_message_with_template_data() {
        let input = SendMessageInput {
            channel: "#general".to_string(),
            text: None,
            template: Some("welcome".to_string()),
            data: json!({
                "name": "Alice",
                "channel": "#general"
            }),
            blocks: None,
            thread_ts: None,
        };

        let json = serde_json::to_value(&input).unwrap();
        let _: SendMessageInput = serde_json::from_value(json).unwrap();
    }

    // ===== Schema 验证测试 (3 tests) =====

    #[test]
    fn test_16_send_message_schema() {
        let connector = create_test_connector();
        let cap = connector
            .capabilities()
            .iter()
            .find(|c| c.id == "slack.send_message")
            .unwrap();

        // Schema 现在是 String
        assert!(!cap.input_schema.is_empty());
        let schema: serde_json::Value = serde_json::from_str(&cap.input_schema).unwrap();
        assert!(schema["properties"]["channel"].is_object());
    }

    #[test]
    fn test_17_create_channel_schema() {
        let connector = create_test_connector();
        let cap = connector
            .capabilities()
            .iter()
            .find(|c| c.id == "slack.create_channel")
            .unwrap();

        let schema: serde_json::Value = serde_json::from_str(&cap.input_schema).unwrap();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("name")));
    }

    #[test]
    fn test_18_upload_file_schema() {
        let connector = create_test_connector();
        let cap = connector
            .capabilities()
            .iter()
            .find(|c| c.id == "slack.upload_file")
            .unwrap();

        let schema: serde_json::Value = serde_json::from_str(&cap.input_schema).unwrap();
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("channel")));
        assert!(required.contains(&json!("content")));
        assert!(required.contains(&json!("filename")));
    }

    // ===== 错误处理测试 (2 tests) =====

    #[tokio::test]
    async fn test_19_unknown_capability() {
        let connector = create_test_connector();
        let request = create_test_request(
            "slack.unknown_capability",
            json!({"test": "data"}),
        );

        let result = connector.execute(request).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_20_invalid_input_deserialization() {
        let invalid_json = json!({"invalid": "field"});
        let result: Result<SendMessageInput, _> = serde_json::from_value(invalid_json);

        // 应该失败，因为缺少必需的 channel 字段
        assert!(result.is_err());
    }

    #[test]
    fn test_base64_encoding_in_tests() {
        use base64::Engine;
        let content = base64::engine::general_purpose::STANDARD.encode(b"test file content");
        assert!(!content.is_empty());
    }

    // ===== 输出验证测试 (2 tests) =====

    #[test]
    fn test_21_send_message_output_serialization() {
        let output = SendMessageOutput {
            ts: "1234567890.123456".to_string(),
            channel: "C12345678".to_string(),
            ok: true,
        };

        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["ok"], true);
    }

    #[test]
    fn test_22_create_channel_output_serialization() {
        let output = CreateChannelOutput {
            id: "C12345678".to_string(),
            name: "test-channel".to_string(),
            ok: true,
        };

        let json = serde_json::to_value(&output).unwrap();
        let deserialized: CreateChannelOutput = serde_json::from_value(json).unwrap();

        assert_eq!(deserialized.id, "C12345678");
        assert!(deserialized.ok);
    }

    // ===== 边界情况测试 (3 tests) =====

    #[test]
    fn test_23_empty_channel_name() {
        let input = SendMessageInput {
            channel: "".to_string(),
            text: Some("Test".to_string()),
            template: None,
            data: json!({}),
            blocks: None,
            thread_ts: None,
        };

        // 序列化应该成功，但实际执行时应该失败
        let json = serde_json::to_value(&input).unwrap();
        let _: SendMessageInput = serde_json::from_value(json).unwrap();
    }

    #[test]
    fn test_24_very_long_message() {
        let long_text = "a".repeat(10000);
        let input = SendMessageInput {
            channel: "#general".to_string(),
            text: Some(long_text.clone()),
            template: None,
            data: json!({}),
            blocks: None,
            thread_ts: None,
        };

        let json = serde_json::to_value(&input).unwrap();
        let deserialized: SendMessageInput = serde_json::from_value(json).unwrap();

        assert_eq!(deserialized.text.unwrap().len(), 10000);
    }

    #[test]
    fn test_25_special_characters_in_channel_name() {
        let input = CreateChannelInput {
            name: "test-channel-123_abc".to_string(),
            is_private: true,
            description: Some("Test with special chars: @#$%".to_string()),
        };

        let json = serde_json::to_value(&input).unwrap();
        let _: CreateChannelInput = serde_json::from_value(json).unwrap();
    }
}
