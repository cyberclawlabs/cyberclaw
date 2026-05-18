//! # Voice Processing Connector
//!
//! 平台无关的公共语音处理能力，不绑定任何 IM 渠道或特定用途。
//! 任何需要语音能力的场景（IM Bot、Web UI、API）都通过此 Connector 接入。
//!
//! ## 支持的 Capabilities
//!
//! - `voice.transcribe`: 语音转文字
//! - `voice.synthesize`: 文字转语音
//! - `voice.intent.classify`: 意图分类（通用）
//! - `voice.summary.render`: 执行结果转语音安全摘要

use crate::im_channel::{AudioFormat, ReplyMode};
use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector, ExecutionStatus,
};
use base64::Engine as _;
use cyberclaw_core::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// STT / TTS Backend traits
// ---------------------------------------------------------------------------

/// STT 后端 trait（可插拔）
#[async_trait::async_trait]
pub trait SttBackend: Send + Sync {
    /// 后端标识
    fn backend_id(&self) -> &str;

    /// 语音转文字
    async fn transcribe(
        &self,
        audio: &[u8],
        format: AudioFormat,
        language: Option<&str>,
    ) -> anyhow::Result<Transcript>;
}

/// TTS 后端 trait（可插拔）
#[async_trait::async_trait]
pub trait TtsBackend: Send + Sync {
    /// 后端标识
    fn backend_id(&self) -> &str;

    /// 文字转语音
    async fn synthesize(&self, text: &str, config: &VoiceConfig) -> anyhow::Result<Vec<u8>>;
}

// ---------------------------------------------------------------------------
// Core Types
// ---------------------------------------------------------------------------

/// 语音转文字结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    /// 识别文本
    pub text: String,
    /// 识别语言
    pub language: Option<String>,
    /// 置信度
    pub confidence: f32,
    /// 音频时长（秒）
    pub duration_secs: f32,
}

/// 语音合成配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceConfig {
    /// 语言
    pub language: String,
    /// 音色 ID
    pub voice_id: Option<String>,
    /// 语速
    pub speed: f32,
    /// 输出格式
    pub output_format: AudioFormat,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            language: "zh-CN".to_string(),
            voice_id: None,
            speed: 1.0,
            output_format: AudioFormat::Opus,
        }
    }
}

// ---------------------------------------------------------------------------
// VoiceSafeSummarizer
// ---------------------------------------------------------------------------

/// 摘要器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummarizerConfig {
    /// 最大字符数
    pub max_chars: usize,
    /// 最大播报时长（秒）
    pub max_duration_secs: u8,
    /// 语言
    pub language: String,
}

impl Default for SummarizerConfig {
    fn default() -> Self {
        Self {
            max_chars: 200,
            max_duration_secs: 8,
            language: "zh-CN".to_string(),
        }
    }
}

/// 语音摘要类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum VoiceSummary {
    /// 简短状态："正在执行"
    BriefStatus { message: String },
    /// 需要确认："将删除 3 个文件，是否继续？"
    DecisionRequired {
        question: String,
        risk_level: String,
    },
    /// 错误摘要："测试失败，接口签名不匹配"
    ErrorSummary { message: String },
    /// 完成摘要："已完成，修改了 2 个文件，测试通过"
    CompletionSummary {
        message: String,
        detail_count: usize,
    },
    /// 通用回复："查询结果：最近 3 天共 5 次部署"
    GeneralReply { message: String },
}

/// 将任意执行结果转为语音安全的摘要
/// 不绑定 agent output — 接受通用文本输入
#[derive(Debug, Clone)]
pub struct VoiceSafeSummarizer {
    config: SummarizerConfig,
}

impl VoiceSafeSummarizer {
    /// 创建新的摘要器
    pub fn new(config: SummarizerConfig) -> Self {
        Self { config }
    }

    /// 渲染语音安全摘要
    pub fn render(&self, summary: &VoiceSummary) -> String {
        let raw = match summary {
            VoiceSummary::BriefStatus { message } => message.clone(),
            VoiceSummary::DecisionRequired {
                question,
                risk_level,
            } => {
                format!("{}风险操作：{}", risk_level, question)
            }
            VoiceSummary::ErrorSummary { message } => format!("错误：{}", message),
            VoiceSummary::CompletionSummary {
                message,
                detail_count,
            } => {
                if *detail_count > 0 {
                    format!("{}，共 {} 项细节", message, detail_count)
                } else {
                    message.clone()
                }
            }
            VoiceSummary::GeneralReply { message } => message.clone(),
        };

        // 截断至最大字符数
        self.truncate_text(&raw)
    }

    /// 截断文本至配置的最大字符数
    fn truncate_text(&self, text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        if chars.len() <= self.config.max_chars {
            text.to_string()
        } else {
            let truncated: String = chars[..self.config.max_chars].iter().collect();
            format!("{}...", truncated)
        }
    }
}

impl Default for VoiceSafeSummarizer {
    fn default() -> Self {
        Self::new(SummarizerConfig::default())
    }
}

// ---------------------------------------------------------------------------
// UserIntent + IntentClassifier
// ---------------------------------------------------------------------------

/// 用户意图 — 通用平台级，不绑定外部 agent
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum UserIntent {
    // --- 通用平台操作 ---
    /// 执行平台能力："跑一下测试" / "查一下部署记录"
    ExecuteCapability { description: String },
    /// 与 Agent 对话："帮我分析一下这个 PR"
    ChatWithAgent { message: String },

    // --- 外部 agent 操控 ---
    /// 操控外部 coding agent："用 Claude Code 改接口"
    InvokeExternalAgent {
        runtime: Option<String>,
        task: String,
    },

    // --- 会话控制 ---
    /// 追加约束："不要改数据库层"
    Followup { constraint: String },
    /// 中断当前执行
    Interrupt,
    /// 查询进度："现在到哪了"
    AskStatus,
    /// 审批确认
    Approve,
    /// 审批拒绝
    Reject,

    // --- 模式切换 ---
    /// 切换到驾驶模式
    EnableDrivingMode,
    /// 退出驾驶模式
    DisableDrivingMode,

    // --- 未识别 ---
    /// 无法分类，直接作为自然语言传给 Agent
    Unclassified { raw_text: String },
}

/// 意图分类器 trait（可插拔）
#[async_trait::async_trait]
pub trait IntentClassifier: Send + Sync {
    /// 分类用户意图
    async fn classify(
        &self,
        text: &str,
        context: &ClassificationContext,
    ) -> anyhow::Result<UserIntent>;
}

/// 分类上下文
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationContext {
    /// 是否有活跃的外部 agent session
    pub has_active_external_session: bool,
    /// 是否有待确认的操作
    pub has_pending_confirmation: bool,
    /// 当前回复模式
    pub reply_mode: ReplyMode,
}

impl Default for ClassificationContext {
    fn default() -> Self {
        Self {
            has_active_external_session: false,
            has_pending_confirmation: false,
            reply_mode: ReplyMode::FollowUser,
        }
    }
}

/// 分类规则
#[derive(Debug, Clone)]
pub struct ClassificationRule {
    /// 匹配后产生的意图模板（用于确定意图类型）
    pub intent_type: ClassificationRuleType,
    /// 关键词列表
    pub keywords: Vec<String>,
    /// 正则表达式（可选）
    pub regex: Option<regex::Regex>,
    /// 优先级（数值越大优先级越高）
    pub priority: u8,
}

/// 分类规则类型
#[derive(Debug, Clone, PartialEq)]
pub enum ClassificationRuleType {
    ExecuteCapability,
    ChatWithAgent,
    InvokeExternalAgent,
    Followup,
    Interrupt,
    AskStatus,
    Approve,
    Reject,
    EnableDrivingMode,
    DisableDrivingMode,
}

/// Phase 1：规则+关键词分类器
#[derive(Debug)]
pub struct RuleBasedClassifier {
    rules: Vec<ClassificationRule>,
}

impl RuleBasedClassifier {
    /// 创建新的规则分类器
    pub fn new(rules: Vec<ClassificationRule>) -> Self {
        Self { rules }
    }

    /// 创建带默认规则的分类器
    pub fn with_defaults() -> Self {
        let rules = vec![
            // 审批相关（优先级最高）
            ClassificationRule {
                intent_type: ClassificationRuleType::Approve,
                keywords: vec![
                    "确认".to_string(),
                    "同意".to_string(),
                    "继续".to_string(),
                    "是的".to_string(),
                    "批准".to_string(),
                ],
                regex: None,
                priority: 10,
            },
            ClassificationRule {
                intent_type: ClassificationRuleType::Reject,
                keywords: vec![
                    "拒绝".to_string(),
                    "不要".to_string(),
                    "停止".to_string(),
                    "取消".to_string(),
                    "不行".to_string(),
                ],
                regex: None,
                priority: 10,
            },
            // 中断
            ClassificationRule {
                intent_type: ClassificationRuleType::Interrupt,
                keywords: vec!["中断".to_string(), "暂停".to_string(), "停一下".to_string()],
                regex: None,
                priority: 9,
            },
            // 状态查询
            ClassificationRule {
                intent_type: ClassificationRuleType::AskStatus,
                keywords: vec![
                    "到哪了".to_string(),
                    "进度".to_string(),
                    "什么状态".to_string(),
                    "怎么样了".to_string(),
                ],
                regex: None,
                priority: 8,
            },
            // 驾驶模式
            ClassificationRule {
                intent_type: ClassificationRuleType::EnableDrivingMode,
                keywords: vec!["驾驶模式".to_string(), "开车模式".to_string()],
                regex: None,
                priority: 7,
            },
            ClassificationRule {
                intent_type: ClassificationRuleType::DisableDrivingMode,
                keywords: vec!["退出驾驶".to_string(), "关闭驾驶".to_string()],
                regex: None,
                priority: 7,
            },
            // 外部 agent 操控
            ClassificationRule {
                intent_type: ClassificationRuleType::InvokeExternalAgent,
                keywords: vec![
                    "claude code".to_string(),
                    "codex".to_string(),
                    "gemini".to_string(),
                ],
                regex: Some(
                    regex::Regex::new(r"(?i)用\s*(claude\s*code|codex|gemini)")
                        .expect("valid regex"),
                ),
                priority: 6,
            },
            // 追加约束
            ClassificationRule {
                intent_type: ClassificationRuleType::Followup,
                keywords: vec!["不要改".to_string(), "别动".to_string(), "约束".to_string()],
                regex: None,
                priority: 5,
            },
            // 执行能力
            ClassificationRule {
                intent_type: ClassificationRuleType::ExecuteCapability,
                keywords: vec![
                    "跑一下".to_string(),
                    "执行".to_string(),
                    "运行".to_string(),
                    "查一下".to_string(),
                    "测试".to_string(),
                    "部署".to_string(),
                ],
                regex: None,
                priority: 4,
            },
        ];
        Self::new(rules)
    }
}

#[async_trait::async_trait]
impl IntentClassifier for RuleBasedClassifier {
    async fn classify(
        &self,
        text: &str,
        context: &ClassificationContext,
    ) -> anyhow::Result<UserIntent> {
        let text_lower = text.to_lowercase();

        // 待确认上下文优先匹配审批意图
        if context.has_pending_confirmation {
            for rule in &self.rules {
                if (rule.intent_type == ClassificationRuleType::Approve
                    || rule.intent_type == ClassificationRuleType::Reject)
                    && rule.keywords.iter().any(|kw| text_lower.contains(kw))
                {
                    return Ok(match rule.intent_type {
                        ClassificationRuleType::Approve => UserIntent::Approve,
                        ClassificationRuleType::Reject => UserIntent::Reject,
                        _ => unreachable!(),
                    });
                }
            }
        }

        // 按优先级排序匹配
        let mut sorted_rules: Vec<&ClassificationRule> = self.rules.iter().collect();
        sorted_rules.sort_by_key(|b| std::cmp::Reverse(b.priority));

        for rule in sorted_rules {
            // 先尝试正则匹配
            if let Some(ref re) = rule.regex {
                if re.is_match(&text_lower) {
                    return Ok(self.build_intent(&rule.intent_type, text));
                }
            }

            // 关键词匹配
            if rule.keywords.iter().any(|kw| text_lower.contains(kw)) {
                return Ok(self.build_intent(&rule.intent_type, text));
            }
        }

        // 如果有活跃的外部 session，当作对外部 agent 的追加输入
        if context.has_active_external_session {
            return Ok(UserIntent::Followup {
                constraint: text.to_string(),
            });
        }

        // 默认：未分类
        Ok(UserIntent::Unclassified {
            raw_text: text.to_string(),
        })
    }
}

impl RuleBasedClassifier {
    fn build_intent(&self, rule_type: &ClassificationRuleType, text: &str) -> UserIntent {
        match rule_type {
            ClassificationRuleType::ExecuteCapability => UserIntent::ExecuteCapability {
                description: text.to_string(),
            },
            ClassificationRuleType::ChatWithAgent => UserIntent::ChatWithAgent {
                message: text.to_string(),
            },
            ClassificationRuleType::InvokeExternalAgent => {
                // 尝试提取 runtime 名称
                let text_lower = text.to_lowercase();
                let runtime = if text_lower.contains("claude code") {
                    Some("claude_code".to_string())
                } else if text_lower.contains("codex") {
                    Some("codex".to_string())
                } else if text_lower.contains("gemini") {
                    Some("gemini".to_string())
                } else {
                    None
                };
                UserIntent::InvokeExternalAgent {
                    runtime,
                    task: text.to_string(),
                }
            }
            ClassificationRuleType::Followup => UserIntent::Followup {
                constraint: text.to_string(),
            },
            ClassificationRuleType::Interrupt => UserIntent::Interrupt,
            ClassificationRuleType::AskStatus => UserIntent::AskStatus,
            ClassificationRuleType::Approve => UserIntent::Approve,
            ClassificationRuleType::Reject => UserIntent::Reject,
            ClassificationRuleType::EnableDrivingMode => UserIntent::EnableDrivingMode,
            ClassificationRuleType::DisableDrivingMode => UserIntent::DisableDrivingMode,
        }
    }
}

// ---------------------------------------------------------------------------
// Mock backends for testing
// ---------------------------------------------------------------------------

/// 测试用 Mock STT 后端
#[derive(Debug)]
pub struct MockSttBackend;

#[async_trait::async_trait]
impl SttBackend for MockSttBackend {
    fn backend_id(&self) -> &str {
        "mock-stt"
    }

    async fn transcribe(
        &self,
        _audio: &[u8],
        _format: AudioFormat,
        language: Option<&str>,
    ) -> anyhow::Result<Transcript> {
        Ok(Transcript {
            text: "mock transcription result".to_string(),
            language: language.map(|l| l.to_string()),
            confidence: 0.95,
            duration_secs: 3.5,
        })
    }
}

/// 测试用 Mock TTS 后端
#[derive(Debug)]
pub struct MockTtsBackend;

#[async_trait::async_trait]
impl TtsBackend for MockTtsBackend {
    fn backend_id(&self) -> &str {
        "mock-tts"
    }

    async fn synthesize(&self, _text: &str, _config: &VoiceConfig) -> anyhow::Result<Vec<u8>> {
        // 返回模拟音频数据
        Ok(vec![0u8; 256])
    }
}

// ---------------------------------------------------------------------------
// VoiceProcessingConnector
// ---------------------------------------------------------------------------

/// 语音处理连接器
pub struct VoiceProcessingConnector {
    id: ConnectorId,
    stt: Arc<dyn SttBackend>,
    tts: Arc<dyn TtsBackend>,
    summarizer: VoiceSafeSummarizer,
    classifier: Arc<dyn IntentClassifier>,
}

impl std::fmt::Debug for VoiceProcessingConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceProcessingConnector")
            .field("id", &self.id)
            .field("stt", &self.stt.backend_id())
            .field("tts", &self.tts.backend_id())
            .finish()
    }
}

impl VoiceProcessingConnector {
    /// 创建新的语音处理连接器
    pub fn new(
        stt: Arc<dyn SttBackend>,
        tts: Arc<dyn TtsBackend>,
        summarizer: VoiceSafeSummarizer,
        classifier: Arc<dyn IntentClassifier>,
    ) -> Self {
        Self {
            id: ConnectorId::from_string("voice-processing".to_string())
                .expect("Failed to create ConnectorId"),
            stt,
            tts,
            summarizer,
            classifier,
        }
    }

    /// 使用 mock 后端创建（用于测试）
    pub fn with_mocks() -> Self {
        Self::new(
            Arc::new(MockSttBackend),
            Arc::new(MockTtsBackend),
            VoiceSafeSummarizer::default(),
            Arc::new(RuleBasedClassifier::with_defaults()),
        )
    }
}

// ---------------------------------------------------------------------------
// Connector impl
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl Connector for VoiceProcessingConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Native
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        static CAPABILITIES: once_cell::sync::Lazy<Vec<CapabilityContract>> =
            once_cell::sync::Lazy::new(|| {
                vec![
                    CapabilityContract {
                        id: "voice.transcribe".to_string(),
                        title: "Voice Transcribe".to_string(),
                        description: Some("语音转文字".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Read],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "audio": {"type": "string", "description": "base64 encoded audio"},
                                "format": {"type": "string"},
                                "language": {"type": "string"}
                            },
                            "required": ["audio", "format"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "transcript": {"type": "object"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "voice.synthesize".to_string(),
                        title: "Voice Synthesize".to_string(),
                        description: Some("文字转语音".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Write],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "text": {"type": "string"},
                                "config": {"type": "object"}
                            },
                            "required": ["text"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "audio": {"type": "string", "description": "base64 encoded audio"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "voice.intent.classify".to_string(),
                        title: "Intent Classify".to_string(),
                        description: Some("意图分类（通用）".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Read],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "text": {"type": "string"},
                                "context": {"type": "object"}
                            },
                            "required": ["text"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "intent": {"type": "object"}
                            }
                        }))
                        .unwrap(),
                    },
                    CapabilityContract {
                        id: "voice.summary.render".to_string(),
                        title: "Voice Summary Render".to_string(),
                        description: Some("执行结果转语音安全摘要".to_string()),
                        risk: RiskLevel::Low,
                        effects: vec![CapabilityEffect::Read],
                        placement: None,
                        timeouts: CapabilityTimeouts::default(),
                        input_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "summary": {"type": "object"}
                            },
                            "required": ["summary"]
                        }))
                        .unwrap(),
                        output_schema: serde_json::to_string(&serde_json::json!({
                            "type": "object",
                            "properties": {
                                "text": {"type": "string"}
                            }
                        }))
                        .unwrap(),
                    },
                ]
            });
        CAPABILITIES.clone()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        let capability_name = request.capability_id.as_str();

        let (output, status, error) = match capability_name {
            "voice.transcribe" => {
                #[derive(Deserialize)]
                struct Input {
                    audio: String,
                    format: AudioFormat,
                    language: Option<String>,
                }
                let input: Input = serde_json::from_value(request.input.clone())?;
                let audio_bytes = base64::engine::general_purpose::STANDARD
                    .decode(&input.audio)
                    .map_err(|e| anyhow::anyhow!("base64 解码失败: {}", e))?;
                match self
                    .stt
                    .transcribe(&audio_bytes, input.format, input.language.as_deref())
                    .await
                {
                    Ok(transcript) => (
                        serde_json::json!({"transcript": transcript}),
                        ExecutionStatus::Success,
                        None,
                    ),
                    Err(e) => (
                        serde_json::Value::Null,
                        ExecutionStatus::Failed,
                        Some(e.to_string()),
                    ),
                }
            }
            "voice.synthesize" => {
                #[derive(Deserialize)]
                struct Input {
                    text: String,
                    config: Option<VoiceConfig>,
                }
                let input: Input = serde_json::from_value(request.input.clone())?;
                let config = input.config.unwrap_or_default();
                match self.tts.synthesize(&input.text, &config).await {
                    Ok(audio) => {
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&audio);
                        (
                            serde_json::json!({"audio": encoded}),
                            ExecutionStatus::Success,
                            None,
                        )
                    }
                    Err(e) => (
                        serde_json::Value::Null,
                        ExecutionStatus::Failed,
                        Some(e.to_string()),
                    ),
                }
            }
            "voice.intent.classify" => {
                #[derive(Deserialize)]
                struct Input {
                    text: String,
                    context: Option<ClassificationContext>,
                }
                let input: Input = serde_json::from_value(request.input.clone())?;
                let ctx = input.context.unwrap_or_default();
                match self.classifier.classify(&input.text, &ctx).await {
                    Ok(intent) => (
                        serde_json::json!({"intent": intent}),
                        ExecutionStatus::Success,
                        None,
                    ),
                    Err(e) => (
                        serde_json::Value::Null,
                        ExecutionStatus::Failed,
                        Some(e.to_string()),
                    ),
                }
            }
            "voice.summary.render" => {
                #[derive(Deserialize)]
                struct Input {
                    summary: VoiceSummary,
                }
                let input: Input = serde_json::from_value(request.input.clone())?;
                let text = self.summarizer.render(&input.summary);
                (
                    serde_json::json!({"text": text}),
                    ExecutionStatus::Success,
                    None,
                )
            }
            other => (
                serde_json::Value::Null,
                ExecutionStatus::Failed,
                Some(format!("未知 capability: {}", other)),
            ),
        };

        Ok(CapabilityExecutionResult {
            execution_id: request.execution_id,
            trace_id: request.trace_id,
            connector_id: self.id.clone(),
            capability_id: request.capability_id,
            output,
            status,
            error,
            actual_runtime: Some(crate::runtime::RuntimeMode::Native),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voice_processing_capabilities() {
        let connector = VoiceProcessingConnector::with_mocks();
        let caps = connector.capabilities();
        assert_eq!(caps.len(), 4);
        assert!(caps.iter().any(|c| c.id == "voice.transcribe"));
        assert!(caps.iter().any(|c| c.id == "voice.synthesize"));
        assert!(caps.iter().any(|c| c.id == "voice.intent.classify"));
        assert!(caps.iter().any(|c| c.id == "voice.summary.render"));
    }

    #[tokio::test]
    async fn test_mock_stt() {
        let stt = MockSttBackend;
        let result = stt
            .transcribe(&[0u8; 10], AudioFormat::Opus, Some("zh-CN"))
            .await
            .unwrap();
        assert!(!result.text.is_empty());
        assert_eq!(result.language.as_deref(), Some("zh-CN"));
        assert!(result.confidence > 0.0);
    }

    #[tokio::test]
    async fn test_mock_tts() {
        let tts = MockTtsBackend;
        let config = VoiceConfig::default();
        let result = tts.synthesize("hello", &config).await.unwrap();
        assert!(!result.is_empty());
    }

    #[test]
    fn test_summarizer_brief_status() {
        let summarizer = VoiceSafeSummarizer::default();
        let summary = VoiceSummary::BriefStatus {
            message: "正在执行".to_string(),
        };
        let text = summarizer.render(&summary);
        assert_eq!(text, "正在执行");
    }

    #[test]
    fn test_summarizer_completion() {
        let summarizer = VoiceSafeSummarizer::default();
        let summary = VoiceSummary::CompletionSummary {
            message: "已完成，修改了 2 个文件".to_string(),
            detail_count: 3,
        };
        let text = summarizer.render(&summary);
        assert!(text.contains("已完成"));
        assert!(text.contains("3 项细节"));
    }

    #[test]
    fn test_summarizer_truncation() {
        let config = SummarizerConfig {
            max_chars: 10,
            ..Default::default()
        };
        let summarizer = VoiceSafeSummarizer::new(config);
        let summary = VoiceSummary::GeneralReply {
            message: "这是一段很长很长很长的文字内容".to_string(),
        };
        let text = summarizer.render(&summary);
        assert!(text.ends_with("..."));
        // 10 chars + "..."
        assert!(text.chars().count() <= 14);
    }

    #[tokio::test]
    async fn test_classify_execute_capability() {
        let classifier = RuleBasedClassifier::with_defaults();
        let ctx = ClassificationContext::default();
        let intent = classifier.classify("跑一下测试", &ctx).await.unwrap();
        assert!(matches!(intent, UserIntent::ExecuteCapability { .. }));
    }

    #[tokio::test]
    async fn test_classify_invoke_external_agent() {
        let classifier = RuleBasedClassifier::with_defaults();
        let ctx = ClassificationContext::default();
        let intent = classifier
            .classify("用 claude code 改一下接口", &ctx)
            .await
            .unwrap();
        match intent {
            UserIntent::InvokeExternalAgent { runtime, .. } => {
                assert_eq!(runtime.as_deref(), Some("claude_code"));
            }
            _ => panic!("Expected InvokeExternalAgent, got {:?}", intent),
        }
    }

    #[tokio::test]
    async fn test_classify_approve_with_context() {
        let classifier = RuleBasedClassifier::with_defaults();
        let ctx = ClassificationContext {
            has_pending_confirmation: true,
            ..Default::default()
        };
        let intent = classifier.classify("确认", &ctx).await.unwrap();
        assert_eq!(intent, UserIntent::Approve);
    }

    #[tokio::test]
    async fn test_classify_unclassified() {
        let classifier = RuleBasedClassifier::with_defaults();
        let ctx = ClassificationContext::default();
        let intent = classifier.classify("今天天气真好", &ctx).await.unwrap();
        assert!(matches!(intent, UserIntent::Unclassified { .. }));
    }
}
