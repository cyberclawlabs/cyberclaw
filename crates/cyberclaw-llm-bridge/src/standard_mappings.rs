//! Standard Tool Mappings
//!
//! 提供标准的 Tool Call 到 Capability 的映射配置。

use crate::mapper::ToolCallMapper;
use crate::tool_filter::is_tool_enabled;
use crate::types::ToolCallMapping;
use crate::BridgeResult;
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use cyberclaw_llm::types::ToolDefinition;
use serde_json::json;
use tracing::info;

/// 注册所有标准工具映射
pub fn register_standard_mappings(mapper: &ToolCallMapper) -> BridgeResult<()> {
    info!("Registering standard tool mappings");

    // Local filesystem capabilities
    register_filesystem_tools(mapper)?;

    // Search capabilities
    register_search_tools(mapper)?;

    // Command execution capabilities
    register_command_tools(mapper)?;

    // Web capabilities
    register_web_tools(mapper)?;

    // Browser CDP capabilities (R-01 — Hermes v0.12)
    register_browser_tools(mapper)?;

    // Security capabilities (BT-04/25 OSV CVE scan)
    register_security_tools(mapper)?;

    // Vision capability (R-02 — Hermes v0.12)
    register_vision_tools(mapper)?;

    // Image generation (R-03 — Hermes v0.12)
    register_image_gen_tools(mapper)?;

    // Audio transcription + synthesis (R-04, R-05 — Hermes v0.12)
    register_audio_tools(mapper)?;

    // Claude Code tool name aliases
    register_claude_alias_tools(mapper)?;
    register_claude_tool_class_aliases(mapper)?;
    register_claude_host_tool_mappings(mapper)?;

    // Sprint 19 W2/W3 — memory and todo connectors
    register_memory_tools(mapper)?;
    register_todo_tools(mapper)?;

    // Sprint 20 W1 — LSP connector aliases. Server only registers
    // the LspConnector when CYBERCLAW_LSP_ENABLED=true; without
    // registration these tool calls dispatch to a missing connector.
    register_lsp_tools(mapper)?;

    // D1 fix (2026-05-12) — task.* and workdir.* mapper drift.
    // The connector facades expose `task_*` / `workdir.*` LLM-side
    // names, but no mapping pointed them at the real `task.*` /
    // `workdir.*` capability_ids. See capability-skill-coverage-2026-05-12.md §D1.
    register_task_tools(mapper)?;
    register_workdir_tools(mapper)?;

    // MCP tool family (Claude Code compatible names)
    register_mcp_tool_mappings(mapper, "mcp-default")?;

    info!("Standard tool mappings registered successfully");
    Ok(())
}

/// 注册 MCP 工具映射（可指定 connector ID）
pub fn register_mcp_tool_mappings(mapper: &ToolCallMapper, connector_id: &str) -> BridgeResult<()> {
    register_mcp_tools(mapper, connector_id)
}

/// Sprint 19 W2 — register `memory_read` / `memory_write` / `memory_search`
/// tool aliases pointing at the in-process MemoryConnector
/// (`apps/cyberclaw-server/src/memory_connector.rs`).
fn register_memory_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    let memory_connector =
        ConnectorId::from_string("memory".to_string()).expect("valid connector id");
    for (tool_name, cap_id, desc) in [
        (
            "memory_read",
            "memory_read",
            "Read agent memory by scope+key",
        ),
        (
            "memory_write",
            "memory_write",
            "Write a value into agent memory",
        ),
        (
            "memory_search",
            "memory_search",
            "List memory records by scope (optional key_prefix filter)",
        ),
    ] {
        mapper.register_mapping(
            ToolCallMapping::new(
                tool_name,
                CapabilityId::from_string(cap_id.to_string()).expect("valid capability id"),
                memory_connector.clone(),
            )
            .with_description(desc),
        )?;
    }
    Ok(())
}

/// Sprint 20 W1 — register LSP tool aliases pointing at the
/// LspConnector (`crates/cyberclaw-connectors/src/local/lsp.rs`).
///
/// Three of the four facade names use dot notation (`lsp.hover`,
/// `lsp.diagnostics`); the 4th group uses underscore-style names
/// (`lsp_hover` etc.) for backwards compat with older agent prompts.
/// Capability ids match the LspConnector contracts:
///
///   - `lsp.hover`              → `lsp.hover`        (passthrough)
///   - `lsp.diagnostics`        → `lsp.diagnostics`  (passthrough)
///   - `lsp.goto_definition`    → `lsp.definition`   (facade name → cap rename)
///   - `lsp.find_references`    → `lsp.references`   (facade name → cap rename)
fn register_lsp_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    let lsp_connector = ConnectorId::from_string("lsp".to_string()).expect("valid connector id");

    let entries = [
        (
            "lsp.hover",
            "lsp.hover",
            "Hover info at (file, line, column)",
        ),
        ("lsp_hover", "lsp.hover", "Hover info — alias"),
        (
            "lsp.diagnostics",
            "lsp.diagnostics",
            "Current diagnostics for a file",
        ),
        ("lsp_diagnostics", "lsp.diagnostics", "Diagnostics — alias"),
        (
            "lsp.goto_definition",
            "lsp.definition",
            "Go to definition for symbol at position",
        ),
        (
            "lsp_goto_definition",
            "lsp.definition",
            "Goto definition — alias",
        ),
        (
            "lsp.find_references",
            "lsp.references",
            "Find references for symbol at position",
        ),
        (
            "lsp_find_references",
            "lsp.references",
            "Find references — alias",
        ),
    ];

    for (tool_name, cap_id, desc) in entries {
        mapper.register_mapping(
            ToolCallMapping::new(
                tool_name,
                CapabilityId::from_string(cap_id.to_string()).expect("valid capability id"),
                lsp_connector.clone(),
            )
            .with_description(desc),
        )?;
    }
    Ok(())
}

/// Sprint 19 W3 — register `todo_read` / `todo_write` tool aliases
/// pointing at the TodoConnector (`apps/cyberclaw-server/src/todo_connector.rs`).
fn register_todo_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    let todo_connector = ConnectorId::from_string("todo".to_string()).expect("valid connector id");
    mapper.register_mapping(
        ToolCallMapping::new(
            "todo_read",
            CapabilityId::from_string("agent:todo:read".to_string()).expect("valid capability id"),
            todo_connector.clone(),
        )
        .with_description("Read the todo list for the specified agent_id"),
    )?;
    mapper.register_mapping(
        ToolCallMapping::new(
            "todo_write",
            CapabilityId::from_string("agent:todo:write".to_string()).expect("valid capability id"),
            todo_connector,
        )
        .with_description("Add / update / remove a todo item by action"),
    )?;
    Ok(())
}

/// 注册文件系统工具
///
/// Tool names match `BuiltinToolRegistry`'s facade names so LLM-emitted
/// `tool_calls` (file_read / file_write / file_edit / bash / search_code /
/// find_files / web_fetch / web_search) resolve to the right capability.
/// Legacy aliases (read_file / write_file / edit_file / execute_command)
/// stay registered for backwards compatibility with older prompts.
fn register_filesystem_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    // fs.read — canonical "file_read" plus the legacy "read_file" alias.
    for tool_name in ["file_read", "read_file"] {
        mapper.register_mapping(
            ToolCallMapping::new(
                tool_name,
                CapabilityId::from_string("fs.read".to_string()).expect("valid capability id"),
                ConnectorId::from_string("local".to_string()).expect("valid connector id"),
            )
            .with_description("Read contents of a file from the filesystem")
            .with_parameter_mapping("file_path", "path"),
        )?;
    }

    // fs.write — canonical "file_write" plus the legacy "write_file".
    for tool_name in ["file_write", "write_file"] {
        mapper.register_mapping(
            ToolCallMapping::new(
                tool_name,
                CapabilityId::from_string("fs.write".to_string()).expect("valid capability id"),
                ConnectorId::from_string("local".to_string()).expect("valid connector id"),
            )
            .with_description("Write contents to a file")
            .with_parameter_mapping("file_path", "path")
            .with_parameter_mapping("content", "content"),
        )?;
    }

    // fs.edit — canonical "file_edit" plus the legacy "edit_file".
    for tool_name in ["file_edit", "edit_file"] {
        mapper.register_mapping(
            ToolCallMapping::new(
                tool_name,
                CapabilityId::from_string("fs.edit".to_string()).expect("valid capability id"),
                ConnectorId::from_string("local".to_string()).expect("valid connector id"),
            )
            .with_description("Replace text in a file")
            .with_parameter_mapping("file_path", "path")
            .with_parameter_mapping("old_text", "old_string")
            .with_parameter_mapping("new_text", "new_string"),
        )?;
    }

    // fs.multiedit — canonical "file_multiedit"
    mapper.register_mapping(
        ToolCallMapping::new(
            "file_multiedit",
            CapabilityId::from_string("fs.multiedit".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Apply multiple text replacements to a file in one atomic operation")
        .with_parameter_mapping("file_path", "path"),
    )?;

    // BT-05 (Hermes benchmark) — apply unified-diff patch.
    mapper.register_mapping(
        ToolCallMapping::new(
            "patch_apply",
            CapabilityId::from_string("fs.patch_apply".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Apply a unified-diff patch to a file")
        .with_parameter_mapping("file_path", "path")
        .with_parameter_mapping("patch", "patch"),
    )?;

    // D1 fix (2026-05-12) — close mapper drift for 4 fs facades that
    // BuiltinToolRegistry/connector facades expose to the LLM but that
    // had no entry in this mapper. Without these mappings the dispatcher
    // saw the bare LLM-side `tool_name` as the capability_id and the
    // local connector returned "Connector local does not support
    // capability `file_*`". See capability-skill-coverage-2026-05-12.md.
    //
    // NOTE: the four target capabilities (`fs.append / fs.stat /
    // fs.delete / fs.list_dir`) are declared in
    // `crates/cyberclaw-connectors/src/local/fs.rs` (functions are
    // marked `#[allow(dead_code)]`) but are NOT yet wired into
    // `LocalConnector::execute()` and not present in
    // `LocalConnector::build_capabilities()`. The mapper part is now
    // correct; the runtime fix for the missing dispatch arms is
    // tracked as a separate Connector-layer change (out of D1 scope,
    // which is mapper-only per the task statement).

    // fs.append — append text to an existing file.
    mapper.register_mapping(
        ToolCallMapping::new(
            "file_append",
            CapabilityId::from_string("fs.append".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Append text to the end of an existing file")
        .with_parameter_mapping("file_path", "path")
        .with_parameter_mapping("content", "content"),
    )?;

    // fs.stat — metadata for a path.
    mapper.register_mapping(
        ToolCallMapping::new(
            "file_stat",
            CapabilityId::from_string("fs.stat".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Return metadata for a path (existence, type, size, mtime, perms)")
        .with_parameter_mapping("file_path", "path"),
    )?;

    // fs.delete — remove a file or directory tree.
    mapper.register_mapping(
        ToolCallMapping::new(
            "file_delete",
            CapabilityId::from_string("fs.delete".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Delete a file. Pass recursive=true to remove a directory tree")
        .with_parameter_mapping("file_path", "path")
        .with_parameter_mapping("recursive", "recursive"),
    )?;

    // fs.list_dir — immediate children of a directory.
    //
    // Note: the legacy mapping in `register_search_tools` pointed
    // `file_list` at `search.glob`, but the FS connector facade
    // (`fs::capability_facades()`) declares `file_list` as a wrapper
    // around `fs.list_dir`. The dispatcher rejected `search.glob` calls
    // here because the LLM passed `path` (per the facade schema) but
    // `search.glob` requires `pattern`. Routing to `fs.list_dir` matches
    // the facade contract and the LLM's emitted arguments.
    mapper.register_mapping(
        ToolCallMapping::new(
            "file_list",
            CapabilityId::from_string("fs.list_dir".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("List the immediate children of a directory")
        .with_parameter_mapping("file_path", "path")
        .with_parameter_mapping("directory", "path")
        .with_parameter_mapping("include_hidden", "include_hidden"),
    )?;

    Ok(())
}

/// 注册搜索工具
fn register_search_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    // search.grep — facade names: file_search (canonical), search_code (legacy).
    for tool_name in ["file_search", "search_code"] {
        mapper.register_mapping(
            ToolCallMapping::new(
                tool_name,
                CapabilityId::from_string("search.grep".to_string()).expect("valid capability id"),
                ConnectorId::from_string("local".to_string()).expect("valid connector id"),
            )
            .with_description("Search for text patterns in files using grep")
            .with_parameter_mapping("query", "pattern")
            .with_parameter_mapping("directory", "path"),
        )?;
    }

    // search.glob — facade name `find_files` (legacy). Note: `file_list`
    // used to route here, but D1 (2026-05-12) moved it back to
    // `fs.list_dir` so the facade contract and the LLM's path argument
    // line up. `search_glob` is registered as an explicit alias so agents
    // that emit the underscore form still reach the real `search.glob`
    // capability.
    for tool_name in ["find_files", "search_glob"] {
        mapper.register_mapping(
            ToolCallMapping::new(
                tool_name,
                CapabilityId::from_string("search.glob".to_string()).expect("valid capability id"),
                ConnectorId::from_string("local".to_string()).expect("valid connector id"),
            )
            .with_description("Find files matching a pattern using glob")
            .with_parameter_mapping("pattern", "pattern")
            .with_parameter_mapping("directory", "base_path"),
        )?;
    }

    Ok(())
}

/// D1 fix (2026-05-12) — register `task_*` LLM-side names to the
/// `host.task.*` capabilities the `local` connector actually dispatches.
///
/// Although `LocalTaskConnector` (connector_id `local_task`) declares
/// `task.create / task.list / task.get / task.update / task.stop /
/// task.output` (dot form), the server bootstrap composes the
/// **`local`** connector with `host.task.*` capability ids (see
/// `crates/cyberclaw-connectors/src/local/host.rs` and
/// `crates/cyberclaw-connectors/src/local/mod.rs::execute`). The
/// `LocalTaskConnector` is currently unwired — `/api/v1/capabilities`
/// returns 75 entries, none of which are `task.*` (only `host.task.*`).
/// Routing the LLM-side names directly to `host.task.*` on `local`
/// matches the dispatcher and lets `task_create` etc. succeed without
/// renaming any underlying capability id. See
/// capability-skill-coverage-2026-05-12.md §D1.
fn register_task_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    let local_connector =
        ConnectorId::from_string("local".to_string()).expect("valid connector id");
    for (tool_name, cap_id, desc) in [
        (
            "task_create",
            "host.task.create",
            "Create a new managed task. Returns a task_id for lifecycle tracking",
        ),
        (
            "task_list",
            "host.task.list",
            "List managed tasks. Pass active_only=true to see only pending/running tasks",
        ),
        (
            "task_get",
            "host.task.get",
            "Fetch the full record for a task by task_id",
        ),
        (
            "task_update",
            "host.task.update",
            "Update a task's status and/or metadata by task_id",
        ),
        (
            "task_stop",
            "host.task.stop",
            "Cancel a running or pending task by task_id",
        ),
        (
            "task_output",
            "host.task.output",
            "Fetch captured text output and structured result for a task by task_id",
        ),
    ] {
        mapper.register_mapping(
            ToolCallMapping::new(
                tool_name,
                CapabilityId::from_string(cap_id.to_string()).expect("valid capability id"),
                local_connector.clone(),
            )
            .with_description(desc),
        )?;
    }
    Ok(())
}

/// D1 fix (2026-05-12) — register `workdir_*` LLM-side names to the
/// `workdir.*` capabilities owned by the `local` connector.
///
/// The connector facade declares the LLM-side name with a literal dot
/// (`workdir.checkpoint`, `workdir.list`), but `tool_name` in many LLM
/// providers' tool-call JSON Schema disallows dot characters. We register
/// the canonical underscore form (`workdir_checkpoint`) **and** keep the
/// dot form as a backwards-compat alias for any prompt that learned the
/// old name. Both route to the same `workdir.*` capability_id.
/// See capability-skill-coverage-2026-05-12.md §D1.
fn register_workdir_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    let local_connector =
        ConnectorId::from_string("local".to_string()).expect("valid connector id");
    let entries: &[(&str, &str, &str)] = &[
        (
            "workdir_checkpoint",
            "workdir.checkpoint",
            "Take a shadow-git snapshot of a workdir. Returns the commit hash",
        ),
        (
            "workdir.checkpoint",
            "workdir.checkpoint",
            "Take a shadow-git snapshot of a workdir (legacy dot alias)",
        ),
        (
            "workdir_list",
            "workdir.list",
            "List checkpoints for a workdir in reverse-chronological order",
        ),
        (
            "workdir.list",
            "workdir.list",
            "List workdir checkpoints (legacy dot alias)",
        ),
    ];
    for (tool_name, cap_id, desc) in entries {
        mapper.register_mapping(
            ToolCallMapping::new(
                *tool_name,
                CapabilityId::from_string((*cap_id).to_string()).expect("valid capability id"),
                local_connector.clone(),
            )
            .with_description(*desc)
            .with_parameter_mapping("path", "workdir")
            .with_parameter_mapping("workdir", "workdir")
            .with_parameter_mapping("limit", "limit"),
        )?;
    }
    Ok(())
}

/// 注册命令执行工具
///
/// R-1 (2026-05-05) — `bash` / `execute_command` facades route to the
/// unrestricted `cmd.run` capability so agents can actually run business
/// validation scripts (python3, node, pytest, etc.). The legacy
/// `cmd.exec` whitelist (ls/cat/grep/rg/...) was too restrictive for
/// general AGI delivery and broke GA-02 / GA-04 self-verification.
/// Governance is enforced inside `cmd.run` via a host-level command
/// content gate (`cyberclaw_governance::cmd_safety`) and via the
/// PolicyEngine, which classifies `cmd.run` as RiskLevel::High.
///
/// F2 (2026-05-12) — `cmd_run` is the canonical tool_id (matches what
/// `/api/v1/capabilities` exposes). `bash` and `execute_command` are
/// legacy aliases retained for backward compatibility only. Canonical
/// name is listed first so that logging and diagnostics show `cmd_run`.
fn register_command_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    for tool_name in ["cmd_run", "bash", "execute_command"] {
        mapper.register_mapping(
            ToolCallMapping::new(
                tool_name,
                CapabilityId::from_string("cmd.run".to_string()).expect("valid capability id"),
                ConnectorId::from_string("local".to_string()).expect("valid connector id"),
            )
            .with_description("Execute a shell command (unrestricted, governance-gated)")
            .with_parameter_mapping("command", "command")
            .with_parameter_mapping("args", "args")
            .with_parameter_mapping("working_dir", "workdir")
            .with_parameter_mapping("workdir", "workdir")
            .with_parameter_mapping("timeout", "timeout_ms")
            .with_parameter_mapping("timeout_ms", "timeout_ms"),
        )?;
    }

    Ok(())
}

/// 注册 Web 工具
fn register_web_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    // WebFetch - 抓取网页内容（Claude Code 同名）
    mapper.register_mapping(
        ToolCallMapping::new(
            "WebFetch",
            CapabilityId::from_string("web.fetch".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Fetch web page content from a URL")
        .with_parameter_mapping("url", "url")
        .with_parameter_mapping("prompt", "prompt")
        .with_parameter_mapping("max_bytes", "max_bytes")
        .with_parameter_mapping("timeout_ms", "timeout_ms"),
    )?;

    // Backward-compatible alias
    mapper.register_mapping(
        ToolCallMapping::new(
            "web_fetch",
            CapabilityId::from_string("web.fetch".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Fetch web page content from a URL")
        .with_parameter_mapping("url", "url")
        .with_parameter_mapping("prompt", "prompt")
        .with_parameter_mapping("max_bytes", "max_bytes")
        .with_parameter_mapping("timeout_ms", "timeout_ms"),
    )?;

    // WebSearch - Web 搜索（Claude Code 同名）
    mapper.register_mapping(
        ToolCallMapping::new(
            "WebSearch",
            CapabilityId::from_string("web.search".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Search web results with query text")
        .with_parameter_mapping("query", "query")
        .with_parameter_mapping("allowed_domains", "allowed_domains")
        .with_parameter_mapping("blocked_domains", "blocked_domains")
        .with_parameter_mapping("max_results", "max_results")
        .with_parameter_mapping("endpoint", "endpoint")
        .with_parameter_mapping("timeout_ms", "timeout_ms"),
    )?;

    // Backward-compatible alias
    mapper.register_mapping(
        ToolCallMapping::new(
            "web_search",
            CapabilityId::from_string("web.search".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Search web results with query text")
        .with_parameter_mapping("query", "query")
        .with_parameter_mapping("allowed_domains", "allowed_domains")
        .with_parameter_mapping("blocked_domains", "blocked_domains")
        .with_parameter_mapping("max_results", "max_results")
        .with_parameter_mapping("endpoint", "endpoint")
        .with_parameter_mapping("timeout_ms", "timeout_ms"),
    )?;

    Ok(())
}

fn register_browser_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    let entries = [
        (
            "browser_navigate",
            "browser.navigate",
            "Navigate an attached CDP browser page",
            vec![("url", "url"), ("wait_until", "wait_until")],
        ),
        (
            "browser_click",
            "browser.click",
            "Click an element in an attached CDP browser page",
            vec![
                ("selector", "selector"),
                ("button", "button"),
                ("click_count", "click_count"),
            ],
        ),
        (
            "browser_fill",
            "browser.fill",
            "Fill an element in an attached CDP browser page",
            vec![
                ("selector", "selector"),
                ("text", "text"),
                ("clear", "clear"),
            ],
        ),
        (
            "browser_evaluate",
            "browser.evaluate",
            "Evaluate JavaScript in an attached CDP browser page",
            vec![
                ("expression", "expression"),
                ("await_promise", "await_promise"),
                ("return_by_value", "return_by_value"),
            ],
        ),
        (
            "browser_screenshot",
            "browser.screenshot",
            "Capture a screenshot from an attached CDP browser page",
            vec![
                ("path", "path"),
                ("format", "format"),
                ("quality", "quality"),
                ("full_page", "full_page"),
            ],
        ),
        (
            "browser_dialog_handle",
            "browser.dialog_handle",
            "Accept or dismiss a JavaScript dialog in an attached CDP browser page",
            vec![("accept", "accept"), ("prompt_text", "prompt_text")],
        ),
    ];

    for (tool, capability, description, mappings) in entries {
        let mut mapping = ToolCallMapping::new(
            tool,
            CapabilityId::from_string(capability.to_string()).expect("valid capability id"),
            ConnectorId::from_string("browser".to_string()).expect("valid connector id"),
        )
        .with_description(description);
        for (from, to) in mappings {
            mapping = mapping.with_parameter_mapping(from, to);
        }
        mapper.register_mapping(mapping)?;
    }

    Ok(())
}

/// Sprint 27 — register `osv_scan` tool pointing at `LocalConnector`'s
/// `security.osv_scan` capability (BT-04/25 from Hermes business test list).
///
/// B-4 (2026-05-05) — extended to register `verify_numeric` as an alias for
/// `verify.numeric_aggregate`, the LLM-side gate that re-computes numeric
/// aggregates over a CSV and reports diff vs claim.
fn register_security_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    mapper.register_mapping(
        ToolCallMapping::new(
            "osv_scan",
            CapabilityId::from_string("security.osv_scan".to_string())
                .expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Scan a lockfile for known CVEs via cargo-audit / OSV")
        .with_parameter_mapping("lockfile_path", "lockfile_path")
        .with_parameter_mapping("ecosystem", "ecosystem"),
    )?;

    // B-4 (2026-05-05) — verify_numeric facade.
    mapper.register_mapping(
        ToolCallMapping::new(
            "verify_numeric",
            CapabilityId::from_string("verify.numeric_aggregate".to_string())
                .expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description(
            "Re-compute sum / avg / count over a CSV column (optional group_by) and report \
             match=true/false vs the claimed value. Use this BEFORE asserting any numeric \
             result so the agent self-checks instead of hallucinating totals.",
        )
        .with_parameter_mapping("csv_path", "csv_path")
        .with_parameter_mapping("column", "column")
        .with_parameter_mapping("group_by", "group_by")
        .with_parameter_mapping("expected", "expected")
        .with_parameter_mapping("tolerance", "tolerance"),
    )?;
    Ok(())
}

/// R-04 + R-05 — register `audio_transcribe` + `audio_synthesize`.
fn register_audio_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    mapper.register_mapping(
        ToolCallMapping::new(
            "audio_transcribe",
            CapabilityId::from_string("audio.transcribe".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Transcribe an audio file via Whisper")
        .with_parameter_mapping("audio_path", "audio_path")
        .with_parameter_mapping("language", "language")
        .with_parameter_mapping("prompt", "prompt")
        .with_parameter_mapping("model", "model"),
    )?;
    mapper.register_mapping(
        ToolCallMapping::new(
            "audio_synthesize",
            CapabilityId::from_string("audio.synthesize".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Generate speech audio from text via OpenAI / ElevenLabs TTS")
        .with_parameter_mapping("text", "text")
        .with_parameter_mapping("provider", "provider")
        .with_parameter_mapping("voice", "voice")
        .with_parameter_mapping("format", "format")
        .with_parameter_mapping("model", "model"),
    )?;
    Ok(())
}

/// R-03 — register `image_generate` pointing at `image.generate`.
fn register_image_gen_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    mapper.register_mapping(
        ToolCallMapping::new(
            "image_generate",
            CapabilityId::from_string("image.generate".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Generate an image via DALL-E / Stability AI")
        .with_parameter_mapping("prompt", "prompt")
        .with_parameter_mapping("size", "size")
        .with_parameter_mapping("provider", "provider")
        .with_parameter_mapping("model", "model")
        .with_parameter_mapping("quality", "quality"),
    )?;
    Ok(())
}

/// R-02 — register `vision_analyze` pointing at `vision.analyze_image`.
fn register_vision_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    mapper.register_mapping(
        ToolCallMapping::new(
            "vision_analyze",
            CapabilityId::from_string("vision.analyze_image".to_string())
                .expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Analyze an image with a vision-capable LLM (Anthropic / OpenAI)")
        .with_parameter_mapping("image", "image")
        .with_parameter_mapping("prompt", "prompt")
        .with_parameter_mapping("mime_type", "mime_type")
        .with_parameter_mapping("max_tokens", "max_tokens")
        .with_parameter_mapping("provider", "provider"),
    )?;
    Ok(())
}

/// 注册 Claude Code 常用工具名别名
fn register_claude_alias_tools(mapper: &ToolCallMapper) -> BridgeResult<()> {
    // Read -> fs.read
    mapper.register_mapping(
        ToolCallMapping::new(
            "Read",
            CapabilityId::from_string("fs.read".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude Read tool alias")
        .with_parameter_mapping("file_path", "path")
        .with_parameter_mapping("offset", "offset")
        .with_parameter_mapping("limit", "limit"),
    )?;

    // Write -> fs.write
    mapper.register_mapping(
        ToolCallMapping::new(
            "Write",
            CapabilityId::from_string("fs.write".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude Write tool alias")
        .with_parameter_mapping("file_path", "path")
        .with_parameter_mapping("content", "content"),
    )?;

    // Edit -> fs.edit
    mapper.register_mapping(
        ToolCallMapping::new(
            "Edit",
            CapabilityId::from_string("fs.edit".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude Edit tool alias")
        .with_parameter_mapping("file_path", "path")
        .with_parameter_mapping("old_string", "old_string")
        .with_parameter_mapping("new_string", "new_string")
        .with_parameter_mapping("old_text", "old_string")
        .with_parameter_mapping("new_text", "new_string")
        .with_parameter_mapping("replace_all", "replace_all"),
    )?;

    // Grep -> search.grep
    mapper.register_mapping(
        ToolCallMapping::new(
            "Grep",
            CapabilityId::from_string("search.grep".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude Grep tool alias")
        .with_parameter_mapping("pattern", "pattern")
        .with_parameter_mapping("path", "path")
        .with_parameter_mapping("glob", "glob")
        .with_parameter_mapping("max_results", "max_results"),
    )?;

    // Glob -> search.glob
    mapper.register_mapping(
        ToolCallMapping::new(
            "Glob",
            CapabilityId::from_string("search.glob".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude Glob tool alias")
        .with_parameter_mapping("pattern", "pattern")
        .with_parameter_mapping("path", "path")
        .with_parameter_mapping("max_results", "max_results"),
    )?;

    // Bash -> cmd.run (R-1 2026-05-05: switched from whitelist-restricted cmd.exec
    // so agents can run business validation scripts; governance enforced via PolicyEngine
    // + host-level command-content gate inside cmd.run).
    mapper.register_mapping(
        ToolCallMapping::new(
            "Bash",
            CapabilityId::from_string("cmd.run".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude Bash tool alias (unrestricted, governance-gated)")
        .with_parameter_mapping("command", "command")
        .with_parameter_mapping("workdir", "workdir")
        .with_parameter_mapping("timeout_ms", "timeout_ms"),
    )?;

    // PowerShell -> cmd.run_powershell (dedicated powershell capability)
    mapper.register_mapping(
        ToolCallMapping::new(
            "PowerShell",
            CapabilityId::from_string("cmd.run_powershell".to_string())
                .expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude PowerShell tool alias (PowerShell 7+ via pwsh)")
        .with_parameter_mapping("command", "script")
        .with_parameter_mapping("script", "script")
        .with_parameter_mapping("workdir", "workdir")
        .with_parameter_mapping("timeout_ms", "timeout_ms"),
    )?;

    Ok(())
}

/// 注册 Claude Code `*Tool` 类名别名（仅映射到已落地能力，禁止假实现）
fn register_claude_tool_class_aliases(mapper: &ToolCallMapper) -> BridgeResult<()> {
    mapper.register_mapping(
        ToolCallMapping::new(
            "FileReadTool",
            CapabilityId::from_string("fs.read".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude FileReadTool alias")
        .with_parameter_mapping("file_path", "path")
        .with_parameter_mapping("offset", "offset")
        .with_parameter_mapping("limit", "limit"),
    )?;

    mapper.register_mapping(
        ToolCallMapping::new(
            "FileWriteTool",
            CapabilityId::from_string("fs.write".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude FileWriteTool alias")
        .with_parameter_mapping("file_path", "path")
        .with_parameter_mapping("content", "content"),
    )?;

    mapper.register_mapping(
        ToolCallMapping::new(
            "FileEditTool",
            CapabilityId::from_string("fs.edit".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude FileEditTool alias")
        .with_parameter_mapping("file_path", "path")
        .with_parameter_mapping("old_string", "old_string")
        .with_parameter_mapping("new_string", "new_string")
        .with_parameter_mapping("old_text", "old_string")
        .with_parameter_mapping("new_text", "new_string")
        .with_parameter_mapping("replace_all", "replace_all"),
    )?;

    mapper.register_mapping(
        ToolCallMapping::new(
            "GrepTool",
            CapabilityId::from_string("search.grep".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude GrepTool alias")
        .with_parameter_mapping("pattern", "pattern")
        .with_parameter_mapping("query", "pattern")
        .with_parameter_mapping("path", "path")
        .with_parameter_mapping("directory", "path")
        .with_parameter_mapping("glob", "glob")
        .with_parameter_mapping("max_results", "max_results"),
    )?;

    mapper.register_mapping(
        ToolCallMapping::new(
            "GlobTool",
            CapabilityId::from_string("search.glob".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude GlobTool alias")
        .with_parameter_mapping("pattern", "pattern")
        .with_parameter_mapping("path", "path")
        .with_parameter_mapping("directory", "path")
        .with_parameter_mapping("max_results", "max_results"),
    )?;

    mapper.register_mapping(
        ToolCallMapping::new(
            "BashTool",
            CapabilityId::from_string("cmd.run".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude BashTool alias (unrestricted, governance-gated)")
        .with_parameter_mapping("command", "command")
        .with_parameter_mapping("workdir", "workdir")
        .with_parameter_mapping("timeout_ms", "timeout_ms"),
    )?;

    mapper.register_mapping(
        ToolCallMapping::new(
            "PowerShellTool",
            CapabilityId::from_string("cmd.run_powershell".to_string())
                .expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude PowerShellTool alias (PowerShell 7+ via pwsh)")
        .with_parameter_mapping("command", "script")
        .with_parameter_mapping("script", "script")
        .with_parameter_mapping("workdir", "workdir")
        .with_parameter_mapping("timeout_ms", "timeout_ms"),
    )?;

    mapper.register_mapping(
        ToolCallMapping::new(
            "WebFetchTool",
            CapabilityId::from_string("web.fetch".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude WebFetchTool alias")
        .with_parameter_mapping("url", "url")
        .with_parameter_mapping("prompt", "prompt")
        .with_parameter_mapping("max_bytes", "max_bytes")
        .with_parameter_mapping("timeout_ms", "timeout_ms"),
    )?;

    mapper.register_mapping(
        ToolCallMapping::new(
            "WebSearchTool",
            CapabilityId::from_string("web.search".to_string()).expect("valid capability id"),
            ConnectorId::from_string("local".to_string()).expect("valid connector id"),
        )
        .with_description("Claude WebSearchTool alias")
        .with_parameter_mapping("query", "query")
        .with_parameter_mapping("allowed_domains", "allowed_domains")
        .with_parameter_mapping("blocked_domains", "blocked_domains")
        .with_parameter_mapping("max_results", "max_results")
        .with_parameter_mapping("endpoint", "endpoint")
        .with_parameter_mapping("timeout_ms", "timeout_ms"),
    )?;

    mapper.register_mapping(
        ToolCallMapping::new(
            "SendMessageTool",
            CapabilityId::from_string("slack.send_message".to_string())
                .expect("valid capability id"),
            ConnectorId::from_string("slack".to_string()).expect("valid connector id"),
        )
        .with_description("Claude SendMessageTool alias (Slack backend)")
        .with_parameter_mapping("channel", "channel")
        .with_parameter_mapping("text", "text")
        .with_parameter_mapping("thread_ts", "thread_ts"),
    )?;

    Ok(())
}

fn host_mapping(tool_name: &str, capability_id: &str) -> ToolCallMapping {
    ToolCallMapping::new(
        tool_name,
        CapabilityId::from_string(capability_id.to_string()).expect("valid capability id"),
        ConnectorId::from_string("local".to_string()).expect("valid connector id"),
    )
}

/// 注册 Claude 宿主类工具映射（Plan/Task/Team/Skill 等）
fn register_claude_host_tool_mappings(mapper: &ToolCallMapper) -> BridgeResult<()> {
    // Agent / Ask / Brief
    mapper.register_mapping(
        host_mapping("Agent", "host.agent.run")
            .with_description("Claude Agent tool")
            .with_parameter_mapping("subagent_type", "agent_type")
            .with_parameter_mapping("prompt", "message")
            .with_parameter_mapping("task", "message"),
    )?;
    mapper.register_mapping(
        host_mapping("Task", "host.agent.run")
            .with_description("Claude legacy Agent alias")
            .with_parameter_mapping("subagent_type", "agent_type")
            .with_parameter_mapping("prompt", "message")
            .with_parameter_mapping("task", "message"),
    )?;
    mapper.register_mapping(
        host_mapping("AgentTool", "host.agent.run")
            .with_description("Claude AgentTool alias")
            .with_parameter_mapping("subagent_type", "agent_type")
            .with_parameter_mapping("prompt", "message")
            .with_parameter_mapping("task", "message"),
    )?;

    mapper.register_mapping(
        host_mapping("AskUserQuestion", "host.ask_user_question")
            .with_description("Claude AskUserQuestion tool"),
    )?;
    mapper.register_mapping(
        host_mapping("AskUserQuestionTool", "host.ask_user_question")
            .with_description("Claude AskUserQuestionTool alias"),
    )?;
    mapper.register_mapping(
        host_mapping("SendMessage", "host.agent.run")
            .with_description("Claude SendMessage tool")
            .with_parameter_mapping("to", "agent_id")
            .with_parameter_mapping("target", "agent_id")
            .with_parameter_mapping("text", "message"),
    )?;

    mapper.register_mapping(
        host_mapping("SendUserMessage", "host.brief").with_description("Claude Brief primary tool"),
    )?;
    mapper.register_mapping(
        host_mapping("Brief", "host.brief").with_description("Claude Brief alias"),
    )?;
    mapper.register_mapping(
        host_mapping("BriefTool", "host.brief").with_description("Claude BriefTool alias"),
    )?;

    // Config / Plan / Worktree
    mapper.register_mapping(
        host_mapping("Config", "host.config").with_description("Claude Config tool"),
    )?;
    mapper.register_mapping(
        host_mapping("ConfigTool", "host.config").with_description("Claude ConfigTool alias"),
    )?;

    mapper.register_mapping(
        host_mapping("EnterPlanMode", "host.plan.enter").with_description("Enter plan mode"),
    )?;
    mapper.register_mapping(
        host_mapping("EnterPlanModeTool", "host.plan.enter")
            .with_description("EnterPlanModeTool alias"),
    )?;
    mapper.register_mapping(
        host_mapping("ExitPlanMode", "host.plan.exit").with_description("Exit plan mode"),
    )?;
    mapper.register_mapping(
        host_mapping("ExitPlanModeTool", "host.plan.exit")
            .with_description("ExitPlanModeTool alias"),
    )?;

    mapper.register_mapping(
        host_mapping("EnterWorktree", "host.worktree.enter")
            .with_description("Enter worktree")
            .with_parameter_mapping("worktree", "path")
            .with_parameter_mapping("name", "path"),
    )?;
    mapper.register_mapping(
        host_mapping("EnterWorktreeTool", "host.worktree.enter")
            .with_description("EnterWorktreeTool alias")
            .with_parameter_mapping("worktree", "path")
            .with_parameter_mapping("name", "path"),
    )?;
    mapper.register_mapping(
        host_mapping("ExitWorktree", "host.worktree.exit").with_description("Exit worktree"),
    )?;
    mapper.register_mapping(
        host_mapping("ExitWorktreeTool", "host.worktree.exit")
            .with_description("ExitWorktreeTool alias"),
    )?;

    // IDE / notebook / shell-like host tools
    mapper.register_mapping(host_mapping("LSP", "host.lsp").with_description("Claude LSP tool"))?;
    mapper.register_mapping(
        host_mapping("LSPTool", "host.lsp").with_description("Claude LSPTool alias"),
    )?;
    mapper.register_mapping(
        host_mapping("McpAuthTool", "host.mcp.auth").with_description("Claude MCP auth tool"),
    )?;
    mapper.register_mapping(
        host_mapping("NotebookEdit", "host.notebook.edit")
            .with_description("Claude NotebookEdit tool")
            .with_parameter_mapping("notebook_path", "notebook_path")
            .with_parameter_mapping("path", "notebook_path"),
    )?;
    mapper.register_mapping(
        host_mapping("NotebookEditTool", "host.notebook.edit")
            .with_description("Claude NotebookEditTool alias")
            .with_parameter_mapping("path", "notebook_path"),
    )?;
    mapper
        .register_mapping(host_mapping("REPL", "host.repl").with_description("Claude REPL tool"))?;
    mapper.register_mapping(
        host_mapping("REPLTool", "host.repl").with_description("Claude REPLTool alias"),
    )?;
    mapper.register_mapping(
        host_mapping("RemoteTrigger", "host.remote.trigger")
            .with_description("Claude RemoteTrigger tool"),
    )?;
    mapper.register_mapping(
        host_mapping("RemoteTriggerTool", "host.remote.trigger")
            .with_description("Claude RemoteTriggerTool alias"),
    )?;

    // Skills / sleep / structured output
    mapper.register_mapping(
        host_mapping("Skill", "host.skill.invoke")
            .with_description("Claude Skill tool")
            .with_parameter_mapping("command", "skill_name")
            .with_parameter_mapping("name", "skill_name"),
    )?;
    mapper.register_mapping(
        host_mapping("SkillTool", "host.skill.invoke")
            .with_description("Claude SkillTool alias")
            .with_parameter_mapping("command", "skill_name")
            .with_parameter_mapping("name", "skill_name"),
    )?;
    mapper.register_mapping(
        host_mapping("Sleep", "host.sleep")
            .with_description("Claude Sleep tool")
            .with_parameter_mapping("duration", "duration_ms")
            .with_parameter_mapping("seconds", "seconds")
            .with_parameter_mapping("milliseconds", "duration_ms"),
    )?;
    mapper.register_mapping(
        host_mapping("SleepTool", "host.sleep")
            .with_description("Claude SleepTool alias")
            .with_parameter_mapping("duration", "duration_ms")
            .with_parameter_mapping("seconds", "seconds")
            .with_parameter_mapping("milliseconds", "duration_ms"),
    )?;
    mapper.register_mapping(
        host_mapping("StructuredOutput", "host.synthetic.output")
            .with_description("Claude StructuredOutput tool"),
    )?;
    mapper.register_mapping(
        host_mapping("SyntheticOutputTool", "host.synthetic.output")
            .with_description("Claude SyntheticOutputTool alias"),
    )?;

    // Task lifecycle
    mapper.register_mapping(
        host_mapping("TaskCreate", "host.task.create")
            .with_description("Claude TaskCreate tool")
            .with_parameter_mapping("title", "subject"),
    )?;
    mapper.register_mapping(
        host_mapping("TaskCreateTool", "host.task.create")
            .with_description("Claude TaskCreateTool alias")
            .with_parameter_mapping("title", "subject"),
    )?;
    mapper.register_mapping(
        host_mapping("TaskGet", "host.task.get")
            .with_description("Claude TaskGet tool")
            .with_parameter_mapping("id", "task_id"),
    )?;
    mapper.register_mapping(
        host_mapping("TaskGetTool", "host.task.get")
            .with_description("Claude TaskGetTool alias")
            .with_parameter_mapping("id", "task_id"),
    )?;
    mapper.register_mapping(
        host_mapping("TaskList", "host.task.list").with_description("Claude TaskList tool"),
    )?;
    mapper.register_mapping(
        host_mapping("TaskListTool", "host.task.list")
            .with_description("Claude TaskListTool alias"),
    )?;
    mapper.register_mapping(
        host_mapping("TaskOutput", "host.task.output")
            .with_description("Claude TaskOutput tool")
            .with_parameter_mapping("id", "task_id")
            .with_parameter_mapping("text", "output"),
    )?;
    mapper.register_mapping(
        host_mapping("TaskOutputTool", "host.task.output")
            .with_description("Claude TaskOutputTool alias")
            .with_parameter_mapping("id", "task_id")
            .with_parameter_mapping("text", "output"),
    )?;
    mapper.register_mapping(
        host_mapping("TaskStop", "host.task.stop")
            .with_description("Claude TaskStop tool")
            .with_parameter_mapping("id", "task_id"),
    )?;
    mapper.register_mapping(
        host_mapping("TaskStopTool", "host.task.stop")
            .with_description("Claude TaskStopTool alias")
            .with_parameter_mapping("id", "task_id"),
    )?;
    mapper.register_mapping(
        host_mapping("TaskUpdate", "host.task.update")
            .with_description("Claude TaskUpdate tool")
            .with_parameter_mapping("id", "task_id")
            .with_parameter_mapping("title", "subject"),
    )?;
    mapper.register_mapping(
        host_mapping("TaskUpdateTool", "host.task.update")
            .with_description("Claude TaskUpdateTool alias")
            .with_parameter_mapping("id", "task_id")
            .with_parameter_mapping("title", "subject"),
    )?;

    // Team / todo / search
    mapper.register_mapping(
        host_mapping("TeamCreate", "host.team.create").with_description("Claude TeamCreate tool"),
    )?;
    mapper.register_mapping(
        host_mapping("TeamCreateTool", "host.team.create")
            .with_description("Claude TeamCreateTool alias"),
    )?;
    mapper.register_mapping(
        host_mapping("TeamDelete", "host.team.delete")
            .with_description("Claude TeamDelete tool")
            .with_parameter_mapping("team", "name")
            .with_parameter_mapping("team_name", "name"),
    )?;
    mapper.register_mapping(
        host_mapping("TeamDeleteTool", "host.team.delete")
            .with_description("Claude TeamDeleteTool alias")
            .with_parameter_mapping("team", "name")
            .with_parameter_mapping("team_name", "name"),
    )?;
    mapper.register_mapping(
        host_mapping("TodoWrite", "host.todo.write").with_description("Claude TodoWrite tool"),
    )?;
    mapper.register_mapping(
        host_mapping("TodoWriteTool", "host.todo.write")
            .with_description("Claude TodoWriteTool alias"),
    )?;
    mapper.register_mapping(
        host_mapping("ToolSearch", "host.tool.search")
            .with_description("Claude ToolSearch tool")
            .with_parameter_mapping("q", "query"),
    )?;
    mapper.register_mapping(
        host_mapping("ToolSearchTool", "host.tool.search")
            .with_description("Claude ToolSearchTool alias")
            .with_parameter_mapping("q", "query"),
    )?;

    // Cron tools in Claude Code
    mapper.register_mapping(
        host_mapping("CronCreate", "host.cron.create")
            .with_description("Claude CronCreate tool")
            .with_parameter_mapping("schedule", "cron")
            .with_parameter_mapping("task", "command"),
    )?;
    mapper.register_mapping(
        host_mapping("CronDelete", "host.cron.delete")
            .with_description("Claude CronDelete tool")
            .with_parameter_mapping("id", "job_id"),
    )?;
    mapper.register_mapping(
        host_mapping("CronList", "host.cron.list").with_description("Claude CronList tool"),
    )?;
    mapper.register_mapping(
        host_mapping("ScheduleCronTool", "host.cron.create")
            .with_description("Legacy schedule cron alias")
            .with_parameter_mapping("schedule", "cron")
            .with_parameter_mapping("task", "command"),
    )?;

    Ok(())
}

/// 注册 MCP 工具（对齐 Claude Code 常用工具名）
fn register_mcp_tools(mapper: &ToolCallMapper, connector_id: &str) -> BridgeResult<()> {
    let connector_id =
        ConnectorId::from_string(connector_id.to_string()).expect("valid mcp connector id");

    mapper.register_mapping(
        ToolCallMapping::new(
            "ListMcpResourcesTool",
            CapabilityId::from_string("mcp.list_resources".to_string())
                .expect("valid capability id"),
            connector_id.clone(),
        )
        .with_description("List resources exposed by MCP server"),
    )?;

    mapper.register_mapping(
        ToolCallMapping::new(
            "ReadMcpResourceTool",
            CapabilityId::from_string("mcp.read_resource".to_string())
                .expect("valid capability id"),
            connector_id.clone(),
        )
        .with_description("Read MCP resource by URI")
        .with_parameter_mapping("uri", "uri")
        .with_parameter_mapping("resource_uri", "uri"),
    )?;

    mapper.register_mapping(
        ToolCallMapping::new(
            "MCPTool",
            CapabilityId::from_string("mcp.call_tool".to_string()).expect("valid capability id"),
            connector_id.clone(),
        )
        .with_description("Call an MCP tool by name")
        .with_parameter_mapping("tool_name", "tool_name")
        .with_parameter_mapping("name", "tool_name")
        .with_parameter_mapping("tool", "tool_name")
        .with_parameter_mapping("arguments", "arguments")
        .with_parameter_mapping("input", "arguments"),
    )?;

    mapper.register_mapping(
        ToolCallMapping::new(
            "mcp",
            CapabilityId::from_string("mcp.call_tool".to_string()).expect("valid capability id"),
            connector_id,
        )
        .with_description("Call MCP tool by generic mcp name")
        .with_parameter_mapping("tool_name", "tool_name")
        .with_parameter_mapping("name", "tool_name")
        .with_parameter_mapping("tool", "tool_name")
        .with_parameter_mapping("arguments", "arguments")
        .with_parameter_mapping("input", "arguments"),
    )?;

    Ok(())
}

/// 获取所有标准工具的 LLM ToolDefinition
///
/// 这些定义可以直接传递给 LLM 的 chat completion API。
pub fn get_standard_tool_definitions() -> Vec<ToolDefinition> {
    let mut tools = vec![
        // File system tools
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "read_file".to_string(),
                description: "Read contents of a file from the filesystem".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Path to the file to read"
                        }
                    },
                    "required": ["file_path"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "write_file".to_string(),
                description: "Write contents to a file".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Path to the file to write"
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write to the file"
                        }
                    },
                    "required": ["file_path", "content"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "edit_file".to_string(),
                description: "Replace text in a file".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Path to the file to edit"
                        },
                        "old_text": {
                            "type": "string",
                            "description": "Text to find and replace"
                        },
                        "new_text": {
                            "type": "string",
                            "description": "Text to replace with"
                        }
                    },
                    "required": ["file_path", "old_text", "new_text"]
                }),
            },
            cache_control: None,
        },
        // Search tools
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "search_code".to_string(),
                description: "Search for text patterns in files using grep".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search pattern (regex supported)"
                        },
                        "directory": {
                            "type": "string",
                            "description": "Directory to search in (optional, defaults to workspace root)"
                        }
                    },
                    "required": ["query"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "find_files".to_string(),
                description: "Find files matching a pattern using glob".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern (e.g., '*.rs', 'src/**/*.ts')"
                        },
                        "directory": {
                            "type": "string",
                            "description": "Base directory to search from (optional)"
                        }
                    },
                    "required": ["pattern"]
                }),
            },
            cache_control: None,
        },
        // Command execution
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "execute_command".to_string(),
                description: "Execute a shell command (requires high privileges)".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "Command to execute"
                        },
                        "args": {
                            "type": "array",
                            "items": {
                                "type": "string"
                            },
                            "description": "Command arguments"
                        },
                        "working_dir": {
                            "type": "string",
                            "description": "Working directory (optional)"
                        }
                    },
                    "required": ["command"]
                }),
            },
            cache_control: None,
        },
        // Web tools
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "WebFetch".to_string(),
                description: "Fetch content from a URL and return extracted page content"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "HTTP/HTTPS URL to fetch"
                        },
                        "prompt": {
                            "type": "string",
                            "description": "Optional extraction intent, aligned with Claude WebFetch input"
                        },
                        "max_bytes": {
                            "type": "integer",
                            "description": "Optional max response bytes to keep"
                        },
                        "timeout_ms": {
                            "type": "integer",
                            "description": "Optional request timeout in milliseconds"
                        }
                    },
                    "required": ["url"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "WebSearch".to_string(),
                description: "Search the web for current information".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query text"
                        },
                        "allowed_domains": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Only include results from these domains"
                        },
                        "blocked_domains": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Exclude results from these domains"
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Optional maximum number of results"
                        },
                        "endpoint": {
                            "type": "string",
                            "description": "Optional custom search endpoint"
                        },
                        "timeout_ms": {
                            "type": "integer",
                            "description": "Optional request timeout in milliseconds"
                        }
                    },
                    "required": ["query"]
                }),
            },
            cache_control: None,
        },
        // Claude aliases
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "Read".to_string(),
                description: "Read a file from the local filesystem".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" },
                        "offset": { "type": "integer" },
                        "limit": { "type": "integer" }
                    },
                    "required": ["file_path"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "Write".to_string(),
                description: "Write content to a file".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["file_path", "content"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "Edit".to_string(),
                description: "Edit content in a file".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" },
                        "old_string": { "type": "string" },
                        "new_string": { "type": "string" },
                        "replace_all": { "type": "boolean" }
                    },
                    "required": ["file_path", "old_string", "new_string"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "Grep".to_string(),
                description: "Search text patterns in files".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string" },
                        "path": { "type": "string" },
                        "glob": { "type": "string" },
                        "max_results": { "type": "integer" }
                    },
                    "required": ["pattern"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "Glob".to_string(),
                description: "Find files by glob pattern".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string" },
                        "path": { "type": "string" },
                        "max_results": { "type": "integer" }
                    },
                    "required": ["pattern"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "Bash".to_string(),
                description: "Execute a shell command".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" },
                        "workdir": { "type": "string" },
                        "timeout_ms": { "type": "integer" }
                    },
                    "required": ["command"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "PowerShell".to_string(),
                description: "Execute a shell command (PowerShell alias)".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" },
                        "workdir": { "type": "string" },
                        "timeout_ms": { "type": "integer" }
                    },
                    "required": ["command"]
                }),
            },
            cache_control: None,
        },
        // Claude class-style aliases
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "FileReadTool".to_string(),
                description: "Claude FileReadTool compatibility alias".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" },
                        "offset": { "type": "integer" },
                        "limit": { "type": "integer" }
                    },
                    "required": ["file_path"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "FileWriteTool".to_string(),
                description: "Claude FileWriteTool compatibility alias".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" },
                        "content": { "type": "string" }
                    },
                    "required": ["file_path", "content"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "FileEditTool".to_string(),
                description: "Claude FileEditTool compatibility alias".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "file_path": { "type": "string" },
                        "old_string": { "type": "string" },
                        "new_string": { "type": "string" },
                        "replace_all": { "type": "boolean" }
                    },
                    "required": ["file_path", "old_string", "new_string"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "GrepTool".to_string(),
                description: "Claude GrepTool compatibility alias".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string" },
                        "path": { "type": "string" },
                        "glob": { "type": "string" },
                        "max_results": { "type": "integer" }
                    },
                    "required": ["pattern"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "GlobTool".to_string(),
                description: "Claude GlobTool compatibility alias".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string" },
                        "path": { "type": "string" },
                        "max_results": { "type": "integer" }
                    },
                    "required": ["pattern"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "BashTool".to_string(),
                description: "Claude BashTool compatibility alias".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" },
                        "workdir": { "type": "string" },
                        "timeout_ms": { "type": "integer" }
                    },
                    "required": ["command"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "PowerShellTool".to_string(),
                description: "Claude PowerShellTool compatibility alias".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string" },
                        "workdir": { "type": "string" },
                        "timeout_ms": { "type": "integer" }
                    },
                    "required": ["command"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "WebFetchTool".to_string(),
                description: "Claude WebFetchTool compatibility alias".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string" },
                        "prompt": { "type": "string" },
                        "max_bytes": { "type": "integer" },
                        "timeout_ms": { "type": "integer" }
                    },
                    "required": ["url"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "WebSearchTool".to_string(),
                description: "Claude WebSearchTool compatibility alias".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "allowed_domains": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "blocked_domains": {
                            "type": "array",
                            "items": { "type": "string" }
                        },
                        "max_results": { "type": "integer" },
                        "endpoint": { "type": "string" },
                        "timeout_ms": { "type": "integer" }
                    },
                    "required": ["query"]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "SendMessageTool".to_string(),
                description: "Claude SendMessageTool compatibility alias (Slack backend)"
                    .to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "channel": { "type": "string" },
                        "text": { "type": "string" },
                        "thread_ts": { "type": "string" }
                    },
                    "required": ["channel", "text"]
                }),
            },
            cache_control: None,
        },
        // MCP tools
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "ListMcpResourcesTool".to_string(),
                description: "List resources from MCP server".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string" }
                    }
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "ReadMcpResourceTool".to_string(),
                description: "Read a resource from MCP server".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string" },
                        "uri": { "type": "string" },
                        "resource_uri": { "type": "string" }
                    },
                    "anyOf": [
                        { "required": ["uri"] },
                        { "required": ["resource_uri"] }
                    ]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "MCPTool".to_string(),
                description: "Call MCP tool with arguments".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string" },
                        "tool_name": { "type": "string" },
                        "name": { "type": "string" },
                        "tool": { "type": "string" },
                        "arguments": { "type": "object" },
                        "input": { "type": "object" }
                    },
                    "anyOf": [
                        { "required": ["tool_name"] },
                        { "required": ["name"] },
                        { "required": ["tool"] }
                    ]
                }),
            },
            cache_control: None,
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionDefinition {
                name: "mcp".to_string(),
                description: "Call MCP tool with arguments".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "server": { "type": "string" },
                        "tool_name": { "type": "string" },
                        "name": { "type": "string" },
                        "tool": { "type": "string" },
                        "arguments": { "type": "object" },
                        "input": { "type": "object" }
                    },
                    "anyOf": [
                        { "required": ["tool_name"] },
                        { "required": ["name"] },
                        { "required": ["tool"] }
                    ]
                }),
            },
            cache_control: None,
        },
    ];
    tools.extend(get_claude_host_tool_definitions());
    tools
}

/// 获取按 allow/deny 规则过滤后的标准工具定义列表
pub fn get_standard_tool_definitions_filtered(
    allow_patterns: &[String],
    deny_patterns: &[String],
) -> Vec<ToolDefinition> {
    get_standard_tool_definitions()
        .into_iter()
        .filter(|tool| is_tool_enabled(&tool.function.name, allow_patterns, deny_patterns))
        .collect()
}

fn simple_tool_definition(name: &str, description: &str, required: &[&str]) -> ToolDefinition {
    let mut parameters = json!({
        "type": "object",
        "properties": {}
    });
    if !required.is_empty() {
        parameters["required"] = json!(required);
    }

    ToolDefinition {
        tool_type: "function".to_string(),
        function: cyberclaw_llm::types::FunctionDefinition {
            name: name.to_string(),
            description: description.to_string(),
            parameters,
        },
        cache_control: None,
    }
}

fn get_claude_host_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        simple_tool_definition("Agent", "Claude Agent tool", &["prompt"]),
        simple_tool_definition("Task", "Claude legacy Agent alias", &["prompt"]),
        simple_tool_definition("AgentTool", "Claude AgentTool alias", &["prompt"]),
        simple_tool_definition(
            "AskUserQuestion",
            "Ask user one or more short questions",
            &["question"],
        ),
        simple_tool_definition(
            "AskUserQuestionTool",
            "Ask user one or more short questions (alias)",
            &["question"],
        ),
        simple_tool_definition(
            "SendMessage",
            "Send message to existing agent session",
            &["to", "text"],
        ),
        simple_tool_definition(
            "SendUserMessage",
            "Send user-facing brief output",
            &["text"],
        ),
        simple_tool_definition("Brief", "Send user-facing brief output (alias)", &["text"]),
        simple_tool_definition(
            "BriefTool",
            "Send user-facing brief output (alias)",
            &["text"],
        ),
        simple_tool_definition("Config", "Read or update runtime config", &[]),
        simple_tool_definition("ConfigTool", "Read or update runtime config (alias)", &[]),
        simple_tool_definition("EnterPlanMode", "Enter plan mode", &[]),
        simple_tool_definition("EnterPlanModeTool", "Enter plan mode (alias)", &[]),
        simple_tool_definition("ExitPlanMode", "Exit plan mode", &[]),
        simple_tool_definition("ExitPlanModeTool", "Exit plan mode (alias)", &[]),
        simple_tool_definition("EnterWorktree", "Enter worktree context", &[]),
        simple_tool_definition("EnterWorktreeTool", "Enter worktree context (alias)", &[]),
        simple_tool_definition("ExitWorktree", "Exit worktree context", &[]),
        simple_tool_definition("ExitWorktreeTool", "Exit worktree context (alias)", &[]),
        simple_tool_definition("LSP", "Run LSP-like IDE operation", &["operation"]),
        simple_tool_definition(
            "LSPTool",
            "Run LSP-like IDE operation (alias)",
            &["operation"],
        ),
        simple_tool_definition("McpAuthTool", "Store or query MCP auth token", &["server"]),
        simple_tool_definition(
            "NotebookEdit",
            "Edit notebook cell content",
            &["notebook_path"],
        ),
        simple_tool_definition(
            "NotebookEditTool",
            "Edit notebook cell content (alias)",
            &["notebook_path"],
        ),
        simple_tool_definition("REPL", "Execute REPL-style command", &["command"]),
        simple_tool_definition(
            "REPLTool",
            "Execute REPL-style command (alias)",
            &["command"],
        ),
        simple_tool_definition("RemoteTrigger", "Trigger remote HTTP endpoint", &["url"]),
        simple_tool_definition(
            "RemoteTriggerTool",
            "Trigger remote HTTP endpoint (alias)",
            &["url"],
        ),
        simple_tool_definition("Skill", "Invoke or inspect a skill", &["skill_name"]),
        simple_tool_definition(
            "SkillTool",
            "Invoke or inspect a skill (alias)",
            &["skill_name"],
        ),
        simple_tool_definition("Sleep", "Sleep for a duration", &[]),
        simple_tool_definition("SleepTool", "Sleep for a duration (alias)", &[]),
        simple_tool_definition(
            "StructuredOutput",
            "Validate structured output",
            &["schema", "data"],
        ),
        simple_tool_definition(
            "SyntheticOutputTool",
            "Validate structured output (alias)",
            &["schema", "data"],
        ),
        simple_tool_definition("TaskCreate", "Create a task", &["subject", "description"]),
        simple_tool_definition(
            "TaskCreateTool",
            "Create a task (alias)",
            &["subject", "description"],
        ),
        simple_tool_definition("TaskGet", "Get task details", &["id"]),
        simple_tool_definition("TaskGetTool", "Get task details (alias)", &["id"]),
        simple_tool_definition("TaskList", "List tasks", &[]),
        simple_tool_definition("TaskListTool", "List tasks (alias)", &[]),
        simple_tool_definition("TaskOutput", "Append task output", &["id", "output"]),
        simple_tool_definition(
            "TaskOutputTool",
            "Append task output (alias)",
            &["id", "output"],
        ),
        simple_tool_definition("TaskStop", "Stop task", &["id"]),
        simple_tool_definition("TaskStopTool", "Stop task (alias)", &["id"]),
        simple_tool_definition("TaskUpdate", "Update task", &["id"]),
        simple_tool_definition("TaskUpdateTool", "Update task (alias)", &["id"]),
        simple_tool_definition("TeamCreate", "Create team", &["name"]),
        simple_tool_definition("TeamCreateTool", "Create team (alias)", &["name"]),
        simple_tool_definition("TeamDelete", "Delete team", &["name"]),
        simple_tool_definition("TeamDeleteTool", "Delete team (alias)", &["name"]),
        simple_tool_definition("TodoWrite", "Write todo list", &["todos"]),
        simple_tool_definition("TodoWriteTool", "Write todo list (alias)", &["todos"]),
        simple_tool_definition("ToolSearch", "Search available tools", &[]),
        simple_tool_definition("ToolSearchTool", "Search available tools (alias)", &[]),
        simple_tool_definition("CronCreate", "Create cron job", &["cron", "command"]),
        simple_tool_definition("CronDelete", "Delete cron job", &["id"]),
        simple_tool_definition("CronList", "List cron jobs", &[]),
        simple_tool_definition(
            "ScheduleCronTool",
            "Create cron job (legacy alias)",
            &["cron", "command"],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_standard_mappings() {
        let mapper = ToolCallMapper::new();
        let result = register_standard_mappings(&mapper);
        assert!(result.is_ok());

        // Verify all tools are registered
        assert!(mapper.has_tool("read_file"));
        assert!(mapper.has_tool("write_file"));
        assert!(mapper.has_tool("edit_file"));
        assert!(mapper.has_tool("search_code"));
        assert!(mapper.has_tool("find_files"));
        assert!(mapper.has_tool("execute_command"));
        assert!(mapper.has_tool("WebFetch"));
        assert!(mapper.has_tool("WebSearch"));
        assert!(mapper.has_tool("web_fetch"));
        assert!(mapper.has_tool("web_search"));
        assert!(mapper.has_tool("browser_navigate"));
        assert!(mapper.has_tool("browser_click"));
        assert!(mapper.has_tool("browser_fill"));
        assert!(mapper.has_tool("browser_evaluate"));
        assert!(mapper.has_tool("browser_screenshot"));
        assert!(mapper.has_tool("browser_dialog_handle"));
        assert!(mapper.has_tool("Read"));
        assert!(mapper.has_tool("Write"));
        assert!(mapper.has_tool("Edit"));
        assert!(mapper.has_tool("Grep"));
        assert!(mapper.has_tool("Glob"));
        assert!(mapper.has_tool("Bash"));
        assert!(mapper.has_tool("FileReadTool"));
        assert!(mapper.has_tool("FileWriteTool"));
        assert!(mapper.has_tool("FileEditTool"));
        assert!(mapper.has_tool("GrepTool"));
        assert!(mapper.has_tool("GlobTool"));
        assert!(mapper.has_tool("BashTool"));
        assert!(mapper.has_tool("PowerShell"));
        assert!(mapper.has_tool("PowerShellTool"));
        assert!(mapper.has_tool("WebFetchTool"));
        assert!(mapper.has_tool("WebSearchTool"));
        assert!(mapper.has_tool("SendMessageTool"));
        assert!(mapper.has_tool("ListMcpResourcesTool"));
        assert!(mapper.has_tool("ReadMcpResourceTool"));
        assert!(mapper.has_tool("MCPTool"));
        assert!(mapper.has_tool("mcp"));
        assert!(mapper.has_tool("Agent"));
        assert!(mapper.has_tool("AskUserQuestion"));
        assert!(mapper.has_tool("SendMessage"));
        assert!(mapper.has_tool("EnterPlanMode"));
        assert!(mapper.has_tool("ExitPlanMode"));
        assert!(mapper.has_tool("TaskCreate"));
        assert!(mapper.has_tool("TaskGet"));
        assert!(mapper.has_tool("TaskList"));
        assert!(mapper.has_tool("TaskUpdate"));
        assert!(mapper.has_tool("ToolSearch"));
        assert!(mapper.has_tool("CronCreate"));
    }

    #[test]
    fn test_get_standard_tool_definitions() {
        let tools = get_standard_tool_definitions();
        assert!(tools.len() >= 80);

        // Verify tool names
        let tool_names: Vec<String> = tools.iter().map(|t| t.function.name.clone()).collect();
        assert!(tool_names.contains(&"read_file".to_string()));
        assert!(tool_names.contains(&"write_file".to_string()));
        assert!(tool_names.contains(&"edit_file".to_string()));
        assert!(tool_names.contains(&"search_code".to_string()));
        assert!(tool_names.contains(&"find_files".to_string()));
        assert!(tool_names.contains(&"execute_command".to_string()));
        assert!(tool_names.contains(&"WebFetch".to_string()));
        assert!(tool_names.contains(&"WebSearch".to_string()));
        assert!(tool_names.contains(&"Read".to_string()));
        assert!(tool_names.contains(&"Write".to_string()));
        assert!(tool_names.contains(&"Edit".to_string()));
        assert!(tool_names.contains(&"Grep".to_string()));
        assert!(tool_names.contains(&"Glob".to_string()));
        assert!(tool_names.contains(&"Bash".to_string()));
        assert!(tool_names.contains(&"PowerShell".to_string()));
        assert!(tool_names.contains(&"FileReadTool".to_string()));
        assert!(tool_names.contains(&"FileWriteTool".to_string()));
        assert!(tool_names.contains(&"FileEditTool".to_string()));
        assert!(tool_names.contains(&"GrepTool".to_string()));
        assert!(tool_names.contains(&"GlobTool".to_string()));
        assert!(tool_names.contains(&"BashTool".to_string()));
        assert!(tool_names.contains(&"PowerShellTool".to_string()));
        assert!(tool_names.contains(&"WebFetchTool".to_string()));
        assert!(tool_names.contains(&"WebSearchTool".to_string()));
        assert!(tool_names.contains(&"SendMessageTool".to_string()));
        assert!(tool_names.contains(&"ListMcpResourcesTool".to_string()));
        assert!(tool_names.contains(&"ReadMcpResourceTool".to_string()));
        assert!(tool_names.contains(&"MCPTool".to_string()));
        assert!(tool_names.contains(&"mcp".to_string()));
        assert!(tool_names.contains(&"Agent".to_string()));
        assert!(tool_names.contains(&"AskUserQuestion".to_string()));
        assert!(tool_names.contains(&"SendMessage".to_string()));
        assert!(tool_names.contains(&"SendUserMessage".to_string()));
        assert!(tool_names.contains(&"TaskCreate".to_string()));
        assert!(tool_names.contains(&"TaskGet".to_string()));
        assert!(tool_names.contains(&"TaskList".to_string()));
        assert!(tool_names.contains(&"TaskUpdate".to_string()));
        assert!(tool_names.contains(&"TodoWrite".to_string()));
        assert!(tool_names.contains(&"ToolSearch".to_string()));
        assert!(tool_names.contains(&"CronCreate".to_string()));
    }

    #[test]
    fn test_get_standard_tool_definitions_filtered() {
        let tools = get_standard_tool_definitions_filtered(
            &[String::from("Web*"), String::from("Read")],
            &[String::from("WebFetch")],
        );
        let tool_names: Vec<String> = tools.iter().map(|t| t.function.name.clone()).collect();

        assert!(tool_names.contains(&"Read".to_string()));
        assert!(tool_names.contains(&"WebSearch".to_string()));
        assert!(!tool_names.contains(&"WebFetch".to_string()));
        assert!(!tool_names.contains(&"Write".to_string()));
    }

    #[test]
    fn test_filesystem_tools_have_correct_capabilities() {
        let mapper = ToolCallMapper::new();
        register_filesystem_tools(&mapper).unwrap();

        // Create test tool calls
        let read_call = cyberclaw_llm::types::ToolCall {
            id: "call-1".to_string(),
            call_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionCall {
                name: "read_file".to_string(),
                arguments: serde_json::json!({"file_path": "/test.txt"}).to_string(),
            },
        };

        let request = mapper
            .map_tool_call(&read_call, "trace-1".to_string())
            .unwrap();
        assert_eq!(request.capability_id.as_ref(), "fs.read");
        assert_eq!(request.connector_id.as_ref(), "local");
    }

    #[test]
    fn test_claude_alias_tools_have_correct_capabilities() {
        let mapper = ToolCallMapper::new();
        register_claude_alias_tools(&mapper).unwrap();

        let read_call = cyberclaw_llm::types::ToolCall {
            id: "call-read".to_string(),
            call_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionCall {
                name: "Read".to_string(),
                arguments: serde_json::json!({"file_path": "/tmp/test.txt"}).to_string(),
            },
        };

        let bash_call = cyberclaw_llm::types::ToolCall {
            id: "call-bash".to_string(),
            call_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionCall {
                name: "Bash".to_string(),
                arguments: serde_json::json!({"command": "echo hello"}).to_string(),
            },
        };

        let read_request = mapper
            .map_tool_call(&read_call, "trace-read".to_string())
            .unwrap();
        assert_eq!(read_request.capability_id.as_ref(), "fs.read");

        let bash_request = mapper
            .map_tool_call(&bash_call, "trace-bash".to_string())
            .unwrap();
        assert_eq!(bash_request.capability_id.as_ref(), "cmd.run");
    }

    #[test]
    fn r1_bash_facade_routes_to_cmd_run() {
        // R-1 (2026-05-05) regression: every flavour of the LLM bash alias
        // must resolve to `local::cmd.run` so agents can run business
        // validation scripts (python3, pytest, etc.). Previously routed to
        // the whitelist-restricted `cmd.exec`, which broke GA-02/GA-04.
        let mapper = ToolCallMapper::new();
        register_command_tools(&mapper).unwrap();
        register_claude_alias_tools(&mapper).unwrap();
        register_claude_tool_class_aliases(&mapper).unwrap();

        for name in ["bash", "execute_command", "Bash", "BashTool"] {
            let call = cyberclaw_llm::types::ToolCall {
                id: format!("call-{}", name),
                call_type: "function".to_string(),
                function: cyberclaw_llm::types::FunctionCall {
                    name: name.to_string(),
                    arguments: serde_json::json!({"command": "echo r1"}).to_string(),
                },
            };
            let req = mapper
                .map_tool_call(&call, format!("trace-{}", name))
                .unwrap_or_else(|e| panic!("R-1: tool `{}` failed to map: {}", name, e));
            assert_eq!(
                req.capability_id.as_ref(),
                "cmd.run",
                "R-1 regression: `{}` must route to cmd.run",
                name
            );
            assert_eq!(req.connector_id.as_ref(), "local");
        }
    }

    #[test]
    fn test_mcp_tools_have_correct_capabilities() {
        let mapper = ToolCallMapper::new();
        register_mcp_tool_mappings(&mapper, "mcp-default").unwrap();

        let list_call = cyberclaw_llm::types::ToolCall {
            id: "call-list".to_string(),
            call_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionCall {
                name: "ListMcpResourcesTool".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let read_call = cyberclaw_llm::types::ToolCall {
            id: "call-read".to_string(),
            call_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionCall {
                name: "ReadMcpResourceTool".to_string(),
                arguments: serde_json::json!({"uri": "file:///tmp/a.txt"}).to_string(),
            },
        };
        let tool_call = cyberclaw_llm::types::ToolCall {
            id: "call-mcp".to_string(),
            call_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionCall {
                name: "MCPTool".to_string(),
                arguments: serde_json::json!({"name":"read_file","input":{"path":"README.md"}})
                    .to_string(),
            },
        };

        let list_req = mapper
            .map_tool_call(&list_call, "trace-list".to_string())
            .unwrap();
        assert_eq!(list_req.capability_id.as_ref(), "mcp.list_resources");
        assert_eq!(list_req.connector_id.as_ref(), "mcp-default");

        let read_req = mapper
            .map_tool_call(&read_call, "trace-read".to_string())
            .unwrap();
        assert_eq!(read_req.capability_id.as_ref(), "mcp.read_resource");
        assert_eq!(read_req.input["uri"], "file:///tmp/a.txt");

        let tool_req = mapper
            .map_tool_call(&tool_call, "trace-tool".to_string())
            .unwrap();
        assert_eq!(tool_req.capability_id.as_ref(), "mcp.call_tool");
        assert_eq!(tool_req.input["tool_name"], "read_file");
        assert_eq!(tool_req.input["arguments"]["path"], "README.md");
    }

    #[test]
    fn test_claude_tool_class_aliases_have_correct_capabilities() {
        let mapper = ToolCallMapper::new();
        register_claude_tool_class_aliases(&mapper).unwrap();

        let read_call = cyberclaw_llm::types::ToolCall {
            id: "call-read-tool".to_string(),
            call_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionCall {
                name: "FileReadTool".to_string(),
                arguments: serde_json::json!({"file_path": "/tmp/demo.txt"}).to_string(),
            },
        };
        let bash_call = cyberclaw_llm::types::ToolCall {
            id: "call-bash-tool".to_string(),
            call_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionCall {
                name: "BashTool".to_string(),
                arguments: serde_json::json!({"command": "echo hi"}).to_string(),
            },
        };
        let web_call = cyberclaw_llm::types::ToolCall {
            id: "call-web-tool".to_string(),
            call_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionCall {
                name: "WebSearchTool".to_string(),
                arguments: serde_json::json!({"query": "cyberclaw"}).to_string(),
            },
        };

        let read_req = mapper
            .map_tool_call(&read_call, "trace-read-tool".to_string())
            .unwrap();
        assert_eq!(read_req.capability_id.as_ref(), "fs.read");

        let bash_req = mapper
            .map_tool_call(&bash_call, "trace-bash-tool".to_string())
            .unwrap();
        assert_eq!(bash_req.capability_id.as_ref(), "cmd.run");

        let web_req = mapper
            .map_tool_call(&web_call, "trace-web-tool".to_string())
            .unwrap();
        assert_eq!(web_req.capability_id.as_ref(), "web.search");
    }

    #[test]
    fn test_claude_host_tools_have_correct_capabilities() {
        let mapper = ToolCallMapper::new();
        register_claude_host_tool_mappings(&mapper).unwrap();

        let task_create = cyberclaw_llm::types::ToolCall {
            id: "call-task-create".to_string(),
            call_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionCall {
                name: "TaskCreate".to_string(),
                arguments: serde_json::json!({
                    "subject": "验证任务",
                    "description": "验证 host task create"
                })
                .to_string(),
            },
        };
        let ask_user = cyberclaw_llm::types::ToolCall {
            id: "call-ask-user".to_string(),
            call_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionCall {
                name: "AskUserQuestion".to_string(),
                arguments: serde_json::json!({
                    "question": "继续吗?"
                })
                .to_string(),
            },
        };
        let cron_create = cyberclaw_llm::types::ToolCall {
            id: "call-cron-create".to_string(),
            call_type: "function".to_string(),
            function: cyberclaw_llm::types::FunctionCall {
                name: "CronCreate".to_string(),
                arguments: serde_json::json!({
                    "cron": "0 * * * *",
                    "command": "echo test"
                })
                .to_string(),
            },
        };

        let task_req = mapper
            .map_tool_call(&task_create, "trace-task-create".to_string())
            .unwrap();
        assert_eq!(task_req.capability_id.as_ref(), "host.task.create");
        assert_eq!(task_req.connector_id.as_ref(), "local");

        let ask_req = mapper
            .map_tool_call(&ask_user, "trace-ask-user".to_string())
            .unwrap();
        assert_eq!(ask_req.capability_id.as_ref(), "host.ask_user_question");
        assert_eq!(ask_req.connector_id.as_ref(), "local");

        let cron_req = mapper
            .map_tool_call(&cron_create, "trace-cron-create".to_string())
            .unwrap();
        assert_eq!(cron_req.capability_id.as_ref(), "host.cron.create");
        assert_eq!(cron_req.connector_id.as_ref(), "local");
    }

    #[test]
    fn test_claude_full_tool_coverage() {
        let mapper = ToolCallMapper::new();
        register_standard_mappings(&mapper).unwrap();

        let required_tools = [
            "Read",
            "Write",
            "Edit",
            "Grep",
            "Glob",
            "Bash",
            "PowerShell",
            "WebFetch",
            "WebSearch",
            "SendMessage",
            "ListMcpResourcesTool",
            "ReadMcpResourceTool",
            "mcp",
            "Agent",
            "AskUserQuestion",
            "SendUserMessage",
            "Config",
            "EnterPlanMode",
            "ExitPlanMode",
            "EnterWorktree",
            "ExitWorktree",
            "LSP",
            "McpAuthTool",
            "NotebookEdit",
            "REPL",
            "RemoteTrigger",
            "Skill",
            "Sleep",
            "StructuredOutput",
            "TaskCreate",
            "TaskGet",
            "TaskList",
            "TaskOutput",
            "TaskStop",
            "TaskUpdate",
            "TeamCreate",
            "TeamDelete",
            "TodoWrite",
            "ToolSearch",
            "CronCreate",
            "CronDelete",
            "CronList",
        ];

        for tool in required_tools {
            assert!(
                mapper.has_tool(tool),
                "Missing Claude-compatible tool: {}",
                tool
            );
        }
    }
}
