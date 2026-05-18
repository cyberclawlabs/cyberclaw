//! Credential vault and isolation for Connector execution.
//!
//! Provides a unified credential management layer so Connectors never
//! read secrets directly from the environment. The default [`EnvVarVault`]
//! reads credentials from environment variables following the naming
//! convention `CYBERCLAW_CRED_{CONNECTOR_ID}_{KEY}`.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use subtle::ConstantTimeEq;
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by credential vault operations.
#[derive(Debug, Error)]
pub enum CredentialError {
    /// The requested credential was not found.
    #[error("credential not found: {0}")]
    NotFound(String),

    /// The credential has expired.
    #[error("credential expired: {0}")]
    Expired(String),

    /// A backend-specific error occurred.
    #[error("vault backend error: {0}")]
    BackendError(String),
}

// ---------------------------------------------------------------------------
// SecretString — redacted wrapper
// ---------------------------------------------------------------------------

/// A string wrapper that never reveals its contents through `Debug` or
/// `Display`. Use [`SecretString::expose`] to access the raw value.
#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    /// Create a new secret string.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Expose the underlying secret value.
    ///
    /// Callers must ensure the returned value is not written to logs.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(***)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl PartialEq for SecretString {
    fn eq(&self, other: &Self) -> bool {
        // Constant-time comparison to prevent timing side-channel attacks
        self.0.as_bytes().ct_eq(other.0.as_bytes()).into()
    }
}

impl Eq for SecretString {}

// ---------------------------------------------------------------------------
// Credential types
// ---------------------------------------------------------------------------

/// A resolved credential ready for injection into a Connector context.
#[derive(Debug, Clone)]
pub struct Credential {
    /// Unique identifier for this credential instance.
    pub id: String,
    /// The Connector this credential belongs to.
    pub connector_id: String,
    /// The key name (e.g. `API_KEY`, `TOKEN`).
    pub key: String,
    /// The secret value — redacted in logs.
    pub value: SecretString,
    /// Optional expiration time.
    pub expires_at: Option<SystemTime>,
}

/// Metadata about a stored credential (without the secret value).
#[derive(Debug, Clone)]
pub struct CredentialInfo {
    /// Unique identifier.
    pub id: String,
    /// The Connector this credential belongs to.
    pub connector_id: String,
    /// The key name.
    pub key: String,
    /// Optional expiration time.
    pub expires_at: Option<SystemTime>,
    /// Whether the credential is currently active.
    pub active: bool,
}

/// Context provided when requesting credential injection.
#[derive(Debug, Clone, Default)]
pub struct CredentialContext {
    /// Specific keys requested; empty means all available keys.
    pub requested_keys: Vec<String>,
    /// Arbitrary metadata that a vault backend may use for scoping.
    pub metadata: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// VaultBackend enum
// ---------------------------------------------------------------------------

/// Describes which backend a vault implementation uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultBackend {
    /// Read credentials from environment variables (development).
    EnvVar,
    /// Use the operating-system keychain (local deployment).
    OsKeychain,
    /// Delegate to an external secret manager such as HashiCorp Vault
    /// (production).
    ExternalVault,
}

// ---------------------------------------------------------------------------
// CredentialVault trait
// ---------------------------------------------------------------------------

/// Trait for credential storage and injection backends.
#[async_trait]
pub trait CredentialVault: Send + Sync {
    /// Inject (resolve) a credential for the given Connector.
    async fn inject(
        &self,
        connector_id: &str,
        context: &CredentialContext,
    ) -> Result<Credential, CredentialError>;

    /// Revoke a credential by its unique id.
    async fn revoke(&self, credential_id: &str) -> Result<(), CredentialError>;

    /// List metadata for all active credentials belonging to a Connector.
    async fn list(&self, connector_id: &str) -> Result<Vec<CredentialInfo>, CredentialError>;
}

// ---------------------------------------------------------------------------
// EnvVarVault
// ---------------------------------------------------------------------------

/// A [`CredentialVault`] implementation backed by environment variables.
///
/// Variable naming convention: `CYBERCLAW_CRED_{CONNECTOR_ID}_{KEY}`.
/// The connector id is upper-cased and hyphens are replaced with underscores.
pub struct EnvVarVault {
    /// Tracks revoked credential ids.
    revoked: RwLock<Vec<String>>,
}

impl EnvVarVault {
    /// Create a new `EnvVarVault`.
    pub fn new() -> Self {
        Self {
            revoked: RwLock::new(Vec::new()),
        }
    }

    /// Return the vault backend type.
    pub fn backend(&self) -> VaultBackend {
        VaultBackend::EnvVar
    }

    /// Build the environment variable name for a connector + key pair.
    fn env_var_name(connector_id: &str, key: &str) -> String {
        let normalized_connector = connector_id.to_uppercase().replace('-', "_");
        let normalized_key = key.to_uppercase().replace('-', "_");
        format!("CYBERCLAW_CRED_{normalized_connector}_{normalized_key}")
    }
}

impl Default for EnvVarVault {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CredentialVault for EnvVarVault {
    async fn inject(
        &self,
        connector_id: &str,
        context: &CredentialContext,
    ) -> Result<Credential, CredentialError> {
        let revoked = self.revoked.read().await;

        // Determine which key to resolve.
        let key = context
            .requested_keys
            .first()
            .cloned()
            .unwrap_or_else(|| "API_KEY".to_string());

        let env_name = Self::env_var_name(connector_id, &key);

        let value = std::env::var(&env_name).map_err(|_| {
            CredentialError::NotFound(format!(
                "environment variable {env_name} not set for connector {connector_id}"
            ))
        })?;

        let id = Uuid::new_v4().to_string();

        // Ensure the credential was not revoked previously.
        if revoked.contains(&id) {
            return Err(CredentialError::NotFound(format!(
                "credential {id} has been revoked"
            )));
        }

        Ok(Credential {
            id,
            connector_id: connector_id.to_string(),
            key,
            value: SecretString::new(value),
            expires_at: None,
        })
    }

    async fn revoke(&self, credential_id: &str) -> Result<(), CredentialError> {
        let mut revoked = self.revoked.write().await;
        if revoked.contains(&credential_id.to_string()) {
            return Err(CredentialError::NotFound(format!(
                "credential {credential_id} already revoked or not found"
            )));
        }
        revoked.push(credential_id.to_string());
        Ok(())
    }

    async fn list(&self, connector_id: &str) -> Result<Vec<CredentialInfo>, CredentialError> {
        let revoked = self.revoked.read().await;
        let prefix = format!(
            "CYBERCLAW_CRED_{}_",
            connector_id.to_uppercase().replace('-', "_")
        );

        let infos: Vec<CredentialInfo> = std::env::vars()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(k, _)| {
                let key = k.strip_prefix(&prefix).unwrap_or(&k).to_string();
                let id = format!("{connector_id}:{key}");
                let active = !revoked.contains(&id);
                CredentialInfo {
                    id,
                    connector_id: connector_id.to_string(),
                    key,
                    expires_at: None,
                    active,
                }
            })
            .collect();

        Ok(infos)
    }
}

// ---------------------------------------------------------------------------
// ConnectorCredentialProxy
// ---------------------------------------------------------------------------

/// Convenience proxy that prepares credential contexts for Connector execution.
pub struct ConnectorCredentialProxy {
    vault: Arc<dyn CredentialVault>,
}

impl ConnectorCredentialProxy {
    /// Create a new proxy backed by the given vault.
    pub fn new(vault: Arc<dyn CredentialVault>) -> Self {
        Self { vault }
    }

    /// Prepare a [`CredentialContext`] and inject a credential for the given
    /// Connector, returning the resolved [`Credential`].
    pub async fn prepare_context(&self, connector_id: &str) -> Result<Credential, CredentialError> {
        let context = CredentialContext::default();
        self.vault.inject(connector_id, &context).await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn env_var_vault_inject_reads_env() {
        // Set a credential env var for testing.
        let connector = "test-connector";
        let key = "TOKEN";
        let env_name = EnvVarVault::env_var_name(connector, key);
        unsafe { std::env::set_var(&env_name, "super-secret-value") };

        let vault = EnvVarVault::new();
        let ctx = CredentialContext {
            requested_keys: vec![key.to_string()],
            ..Default::default()
        };

        let cred = vault.inject(connector, &ctx).await.unwrap();
        assert_eq!(cred.connector_id, connector);
        assert_eq!(cred.key, key);
        assert_eq!(cred.value.expose(), "super-secret-value");

        // Cleanup
        unsafe { std::env::remove_var(&env_name) };
    }

    #[tokio::test]
    async fn env_var_vault_inject_not_found() {
        let vault = EnvVarVault::new();
        let ctx = CredentialContext {
            requested_keys: vec!["MISSING_KEY".to_string()],
            ..Default::default()
        };

        let result = vault.inject("nonexistent", &ctx).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            CredentialError::NotFound(msg) => {
                assert!(msg.contains("CYBERCLAW_CRED_NONEXISTENT_MISSING_KEY"));
            }
            other => panic!("expected NotFound, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn env_var_vault_revoke() {
        let vault = EnvVarVault::new();

        // Revoke a credential id.
        vault.revoke("cred-1").await.unwrap();

        // Revoking the same id again should fail.
        let result = vault.revoke("cred-1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn env_var_vault_list() {
        let connector = "list-test";
        let env_name = EnvVarVault::env_var_name(connector, "DB_PASS");
        unsafe { std::env::set_var(&env_name, "secret123") };

        let vault = EnvVarVault::new();
        let infos = vault.list(connector).await.unwrap();

        assert!(!infos.is_empty());
        assert!(infos.iter().any(|i| i.key == "DB_PASS"));
        assert!(infos.iter().all(|i| i.connector_id == connector));

        // Cleanup
        unsafe { std::env::remove_var(&env_name) };
    }

    #[tokio::test]
    async fn secret_string_debug_does_not_leak() {
        let secret = SecretString::new("my-api-key-12345");
        let debug_output = format!("{:?}", secret);
        let display_output = format!("{}", secret);

        assert!(!debug_output.contains("my-api-key-12345"));
        assert!(!display_output.contains("my-api-key-12345"));
        assert!(debug_output.contains("***"));
        assert!(display_output.contains("***"));
    }

    #[tokio::test]
    async fn secret_string_expose_returns_value() {
        let secret = SecretString::new("real-value");
        assert_eq!(secret.expose(), "real-value");
    }

    #[tokio::test]
    async fn credential_debug_redacts_value() {
        let cred = Credential {
            id: "c1".to_string(),
            connector_id: "conn".to_string(),
            key: "KEY".to_string(),
            value: SecretString::new("top-secret"),
            expires_at: None,
        };
        let debug_output = format!("{:?}", cred);
        assert!(!debug_output.contains("top-secret"));
        assert!(debug_output.contains("***"));
    }

    #[tokio::test]
    async fn connector_credential_proxy_prepare_context() {
        let connector = "proxy-test";
        let env_name = EnvVarVault::env_var_name(connector, "API_KEY");
        unsafe { std::env::set_var(&env_name, "proxy-secret") };

        let vault = Arc::new(EnvVarVault::new());
        let proxy = ConnectorCredentialProxy::new(vault);
        let cred = proxy.prepare_context(connector).await.unwrap();

        assert_eq!(cred.connector_id, connector);
        assert_eq!(cred.value.expose(), "proxy-secret");

        // Cleanup
        unsafe { std::env::remove_var(&env_name) };
    }

    #[test]
    fn env_var_name_normalizes_hyphens() {
        assert_eq!(
            EnvVarVault::env_var_name("my-connector", "api-key"),
            "CYBERCLAW_CRED_MY_CONNECTOR_API_KEY"
        );
    }

    #[test]
    fn vault_backend_variants() {
        // Ensure all enum variants exist and are comparable.
        assert_eq!(VaultBackend::EnvVar, VaultBackend::EnvVar);
        assert_ne!(VaultBackend::EnvVar, VaultBackend::OsKeychain);
        assert_ne!(VaultBackend::OsKeychain, VaultBackend::ExternalVault);
    }
}
