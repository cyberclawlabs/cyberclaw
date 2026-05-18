//! Health probe for OpenViking instance.

use reqwest::Client;
use std::time::Duration;
use tracing::debug;

/// Check if the OpenViking instance is reachable and healthy.
///
/// Accepts an existing [`Client`] to avoid per-call allocation. When called
/// from [`OpenVikingConnector`] the connector's shared client should be passed.
pub async fn check_health(
    client: &Client,
    base_url: &str,
    timeout_ms: u64,
) -> anyhow::Result<bool> {
    let url = format!("{}/api/v1/health", base_url.trim_end_matches('/'));
    debug!("OpenViking health check: {}", url);

    match client
        .get(&url)
        .timeout(Duration::from_millis(timeout_ms))
        .send()
        .await
    {
        Ok(resp) => Ok(resp.status().is_success()),
        Err(_) => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_check_unreachable_returns_false() {
        let client = Client::builder().build().expect("reqwest client");
        // Non-routable address — should fail fast
        let result = check_health(&client, "http://192.0.2.1:1", 200)
            .await
            .unwrap();
        assert!(!result);
    }
}
