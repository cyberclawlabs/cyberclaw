//! GitHub Connector 使用示例
//!
//! 运行方式:
//! ```bash
//! export GITHUB_TOKEN="your_token"
//! cargo run --example github_connector_example
//! ```

use cyberclaw_connectors::github_connector::{Authenticator, GitHubAuth, GitHubConnector};
use cyberclaw_connectors::types::Connector;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    println!("=== GitHub Connector Example ===\n");

    // 1. 创建认证器
    let token = std::env::var("GITHUB_TOKEN").unwrap_or_else(|_| "test_token".to_string());

    let auth: Arc<dyn Authenticator> = Arc::new(GitHubAuth::from_token(token));

    println!("✓ Authentication configured");

    // 2. 创建 Connector
    let connector = GitHubConnector::new(auth);

    println!("✓ Connector ID: {}", connector.id());
    println!("✓ Runtime mode: {:?}", connector.runtime());
    println!("✓ Capabilities count: {}", connector.capabilities().len());

    // 3. 列出所有能力
    println!("\nAvailable capabilities:");
    for cap in connector.capabilities() {
        println!("  - {} (Risk: {:?})", cap.id, cap.risk);
    }

    println!("\n✓ GitHub Connector initialized successfully!");

    Ok(())
}
