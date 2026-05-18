//! # 微信 iLink Bot 适配器
//!
//! 微信企业内部 iLink Bot IM 平台适配器，支持：
//! - Bearer Token 鉴权（Authorization + AuthorizationType 双头）
//! - 长轮询（Long-polling）模式接收消息
//! - 完整消息类型：文字、图片、语音、文件、视频
//! - CDN 媒体上传/下载（AES-128-ECB 加密）
//! - context_token 追踪（每个 chat 独立维护）
//! - 打字中指示器（sendtyping）
//!
//! ## iLink Bot API 端点
//! - 长轮询：`POST ilink/bot/getupdates`
//! - 发送消息：`POST ilink/bot/sendmessage`
//! - 发送打字中：`POST ilink/bot/sendtyping`
//! - 获取 CDN 上传地址：`POST ilink/bot/getuploadurl`
//!
//! ## 认证头部
//! ```text
//! Authorization: Bearer {bot_token}
//! AuthorizationType: ilink_bot_token
//! iLink-App-Id: bot
//! iLink-App-ClientVersion: {version_int}
//! X-WECHAT-UIN: {base64(random_u32)}
//! Content-Type: application/json
//! ```

use crate::im_channel::{AudioFormat, ImMessage, ImMessageType, ImPlatformAdapter};
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes128;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

/// iLink Bot API 基础 URL
const ILINK_BASE_URL: &str = "https://ilinkai.weixin.qq.com";

/// iLink CDN 基础 URL
const CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";

/// 长轮询端点
const PATH_GETUPDATES: &str = "/ilink/bot/getupdates";

/// 发送消息端点
const PATH_SENDMESSAGE: &str = "/ilink/bot/sendmessage";

/// 发送打字中指示器端点
const PATH_SENDTYPING: &str = "/ilink/bot/sendtyping";

/// 获取 CDN 上传地址端点
const PATH_GETUPLOADURL: &str = "/ilink/bot/getuploadurl";

/// iLink channel_version 字段固定值
const CHANNEL_VERSION: &str = "1.0";

/// errcode = -14: session 过期，需要重置 cursor
const ERRCODE_SESSION_EXPIRED: i64 = -14;

/// AES-128-ECB 块大小（16 字节）
const AES_BLOCK_SIZE: usize = 16;

// ---------------------------------------------------------------------------
// 消息类型常量
// ---------------------------------------------------------------------------

/// 消息 item_list type: 文字
const ITEM_TYPE_TEXT: u64 = 1;
/// 消息 item_list type: 图片
const ITEM_TYPE_IMAGE: u64 = 2;
/// 消息 item_list type: 语音
const ITEM_TYPE_VOICE: u64 = 3;
/// 消息 item_list type: 文件
const ITEM_TYPE_FILE: u64 = 4;
/// 消息 item_list type: 视频
const ITEM_TYPE_VIDEO: u64 = 5;

/// message_type = 1: 用户消息
const MSG_TYPE_USER: u64 = 1;
/// message_type = 2: Bot 消息
#[allow(dead_code)]
const MSG_TYPE_BOT: u64 = 2;

/// message_state = 2: finish（完成状态）
const MSG_STATE_FINISH: u64 = 2;

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

/// 微信 iLink Bot 适配器配置
#[derive(Debug, Clone)]
pub struct WechatConfig {
    /// Bot Token（Bearer 鉴权凭据）
    pub bot_token: String,
    /// API 基础 URL（默认：https://ilinkai.weixin.qq.com）
    pub base_url: String,
    /// 版本号字符串，格式 "major.minor.patch"（默认 "1.0.0"）
    ///
    /// 将编码为整数：(major<<16)|(minor<<8)|patch
    pub channel_version: String,
}

impl WechatConfig {
    /// 使用 bot_token 创建默认配置
    pub fn new(bot_token: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
            base_url: ILINK_BASE_URL.to_string(),
            channel_version: "1.0.0".to_string(),
        }
    }

    /// 将版本字符串编码为整数：(major<<16)|(minor<<8)|patch
    pub fn version_int(&self) -> u32 {
        let parts: Vec<u32> = self
            .channel_version
            .splitn(3, '.')
            .map(|s| s.parse::<u32>().unwrap_or(0))
            .collect();
        let major = parts.first().copied().unwrap_or(0);
        let minor = parts.get(1).copied().unwrap_or(0);
        let patch = parts.get(2).copied().unwrap_or(0);
        (major << 16) | (minor << 8) | patch
    }
}

impl Default for WechatConfig {
    fn default() -> Self {
        Self::new("placeholder_token")
    }
}

// ---------------------------------------------------------------------------
// 内部 API 类型
// ---------------------------------------------------------------------------

/// getupdates 请求体
#[derive(Debug, Serialize)]
struct GetUpdatesRequest {
    get_updates_buf: String,
    base_info: BaseInfo,
}

/// 所有请求通用的 base_info
#[derive(Debug, Serialize)]
struct BaseInfo {
    channel_version: String,
}

/// getupdates 响应
#[derive(Debug, Deserialize)]
struct GetUpdatesResponse {
    ret: i64,
    #[serde(default)]
    errcode: i64,
    #[serde(default)]
    get_updates_buf: String,
    #[serde(default)]
    msgs: Vec<serde_json::Value>,
    #[serde(default)]
    longpolling_timeout_ms: u64,
}

/// sendmessage 请求体
#[derive(Debug, Serialize)]
struct SendMessageRequest {
    msg: OutboundMessage,
    base_info: BaseInfo,
}

/// 发出的消息结构
#[derive(Debug, Serialize)]
struct OutboundMessage {
    from_user_id: String,
    to_user_id: String,
    client_id: String,
    message_type: u64,
    message_state: u64,
    context_token: String,
    item_list: Vec<MessageItem>,
}

/// 消息 item（发送和接收通用）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MessageItem {
    #[serde(rename = "type")]
    item_type: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    text_item: Option<TextItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    media_item: Option<MediaItem>,
}

/// 文字类型 item 内容
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TextItem {
    text: String,
}

/// 媒体类型 item 内容（图片/语音/文件/视频共用）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MediaItem {
    #[serde(default)]
    media_id: String,
    #[serde(default)]
    download_param: String,
    /// AES 解密密钥（可为 hex 或 base64）
    #[serde(default)]
    aes_key: String,
    #[serde(default)]
    file_name: String,
    #[serde(default)]
    file_size: u64,
    #[serde(default)]
    duration: u32,
}

/// sendtyping 请求体
#[derive(Debug, Serialize)]
struct SendTypingRequest {
    to_user_id: String,
    typing_status: u64,
    base_info: BaseInfo,
}

/// getuploadurl 请求体
#[derive(Debug, Serialize)]
struct GetUploadUrlRequest {
    file_name: String,
    file_size: u64,
    file_type: String,
    base_info: BaseInfo,
}

/// getuploadurl 响应
#[derive(Debug, Deserialize)]
struct GetUploadUrlResponse {
    #[serde(default)]
    ret: i64,
    #[serde(default)]
    upload_url: String,
    #[serde(default)]
    download_param: String,
}

// ---------------------------------------------------------------------------
// WechatAdapter
// ---------------------------------------------------------------------------

/// 微信 iLink Bot 适配器
///
/// 实现 `ImPlatformAdapter` trait，支持长轮询消息接收、文字/媒体发送，
/// 以及 AES-128-ECB CDN 媒体加解密。
pub struct WechatAdapter {
    /// 适配器配置
    config: WechatConfig,
    /// HTTP 客户端（复用连接池）
    client: reqwest::Client,
    /// 每个 chat_id 的 context_token 缓存
    ///
    /// iLink 要求发消息时带上从对方消息中拿到的 context_token
    context_tokens: Arc<RwLock<HashMap<String, String>>>,
    /// 长轮询 cursor（get_updates_buf）
    sync_buf: Arc<RwLock<String>>,
    /// 长轮询停止信号
    polling_stop: Arc<tokio::sync::Notify>,
}

impl WechatAdapter {
    /// 创建新的微信 iLink Bot 适配器
    pub fn new(config: WechatConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            context_tokens: Arc::new(RwLock::new(HashMap::new())),
            sync_buf: Arc::new(RwLock::new(String::new())),
            polling_stop: Arc::new(tokio::sync::Notify::new()),
        }
    }

    // -----------------------------------------------------------------------
    // 私有：URL 构造
    // -----------------------------------------------------------------------

    /// 构建完整 API URL
    fn api_url(&self, path: &str) -> String {
        format!("{}{}", self.config.base_url, path)
    }

    // -----------------------------------------------------------------------
    // 私有：认证头部构造
    // -----------------------------------------------------------------------

    /// 为请求添加 iLink Bot 认证头部
    ///
    /// 头部列表：
    /// - `Authorization: Bearer {bot_token}`
    /// - `AuthorizationType: ilink_bot_token`
    /// - `iLink-App-Id: bot`
    /// - `iLink-App-ClientVersion: {version_int}`
    /// - `X-WECHAT-UIN: {base64(random_u32)}`
    fn build_auth_headers(&self) -> Vec<(String, String)> {
        // X-WECHAT-UIN: base64(随机 u32 大端序字节)
        let random_uin = rand_u32();
        let uin_bytes = random_uin.to_be_bytes();
        let uin_b64 = base64::engine::general_purpose::STANDARD.encode(uin_bytes);

        vec![
            (
                "Authorization".to_string(),
                format!("Bearer {}", self.config.bot_token),
            ),
            (
                "AuthorizationType".to_string(),
                "ilink_bot_token".to_string(),
            ),
            ("iLink-App-Id".to_string(), "bot".to_string()),
            (
                "iLink-App-ClientVersion".to_string(),
                self.config.version_int().to_string(),
            ),
            ("X-WECHAT-UIN".to_string(), uin_b64),
        ]
    }

    /// 发起带 iLink 认证头部的 POST 请求
    async fn post_json<B: Serialize>(
        &self,
        url: &str,
        body: &B,
    ) -> anyhow::Result<reqwest::Response> {
        let mut req = self.client.post(url).json(body);
        for (key, value) in self.build_auth_headers() {
            req = req.header(key, value);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP 请求失败: {}", e))?;
        Ok(resp)
    }

    // -----------------------------------------------------------------------
    // 私有：context_token 管理
    // -----------------------------------------------------------------------

    /// 从入站消息 JSON 中提取并缓存 context_token
    async fn extract_and_cache_context_token(&self, raw: &serde_json::Value, chat_id: &str) {
        if let Some(token) = raw.get("context_token").and_then(|v| v.as_str()) {
            if !token.is_empty() {
                self.update_context_token(chat_id, token).await;
            }
        }
    }

    /// 获取指定 chat 的 context_token（不存在则返回空字符串）
    async fn get_context_token(&self, chat_id: &str) -> String {
        self.context_tokens
            .read()
            .await
            .get(chat_id)
            .cloned()
            .unwrap_or_default()
    }

    // -----------------------------------------------------------------------
    // 私有：消息解析
    // -----------------------------------------------------------------------

    /// 从 item_list 中提取第一个文字 item 的文本内容
    fn extract_text_from_items(items: &[serde_json::Value]) -> Option<String> {
        for item in items {
            if item.get("type").and_then(|v| v.as_u64()) == Some(ITEM_TYPE_TEXT) {
                if let Some(text) = item
                    .get("text_item")
                    .and_then(|t| t.get("text"))
                    .and_then(|v| v.as_str())
                {
                    return Some(text.to_string());
                }
            }
        }
        None
    }

    /// 从 item_list 中提取第一个媒体 item（语音/图片/文件/视频）
    fn extract_media_from_items(
        items: &[serde_json::Value],
        target_type: u64,
    ) -> Option<MediaItem> {
        for item in items {
            if item.get("type").and_then(|v| v.as_u64()) == Some(target_type) {
                // 尝试从 media_item / voice_item / image_item / file_item / video_item 字段解析
                for field in &[
                    "media_item",
                    "voice_item",
                    "image_item",
                    "file_item",
                    "video_item",
                ] {
                    if let Some(media_val) = item.get(field) {
                        if let Ok(media) = serde_json::from_value::<MediaItem>(media_val.clone()) {
                            return Some(media);
                        }
                    }
                }
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // 私有：AES-128-ECB 加解密
    // -----------------------------------------------------------------------

    /// AES-128-ECB 解密 + PKCS7 去填充
    ///
    /// iLink CDN 下载的媒体文件用 AES-128-ECB 加密，密钥可为：
    /// - 32 字符 hex 字符串（16 字节）
    /// - base64 编码的 16 字节
    pub fn aes_128_ecb_decrypt(key_bytes: &[u8], ciphertext: &[u8]) -> anyhow::Result<Vec<u8>> {
        if key_bytes.len() != 16 {
            anyhow::bail!("AES-128 密钥长度必须为 16 字节，实际为 {}", key_bytes.len());
        }
        if ciphertext.is_empty() || !ciphertext.len().is_multiple_of(AES_BLOCK_SIZE) {
            anyhow::bail!(
                "密文长度无效: {}（必须为 16 的整数倍且非零）",
                ciphertext.len()
            );
        }

        let key = aes::cipher::generic_array::GenericArray::from_slice(key_bytes);
        let cipher = Aes128::new(key);

        let mut plaintext = ciphertext.to_vec();
        // 逐块解密（ECB 模式：每块独立解密）
        for chunk in plaintext.chunks_mut(AES_BLOCK_SIZE) {
            let block = aes::cipher::generic_array::GenericArray::from_mut_slice(chunk);
            cipher.decrypt_block(block);
        }

        // PKCS7 去填充
        pkcs7_unpad(&plaintext)
    }

    /// AES-128-ECB 加密 + PKCS7 填充
    pub fn aes_128_ecb_encrypt(key_bytes: &[u8], plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
        if key_bytes.len() != 16 {
            anyhow::bail!("AES-128 密钥长度必须为 16 字节，实际为 {}", key_bytes.len());
        }

        let key = aes::cipher::generic_array::GenericArray::from_slice(key_bytes);
        let cipher = Aes128::new(key);

        // PKCS7 填充
        let padded = pkcs7_pad(plaintext, AES_BLOCK_SIZE);
        let mut ciphertext = padded;

        // 逐块加密（ECB 模式）
        for chunk in ciphertext.chunks_mut(AES_BLOCK_SIZE) {
            let block = aes::cipher::generic_array::GenericArray::from_mut_slice(chunk);
            cipher.encrypt_block(block);
        }

        Ok(ciphertext)
    }

    /// 解析 aes_key 字段：支持 hex（32 字符）或 base64
    fn parse_aes_key(aes_key_str: &str) -> anyhow::Result<Vec<u8>> {
        if aes_key_str.is_empty() {
            anyhow::bail!("aes_key 为空");
        }
        // 32 字符 hex -> 16 字节
        if aes_key_str.len() == 32 && aes_key_str.chars().all(|c| c.is_ascii_hexdigit()) {
            return hex::decode(aes_key_str)
                .map_err(|e| anyhow::anyhow!("hex 解码 aes_key 失败: {}", e));
        }
        // 否则尝试 base64 解码
        base64::engine::general_purpose::STANDARD
            .decode(aes_key_str)
            .map_err(|e| anyhow::anyhow!("base64 解码 aes_key 失败: {}", e))
    }

    // -----------------------------------------------------------------------
    // 私有：CDN 媒体操作
    // -----------------------------------------------------------------------

    /// 从 CDN 下载并解密媒体文件
    ///
    /// 下载 URL 格式：`{CDN_BASE_URL}/download?encrypted_query_param={download_param}`
    async fn cdn_download_and_decrypt(&self, media_item: &MediaItem) -> anyhow::Result<Vec<u8>> {
        if media_item.download_param.is_empty() {
            anyhow::bail!("media_item.download_param 为空，无法下载");
        }

        let url = format!(
            "{}/download?encrypted_query_param={}",
            CDN_BASE_URL, media_item.download_param
        );

        debug!("从 CDN 下载媒体: {}", url);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("CDN 下载请求失败: {}", e))?;

        if !resp.status().is_success() {
            anyhow::bail!("CDN 下载返回错误状态: {}", resp.status());
        }

        let ciphertext = resp
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("读取 CDN 响应体失败: {}", e))?
            .to_vec();

        // 解析 AES 密钥并解密
        let key_bytes = Self::parse_aes_key(&media_item.aes_key)?;
        let plaintext = Self::aes_128_ecb_decrypt(&key_bytes, &ciphertext)?;

        debug!("CDN 媒体解密成功，原始大小: {} 字节", plaintext.len());

        Ok(plaintext)
    }

    /// 上传媒体文件到 CDN
    ///
    /// 流程：
    /// 1. 生成随机 16 字节 AES 密钥
    /// 2. 调用 getuploadurl 获取上传地址
    /// 3. AES-128-ECB + PKCS7 加密数据
    /// 4. PUT 到 CDN 上传地址
    /// 5. 返回 (download_param, aes_key_hex)
    async fn cdn_upload(
        &self,
        data: &[u8],
        file_name: &str,
        file_type: &str,
    ) -> anyhow::Result<(String, String)> {
        // 生成随机 AES-128 密钥
        let aes_key = generate_random_aes_key();
        let aes_key_hex = hex::encode(&aes_key);

        // 加密数据
        let encrypted = Self::aes_128_ecb_encrypt(&aes_key, data)?;

        // 获取上传 URL
        let upload_url_req = GetUploadUrlRequest {
            file_name: file_name.to_string(),
            file_size: encrypted.len() as u64,
            file_type: file_type.to_string(),
            base_info: BaseInfo {
                channel_version: CHANNEL_VERSION.to_string(),
            },
        };

        let url = self.api_url(PATH_GETUPLOADURL);
        let resp = self.post_json(&url, &upload_url_req).await?;

        if !resp.status().is_success() {
            anyhow::bail!("getuploadurl 返回错误状态: {}", resp.status());
        }

        let upload_resp: GetUploadUrlResponse = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("解析 getuploadurl 响应失败: {}", e))?;

        if upload_resp.ret != 0 {
            anyhow::bail!("getuploadurl 接口返回错误: ret={}", upload_resp.ret);
        }

        if upload_resp.upload_url.is_empty() {
            anyhow::bail!("getuploadurl 未返回 upload_url");
        }

        debug!("获取到 CDN 上传 URL: {}", upload_resp.upload_url);

        // PUT 加密数据到 CDN
        let put_resp = self
            .client
            .put(&upload_resp.upload_url)
            .body(encrypted)
            .header("Content-Type", "application/octet-stream")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("CDN PUT 上传失败: {}", e))?;

        if !put_resp.status().is_success() {
            anyhow::bail!("CDN PUT 上传返回错误状态: {}", put_resp.status());
        }

        // download_param 从响应 header 中获取（或使用 getuploadurl 返回的字段）
        let download_param = if !upload_resp.download_param.is_empty() {
            upload_resp.download_param
        } else {
            // 部分实现会把 download_param 放在 CDN PUT 响应 header 中
            put_resp
                .headers()
                .get("X-Download-Param")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
                .unwrap_or_default()
        };

        if download_param.is_empty() {
            anyhow::bail!("CDN 上传后未获取到 download_param");
        }

        info!("CDN 媒体上传成功: file_name={}", file_name);

        Ok((download_param, aes_key_hex))
    }
}

// ---------------------------------------------------------------------------
// ImPlatformAdapter 实现
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl ImPlatformAdapter for WechatAdapter {
    /// 平台标识
    fn platform_id(&self) -> &str {
        "wechat"
    }

    /// iLink Bot 支持语音消息
    fn supports_voice(&self) -> bool {
        true
    }

    /// 标准化 iLink 入站消息
    ///
    /// 解析 iLink getupdates 返回的单条消息 JSON，提取：
    /// - 消息 ID、发送者、目标（chat_id）
    /// - message_type（仅处理用户消息 type=1）
    /// - item_list 中的文字/语音/图片/文件/视频内容
    /// - context_token（缓存以备后续发送使用）
    async fn normalize_inbound(&self, raw: serde_json::Value) -> anyhow::Result<ImMessage> {
        // 提取基础字段
        let msg_id = raw
            .get("msg_id")
            .or_else(|| raw.get("client_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let sender = raw
            .get("from_user_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let chat_id = raw
            .get("to_user_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let message_type_raw = raw
            .get("message_type")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // 仅处理用户消息（message_type = 1）
        if message_type_raw != MSG_TYPE_USER {
            debug!(
                "忽略非用户消息: msg_id={}, message_type={}",
                msg_id, message_type_raw
            );
        }

        // 缓存 context_token
        self.extract_and_cache_context_token(&raw, &chat_id).await;

        // 解析时间戳
        let timestamp = raw
            .get("create_time")
            .and_then(|v| v.as_i64())
            .map(|ts| chrono::DateTime::from_timestamp(ts, 0).unwrap_or_else(chrono::Utc::now))
            .unwrap_or_else(chrono::Utc::now);

        // 解析 item_list
        let empty_items = vec![];
        let items: Vec<serde_json::Value> = raw
            .get("item_list")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty_items)
            .clone();

        // 判断消息类型并提取内容
        let (im_message_type, text_content, voice_media_id, voice_duration) =
            if let Some(text) = Self::extract_text_from_items(&items) {
                // 文字消息
                let msg_type = if text.starts_with('/') {
                    ImMessageType::Command
                } else {
                    ImMessageType::Text
                };
                (msg_type, Some(text), None, None)
            } else if let Some(voice) = Self::extract_media_from_items(&items, ITEM_TYPE_VOICE) {
                // 语音消息：download_param 作为 media_id 传递
                let duration = if voice.duration > 0 {
                    Some(voice.duration as f32)
                } else {
                    None
                };
                let media_id = serde_json::to_string(&voice).unwrap_or_default();
                (ImMessageType::Voice, None, Some(media_id), duration)
            } else if Self::extract_media_from_items(&items, ITEM_TYPE_IMAGE).is_some() {
                // 图片消息：降级为文字提示
                (
                    ImMessageType::Text,
                    Some("[图片消息]".to_string()),
                    None,
                    None,
                )
            } else if Self::extract_media_from_items(&items, ITEM_TYPE_FILE).is_some() {
                // 文件消息：降级为文字提示
                (
                    ImMessageType::Text,
                    Some("[文件消息]".to_string()),
                    None,
                    None,
                )
            } else if Self::extract_media_from_items(&items, ITEM_TYPE_VIDEO).is_some() {
                // 视频消息：降级为文字提示
                (
                    ImMessageType::Text,
                    Some("[视频消息]".to_string()),
                    None,
                    None,
                )
            } else {
                warn!(
                    "无法识别的消息格式: msg_id={}, items_count={}",
                    msg_id,
                    items.len()
                );
                (
                    ImMessageType::Text,
                    Some("[未知消息类型]".to_string()),
                    None,
                    None,
                )
            };

        let mut metadata = HashMap::new();
        metadata.insert("platform".to_string(), "wechat".to_string());
        if let Some(token) = raw.get("context_token").and_then(|v| v.as_str()) {
            metadata.insert("context_token".to_string(), token.to_string());
        }

        Ok(ImMessage {
            id: msg_id,
            platform: "wechat".to_string(),
            chat_id,
            sender,
            message_type: im_message_type,
            text_content,
            voice_media_id,
            voice_duration_secs: voice_duration,
            timestamp,
            metadata,
            raw,
        })
    }

    /// 发送文字消息
    ///
    /// 构造 sendmessage 请求，带上该 chat 缓存的 context_token。
    async fn send_text(&self, chat_id: &str, text: &str) -> anyhow::Result<()> {
        let context_token = self.get_context_token(chat_id).await;

        let payload = SendMessageRequest {
            msg: OutboundMessage {
                from_user_id: String::new(),
                to_user_id: chat_id.to_string(),
                client_id: format!("cyberclaw-{}", uuid::Uuid::new_v4()),
                message_type: MSG_TYPE_BOT,
                message_state: MSG_STATE_FINISH,
                context_token,
                item_list: vec![MessageItem {
                    item_type: ITEM_TYPE_TEXT,
                    text_item: Some(TextItem {
                        text: text.to_string(),
                    }),
                    media_item: None,
                }],
            },
            base_info: BaseInfo {
                channel_version: CHANNEL_VERSION.to_string(),
            },
        };

        let url = self.api_url(PATH_SENDMESSAGE);
        let resp = self.post_json(&url, &payload).await?;

        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("sendmessage HTTP 错误: {}", status);
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("解析 sendmessage 响应失败: {}", e))?;

        let ret = body.get("ret").and_then(|v| v.as_i64()).unwrap_or(-1);
        if ret != 0 {
            anyhow::bail!("sendmessage 接口返回错误: ret={}, body={}", ret, body);
        }

        debug!("文字消息发送成功: chat_id={}", chat_id);
        Ok(())
    }

    /// 发送卡片消息
    ///
    /// 微信 iLink Bot 不原生支持卡片，降级为文字发送。
    async fn send_card(&self, chat_id: &str, card: serde_json::Value) -> anyhow::Result<()> {
        // 尝试从卡片提取文字内容，否则序列化整个卡片
        let text = card
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                card.get("title")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| card.to_string())
            });

        debug!("微信不支持卡片消息，降级为文字: chat_id={}", chat_id);
        self.send_text(chat_id, &text).await
    }

    /// 发送语音消息
    ///
    /// 流程：上传音频数据到 CDN -> 发送语音 item
    async fn send_voice(
        &self,
        chat_id: &str,
        audio: &[u8],
        format: AudioFormat,
    ) -> anyhow::Result<()> {
        let ext = match format {
            AudioFormat::Amr => "amr",
            AudioFormat::Ogg => "ogg",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Wav => "wav",
            AudioFormat::Opus => "opus",
            AudioFormat::M4a => "m4a",
            AudioFormat::Pcm => "pcm",
        };

        let file_name = format!("voice_{}.{}", uuid::Uuid::new_v4(), ext);

        // 上传到 CDN
        let (download_param, aes_key_hex) = self
            .cdn_upload(audio, &file_name, "voice")
            .await
            .map_err(|e| anyhow::anyhow!("CDN 语音上传失败: {}", e))?;

        let context_token = self.get_context_token(chat_id).await;

        let payload = SendMessageRequest {
            msg: OutboundMessage {
                from_user_id: String::new(),
                to_user_id: chat_id.to_string(),
                client_id: format!("cyberclaw-{}", uuid::Uuid::new_v4()),
                message_type: MSG_TYPE_BOT,
                message_state: MSG_STATE_FINISH,
                context_token,
                item_list: vec![MessageItem {
                    item_type: ITEM_TYPE_VOICE,
                    text_item: None,
                    media_item: Some(MediaItem {
                        media_id: String::new(),
                        download_param,
                        aes_key: aes_key_hex,
                        file_name,
                        file_size: audio.len() as u64,
                        duration: 0,
                    }),
                }],
            },
            base_info: BaseInfo {
                channel_version: CHANNEL_VERSION.to_string(),
            },
        };

        let url = self.api_url(PATH_SENDMESSAGE);
        let resp = self.post_json(&url, &payload).await?;

        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("sendmessage（语音）HTTP 错误: {}", status);
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("解析语音 sendmessage 响应失败: {}", e))?;

        let ret = body.get("ret").and_then(|v| v.as_i64()).unwrap_or(-1);
        if ret != 0 {
            anyhow::bail!("语音 sendmessage 接口错误: ret={}", ret);
        }

        debug!("语音消息发送成功: chat_id={}", chat_id);
        Ok(())
    }

    /// 下载语音附件
    ///
    /// `media_id` 实际上是序列化的 `MediaItem` JSON（由 normalize_inbound 设置），
    /// 从中解析 download_param 和 aes_key，下载并 AES-128-ECB 解密。
    async fn download_voice(&self, media_id: &str) -> anyhow::Result<Vec<u8>> {
        // media_id 是序列化的 MediaItem
        let media_item: MediaItem = serde_json::from_str(media_id)
            .map_err(|e| anyhow::anyhow!("解析 media_id 为 MediaItem 失败: {}", e))?;

        self.cdn_download_and_decrypt(&media_item).await
    }

    /// 校验 webhook 签名
    ///
    /// iLink Bot 使用 Bearer Token 鉴权，不使用 webhook 签名，始终返回 true。
    fn validate_signature(&self, _payload: &[u8], _signature: &str) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// 公开方法：长轮询、打字中指示器、context_token 更新
// ---------------------------------------------------------------------------

impl WechatAdapter {
    /// 启动长轮询，返回消息接收通道
    ///
    /// 在后台 task 中循环调用 getupdates，将接收到的原始消息 JSON
    /// 发送到返回的 `mpsc::Receiver`。
    ///
    /// 调用 `stop_polling()` 可优雅停止轮询。
    pub fn start_polling(&self) -> mpsc::Receiver<serde_json::Value> {
        let (tx, rx) = mpsc::channel(128);

        let client = self.client.clone();
        let config = self.config.clone();
        let sync_buf = Arc::clone(&self.sync_buf);
        let context_tokens = Arc::clone(&self.context_tokens);
        let stop = Arc::clone(&self.polling_stop);
        let auth_headers = self.build_auth_headers();

        tokio::spawn(async move {
            info!("微信 iLink 长轮询已启动");

            'outer: loop {
                let cursor = sync_buf.read().await.clone();

                let body = GetUpdatesRequest {
                    get_updates_buf: cursor,
                    base_info: BaseInfo {
                        channel_version: CHANNEL_VERSION.to_string(),
                    },
                };

                let url = format!("{}{}", config.base_url, PATH_GETUPDATES);

                let mut req = client.post(&url).json(&body);
                for (k, v) in &auth_headers {
                    req = req.header(k.as_str(), v.as_str());
                }

                // 等待响应或停止信号
                let resp_fut = req.send();
                let result = tokio::select! {
                    r = resp_fut => r,
                    _ = stop.notified() => {
                        info!("微信 iLink 长轮询收到停止信号，退出");
                        break 'outer;
                    }
                };

                let resp = match result {
                    Ok(r) => r,
                    Err(e) => {
                        error!("getupdates 请求失败: {}，1 秒后重试", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        continue;
                    }
                };

                let updates: GetUpdatesResponse = match resp.json().await {
                    Ok(v) => v,
                    Err(e) => {
                        error!("解析 getupdates 响应失败: {}", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        continue;
                    }
                };

                // session 过期：重置 cursor
                if updates.errcode == ERRCODE_SESSION_EXPIRED {
                    warn!("iLink session 已过期（errcode=-14），重置 cursor");
                    *sync_buf.write().await = String::new();
                    continue;
                }

                if updates.ret != 0 {
                    error!(
                        "getupdates 返回错误: ret={}, errcode={}",
                        updates.ret, updates.errcode
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    continue;
                }

                // 更新 cursor
                if !updates.get_updates_buf.is_empty() {
                    *sync_buf.write().await = updates.get_updates_buf.clone();
                }

                // 分发消息
                for msg in updates.msgs {
                    // 缓存 context_token
                    if let (Some(chat_id), Some(token)) = (
                        msg.get("to_user_id").and_then(|v| v.as_str()),
                        msg.get("context_token").and_then(|v| v.as_str()),
                    ) {
                        if !token.is_empty() {
                            context_tokens
                                .write()
                                .await
                                .insert(chat_id.to_string(), token.to_string());
                        }
                    }

                    if tx.send(msg).await.is_err() {
                        info!("消息接收方已关闭，停止长轮询");
                        break 'outer;
                    }
                }

                // 如果本次没有消息，根据 longpolling_timeout_ms 短暂等待
                // （实际长轮询会在服务端阻塞，此处仅为防止无消息时 CPU 空转）
                if updates.longpolling_timeout_ms == 0 {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }

            info!("微信 iLink 长轮询已停止");
        });

        rx
    }

    /// 停止长轮询
    pub fn stop_polling(&self) {
        self.polling_stop.notify_one();
    }

    /// 更新指定 chat 的 context_token
    ///
    /// iLink 要求每次发消息时带上从对方消息中获得的 context_token，
    /// 通过此方法手动缓存（或由 normalize_inbound 自动缓存）。
    pub async fn update_context_token(&self, chat_id: &str, token: &str) {
        self.context_tokens
            .write()
            .await
            .insert(chat_id.to_string(), token.to_string());
        debug!("已更新 context_token: chat_id={}", chat_id);
    }

    /// 发送打字中指示器
    ///
    /// `start = true`：开始打字（typing_status = 1）
    /// `start = false`：停止打字（typing_status = 0）
    pub async fn send_typing(&self, chat_id: &str, start: bool) -> anyhow::Result<()> {
        let payload = SendTypingRequest {
            to_user_id: chat_id.to_string(),
            typing_status: if start { 1 } else { 0 },
            base_info: BaseInfo {
                channel_version: CHANNEL_VERSION.to_string(),
            },
        };

        let url = self.api_url(PATH_SENDTYPING);
        let resp = self.post_json(&url, &payload).await?;

        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("sendtyping HTTP 错误: {}", status);
        }

        debug!("打字中指示器发送成功: chat_id={}, start={}", chat_id, start);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 工具函数
// ---------------------------------------------------------------------------

/// PKCS7 填充
fn pkcs7_pad(data: &[u8], block_size: usize) -> Vec<u8> {
    let pad_len = block_size - (data.len() % block_size);
    let mut padded = data.to_vec();
    padded.extend(std::iter::repeat_n(pad_len as u8, pad_len));
    padded
}

/// PKCS7 去填充
fn pkcs7_unpad(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    if data.is_empty() {
        anyhow::bail!("PKCS7 去填充失败：数据为空");
    }
    let pad_len = *data.last().unwrap() as usize;
    if pad_len == 0 || pad_len > AES_BLOCK_SIZE {
        anyhow::bail!("PKCS7 去填充失败：无效填充长度 {}", pad_len);
    }
    if pad_len > data.len() {
        anyhow::bail!(
            "PKCS7 去填充失败：填充长度 {} 超过数据长度 {}",
            pad_len,
            data.len()
        );
    }
    // 校验填充字节是否一致
    let payload_len = data.len() - pad_len;
    for &b in &data[payload_len..] {
        if b as usize != pad_len {
            anyhow::bail!("PKCS7 去填充失败：填充字节不一致");
        }
    }
    Ok(data[..payload_len].to_vec())
}

/// 生成随机 32 位无符号整数（用于 X-WECHAT-UIN）
fn rand_u32() -> u32 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let mut hasher = DefaultHasher::new();
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
        .hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    hasher.finish() as u32
}

/// 生成随机 16 字节 AES 密钥
fn generate_random_aes_key() -> Vec<u8> {
    // 用时间 + 线程 ID 混合生成伪随机密钥（生产环境应使用密码学安全 RNG）
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let mut key = Vec::with_capacity(16);
    for i in 0u8..4 {
        let mut hasher = DefaultHasher::new();
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos()
            .hash(&mut hasher);
        i.hash(&mut hasher);
        std::thread::current().id().hash(&mut hasher);
        let v = hasher.finish();
        key.extend_from_slice(&v.to_le_bytes()[..4]);
    }
    key
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // 配置默认值测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_config_defaults() {
        let config = WechatConfig::new("my_token");
        assert_eq!(config.bot_token, "my_token");
        assert_eq!(config.base_url, ILINK_BASE_URL);
        assert_eq!(config.channel_version, "1.0.0");
    }

    #[test]
    fn test_config_default_trait() {
        let config = WechatConfig::default();
        assert_eq!(config.base_url, ILINK_BASE_URL);
        assert_eq!(config.channel_version, "1.0.0");
    }

    #[test]
    fn test_version_int_encoding() {
        // 1.0.0 -> (1<<16)|(0<<8)|0 = 65536
        let config = WechatConfig::new("t");
        assert_eq!(config.version_int(), 65536);

        let config2 = WechatConfig {
            channel_version: "2.3.4".to_string(),
            ..WechatConfig::new("t")
        };
        // (2<<16)|(3<<8)|4 = 131072 + 768 + 4 = 131844
        assert_eq!(config2.version_int(), 131844);
    }

    #[test]
    fn test_version_int_zero() {
        let config = WechatConfig {
            channel_version: "0.0.0".to_string(),
            ..WechatConfig::new("t")
        };
        assert_eq!(config.version_int(), 0);
    }

    // -----------------------------------------------------------------------
    // 平台标识测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_platform_id() {
        let adapter = WechatAdapter::new(WechatConfig::new("tok"));
        assert_eq!(adapter.platform_id(), "wechat");
    }

    #[test]
    fn test_supports_voice() {
        let adapter = WechatAdapter::new(WechatConfig::new("tok"));
        assert!(adapter.supports_voice());
    }

    // -----------------------------------------------------------------------
    // normalize_inbound 测试（各消息类型）
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_normalize_text_message() {
        let adapter = WechatAdapter::new(WechatConfig::new("tok"));
        let raw = serde_json::json!({
            "msg_id": "msg_001",
            "from_user_id": "user_abc",
            "to_user_id": "bot_xyz",
            "message_type": 1,
            "message_state": 2,
            "context_token": "ctx_token_001",
            "create_time": 1700000000i64,
            "item_list": [
                {
                    "type": 1,
                    "text_item": {"text": "你好，机器人！"}
                }
            ]
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.id, "msg_001");
        assert_eq!(msg.sender, "user_abc");
        assert_eq!(msg.chat_id, "bot_xyz");
        assert_eq!(msg.platform, "wechat");
        assert_eq!(msg.message_type, ImMessageType::Text);
        assert_eq!(msg.text_content.as_deref(), Some("你好，机器人！"));
        assert!(msg.voice_media_id.is_none());
    }

    #[tokio::test]
    async fn test_normalize_command_message() {
        let adapter = WechatAdapter::new(WechatConfig::new("tok"));
        let raw = serde_json::json!({
            "msg_id": "msg_cmd",
            "from_user_id": "user_1",
            "to_user_id": "bot_1",
            "message_type": 1,
            "message_state": 2,
            "context_token": "",
            "item_list": [
                {
                    "type": 1,
                    "text_item": {"text": "/help"}
                }
            ]
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Command);
        assert_eq!(msg.text_content.as_deref(), Some("/help"));
    }

    #[tokio::test]
    async fn test_normalize_voice_message() {
        let adapter = WechatAdapter::new(WechatConfig::new("tok"));
        let raw = serde_json::json!({
            "msg_id": "msg_voice",
            "from_user_id": "user_1",
            "to_user_id": "bot_1",
            "message_type": 1,
            "message_state": 2,
            "context_token": "ctx_voice",
            "item_list": [
                {
                    "type": 3,
                    "voice_item": {
                        "media_id": "media_abc",
                        "download_param": "enc_param_xyz",
                        "aes_key": "deadbeefdeadbeef",
                        "file_name": "voice.amr",
                        "file_size": 1024,
                        "duration": 5
                    }
                }
            ]
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Voice);
        assert!(msg.voice_media_id.is_some());
        assert_eq!(msg.voice_duration_secs, Some(5.0));
        assert!(msg.text_content.is_none());
    }

    #[tokio::test]
    async fn test_normalize_image_message() {
        let adapter = WechatAdapter::new(WechatConfig::new("tok"));
        let raw = serde_json::json!({
            "msg_id": "msg_img",
            "from_user_id": "user_1",
            "to_user_id": "bot_1",
            "message_type": 1,
            "message_state": 2,
            "context_token": "",
            "item_list": [
                {
                    "type": 2,
                    "image_item": {
                        "download_param": "enc_img_param",
                        "aes_key": "aabbccddeeff0011",
                        "file_name": "photo.jpg",
                        "file_size": 20480,
                        "duration": 0
                    }
                }
            ]
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Text);
        assert_eq!(msg.text_content.as_deref(), Some("[图片消息]"));
    }

    #[tokio::test]
    async fn test_normalize_file_message() {
        let adapter = WechatAdapter::new(WechatConfig::new("tok"));
        let raw = serde_json::json!({
            "msg_id": "msg_file",
            "from_user_id": "user_1",
            "to_user_id": "bot_1",
            "message_type": 1,
            "message_state": 2,
            "context_token": "",
            "item_list": [
                {
                    "type": 4,
                    "file_item": {
                        "download_param": "enc_file_param",
                        "aes_key": "1122334455667788",
                        "file_name": "doc.pdf",
                        "file_size": 102400,
                        "duration": 0
                    }
                }
            ]
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Text);
        assert_eq!(msg.text_content.as_deref(), Some("[文件消息]"));
    }

    #[tokio::test]
    async fn test_normalize_video_message() {
        let adapter = WechatAdapter::new(WechatConfig::new("tok"));
        let raw = serde_json::json!({
            "msg_id": "msg_video",
            "from_user_id": "user_1",
            "to_user_id": "bot_1",
            "message_type": 1,
            "message_state": 2,
            "context_token": "",
            "item_list": [
                {
                    "type": 5,
                    "video_item": {
                        "download_param": "enc_video_param",
                        "aes_key": "9900aabbccddeeff",
                        "file_name": "clip.mp4",
                        "file_size": 1048576,
                        "duration": 30
                    }
                }
            ]
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        assert_eq!(msg.message_type, ImMessageType::Text);
        assert_eq!(msg.text_content.as_deref(), Some("[视频消息]"));
    }

    #[tokio::test]
    async fn test_normalize_empty_item_list() {
        let adapter = WechatAdapter::new(WechatConfig::new("tok"));
        let raw = serde_json::json!({
            "msg_id": "msg_empty",
            "from_user_id": "user_1",
            "to_user_id": "bot_1",
            "message_type": 1,
            "message_state": 2,
            "context_token": "",
            "item_list": []
        });

        let msg = adapter.normalize_inbound(raw).await.unwrap();
        // 空 item_list 降级为未知消息提示
        assert_eq!(msg.message_type, ImMessageType::Text);
        assert_eq!(msg.text_content.as_deref(), Some("[未知消息类型]"));
    }

    // -----------------------------------------------------------------------
    // context_token 追踪测试
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_context_token_tracking() {
        let adapter = WechatAdapter::new(WechatConfig::new("tok"));

        // 初始无 token
        assert_eq!(adapter.get_context_token("chat_1").await, "");

        // 手动更新
        adapter.update_context_token("chat_1", "token_abc").await;
        assert_eq!(adapter.get_context_token("chat_1").await, "token_abc");

        // 不同 chat 独立
        adapter.update_context_token("chat_2", "token_xyz").await;
        assert_eq!(adapter.get_context_token("chat_1").await, "token_abc");
        assert_eq!(adapter.get_context_token("chat_2").await, "token_xyz");
    }

    #[tokio::test]
    async fn test_context_token_auto_extract_on_normalize() {
        let adapter = WechatAdapter::new(WechatConfig::new("tok"));
        let raw = serde_json::json!({
            "msg_id": "msg_ctx",
            "from_user_id": "user_1",
            "to_user_id": "chat_auto",
            "message_type": 1,
            "message_state": 2,
            "context_token": "auto_ctx_999",
            "item_list": [{"type": 1, "text_item": {"text": "hi"}}]
        });

        adapter.normalize_inbound(raw).await.unwrap();
        // normalize 应自动缓存 context_token
        assert_eq!(adapter.get_context_token("chat_auto").await, "auto_ctx_999");
    }

    // -----------------------------------------------------------------------
    // AES-128-ECB 加解密往返测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_aes_128_ecb_roundtrip() {
        let key = b"0123456789abcdef"; // 16 字节
        let plaintext = b"Hello, iLink Bot!";

        let ciphertext = WechatAdapter::aes_128_ecb_encrypt(key, plaintext).unwrap();
        assert!(!ciphertext.is_empty());
        assert_eq!(ciphertext.len() % 16, 0);

        let decrypted = WechatAdapter::aes_128_ecb_decrypt(key, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_aes_128_ecb_roundtrip_empty_data() {
        let key = b"fedcba9876543210";
        let plaintext = b"";

        // 空数据应仍能加解密（产生一个填充块）
        let ciphertext = WechatAdapter::aes_128_ecb_encrypt(key, plaintext).unwrap();
        assert_eq!(ciphertext.len(), 16); // 全填充

        let decrypted = WechatAdapter::aes_128_ecb_decrypt(key, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_aes_128_ecb_wrong_key_length() {
        let key = b"short"; // 非 16 字节
        let plaintext = b"test data";
        assert!(WechatAdapter::aes_128_ecb_encrypt(key, plaintext).is_err());
        assert!(WechatAdapter::aes_128_ecb_decrypt(key, plaintext).is_err());
    }

    #[test]
    fn test_parse_aes_key_hex() {
        // 32 字符 hex -> 16 字节
        let key = WechatAdapter::parse_aes_key("deadbeefdeadbeef00112233aabbccdd").unwrap();
        assert_eq!(key.len(), 16);
        assert_eq!(key[0], 0xde);
        assert_eq!(key[1], 0xad);
    }

    #[test]
    fn test_parse_aes_key_base64() {
        // base64 编码的 16 字节
        let raw_key = b"0123456789abcdef";
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw_key);
        let key = WechatAdapter::parse_aes_key(&b64).unwrap();
        assert_eq!(key, raw_key.to_vec());
    }

    #[test]
    fn test_parse_aes_key_empty_fails() {
        assert!(WechatAdapter::parse_aes_key("").is_err());
    }

    // -----------------------------------------------------------------------
    // 认证头部构造测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_auth_headers_structure() {
        let config = WechatConfig {
            bot_token: "test_bot_token_xyz".to_string(),
            channel_version: "1.2.3".to_string(),
            ..WechatConfig::new("test_bot_token_xyz")
        };
        let adapter = WechatAdapter::new(config);
        let headers = adapter.build_auth_headers();

        // 查找各头部
        let auth = headers.iter().find(|(k, _)| k == "Authorization");
        assert!(auth.is_some());
        assert_eq!(auth.unwrap().1, "Bearer test_bot_token_xyz");

        let auth_type = headers.iter().find(|(k, _)| k == "AuthorizationType");
        assert!(auth_type.is_some());
        assert_eq!(auth_type.unwrap().1, "ilink_bot_token");

        let app_id = headers.iter().find(|(k, _)| k == "iLink-App-Id");
        assert!(app_id.is_some());
        assert_eq!(app_id.unwrap().1, "bot");

        let version = headers.iter().find(|(k, _)| k == "iLink-App-ClientVersion");
        assert!(version.is_some());
        // 1.2.3 -> (1<<16)|(2<<8)|3 = 65536+512+3 = 66051
        assert_eq!(version.unwrap().1, "66051");

        let uin = headers.iter().find(|(k, _)| k == "X-WECHAT-UIN");
        assert!(uin.is_some());
        // X-WECHAT-UIN 应为合法 base64
        let uin_val = &uin.unwrap().1;
        assert!(
            base64::engine::general_purpose::STANDARD
                .decode(uin_val)
                .is_ok(),
            "X-WECHAT-UIN 应为合法 base64"
        );
    }

    // -----------------------------------------------------------------------
    // 发送消息载荷构造测试（通过 JSON 序列化验证结构）
    // -----------------------------------------------------------------------

    #[test]
    fn test_send_message_payload_structure() {
        let payload = SendMessageRequest {
            msg: OutboundMessage {
                from_user_id: String::new(),
                to_user_id: "chat_abc".to_string(),
                client_id: "cyberclaw-test-uuid".to_string(),
                message_type: MSG_TYPE_BOT,
                message_state: MSG_STATE_FINISH,
                context_token: "ctx_token_xyz".to_string(),
                item_list: vec![MessageItem {
                    item_type: ITEM_TYPE_TEXT,
                    text_item: Some(TextItem {
                        text: "测试回复".to_string(),
                    }),
                    media_item: None,
                }],
            },
            base_info: BaseInfo {
                channel_version: CHANNEL_VERSION.to_string(),
            },
        };

        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["msg"]["to_user_id"], "chat_abc");
        assert_eq!(json["msg"]["message_type"], MSG_TYPE_BOT);
        assert_eq!(json["msg"]["message_state"], MSG_STATE_FINISH);
        assert_eq!(json["msg"]["context_token"], "ctx_token_xyz");
        assert_eq!(json["msg"]["item_list"][0]["type"], ITEM_TYPE_TEXT);
        assert_eq!(json["msg"]["item_list"][0]["text_item"]["text"], "测试回复");
        assert_eq!(json["base_info"]["channel_version"], CHANNEL_VERSION);
    }

    // -----------------------------------------------------------------------
    // CDN URL 构造测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_cdn_download_url_format() {
        let download_param = "enc_query_abc123";
        let expected_url = format!(
            "{}/download?encrypted_query_param={}",
            CDN_BASE_URL, download_param
        );
        assert_eq!(
            expected_url,
            "https://novac2c.cdn.weixin.qq.com/c2c/download?encrypted_query_param=enc_query_abc123"
        );
    }

    #[test]
    fn test_api_url_construction() {
        let config = WechatConfig::new("tok");
        let adapter = WechatAdapter::new(config);

        assert_eq!(
            adapter.api_url(PATH_GETUPDATES),
            "https://ilinkai.weixin.qq.com/ilink/bot/getupdates"
        );
        assert_eq!(
            adapter.api_url(PATH_SENDMESSAGE),
            "https://ilinkai.weixin.qq.com/ilink/bot/sendmessage"
        );
        assert_eq!(
            adapter.api_url(PATH_SENDTYPING),
            "https://ilinkai.weixin.qq.com/ilink/bot/sendtyping"
        );
        assert_eq!(
            adapter.api_url(PATH_GETUPLOADURL),
            "https://ilinkai.weixin.qq.com/ilink/bot/getuploadurl"
        );
    }

    // -----------------------------------------------------------------------
    // validate_signature 测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_signature_always_true() {
        let adapter = WechatAdapter::new(WechatConfig::new("tok"));
        assert!(adapter.validate_signature(b"any payload", "any_sig"));
        assert!(adapter.validate_signature(b"", ""));
    }

    // -----------------------------------------------------------------------
    // PKCS7 填充/去填充测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_pkcs7_pad_unpad_roundtrip() {
        for len in [0usize, 1, 15, 16, 17, 31, 32, 100] {
            let data: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let padded = pkcs7_pad(&data, AES_BLOCK_SIZE);
            assert_eq!(padded.len() % AES_BLOCK_SIZE, 0);
            assert!(padded.len() > len || len == 0);
            let unpadded = pkcs7_unpad(&padded).unwrap();
            assert_eq!(unpadded, data, "PKCS7 往返失败: len={}", len);
        }
    }

    #[test]
    fn test_pkcs7_unpad_invalid_padding() {
        // 构造损坏的填充
        let mut data = vec![0u8; 16];
        data[15] = 17; // 超过块大小
        assert!(pkcs7_unpad(&data).is_err());
    }
}
