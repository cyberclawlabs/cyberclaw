//! Tool Calling extension for Agent Runtime
//!
//! 为 Agent Runtime 添加 Tool Calling 循环支持。
//!
//! # 功能
//!
//! - **Tool Calling 循环**: 最多执行 10 轮 tool call
//! - **自动结果传递**: 将执行结果返回给 LLM
//! - **错误恢复**: 优雅处理可恢复错误
//! - **消息历史管理**: 维护完整的对话历史

#[cfg(feature = "tool-calling")]
use cyberclaw_llm_bridge::{ToolCallMapper, ToolExecutor};
#[cfg(feature = "tool-calling")]
use cyberclaw_llm::prelude::*;
#[cfg(feature = "tool-calling")]
use std::sync::Arc;
#[cfg(feature = "tool-calling")]
use tracing::{debug, info, warn};

/// Tool Calling 配置
#[cfg(feature = "tool-calling")]
#[derive(Debug, Clone)]
pub struct ToolCallingConfig {
    /// 最大循环轮次
    pub max_iterations: u32,
    /// 是否在遇到不可恢复错误时中止
    pub abort_on_fatal_error: bool,
    /// 是否记录详细日志
    pub verbose_logging: bool,
}

#[cfg(feature = "tool-calling")]
impl Default for ToolCallingConfig {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            abort_on_fatal_error: true,
            verbose_logging: false,
        }
    }
}

/// Tool Calling 循环执行器
#[cfg(feature = "tool-calling")]
pub struct ToolCallingLoop {
    /// Tool 执行器
    executor: Arc<ToolExecutor>,
    /// LLM Provider
    llm_provider: Arc<LlmProvider>,
    /// 配置
    config: ToolCallingConfig,
}

#[cfg(feature = "tool-calling")]
impl ToolCallingLoop {
    /// 创建新的 Tool Calling 循环
    pub fn new(
        executor: Arc<ToolExecutor>,
        llm_provider: Arc<LlmProvider>,
        config: ToolCallingConfig,
    ) -> Self {
        Self {
            executor,
            llm_provider,
            config,
        }
    }

    /// 执行 Tool Calling 循环
    ///
    /// # 循环逻辑
    ///
    /// 1. 调用 LLM 获取响应
    /// 2. 检查是否有 tool_calls
    /// 3. 如果有,执行所有 tool calls
    /// 4. 将结果添加到消息历史
    /// 5. 重复步骤 1-4,最多 max_iterations 次
    /// 6. 返回最终的 LLM 响应
    pub async fn execute(
        &self,
        mut messages: Vec<Message>,
        model: String,
        tools: Vec<ToolDefinition>,
        trace_id: String,
    ) -> anyhow::Result<String> {
        let mut iteration = 0;

        loop {
            iteration += 1;

            if iteration > self.config.max_iterations {
                warn!(
                    "Reached maximum iterations ({}), stopping tool calling loop",
                    self.config.max_iterations
                );
                break;
            }

            if self.config.verbose_logging {
                debug!(
                    iteration = iteration,
                    max_iterations = self.config.max_iterations,
                    message_count = messages.len(),
                    "Tool calling loop iteration"
                );
            }

            // 1. 调用 LLM
            let request = ChatRequest {
                model: model.clone(),
                messages: messages.clone(),
                tools: Some(tools.clone()),
                tool_choice: None, // 让 LLM 自主决定
                ..Default::default()
            };

            let response = self
                .llm_provider
                .client()
                .chat_completion(request)
                .await
                .map_err(|e| anyhow::anyhow!("LLM call failed: {}", e))?;

            let choice = response
                .choices
                .first()
                .ok_or_else(|| anyhow::anyhow!("No response from LLM"))?;

            // 2. 检查是否有 tool_calls
            let tool_calls = match &choice.message.tool_calls {
                Some(calls) if !calls.is_empty() => calls,
                _ => {
                    // 没有 tool calls,返回最终响应
                    info!(
                        iteration = iteration,
                        "No tool calls in response, finishing loop"
                    );
                    return Ok(choice.message.content.clone());
                }
            };

            info!(
                iteration = iteration,
                tool_count = tool_calls.len(),
                "Executing tool calls"
            );

            // 3. 将 assistant 消息添加到历史 (包含 tool_calls)
            messages.push(choice.message.clone());

            // 4. 执行所有 tool calls
            for tool_call in tool_calls {
                if self.config.verbose_logging {
                    debug!(
                        tool_call_id = %tool_call.id,
                        tool_name = %tool_call.function.name,
                        "Executing tool call"
                    );
                }

                // 执行 tool
                let result = self.executor.execute_tool(tool_call, trace_id.clone()).await?;

                // 5. 将结果添加到消息历史
                let tool_message = match result {
                    cyberclaw_llm_bridge::ToolExecutionResult::Success {
                        tool_call_id,
                        result,
                        ..
                    } => {
                        if self.config.verbose_logging {
                            debug!(
                                tool_call_id = %tool_call_id,
                                "Tool execution succeeded"
                            );
                        }
                        Message::tool(tool_call_id, serde_json::to_string(&result)?)
                    }
                    cyberclaw_llm_bridge::ToolExecutionResult::Error {
                        tool_call_id,
                        error,
                        recoverable,
                        ..
                    } => {
                        warn!(
                            tool_call_id = %tool_call_id,
                            recoverable = recoverable,
                            error = %error,
                            "Tool execution failed"
                        );

                        // 不可恢复错误,可选择性中止
                        if !recoverable && self.config.abort_on_fatal_error {
                            return Err(anyhow::anyhow!(
                                "Fatal tool execution error: {}",
                                error
                            ));
                        }

                        // 将错误信息返回给 LLM,让其决定如何处理
                        Message::tool(
                            tool_call_id,
                            serde_json::json!({
                                "error": error,
                                "recoverable": recoverable
                            })
                            .to_string(),
                        )
                    }
                };

                messages.push(tool_message);
            }

            // 继续下一轮循环
        }

        // 达到最大循环次数,返回最后一条 assistant 消息
        messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant)
            .map(|m| m.content.clone())
            .ok_or_else(|| anyhow::anyhow!("No assistant message found in history"))
    }
}

#[cfg(test)]
#[cfg(feature = "tool-calling")]
mod tests {
    use super::*;

    #[test]
    fn test_tool_calling_config_default() {
        let config = ToolCallingConfig::default();
        assert_eq!(config.max_iterations, 10);
        assert!(config.abort_on_fatal_error);
        assert!(!config.verbose_logging);
    }
}
