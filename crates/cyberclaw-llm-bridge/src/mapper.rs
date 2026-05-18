//! Tool Call to Capability mapper

use crate::error::{BridgeError, BridgeResult};
use crate::tool_filter::is_tool_enabled;
use crate::types::ToolCallMapping;
use cyberclaw_connectors::types::CapabilityExecutionRequest;
use cyberclaw_core::ids::{ActorId, ExecutionId, WorkspaceId};
use cyberclaw_core::prelude::{ActorRef, WorkspaceRef};
use cyberclaw_llm::types::ToolCall;
use std::collections::HashMap;
use std::sync::RwLock;
use tracing::{debug, warn};

/// Sprint 44 — translator callback signature passed to
/// [`ToolCallMapper::map_tool_call_with_compat`].
///
/// Returns `Some(facade_name)` when the LLM-side `name` is recognised as
/// an alias for a cyberclaw facade tool, or `None` to passthrough the
/// original name unchanged.
pub type ToolNameTranslator<'a> = dyn Fn(&str) -> Option<String> + 'a;

/// Tool Call 映射器
///
/// 将 LLM 的 function call 转换为 Capability 执行请求。
pub struct ToolCallMapper {
    /// Tool 名称到 Capability 的映射表
    mappings: RwLock<HashMap<String, ToolCallMapping>>,
    /// 默认工作空间 (用于生成请求)
    default_workspace: WorkspaceRef,
    /// 默认 Actor (用于生成请求)
    default_actor: ActorRef,
}

impl ToolCallMapper {
    /// 创建新的映射器
    pub fn new() -> Self {
        use cyberclaw_core::workspace::{WorkspaceMaterializationMode, WorkspaceMode};

        Self {
            mappings: RwLock::new(HashMap::new()),
            default_workspace: WorkspaceRef {
                id: WorkspaceId::from_string("default".to_string()).expect("valid workspace id"),
                mode: WorkspaceMode::Shared,
                materialization_mode: Some(WorkspaceMaterializationMode::LocalEphemeral),
                home_node_id: None,
                backing_store: None,
                root: "/tmp/cyberclaw-workspace".to_string(),
                writable_roots: vec!["/tmp/cyberclaw-workspace".to_string()],
            },
            default_actor: Self::create_system_actor(),
        }
    }

    /// 创建带默认工作空间的映射器
    pub fn with_workspace(workspace: WorkspaceRef) -> Self {
        Self {
            mappings: RwLock::new(HashMap::new()),
            default_workspace: workspace,
            default_actor: Self::create_system_actor(),
        }
    }

    /// 创建系统 Actor
    fn create_system_actor() -> ActorRef {
        use cyberclaw_core::identity::ActorType;

        ActorRef {
            id: ActorId::from_string("system".to_string()).expect("valid actor id"),
            actor_type: ActorType::System,
            tenant_id: None,
            home_node_id: None,
            display_name: "System".to_string(),
        }
    }

    /// 注册 Tool 映射
    pub fn register_mapping(&self, mapping: ToolCallMapping) -> BridgeResult<()> {
        let mut mappings = self
            .mappings
            .write()
            .map_err(|e| BridgeError::Internal(format!("Lock poisoned: {}", e)))?;

        debug!(
            "Registering tool mapping: {} -> {} ({})",
            mapping.tool_name, mapping.capability_id, mapping.connector_id
        );

        mappings.insert(mapping.tool_name.clone(), mapping);
        Ok(())
    }

    /// 批量注册映射
    pub fn register_mappings(&self, mappings: Vec<ToolCallMapping>) -> BridgeResult<()> {
        for mapping in mappings {
            self.register_mapping(mapping)?;
        }
        Ok(())
    }

    /// 将 ToolCall 转换为 CapabilityExecutionRequest
    pub fn map_tool_call(
        &self,
        tool_call: &ToolCall,
        trace_id: String,
    ) -> BridgeResult<CapabilityExecutionRequest> {
        self.map_tool_call_with_compat(tool_call, trace_id, None)
    }

    /// Sprint 44 — `map_tool_call` 的 SkillCompat 感知变体。
    ///
    /// 若 `translator` 提供：先把 LLM 用的工具名翻译成 cyberclaw facade 名，
    /// 再走原有的 alias 解析路径。翻译失败（None）时原样透传，行为与
    /// [`map_tool_call`] 完全一致——保证向后兼容。
    ///
    /// 参数 `translator`:
    /// - `Some(callback)` — 给定一个 LLM 工具名，返回 `Some(facade_name)` 表示
    ///   已翻译，`None` 表示该 ecosystem 不认识此工具名（透传）
    /// - `None` — 完全跳过翻译（等同于 [`map_tool_call`]）
    pub fn map_tool_call_with_compat(
        &self,
        tool_call: &ToolCall,
        trace_id: String,
        translator: Option<&ToolNameTranslator<'_>>,
    ) -> BridgeResult<CapabilityExecutionRequest> {
        let mappings = self
            .mappings
            .read()
            .map_err(|e| BridgeError::Internal(format!("Lock poisoned: {}", e)))?;

        // 先尝试翻译；翻译命中时优先用 facade 名查表
        let original_name = tool_call.function.name.as_str();
        let translated = translator.and_then(|f| f(original_name));
        let lookup_name: &str = translated.as_deref().unwrap_or(original_name);

        // 查找映射配置
        let mapping = mappings.get(lookup_name).ok_or_else(|| {
            warn!(
                "Unknown tool: {} (translated lookup: {})",
                original_name, lookup_name
            );
            BridgeError::UnknownTool {
                tool_name: original_name.to_string(),
            }
        })?;

        // 解析参数
        let args: serde_json::Value =
            serde_json::from_str(&tool_call.function.arguments).map_err(|e| {
                BridgeError::ArgumentParseFailed {
                    message: format!("Invalid JSON arguments: {}", e),
                }
            })?;

        // 转换参数
        let capability_input = mapping.transform_arguments(args)?;

        debug!(
            "Mapped tool call {} to capability {} (connector: {})",
            tool_call.function.name, mapping.capability_id, mapping.connector_id
        );

        Ok(CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id,
            actor: self.default_actor.clone(),
            workspace: self.default_workspace.clone(),
            connector_id: mapping.connector_id.clone(),
            capability_id: mapping.capability_id.clone(),
            input: capability_input,
        })
    }

    /// 获取已注册的 Tool 列表
    pub fn list_tools(&self) -> BridgeResult<Vec<String>> {
        let mappings = self
            .mappings
            .read()
            .map_err(|e| BridgeError::Internal(format!("Lock poisoned: {}", e)))?;

        Ok(mappings.keys().cloned().collect())
    }

    /// 获取稳定排序的 Tool 列表
    pub fn list_tools_sorted(&self) -> BridgeResult<Vec<String>> {
        let mut tools = self.list_tools()?;
        tools.sort();
        Ok(tools)
    }

    /// Sprint 20 W1 — return every alias and the `(connector_id,
    /// capability_id)` pair it routes to. Used by the drift detector
    /// in `apps/cyberclaw-server/src/connector_drift.rs` to verify
    /// every mapper alias targets a connector that's actually
    /// registered in `ConnectorRegistry`.
    pub fn list_mappings_with_targets(
        &self,
    ) -> BridgeResult<
        Vec<(
            String,
            cyberclaw_core::ids::ConnectorId,
            cyberclaw_core::ids::CapabilityId,
        )>,
    > {
        let mappings = self
            .mappings
            .read()
            .map_err(|e| BridgeError::Internal(format!("Lock poisoned: {}", e)))?;
        Ok(mappings
            .iter()
            .map(|(name, m)| {
                (
                    name.clone(),
                    m.connector_id.clone(),
                    m.capability_id.clone(),
                )
            })
            .collect())
    }

    /// 检查 Tool 是否已注册
    pub fn has_tool(&self, tool_name: &str) -> bool {
        self.mappings
            .read()
            .map(|m| m.contains_key(tool_name))
            .unwrap_or(false)
    }

    /// 按 allow/deny 规则裁剪已注册工具，并返回被移除的工具名（稳定排序）
    pub fn apply_visibility_filters(
        &self,
        allow_patterns: &[String],
        deny_patterns: &[String],
    ) -> BridgeResult<Vec<String>> {
        let mut mappings = self
            .mappings
            .write()
            .map_err(|e| BridgeError::Internal(format!("Lock poisoned: {}", e)))?;

        let mut removed = Vec::new();
        mappings.retain(|tool_name, _| {
            let enabled = is_tool_enabled(tool_name, allow_patterns, deny_patterns);
            if !enabled {
                removed.push(tool_name.clone());
            }
            enabled
        });

        removed.sort();
        Ok(removed)
    }
}

impl Default for ToolCallMapper {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::ids::{CapabilityId, ConnectorId};
    use cyberclaw_llm::types::{FunctionCall, ToolCall};

    fn create_test_tool_call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call-123".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: serde_json::to_string(&args).unwrap(),
            },
        }
    }

    #[test]
    fn test_register_and_map_tool_call() {
        let mapper = ToolCallMapper::new();

        // 注册映射
        let mapping = ToolCallMapping::new(
            "read_file",
            CapabilityId::from_string("fs.read".to_string()).unwrap(),
            ConnectorId::from_string("filesystem".to_string()).unwrap(),
        );

        mapper.register_mapping(mapping).unwrap();

        // 创建 tool call
        let tool_call = create_test_tool_call(
            "read_file",
            serde_json::json!({
                "path": "/tmp/test.txt"
            }),
        );

        // 转换
        let request = mapper
            .map_tool_call(&tool_call, "trace-123".to_string())
            .unwrap();

        assert_eq!(request.capability_id.as_ref(), "fs.read");
        assert_eq!(request.connector_id.as_ref(), "filesystem");
        assert_eq!(request.input["path"], "/tmp/test.txt");
        assert_eq!(request.trace_id, "trace-123");
    }

    #[test]
    fn test_map_unknown_tool() {
        let mapper = ToolCallMapper::new();

        let tool_call = create_test_tool_call(
            "unknown_tool",
            serde_json::json!({
                "param": "value"
            }),
        );

        let result = mapper.map_tool_call(&tool_call, "trace-123".to_string());
        assert!(result.is_err());

        match result {
            Err(BridgeError::UnknownTool { tool_name }) => {
                assert_eq!(tool_name, "unknown_tool");
            }
            _ => panic!("Expected UnknownTool error"),
        }
    }

    #[test]
    fn test_map_with_parameter_mapping() {
        let mapper = ToolCallMapper::new();

        // 注册带参数映射的配置
        let mapping = ToolCallMapping::new(
            "write_file",
            CapabilityId::from_string("fs.write".to_string()).unwrap(),
            ConnectorId::from_string("filesystem".to_string()).unwrap(),
        )
        .with_parameter_mapping("file_path", "path")
        .with_parameter_mapping("content", "data");

        mapper.register_mapping(mapping).unwrap();

        let tool_call = create_test_tool_call(
            "write_file",
            serde_json::json!({
                "file_path": "/tmp/output.txt",
                "content": "hello world"
            }),
        );

        let request = mapper
            .map_tool_call(&tool_call, "trace-123".to_string())
            .unwrap();

        assert_eq!(request.input["path"], "/tmp/output.txt");
        assert_eq!(request.input["data"], "hello world");
    }

    #[test]
    fn test_list_tools() {
        let mapper = ToolCallMapper::new();

        mapper
            .register_mapping(ToolCallMapping::new(
                "tool1",
                CapabilityId::from_string("cap1".to_string()).unwrap(),
                ConnectorId::from_string("conn1".to_string()).unwrap(),
            ))
            .unwrap();

        mapper
            .register_mapping(ToolCallMapping::new(
                "tool2",
                CapabilityId::from_string("cap2".to_string()).unwrap(),
                ConnectorId::from_string("conn2".to_string()).unwrap(),
            ))
            .unwrap();

        let tools = mapper.list_tools().unwrap();
        assert_eq!(tools.len(), 2);
        assert!(tools.contains(&"tool1".to_string()));
        assert!(tools.contains(&"tool2".to_string()));
    }

    #[test]
    fn test_list_tools_sorted() {
        let mapper = ToolCallMapper::new();

        mapper
            .register_mapping(ToolCallMapping::new(
                "z-tool",
                CapabilityId::from_string("cap.z".to_string()).unwrap(),
                ConnectorId::from_string("conn.z".to_string()).unwrap(),
            ))
            .unwrap();
        mapper
            .register_mapping(ToolCallMapping::new(
                "a-tool",
                CapabilityId::from_string("cap.a".to_string()).unwrap(),
                ConnectorId::from_string("conn.a".to_string()).unwrap(),
            ))
            .unwrap();

        let tools = mapper.list_tools_sorted().unwrap();
        assert_eq!(tools, vec!["a-tool".to_string(), "z-tool".to_string()]);
    }

    #[test]
    fn test_apply_visibility_filters_allow_and_deny() {
        let mapper = ToolCallMapper::new();
        mapper
            .register_mapping(ToolCallMapping::new(
                "Read",
                CapabilityId::from_string("fs.read".to_string()).unwrap(),
                ConnectorId::from_string("local".to_string()).unwrap(),
            ))
            .unwrap();
        mapper
            .register_mapping(ToolCallMapping::new(
                "Write",
                CapabilityId::from_string("fs.write".to_string()).unwrap(),
                ConnectorId::from_string("local".to_string()).unwrap(),
            ))
            .unwrap();
        mapper
            .register_mapping(ToolCallMapping::new(
                "WebSearch",
                CapabilityId::from_string("web.search".to_string()).unwrap(),
                ConnectorId::from_string("local".to_string()).unwrap(),
            ))
            .unwrap();

        let removed = mapper
            .apply_visibility_filters(
                &[String::from("W*"), String::from("Read")],
                &[String::from("Write")],
            )
            .unwrap();

        assert_eq!(removed, vec!["Write".to_string()]);
        let visible = mapper.list_tools_sorted().unwrap();
        assert_eq!(visible, vec!["Read".to_string(), "WebSearch".to_string()]);
    }

    #[test]
    fn test_apply_visibility_filters_deny_only() {
        let mapper = ToolCallMapper::new();
        mapper
            .register_mapping(ToolCallMapping::new(
                "TaskCreate",
                CapabilityId::from_string("host.task.create".to_string()).unwrap(),
                ConnectorId::from_string("local".to_string()).unwrap(),
            ))
            .unwrap();
        mapper
            .register_mapping(ToolCallMapping::new(
                "TaskList",
                CapabilityId::from_string("host.task.list".to_string()).unwrap(),
                ConnectorId::from_string("local".to_string()).unwrap(),
            ))
            .unwrap();

        let removed = mapper
            .apply_visibility_filters(&[], &[String::from("Task*")])
            .unwrap();
        assert_eq!(
            removed,
            vec!["TaskCreate".to_string(), "TaskList".to_string()]
        );
        assert!(mapper.list_tools().unwrap().is_empty());
    }

    #[test]
    fn test_has_tool() {
        let mapper = ToolCallMapper::new();

        mapper
            .register_mapping(ToolCallMapping::new(
                "existing_tool",
                CapabilityId::from_string("cap".to_string()).unwrap(),
                ConnectorId::from_string("conn".to_string()).unwrap(),
            ))
            .unwrap();

        assert!(mapper.has_tool("existing_tool"));
        assert!(!mapper.has_tool("missing_tool"));
    }

    #[test]
    fn test_invalid_json_arguments() {
        let mapper = ToolCallMapper::new();

        mapper
            .register_mapping(ToolCallMapping::new(
                "test_tool",
                CapabilityId::from_string("test.cap".to_string()).unwrap(),
                ConnectorId::from_string("test".to_string()).unwrap(),
            ))
            .unwrap();

        let tool_call = ToolCall {
            id: "call-123".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "test_tool".to_string(),
                arguments: "invalid json".to_string(),
            },
        };

        let result = mapper.map_tool_call(&tool_call, "trace-123".to_string());
        assert!(result.is_err());

        match result {
            Err(BridgeError::ArgumentParseFailed { .. }) => {}
            _ => panic!("Expected ArgumentParseFailed error"),
        }
    }

    // ---------------------------------------------------------------
    // Sprint 44 — SkillCompat-aware mapping tests
    // ---------------------------------------------------------------

    /// Build a mapper pre-populated with cyberclaw facade names that the
    /// translators in skill-runtime alias to (`browser.navigate`, `cmd.run`,
    /// `fs.read`, etc.).
    fn facade_mapper() -> ToolCallMapper {
        let mapper = ToolCallMapper::new();
        let entries: &[(&str, &str, &str)] = &[
            ("browser.navigate", "browser.navigate", "browser"),
            ("cmd.run", "cmd.run", "local"),
            ("fs.read", "fs.read", "local"),
        ];
        for (alias, cap, conn) in entries {
            mapper
                .register_mapping(ToolCallMapping::new(
                    *alias,
                    CapabilityId::from_string(cap.to_string()).unwrap(),
                    ConnectorId::from_string(conn.to_string()).unwrap(),
                ))
                .unwrap();
        }
        mapper
    }

    #[test]
    fn resolve_with_compat_no_bindings_falls_back_to_original() {
        // 无 translator → 行为与 map_tool_call 一致：raw name 必须直接命中。
        let mapper = facade_mapper();
        let tool_call = create_test_tool_call("fs.read", serde_json::json!({ "path": "/tmp/x" }));

        let req = mapper
            .map_tool_call_with_compat(&tool_call, "trace-1".to_string(), None)
            .unwrap();
        assert_eq!(req.capability_id.as_ref(), "fs.read");
        assert_eq!(req.connector_id.as_ref(), "local");
    }

    #[test]
    fn resolve_with_compat_translates_hermes_browser_navigate() {
        // 模拟 hermes ecosystem translator：browser_navigate → browser.navigate
        let mapper = facade_mapper();
        let translator = |name: &str| -> Option<String> {
            match name {
                "browser_navigate" => Some("browser.navigate".to_string()),
                "fs_read" => Some("fs.read".to_string()),
                _ => None,
            }
        };

        let tool_call = create_test_tool_call(
            "browser_navigate",
            serde_json::json!({ "url": "https://example.com" }),
        );

        let req = mapper
            .map_tool_call_with_compat(&tool_call, "trace-2".to_string(), Some(&translator))
            .unwrap();
        assert_eq!(req.capability_id.as_ref(), "browser.navigate");
        assert_eq!(req.connector_id.as_ref(), "browser");
        assert_eq!(req.input["url"], "https://example.com");
    }

    #[test]
    fn resolve_with_compat_translates_anthropic_bash() {
        // 模拟 anthropic ecosystem translator：Bash → cmd.run
        let mapper = facade_mapper();
        let translator = |name: &str| -> Option<String> {
            match name {
                "Bash" => Some("cmd.run".to_string()),
                "Read" => Some("fs.read".to_string()),
                _ => None,
            }
        };

        let tool_call = create_test_tool_call("Bash", serde_json::json!({ "command": "ls -la" }));

        let req = mapper
            .map_tool_call_with_compat(&tool_call, "trace-3".to_string(), Some(&translator))
            .unwrap();
        assert_eq!(req.capability_id.as_ref(), "cmd.run");
        assert_eq!(req.connector_id.as_ref(), "local");
    }

    #[test]
    fn resolve_with_compat_passthrough_unknown() {
        // 翻译失败（None）+ 原 raw name 也未注册 → UnknownTool（透传后照常解析）
        let mapper = facade_mapper();
        let translator = |_: &str| -> Option<String> { None };

        let tool_call = create_test_tool_call("totally_unknown_tool", serde_json::json!({}));

        let result =
            mapper.map_tool_call_with_compat(&tool_call, "trace-4".to_string(), Some(&translator));
        match result {
            Err(BridgeError::UnknownTool { tool_name }) => {
                assert_eq!(tool_name, "totally_unknown_tool");
            }
            _ => panic!("expected UnknownTool"),
        }
    }
}
