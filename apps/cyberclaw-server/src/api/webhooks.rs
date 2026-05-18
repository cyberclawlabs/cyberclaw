//! Webhooks API - 外部平台 Webhook 接收接口
//!
//! 接收来自外部平台（Slack、Discord、GitHub 等）的 webhook 载荷，
//! 通过 `NormalizedMessage` 标准化后路由到对应 Agent。

use axum::body::Bytes;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::post,
    Json, Router,
};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tracing::{info, warn};

use cyberclaw_connectors::{
    AudioFormat, CapabilityExecutionResult, ImMessage, ImMessageType, ReplyMode, SessionBinding,
    UserIntent, VoiceSummary,
};
use cyberclaw_core::ids::TenantId;

use crate::error::ApiError;
use crate::state::AppState;

type HmacSha256 = Hmac<Sha256>;

/// Webhook 接收确认响应
#[derive(Debug, Serialize)]
pub struct WebhookReceiptResponse {
    /// 是否成功接收
    pub received: bool,
    /// 标准化后的消息 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// 来源平台
    pub platform: String,
    /// 附加信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<String>,
}

/// 创建 Webhooks API 路由
///
/// Webhook 路由不需要 JWT 认证，但会校验 webhook 签名（如提供）。
pub fn create_webhooks_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/webhooks/:platform", post(receive_webhook))
        .route("/webhooks/im/:platform", post(receive_im_webhook))
}

/// IM Webhook 接收确认响应
#[derive(Debug, Serialize)]
pub struct ImWebhookReceiptResponse {
    /// 是否成功接收
    pub received: bool,
    /// 来源平台
    pub platform: String,
    /// 标准化消息 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// 会话 chat_id
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    /// 识别意图
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handled_intent: Option<String>,
    /// 附加信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<String>,
}

/// POST /webhooks/:platform - 接收外部 webhook 载荷
///
/// 将入站 webhook 载荷通过 `MessageGatewayConnector` 标准化为 `NormalizedMessage`，
/// 并路由到对应的 Agent。
///
/// 如果请求头中包含 `X-Webhook-Signature`，会尝试校验签名。
async fn receive_webhook(
    State(state): State<Arc<AppState>>,
    Path(platform): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<WebhookReceiptResponse>, ApiError> {
    info!(platform = %platform, "Received webhook");
    let current_env = current_environment();

    // 尝试从请求头中提取签名
    let signature = headers
        .get("X-Webhook-Signature")
        .or_else(|| headers.get("x-hub-signature-256"))
        .or_else(|| headers.get("x-signature"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // 校验 webhook 签名 — 对 raw bytes 验签，防止 JSON 重序列化导致签名绕过
    if let Some(secret) = state.webhook_secrets.get(&platform) {
        // 平台配置了签名密钥 — 必须验证
        match &signature {
            Some(sig) => {
                if !validate_webhook_hmac(secret, &body, sig) {
                    warn!(platform = %platform, "Webhook signature validation failed");
                    return Err(ApiError::Unauthorized(
                        "Invalid webhook signature".to_string(),
                    ));
                }
                info!(platform = %platform, "Webhook signature validated successfully");
            }
            None => {
                warn!(platform = %platform, "Webhook signature missing but secret configured");
                return Err(ApiError::Unauthorized(
                    "Missing webhook signature".to_string(),
                ));
            }
        }
    } else {
        // P1-1 fix: 未配置 secret 的平台，生产环境一律拒绝
        if current_env != "development" && current_env != "test" {
            warn!(
                platform = %platform,
                "Webhook rejected: no secret configured for platform in production"
            );
            return Err(ApiError::Unauthorized(
                "Webhook secret not configured for this platform".to_string(),
            ));
        }
        if signature.is_some() {
            warn!(
                platform = %platform,
                "Webhook signature provided but no secret configured; skipping validation (dev mode)"
            );
        }
    }

    // 验签通过后再解析 JSON — 确保解析只发生在签名验证之后
    let payload: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| ApiError::InvalidRequest(format!("Invalid JSON payload: {}", e)))?;

    // 通过 ConnectorRegistry 查找 message-gateway connector 并尝试标准化消息
    // 使用 CapabilityDispatcher 的 gateway.receive 能力
    let receive_input = serde_json::json!({
        "platform": platform,
        "payload": payload,
    });

    // 构造 CapabilityExecutionRequest 并通过 dispatcher 执行
    let capability_id =
        cyberclaw_core::ids::CapabilityId::from_string("gateway.receive".to_string())
            .map_err(|e| ApiError::InternalError(format!("Invalid capability ID: {}", e)))?;

    let connector_id = cyberclaw_core::ids::ConnectorId::from_string("message-gateway".to_string())
        .map_err(|e| ApiError::InternalError(format!("Invalid connector ID: {}", e)))?;

    let execution_id =
        cyberclaw_core::ids::ExecutionId::from_string(format!("webhook-{}", uuid::Uuid::new_v4()))
            .map_err(|e| ApiError::InternalError(format!("Invalid execution ID: {}", e)))?;

    // P1 fix: use Connector actor type with platform-specific ID instead of System,
    // so tenant-scoped governance rules (quota, policy matching) apply correctly
    let actor_id = cyberclaw_core::ids::ActorId::from_string(format!("webhook-{}", platform))
        .map_err(|e| ApiError::InternalError(format!("Invalid actor ID: {}", e)))?;

    let workspace_id = cyberclaw_core::ids::WorkspaceId::from_string("default".to_string())
        .map_err(|e| ApiError::InternalError(format!("Invalid workspace ID: {}", e)))?;
    let tenant_id =
        resolve_webhook_tenant_binding(&platform, state.webhook_tenants.as_ref(), &current_env)?;

    let request = cyberclaw_connectors::CapabilityExecutionRequest {
        execution_id,
        trace_id: format!("webhook-trace-{}", uuid::Uuid::new_v4()),
        connector_id,
        capability_id,
        input: receive_input,
        actor: cyberclaw_core::identity::ActorRef {
            id: actor_id,
            actor_type: cyberclaw_core::identity::ActorType::Connector,
            tenant_id,
            home_node_id: None,
            display_name: format!("Webhook:{}", platform),
        },
        workspace: cyberclaw_core::workspace::WorkspaceRef {
            id: workspace_id,
            mode: cyberclaw_core::workspace::WorkspaceMode::Ephemeral,
            materialization_mode: None,
            home_node_id: None,
            backing_store: None,
            root: "/tmp/webhook".to_string(),
            writable_roots: vec![],
        },
    };

    // P0-2 fix: route through PolicyEngine before dispatch.
    // Build EvaluationContext and check governance before executing.
    {
        use cyberclaw_core::capability::{CapabilityRef, RiskLevel};
        use cyberclaw_governance::engine::EvaluationContext;
        use cyberclaw_governance::GovernanceDecision;

        let (risk, effects, placement) = state
            .connector_registry
            .get_capability(&request.capability_id)
            .map(|(_conn_id, contract)| (contract.risk, contract.effects, contract.placement))
            .unwrap_or_else(|| (RiskLevel::Medium, vec![], None));

        let eval_context = EvaluationContext {
            capability: CapabilityRef {
                id: request.capability_id.clone(),
                connector_id: request.connector_id.clone(),
                risk,
                effects,
                placement,
            },
            actor: request.actor.clone(),
            execution_id: request.execution_id.clone(),
            reason: Some(format!("webhook from platform: {}", platform)),
        };

        let eval_result = state
            .policy_engine
            .evaluate_capability(eval_context)
            .await
            .map_err(|e| {
                warn!(platform = %platform, error = %e, "Policy evaluation failed for webhook");
                ApiError::InternalError("Governance evaluation failed".to_string())
            })?;

        match eval_result.decision {
            GovernanceDecision::Allow { .. } => {
                info!(platform = %platform, "Webhook capability governance check passed");
            }
            GovernanceDecision::Deny { reason } => {
                warn!(platform = %platform, reason = %reason, "Webhook capability denied by governance");
                return Err(ApiError::Unauthorized(
                    "Webhook action denied by governance policy".to_string(),
                ));
            }
            GovernanceDecision::ReviewRequired { reason, .. } => {
                warn!(platform = %platform, reason = %reason, "Webhook capability requires review");
                return Err(ApiError::Unauthorized(
                    "Webhook action requires approval".to_string(),
                ));
            }
        }
    }

    match state.capability_dispatcher.dispatch(request).await {
        Ok(result) => {
            let message_id = result
                .output
                .get("id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            info!(
                platform = %platform,
                message_id = ?message_id,
                "Webhook processed successfully"
            );

            Ok(Json(WebhookReceiptResponse {
                received: true,
                message_id,
                platform,
                info: None,
            }))
        }
        Err(e) => {
            warn!(
                platform = %platform,
                error = %e,
                "Webhook processing failed, returning receipt anyway"
            );

            // P2-1 fix: 返回通用消息，不泄漏内部错误细节
            // 即使处理失败也返回 200，避免外部平台重试风暴
            Ok(Json(WebhookReceiptResponse {
                received: true,
                message_id: None,
                platform,
                info: Some("Processing deferred".to_string()),
            }))
        }
    }
}

/// POST /webhooks/im/:platform - 接收 IM webhook，支持语音转写和外部 Agent 远程控制
async fn receive_im_webhook(
    State(state): State<Arc<AppState>>,
    Path(platform): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ImWebhookReceiptResponse>, ApiError> {
    info!(platform = %platform, "Received IM webhook");

    let signature = headers
        .get("X-Webhook-Signature")
        .or_else(|| headers.get("x-hub-signature-256"))
        .or_else(|| headers.get("x-signature"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // 复用现有 webhook 验签策略
    if let Some(secret) = state.webhook_secrets.get(&platform) {
        match &signature {
            Some(sig) => {
                if !validate_webhook_hmac(secret, &body, sig) {
                    warn!(platform = %platform, "IM webhook signature validation failed");
                    return Err(ApiError::Unauthorized(
                        "Invalid webhook signature".to_string(),
                    ));
                }
            }
            None => {
                warn!(
                    platform = %platform,
                    "IM webhook signature missing but secret configured"
                );
                return Err(ApiError::Unauthorized(
                    "Missing webhook signature".to_string(),
                ));
            }
        }
    } else {
        let current_env = std::env::var("ENVIRONMENT")
            .unwrap_or_else(|_| "production".to_string())
            .to_lowercase();
        if current_env != "development" && current_env != "test" {
            return Err(ApiError::Unauthorized(
                "Webhook secret not configured for this platform".to_string(),
            ));
        }
    }

    let payload: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| ApiError::InvalidRequest(format!("Invalid JSON payload: {}", e)))?;

    // 1) 标准化入站消息
    let receive_result = execute_capability_with_governance(
        state.clone(),
        &platform,
        "im-channel",
        "channel.message.receive",
        serde_json::json!({ "platform": platform.clone(), "payload": payload }),
        "Receive IM inbound message",
    )
    .await?;

    if receive_result
        .output
        .get("duplicate")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Ok(Json(ImWebhookReceiptResponse {
            received: true,
            platform: platform.clone(),
            message_id: None,
            chat_id: None,
            handled_intent: None,
            info: Some("Duplicate message ignored".to_string()),
        }));
    }

    let message: ImMessage =
        serde_json::from_value(receive_result.output.clone()).map_err(|e| {
            ApiError::InternalError(format!("Failed to parse normalized IM message: {}", e))
        })?;
    let message_id = message.id.clone();
    let chat_id = message.chat_id.clone();
    let message_platform = message.platform.clone();
    let inbound_message_type = message.message_type.clone();

    // 2) 读取会话绑定状态
    let status_result = execute_capability_with_governance(
        state.clone(),
        &platform,
        "im-channel",
        "channel.session.status",
        serde_json::json!({ "chat_id": chat_id.clone() }),
        "Inspect IM session binding",
    )
    .await?;
    let existing_binding: Option<SessionBinding> = serde_json::from_value(
        status_result
            .output
            .get("binding")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
    .map_err(|e| ApiError::InternalError(format!("Failed to parse IM session binding: {}", e)))?;

    // 3) 提取用户文本（文字直接取 text_content，语音先下载再 STT）
    let mut user_text = message.text_content.clone().unwrap_or_default();
    if user_text.trim().is_empty()
        && matches!(
            message.message_type,
            ImMessageType::Voice | ImMessageType::Audio
        )
    {
        if let Some(media_id) = message.voice_media_id.clone() {
            let audio_result = execute_capability_with_governance(
                state.clone(),
                &platform,
                "im-channel",
                "channel.voice.receive",
                serde_json::json!({
                    "platform": message_platform.clone(),
                    "media_id": media_id,
                }),
                "Download IM voice media",
            )
            .await?;
            let audio_b64 = audio_result
                .output
                .get("audio")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ApiError::InternalError("Voice media missing audio payload".to_string())
                })?;
            let transcribe_result = execute_capability_with_governance(
                state.clone(),
                &platform,
                "voice-processing",
                "voice.transcribe",
                serde_json::json!({
                    "audio": audio_b64,
                    "format": default_audio_format_for_platform(&message.platform),
                    "language": "zh-CN",
                }),
                "Transcribe inbound voice",
            )
            .await?;
            user_text = transcribe_result
                .output
                .get("transcript")
                .and_then(|v| v.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
        }
    }

    if user_text.trim().is_empty() {
        let _ = execute_capability_with_governance(
            state.clone(),
            &platform,
            "im-channel",
            "channel.message.send",
            serde_json::json!({
                "platform": message_platform.clone(),
                "chat_id": chat_id.clone(),
                "text": "未识别到可处理的文字或语音内容，请重试。",
            }),
            "Send empty-input hint",
        )
        .await;
        return Ok(Json(ImWebhookReceiptResponse {
            received: true,
            platform: platform.clone(),
            message_id: Some(message_id.clone()),
            chat_id: Some(chat_id.clone()),
            handled_intent: None,
            info: Some("Empty input".to_string()),
        }));
    }

    // 4) 意图识别
    let default_reply_mode = existing_binding
        .as_ref()
        .map(|b| b.reply_mode.clone())
        .unwrap_or(ReplyMode::FollowUser);
    let has_active_external_session = existing_binding
        .as_ref()
        .and_then(|b| b.external_session_id.as_ref())
        .is_some();
    let classify_result = execute_capability_with_governance(
        state.clone(),
        &platform,
        "voice-processing",
        "voice.intent.classify",
        serde_json::json!({
            "text": user_text,
            "context": {
                "has_active_external_session": has_active_external_session,
                "has_pending_confirmation": false,
                "reply_mode": default_reply_mode.clone(),
            }
        }),
        "Classify IM user intent",
    )
    .await?;
    let intent: UserIntent = serde_json::from_value(
        classify_result
            .output
            .get("intent")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
    .map_err(|e| ApiError::InternalError(format!("Failed to parse user intent: {}", e)))?;

    let mut reply_mode = default_reply_mode;
    let mut external_session_id = existing_binding
        .as_ref()
        .and_then(|b| b.external_session_id.clone());
    let response_text: String;
    let intent_name = match intent {
        UserIntent::InvokeExternalAgent { runtime, task } => {
            let runtime_id = runtime
                .as_deref()
                .map(normalize_external_runtime)
                .unwrap_or("claude_code");
            if external_session_id.is_none() {
                let spawn = execute_capability_with_governance(
                    state.clone(),
                    &platform,
                    "acp-runtime",
                    "external_agent.session.spawn",
                    serde_json::json!({
                        "runtime": runtime_id,
                    }),
                    "Spawn external coding agent session",
                )
                .await?;
                external_session_id = spawn
                    .output
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
            if let Some(session_id) = external_session_id.clone() {
                execute_capability_with_governance(
                    state.clone(),
                    &platform,
                    "acp-runtime",
                    "external_agent.prompt.send",
                    serde_json::json!({
                        "session_id": session_id,
                        "prompt": task,
                    }),
                    "Send initial task to external coding agent",
                )
                .await?;
                response_text = format!("任务已发送到 {} 会话。", runtime_id);
            } else {
                response_text = "外部会话创建失败，请重试。".to_string();
            }
            "InvokeExternalAgent"
        }
        UserIntent::Followup { constraint } => {
            if let Some(session_id) = external_session_id.clone() {
                execute_capability_with_governance(
                    state.clone(),
                    &platform,
                    "acp-runtime",
                    "external_agent.prompt.followup",
                    serde_json::json!({
                        "session_id": session_id,
                        "message": constraint,
                    }),
                    "Send follow-up instruction",
                )
                .await?;
                response_text = "已追加约束到当前会话。".to_string();
            } else {
                response_text = "当前没有活跃外部会话，请先说“用 Claude Code ...”。".to_string();
            }
            "Followup"
        }
        UserIntent::AskStatus => {
            if let Some(session_id) = external_session_id.clone() {
                let status = execute_capability_with_governance(
                    state.clone(),
                    &platform,
                    "acp-runtime",
                    "external_agent.session.status",
                    serde_json::json!({
                        "session_id": session_id,
                    }),
                    "Query external session status",
                )
                .await?;
                response_text = format!("当前会话状态：{}", status.output);
            } else {
                response_text = "当前没有活跃会话。".to_string();
            }
            "AskStatus"
        }
        UserIntent::Interrupt => {
            if let Some(session_id) = external_session_id.clone() {
                execute_capability_with_governance(
                    state.clone(),
                    &platform,
                    "acp-runtime",
                    "external_agent.prompt.interrupt",
                    serde_json::json!({
                        "session_id": session_id,
                        "reason": "voice interrupt",
                    }),
                    "Interrupt external session",
                )
                .await?;
                response_text = "已中断当前会话。".to_string();
            } else {
                response_text = "当前没有可中断的会话。".to_string();
            }
            "Interrupt"
        }
        UserIntent::EnableDrivingMode => {
            reply_mode = ReplyMode::DrivingMode;
            response_text = "已开启驾驶模式：将优先语音回复并收敛输出内容。".to_string();
            "EnableDrivingMode"
        }
        UserIntent::DisableDrivingMode => {
            reply_mode = ReplyMode::FollowUser;
            response_text = "已退出驾驶模式。".to_string();
            "DisableDrivingMode"
        }
        UserIntent::Approve => {
            response_text = "已确认。".to_string();
            "Approve"
        }
        UserIntent::Reject => {
            response_text = "已拒绝。".to_string();
            "Reject"
        }
        UserIntent::ExecuteCapability { description } => {
            response_text = format!(
                "已收到执行请求：{}。当前 MVP 仅打通外部编码代理控制链路。",
                description
            );
            "ExecuteCapability"
        }
        UserIntent::ChatWithAgent { message } => {
            response_text = format!(
                "已收到对话内容：{}。当前 MVP 仅打通外部编码代理控制链路。",
                message
            );
            "ChatWithAgent"
        }
        UserIntent::Unclassified { raw_text } => {
            response_text = format!(
                "未识别意图：{}。你可以说“用 Claude Code 改 ...”。",
                raw_text
            );
            "Unclassified"
        }
    };

    // 5) 更新 chat 绑定状态
    let session_id = existing_binding
        .as_ref()
        .map(|b| b.session_id.clone())
        .unwrap_or_else(|| format!("im-{}", uuid::Uuid::new_v4()));
    let execution_id = existing_binding
        .as_ref()
        .and_then(|b| b.execution_id.clone());
    let _ = execute_capability_with_governance(
        state.clone(),
        &platform,
        "im-channel",
        "channel.session.bind",
        serde_json::json!({
            "chat_id": chat_id.clone(),
            "platform": message_platform.clone(),
            "session_id": session_id,
            "execution_id": execution_id,
            "external_session_id": external_session_id,
            "reply_mode": reply_mode,
        }),
        "Bind IM chat session state",
    )
    .await;

    // 6) 回复（驾驶模式/语音消息优先语音回包）
    let prefer_voice = match reply_mode {
        ReplyMode::VoiceOnly | ReplyMode::DrivingMode => true,
        ReplyMode::TextOnly => false,
        ReplyMode::FollowUser => {
            matches!(
                inbound_message_type,
                ImMessageType::Voice | ImMessageType::Audio
            )
        }
    };

    if prefer_voice {
        let rendered = execute_capability_with_governance(
            state.clone(),
            &platform,
            "voice-processing",
            "voice.summary.render",
            serde_json::json!({
                "summary": VoiceSummary::GeneralReply {
                    message: response_text.clone()
                }
            }),
            "Render voice-safe summary",
        )
        .await;
        let spoken_text = match rendered {
            Ok(result) => result
                .output
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or(response_text.as_str())
                .to_string(),
            Err(_) => response_text.clone(),
        };
        let synth = execute_capability_with_governance(
            state.clone(),
            &platform,
            "voice-processing",
            "voice.synthesize",
            serde_json::json!({
                "text": spoken_text,
                "config": {
                    "language": "zh-CN",
                    "output_format": AudioFormat::Opus,
                }
            }),
            "Synthesize voice reply",
        )
        .await;
        if let Ok(synthesized) = synth {
            if let Some(audio_b64) = synthesized.output.get("audio").and_then(|v| v.as_str()) {
                let _ = execute_capability_with_governance(
                    state.clone(),
                    &platform,
                    "im-channel",
                    "channel.voice.send",
                    serde_json::json!({
                        "platform": message_platform.clone(),
                        "chat_id": chat_id.clone(),
                        "audio": audio_b64,
                        "format": AudioFormat::Opus,
                    }),
                    "Send voice reply",
                )
                .await;
            }
        } else {
            let _ = execute_capability_with_governance(
                state.clone(),
                &platform,
                "im-channel",
                "channel.message.send",
                serde_json::json!({
                    "platform": message_platform.clone(),
                    "chat_id": chat_id.clone(),
                    "text": response_text,
                }),
                "Fallback text reply",
            )
            .await;
        }
    } else {
        let _ = execute_capability_with_governance(
            state.clone(),
            &platform,
            "im-channel",
            "channel.message.send",
            serde_json::json!({
                "platform": message_platform.clone(),
                "chat_id": chat_id.clone(),
                "text": response_text,
            }),
            "Send text reply",
        )
        .await;
    }

    Ok(Json(ImWebhookReceiptResponse {
        received: true,
        platform,
        message_id: Some(message_id),
        chat_id: Some(chat_id),
        handled_intent: Some(intent_name.to_string()),
        info: None,
    }))
}

fn normalize_external_runtime(input: &str) -> &str {
    match input {
        "claude" | "claude_code" | "claude-code" => "claude_code",
        "codex" => "codex",
        "gemini" | "gemini_cli" | "gemini-cli" => "gemini_cli",
        "opencode" => "opencode",
        _ => "claude_code",
    }
}

fn default_audio_format_for_platform(platform: &str) -> AudioFormat {
    match platform {
        "telegram" => AudioFormat::Ogg,
        "lark" | "feishu" => AudioFormat::Opus,
        _ => AudioFormat::Opus,
    }
}

async fn execute_capability_with_governance(
    state: Arc<AppState>,
    platform: &str,
    connector_id_str: &str,
    capability_id_str: &str,
    input: serde_json::Value,
    reason: &str,
) -> Result<CapabilityExecutionResult, ApiError> {
    use cyberclaw_core::capability::{CapabilityRef, RiskLevel};
    use cyberclaw_governance::engine::EvaluationContext;
    use cyberclaw_governance::GovernanceDecision;

    let capability_id =
        cyberclaw_core::ids::CapabilityId::from_string(capability_id_str.to_string())
            .map_err(|e| ApiError::InternalError(format!("Invalid capability ID: {}", e)))?;
    let connector_id = cyberclaw_core::ids::ConnectorId::from_string(connector_id_str.to_string())
        .map_err(|e| ApiError::InternalError(format!("Invalid connector ID: {}", e)))?;
    let execution_id = cyberclaw_core::ids::ExecutionId::from_string(format!(
        "im-webhook-{}",
        uuid::Uuid::new_v4()
    ))
    .map_err(|e| ApiError::InternalError(format!("Invalid execution ID: {}", e)))?;
    let actor_id = cyberclaw_core::ids::ActorId::from_string(format!(
        "im-webhook-{}",
        platform.to_lowercase()
    ))
    .map_err(|e| ApiError::InternalError(format!("Invalid actor ID: {}", e)))?;
    let workspace_id = cyberclaw_core::ids::WorkspaceId::from_string("default".to_string())
        .map_err(|e| ApiError::InternalError(format!("Invalid workspace ID: {}", e)))?;
    let tenant_id = resolve_webhook_tenant_binding(
        platform,
        state.webhook_tenants.as_ref(),
        &current_environment(),
    )?;

    let request = cyberclaw_connectors::CapabilityExecutionRequest {
        execution_id: execution_id.clone(),
        trace_id: format!("im-webhook-trace-{}", uuid::Uuid::new_v4()),
        connector_id: connector_id.clone(),
        capability_id: capability_id.clone(),
        input,
        actor: cyberclaw_core::identity::ActorRef {
            id: actor_id,
            actor_type: cyberclaw_core::identity::ActorType::Connector,
            tenant_id,
            home_node_id: None,
            display_name: format!("IMWebhook:{}", platform),
        },
        workspace: cyberclaw_core::workspace::WorkspaceRef {
            id: workspace_id,
            mode: cyberclaw_core::workspace::WorkspaceMode::Ephemeral,
            materialization_mode: None,
            home_node_id: None,
            backing_store: None,
            root: "/tmp/webhook/im".to_string(),
            writable_roots: vec![],
        },
    };

    let (risk, effects, placement) = state
        .connector_registry
        .get_capability(&request.capability_id)
        .map(|(_conn_id, contract)| (contract.risk, contract.effects, contract.placement))
        .unwrap_or_else(|| (RiskLevel::Medium, vec![], None));

    let eval_context = EvaluationContext {
        capability: CapabilityRef {
            id: request.capability_id.clone(),
            connector_id: request.connector_id.clone(),
            risk,
            effects,
            placement,
        },
        actor: request.actor.clone(),
        execution_id: request.execution_id.clone(),
        reason: Some(reason.to_string()),
    };

    let eval_result = state
        .policy_engine
        .evaluate_capability(eval_context)
        .await
        .map_err(|e| ApiError::InternalError(format!("Governance evaluation failed: {}", e)))?;
    match eval_result.decision {
        GovernanceDecision::Allow { .. } => {}
        GovernanceDecision::Deny { reason } => {
            return Err(ApiError::Unauthorized(format!(
                "Capability denied by governance: {}",
                reason
            )));
        }
        GovernanceDecision::ReviewRequired { reason, .. } => {
            return Err(ApiError::Unauthorized(format!(
                "Capability requires approval: {}",
                reason
            )));
        }
    }

    let result = state
        .capability_dispatcher
        .dispatch(request)
        .await
        .map_err(|e| ApiError::InternalError(format!("Capability dispatch failed: {}", e)))?;
    if matches!(result.status, cyberclaw_connectors::ExecutionStatus::Failed) {
        let message = result
            .error
            .clone()
            .unwrap_or_else(|| "Unknown connector error".to_string());
        return Err(ApiError::InternalError(message));
    }
    Ok(result)
}

fn current_environment() -> String {
    std::env::var("ENVIRONMENT")
        .unwrap_or_else(|_| "production".to_string())
        .to_lowercase()
}

fn resolve_webhook_tenant_binding(
    platform: &str,
    bindings: &HashMap<String, TenantId>,
    environment: &str,
) -> Result<Option<TenantId>, ApiError> {
    if let Some(tenant_id) = bindings.get(&platform.to_lowercase()) {
        return Ok(Some(tenant_id.clone()));
    }

    if environment == "development" || environment == "test" {
        return Ok(None);
    }

    Err(ApiError::Unauthorized(
        "Webhook tenant not configured for this platform".to_string(),
    ))
}

/// 使用 HMAC-SHA256 校验 webhook 签名（常数时间比较）
fn validate_webhook_hmac(secret: &str, payload: &[u8], signature: &str) -> bool {
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(payload);
    let expected = hex::encode(mac.finalize().into_bytes());
    // Constant-time comparison to prevent timing side-channel attacks
    let result: bool = expected.as_bytes().ct_eq(signature.as_bytes()).into();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_receipt_response_serialization() {
        let response = WebhookReceiptResponse {
            received: true,
            message_id: Some("msg-001".to_string()),
            platform: "github".to_string(),
            info: None,
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["received"], true);
        assert_eq!(json["message_id"], "msg-001");
        assert_eq!(json["platform"], "github");
        // info should be absent when None
        assert!(json.get("info").is_none());
    }

    #[test]
    fn test_validate_webhook_hmac_valid() {
        let secret = "test-secret";
        let payload = b"test payload";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(payload);
        let sig = hex::encode(mac.finalize().into_bytes());
        assert!(validate_webhook_hmac(secret, payload, &sig));
    }

    #[test]
    fn test_validate_webhook_hmac_invalid() {
        assert!(!validate_webhook_hmac(
            "secret",
            b"payload",
            "bad-signature"
        ));
    }

    #[test]
    fn test_webhook_receipt_response_with_info() {
        let response = WebhookReceiptResponse {
            received: true,
            message_id: None,
            platform: "slack".to_string(),
            info: Some("Processing deferred".to_string()),
        };
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["received"], true);
        assert!(json.get("message_id").is_none());
        assert_eq!(json["info"], "Processing deferred");
    }

    #[test]
    fn test_resolve_webhook_tenant_binding_rejects_missing_binding_in_production() {
        let bindings = std::collections::HashMap::new();

        let result = resolve_webhook_tenant_binding("telegram", &bindings, "production");

        assert!(matches!(result, Err(ApiError::Unauthorized(_))));
    }

    #[test]
    fn test_resolve_webhook_tenant_binding_allows_missing_binding_in_test() {
        let bindings = std::collections::HashMap::new();

        let result = resolve_webhook_tenant_binding("telegram", &bindings, "test").unwrap();

        assert!(result.is_none());
    }
}
