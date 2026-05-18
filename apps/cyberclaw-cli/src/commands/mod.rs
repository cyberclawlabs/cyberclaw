//! CLI 命令模块
//!
//! 包含所有 cyberclaw-cli 的命令实现。

pub mod agent;
pub mod audit;
pub mod browser;
pub mod capability;
pub mod chat;
pub mod chat_tui;
pub mod clipboard;
pub mod cluster;
pub mod connector;
pub mod doctor;
pub mod mcp;
pub mod memory;
pub mod onboard;
pub mod package;
pub mod review;
pub mod skill;
pub mod task;
pub mod tools;
pub mod workflow;

pub use agent::{handle_agent_command, AgentCommand};
pub use audit::{handle_audit_command, AuditCommand};
pub use browser::{handle_browser_command, BrowserCommand};
pub use capability::{handle_capability_command, CapabilityCommand};
pub use chat::{run as run_chat, ChatArgs};
pub use clipboard::{handle_clipboard_command, ClipboardCommand};
pub use cluster::{handle_cluster_command, ClusterCommand};
pub use connector::{handle_connector_command, ConnectorCommand};
pub use doctor::{handle_doctor, DoctorArgs};
pub use mcp::{handle_mcp_command, McpCommand};
pub use memory::{handle_memory_command, MemoryCommand};
pub use onboard::{handle_onboard, OnboardArgs};
pub use package::{handle_package_command, PackageCommand};
pub use review::{handle_review_command, ReviewCommand};
pub use skill::{handle_skill_command, SkillCommand};
pub use task::{handle_task_command, TaskCommand};
pub use tools::{handle_tools_command, ToolsCommand};
pub use workflow::{handle_workflow_command, WorkflowCommand};
