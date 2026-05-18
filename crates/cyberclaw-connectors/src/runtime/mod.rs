//! Process runtime module for isolated capability execution
//!
//! This module provides runtime isolation for capability execution with support for:
//! - **Native mode**: Direct execution in current process (no isolation)
//! - **Process mode**: Subprocess execution with timeout and resource limits
//! - **Container mode**: Full container isolation (Docker/podman) - future implementation
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │  Capability Dispatcher                                   │
//! └────────────────────┬────────────────────────────────────┘
//!                      │
//!        ┌─────────────┼─────────────┐
//!        │             │             │
//!   ┌────▼────┐  ┌────▼─────┐  ┌───▼──────┐
//!   │ Native  │  │ Process  │  │Container │
//!   │ Runtime │  │ Executor │  │ Runtime  │
//!   └─────────┘  └──────────┘  └──────────┘
//!        │             │             │
//!   Direct Call  Subprocess     Docker API
//! ```
//!
//! # Security Model
//!
//! Runtime selection is based on capability risk level (from PolicyEngine):
//!
//! | Risk Level | Runtime Mode | Isolation Level |
//! |------------|--------------|-----------------|
//! | Low        | Native       | None (shared process) |
//! | Medium     | Process      | Process isolation |
//! | High       | Container    | Full isolation (namespaces/cgroups) |
//! | Critical   | Container    | Full isolation + network restrictions |
//!
//! # Examples
//!
//! ```no_run
//! use cyberclaw_connectors::runtime::{
//!     ProcessExecutor, ProcessConfig, ProcessRuntime, RuntimeMode
//! };
//! use cyberclaw_core::prelude::RiskLevel;
//! use std::time::Duration;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Select runtime based on risk level
//!     let risk = RiskLevel::Medium;
//!     let mode = RuntimeMode::from_risk_level(risk);
//!
//!     // Create executor for process isolation
//!     let executor = ProcessExecutor::new();
//!
//!     // Configure execution with timeout and limits
//!     let config = ProcessConfig::builder()
//!         .timeout(Duration::from_secs(30))
//!         .max_file_descriptors(1024)
//!         .build();
//!
//!     // Execute capability
//!     let result = executor.execute_command(
//!         "cargo",
//!         vec!["test".to_string()],
//!         config
//!     ).await?;
//!
//!     if result.is_success() {
//!         println!("Success: {}", result.stdout);
//!     } else {
//!         eprintln!("Failed: {} (exit code {})", result.stderr, result.exit_code);
//!     }
//!
//!     Ok(())
//! }
//! ```

mod config;
mod container;
mod mode;
mod process;
mod selector;
mod traits;

pub use config::{ProcessConfig, ProcessConfigBuilder};
pub use container::{ContainerBackend, ContainerConfig, ContainerRuntime, NetworkMode};
pub use mode::RuntimeMode;
pub use process::ProcessExecutor;
pub use selector::{RuntimeSelectionStrategy, RuntimeSelector, RuntimeSelectorConfig};
pub use traits::{ProcessResult, ProcessRuntime};
