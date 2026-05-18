//! Type definitions for LLM Bridge

use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Tool 到 Capability 的映射配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallMapping {
    /// Tool 名称
    pub tool_name: String,
    /// 目标 Capability ID
    pub capability_id: CapabilityId,
    /// 目标 Connector ID
    pub connector_id: ConnectorId,
    /// 参数映射规则 (tool_param_name -> capability_param_name)
    #[serde(default)]
    pub parameter_mapping: HashMap<String, String>,
    /// Tool 描述 (可选)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl ToolCallMapping {
    /// 创建新的映射配置
    pub fn new(
        tool_name: impl Into<String>,
        capability_id: CapabilityId,
        connector_id: ConnectorId,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            capability_id,
            connector_id,
            parameter_mapping: HashMap::new(),
            description: None,
        }
    }

    /// 添加描述
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// 添加参数映射
    pub fn with_parameter_mapping(
        mut self,
        tool_param: impl Into<String>,
        capability_param: impl Into<String>,
    ) -> Self {
        self.parameter_mapping
            .insert(tool_param.into(), capability_param.into());
        self
    }

    /// 转换参数
    pub fn transform_arguments(
        &self,
        args: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        if self.parameter_mapping.is_empty() {
            // 无映射规则,直接返回原参数
            return Ok(args);
        }

        let args_obj = args
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Arguments must be an object"))?;

        let mut transformed = serde_json::Map::new();

        for (tool_param, capability_param) in &self.parameter_mapping {
            if let Some(value) = args_obj.get(tool_param) {
                transformed.insert(capability_param.clone(), value.clone());
            }
        }

        // 保留未映射的参数
        for (key, value) in args_obj {
            if !self.parameter_mapping.contains_key(key) {
                transformed.insert(key.clone(), value.clone());
            }
        }

        Ok(serde_json::Value::Object(transformed))
    }
}

/// Tool 执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ToolExecutionResult {
    /// 执行成功
    Success {
        /// 工具调用 ID
        tool_call_id: String,
        /// 工具名称
        tool_name: String,
        /// 执行结果
        result: serde_json::Value,
    },

    /// 执行失败 (可恢复)
    Error {
        /// 工具调用 ID
        tool_call_id: String,
        /// 工具名称
        tool_name: String,
        /// 错误信息
        error: String,
        /// 是否可恢复 (true 表示可以让 LLM 重试或调整策略)
        recoverable: bool,
    },
}

impl ToolExecutionResult {
    /// 创建成功结果
    pub fn success(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        result: serde_json::Value,
    ) -> Self {
        Self::Success {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            result,
        }
    }

    /// 创建错误结果
    pub fn error(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        error: impl Into<String>,
        recoverable: bool,
    ) -> Self {
        Self::Error {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            error: error.into(),
            recoverable,
        }
    }

    /// 是否成功
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    /// 是否可恢复错误
    pub fn is_recoverable(&self) -> bool {
        match self {
            Self::Error { recoverable, .. } => *recoverable,
            _ => false,
        }
    }

    /// 获取工具调用 ID
    pub fn tool_call_id(&self) -> &str {
        match self {
            Self::Success { tool_call_id, .. } | Self::Error { tool_call_id, .. } => tool_call_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_call_mapping_transform_no_mapping() {
        let mapping = ToolCallMapping::new(
            "test_tool",
            CapabilityId::from_string("test.capability".to_string()).unwrap(),
            ConnectorId::from_string("test".to_string()).unwrap(),
        );

        let args = serde_json::json!({
            "param1": "value1",
            "param2": 42
        });

        let result = mapping.transform_arguments(args.clone()).unwrap();
        assert_eq!(result, args);
    }

    #[test]
    fn test_tool_call_mapping_transform_with_mapping() {
        let mapping = ToolCallMapping::new(
            "test_tool",
            CapabilityId::from_string("test.capability".to_string()).unwrap(),
            ConnectorId::from_string("test".to_string()).unwrap(),
        )
        .with_parameter_mapping("file_path", "path")
        .with_parameter_mapping("content", "data");

        let args = serde_json::json!({
            "file_path": "/tmp/test.txt",
            "content": "hello",
            "other": "preserved"
        });

        let result = mapping.transform_arguments(args).unwrap();

        assert_eq!(result["path"], "/tmp/test.txt");
        assert_eq!(result["data"], "hello");
        assert_eq!(result["other"], "preserved");
    }

    #[test]
    fn test_tool_execution_result_success() {
        let result = ToolExecutionResult::success(
            "call-123",
            "test_tool",
            serde_json::json!({"status": "ok"}),
        );

        assert!(result.is_success());
        assert!(!result.is_recoverable());
        assert_eq!(result.tool_call_id(), "call-123");
    }

    #[test]
    fn test_tool_execution_result_error_recoverable() {
        let result = ToolExecutionResult::error("call-456", "test_tool", "timeout", true);

        assert!(!result.is_success());
        assert!(result.is_recoverable());
        assert_eq!(result.tool_call_id(), "call-456");
    }
}
