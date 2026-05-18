use crate::error::LlmResult;
use crate::types::{ChatChunk, ChatRequest, ChatResponse};
use async_trait::async_trait;
use futures::stream::Stream;

/// LLM 客户端统一接口
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// 聊天补全（非流式）
    ///
    /// # Arguments
    ///
    /// * `request` - 聊天请求
    ///
    /// # Returns
    ///
    /// 聊天响应
    async fn chat_completion(&self, request: ChatRequest) -> LlmResult<ChatResponse>;

    /// 聊天补全（流式）
    ///
    /// # Arguments
    ///
    /// * `request` - 聊天请求（需设置 stream = true）
    ///
    /// # Returns
    ///
    /// 流式响应流
    async fn chat_completion_stream(
        &self,
        request: ChatRequest,
    ) -> LlmResult<Box<dyn Stream<Item = LlmResult<ChatChunk>> + Send + Unpin>>;

    /// 获取提供商名称
    fn provider(&self) -> &str;

    /// 验证连接配置
    ///
    /// 用于启动时验证 API 密钥和连接是否正常
    async fn validate_connection(&self) -> LlmResult<()>;
}
