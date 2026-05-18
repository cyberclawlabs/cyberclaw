//! Tool executor with Capability integration

use crate::error::{BridgeError, BridgeResult};
use crate::mapper::ToolCallMapper;
use crate::types::ToolExecutionResult;
use cyberclaw_connectors::dispatcher::CapabilityDispatcher;
use cyberclaw_connectors::types::ExecutionStatus;
use cyberclaw_llm::types::ToolCall;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

/// Tool 执行器配置
#[derive(Debug, Clone)]
pub struct ToolExecutorConfig {
    /// 单个 tool 执行超时 (毫秒)
    pub execution_timeout_ms: u64,
    /// 最大重试次数
    pub max_retries: u32,
    /// 是否将超时视为可恢复错误
    pub timeout_recoverable: bool,
    /// 是否将速率限制视为可恢复错误
    pub rate_limit_recoverable: bool,
}

impl Default for ToolExecutorConfig {
    fn default() -> Self {
        Self {
            execution_timeout_ms: 30_000, // 30 秒默认超时
            max_retries: 2,               // 默认重试 2 次
            timeout_recoverable: true,    // 超时可恢复
            rate_limit_recoverable: true, // 速率限制可恢复
        }
    }
}

/// Tool 执行器
///
/// 通过 CapabilityDispatcher 执行 Tool Call 并格式化结果。
pub struct ToolExecutor {
    /// Capability 调度器
    dispatcher: Arc<CapabilityDispatcher>,
    /// Tool Call 映射器
    mapper: Arc<ToolCallMapper>,
    /// 配置
    config: ToolExecutorConfig,
}

impl ToolExecutor {
    /// 创建新的执行器
    pub fn new(dispatcher: Arc<CapabilityDispatcher>, mapper: Arc<ToolCallMapper>) -> Self {
        Self {
            dispatcher,
            mapper,
            config: ToolExecutorConfig::default(),
        }
    }

    /// 创建带自定义配置的执行器
    pub fn with_config(
        dispatcher: Arc<CapabilityDispatcher>,
        mapper: Arc<ToolCallMapper>,
        config: ToolExecutorConfig,
    ) -> Self {
        Self {
            dispatcher,
            mapper,
            config,
        }
    }

    /// 执行单个 Tool Call
    ///
    /// 返回格式化的执行结果,包括成功和错误情况。
    pub async fn execute_tool(
        &self,
        tool_call: &ToolCall,
        trace_id: String,
    ) -> BridgeResult<ToolExecutionResult> {
        let tool_name = tool_call.function.name.clone();
        let tool_call_id = tool_call.id.clone();

        info!(
            tool_call_id = %tool_call_id,
            tool_name = %tool_name,
            "Executing tool call"
        );

        // 1. 映射 ToolCall 到 CapabilityExecutionRequest
        let request = match self.mapper.map_tool_call(tool_call, trace_id.clone()) {
            Ok(req) => req,
            Err(e) => {
                error!("Failed to map tool call: {:?}", e);
                return Ok(ToolExecutionResult::error(
                    tool_call_id,
                    tool_name,
                    e.to_string(),
                    false, // 映射失败不可恢复
                ));
            }
        };

        debug!(
            capability_id = %request.capability_id,
            connector_id = %request.connector_id,
            "Mapped to capability"
        );

        // 2. 执行 Capability (带超时和重试)
        let execution_result = self
            .execute_with_retry_and_timeout(request, &tool_call_id, &tool_name)
            .await;

        // 3. 格式化结果
        match execution_result {
            Ok(result) => {
                match result.status {
                    ExecutionStatus::Success => {
                        info!(
                            tool_call_id = %tool_call_id,
                            tool_name = %tool_name,
                            "Tool execution succeeded"
                        );
                        Ok(ToolExecutionResult::success(
                            tool_call_id,
                            tool_name,
                            result.output,
                        ))
                    }
                    ExecutionStatus::Failed => {
                        warn!(
                            tool_call_id = %tool_call_id,
                            tool_name = %tool_name,
                            error = ?result.error,
                            "Tool execution failed"
                        );
                        Ok(ToolExecutionResult::error(
                            tool_call_id,
                            tool_name,
                            result.error.unwrap_or_else(|| "Unknown error".to_string()),
                            true, // Capability 执行失败通常可恢复
                        ))
                    }
                    // 其他状态作为错误处理
                    _ => {
                        warn!(
                            tool_call_id = %tool_call_id,
                            tool_name = %tool_name,
                            status = ?result.status,
                            "Unexpected execution status"
                        );
                        Ok(ToolExecutionResult::error(
                            tool_call_id,
                            tool_name,
                            result
                                .error
                                .unwrap_or_else(|| "Unexpected status".to_string()),
                            true,
                        ))
                    }
                }
            }
            Err(e) => {
                error!(
                    tool_call_id = %tool_call_id,
                    tool_name = %tool_name,
                    error = %e,
                    "Tool execution error"
                );
                Ok(ToolExecutionResult::error(
                    tool_call_id,
                    tool_name,
                    e.to_string(),
                    self.is_error_recoverable(&e),
                ))
            }
        }
    }

    /// 批量执行 Tool Calls
    pub async fn execute_tools(
        &self,
        tool_calls: &[ToolCall],
        trace_id: String,
    ) -> BridgeResult<Vec<ToolExecutionResult>> {
        let mut results = Vec::with_capacity(tool_calls.len());

        for tool_call in tool_calls {
            let result = self.execute_tool(tool_call, trace_id.clone()).await?;
            results.push(result);
        }

        Ok(results)
    }

    /// 带重试和超时的执行
    async fn execute_with_retry_and_timeout(
        &self,
        request: cyberclaw_connectors::types::CapabilityExecutionRequest,
        tool_call_id: &str,
        tool_name: &str,
    ) -> BridgeResult<cyberclaw_connectors::types::CapabilityExecutionResult> {
        let mut attempts = 0;
        let max_attempts = self.config.max_retries + 1;

        loop {
            attempts += 1;

            debug!(
                tool_call_id = %tool_call_id,
                tool_name = %tool_name,
                attempt = attempts,
                max_attempts = max_attempts,
                "Executing capability"
            );

            // 执行带超时
            let timeout_duration = Duration::from_millis(self.config.execution_timeout_ms);
            let execution = timeout(timeout_duration, self.dispatcher.dispatch(request.clone()));

            match execution.await {
                Ok(Ok(result)) => {
                    // 成功
                    return Ok(result);
                }
                Ok(Err(e)) => {
                    // Dispatcher 错误
                    warn!(
                        tool_call_id = %tool_call_id,
                        attempt = attempts,
                        error = %e,
                        "Capability execution failed"
                    );

                    // 速率限制错误不重试
                    if e.to_string().contains("Rate limit") {
                        return Err(BridgeError::ExecutionFailed {
                            message: format!("Rate limit exceeded: {}", e),
                        });
                    }

                    // 达到最大重试次数
                    if attempts >= max_attempts {
                        return Err(BridgeError::ExecutionFailed {
                            message: format!("Failed after {} attempts: {}", attempts, e),
                        });
                    }

                    // 继续重试
                    debug!("Retrying after error...");
                    tokio::time::sleep(Duration::from_millis(100 * attempts as u64)).await;
                }
                Err(_) => {
                    // 超时
                    warn!(
                        tool_call_id = %tool_call_id,
                        attempt = attempts,
                        timeout_ms = self.config.execution_timeout_ms,
                        "Capability execution timeout"
                    );

                    // 达到最大重试次数
                    if attempts >= max_attempts {
                        return Err(BridgeError::Timeout {
                            timeout_ms: self.config.execution_timeout_ms,
                        });
                    }

                    // 继续重试
                    debug!("Retrying after timeout...");
                    tokio::time::sleep(Duration::from_millis(200 * attempts as u64)).await;
                }
            }
        }
    }

    /// 判断错误是否可恢复
    fn is_error_recoverable(&self, error: &BridgeError) -> bool {
        match error {
            BridgeError::Timeout { .. } => self.config.timeout_recoverable,
            BridgeError::RateLimitExceeded { .. } => self.config.rate_limit_recoverable,
            BridgeError::ExecutionFailed { message } => {
                // 网络错误、临时失败等可恢复
                message.contains("timeout")
                    || message.contains("network")
                    || message.contains("temporary")
            }
            BridgeError::UnknownTool { .. } | BridgeError::ArgumentParseFailed { .. } => false,
            BridgeError::Internal(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_connectors::registry::ConnectorRegistry;

    #[tokio::test]
    async fn test_executor_config_default() {
        let config = ToolExecutorConfig::default();
        assert_eq!(config.execution_timeout_ms, 30_000);
        assert_eq!(config.max_retries, 2);
        assert!(config.timeout_recoverable);
        assert!(config.rate_limit_recoverable);
    }

    #[tokio::test]
    async fn test_is_error_recoverable() {
        let registry = Arc::new(ConnectorRegistry::new());
        let dispatcher = Arc::new(CapabilityDispatcher::new(registry));
        let mapper = Arc::new(ToolCallMapper::new());
        let executor = ToolExecutor::new(dispatcher, mapper);

        // Timeout 可恢复
        let error = BridgeError::Timeout { timeout_ms: 1000 };
        assert!(executor.is_error_recoverable(&error));

        // UnknownTool 不可恢复
        let error = BridgeError::UnknownTool {
            tool_name: "test".to_string(),
        };
        assert!(!executor.is_error_recoverable(&error));

        // ArgumentParseFailed 不可恢复
        let error = BridgeError::ArgumentParseFailed {
            message: "invalid json".to_string(),
        };
        assert!(!executor.is_error_recoverable(&error));
    }

    // 注意: 完整的集成测试需要真实的 Connector,在集成测试中实现
}
