//! Persistence configuration and store initialization.
//!
//! Reads environment variables to select and configure the storage backend,
//! then creates an appropriate [`StateStore`] implementation.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use cyberclaw_store::{create_store, StateStore, StoreBackend};

/// Persistence configuration derived from environment variables.
///
/// | Variable | Description | Default |
/// |----------|-------------|---------|
/// | `CYBERCLAW_STORE_BACKEND` | `"memory"`, `"sqlite"`, or `"postgres"` | `"memory"` |
/// | `CYBERCLAW_STORE_SQLITE_PATH` | SQLite database file path | `"cyberclaw.db"` |
/// | `CYBERCLAW_STORE_POSTGRES_URL` | PostgreSQL connection URL | (none) |
#[derive(Debug, Clone)]
pub struct PersistenceConfig {
    /// Selected backend name (lowercase).
    pub backend: String,
    /// SQLite file path (used when backend is `"sqlite"`).
    pub sqlite_path: String,
    /// PostgreSQL connection URL (used when backend is `"postgres"`).
    pub postgres_url: Option<String>,
}

impl PersistenceConfig {
    /// Read persistence configuration from environment variables.
    pub fn from_env() -> Self {
        let backend = std::env::var("CYBERCLAW_STORE_BACKEND")
            .unwrap_or_else(|_| "memory".to_string())
            .to_lowercase();
        let sqlite_path = std::env::var("CYBERCLAW_STORE_SQLITE_PATH")
            .unwrap_or_else(|_| "cyberclaw.db".to_string());
        let postgres_url = std::env::var("CYBERCLAW_STORE_POSTGRES_URL").ok();

        Self {
            backend,
            sqlite_path,
            postgres_url,
        }
    }

    /// Convert this configuration into a [`StoreBackend`] descriptor.
    ///
    /// Returns an error if the backend name is unrecognised or required
    /// parameters are missing.
    pub fn to_store_backend(&self) -> Result<StoreBackend> {
        match self.backend.as_str() {
            "memory" => Ok(StoreBackend::Memory),
            "sqlite" => Ok(StoreBackend::Sqlite {
                path: self.sqlite_path.clone(),
            }),
            "postgres" => {
                let url = self
                    .postgres_url
                    .clone()
                    .filter(|u| !u.is_empty())
                    .context(
                        "CYBERCLAW_STORE_POSTGRES_URL must be set when backend is 'postgres'",
                    )?;
                Ok(StoreBackend::Postgres { url })
            }
            other => bail!(
                "unknown store backend '{}' (expected memory|sqlite|postgres)",
                other
            ),
        }
    }
}

impl std::fmt::Display for PersistenceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.backend.as_str() {
            "sqlite" => write!(f, "sqlite(path={})", self.sqlite_path),
            "postgres" => write!(f, "postgres(url=****)"),
            _ => write!(f, "{}", self.backend),
        }
    }
}

/// Create a [`StateStore`] from the given persistence configuration.
pub async fn init_store(config: &PersistenceConfig) -> Result<Arc<dyn StateStore>> {
    let backend = config.to_store_backend()?;
    let store = create_store(backend)
        .await
        .context("failed to create state store")?;
    Ok(Arc::from(store))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn clear_env() {
        std::env::remove_var("CYBERCLAW_STORE_BACKEND");
        std::env::remove_var("CYBERCLAW_STORE_SQLITE_PATH");
        std::env::remove_var("CYBERCLAW_STORE_POSTGRES_URL");
    }

    #[test]
    #[serial]
    fn default_config_is_memory() {
        clear_env();
        let config = PersistenceConfig::from_env();
        assert_eq!(config.backend, "memory");
    }

    #[test]
    #[serial]
    fn env_parse_memory_backend() {
        clear_env();
        std::env::set_var("CYBERCLAW_STORE_BACKEND", "memory");
        let config = PersistenceConfig::from_env();
        let backend = config.to_store_backend().unwrap();
        assert_eq!(backend, StoreBackend::Memory);
        clear_env();
    }

    #[test]
    #[serial]
    fn env_parse_sqlite_backend() {
        clear_env();
        std::env::set_var("CYBERCLAW_STORE_BACKEND", "sqlite");
        std::env::set_var("CYBERCLAW_STORE_SQLITE_PATH", "/tmp/test.db");
        let config = PersistenceConfig::from_env();
        assert_eq!(config.sqlite_path, "/tmp/test.db");
        let backend = config.to_store_backend().unwrap();
        assert_eq!(
            backend,
            StoreBackend::Sqlite {
                path: "/tmp/test.db".to_string()
            }
        );
        clear_env();
    }

    #[test]
    #[serial]
    fn env_parse_sqlite_default_path() {
        clear_env();
        std::env::set_var("CYBERCLAW_STORE_BACKEND", "sqlite");
        let config = PersistenceConfig::from_env();
        assert_eq!(config.sqlite_path, "cyberclaw.db");
        clear_env();
    }

    #[test]
    #[serial]
    fn env_parse_postgres_backend() {
        clear_env();
        std::env::set_var("CYBERCLAW_STORE_BACKEND", "postgres");
        std::env::set_var("CYBERCLAW_STORE_POSTGRES_URL", "postgres://localhost/test");
        let config = PersistenceConfig::from_env();
        let backend = config.to_store_backend().unwrap();
        assert_eq!(
            backend,
            StoreBackend::Postgres {
                url: "postgres://localhost/test".to_string()
            }
        );
        clear_env();
    }

    #[test]
    #[serial]
    fn invalid_backend_name_error() {
        clear_env();
        let config = PersistenceConfig {
            backend: "redis".to_string(),
            sqlite_path: "cyberclaw.db".to_string(),
            postgres_url: None,
        };
        let err = config.to_store_backend().unwrap_err();
        assert!(
            err.to_string().contains("unknown store backend 'redis'"),
            "unexpected error: {}",
            err
        );
        clear_env();
    }

    #[test]
    #[serial]
    fn missing_postgres_url_error() {
        clear_env();
        std::env::set_var("CYBERCLAW_STORE_BACKEND", "postgres");
        let config = PersistenceConfig::from_env();
        let err = config.to_store_backend().unwrap_err();
        assert!(
            err.to_string().contains("CYBERCLAW_STORE_POSTGRES_URL"),
            "unexpected error: {}",
            err
        );
        clear_env();
    }

    #[tokio::test]
    #[serial]
    async fn init_store_memory_backend() {
        clear_env();
        let config = PersistenceConfig::from_env();
        let store = init_store(&config).await.unwrap();
        // Verify the store is functional
        let record = cyberclaw_store::ExecutionRecord {
            id: uuid::Uuid::new_v4(),
            agent_id: "test-agent".to_string(),
            skill_id: None,
            status: "running".to_string(),
            input: serde_json::json!({"hello": "world"}),
            output: None,
            error: None,
            started_at: chrono::Utc::now(),
            completed_at: None,
        };
        store.save_execution(record.clone()).await.unwrap();
        let retrieved = store.get_execution(record.id).await.unwrap();
        assert_eq!(retrieved.id, record.id);
    }

    #[test]
    #[serial]
    fn config_display_memory() {
        let config = PersistenceConfig {
            backend: "memory".to_string(),
            sqlite_path: "cyberclaw.db".to_string(),
            postgres_url: None,
        };
        assert_eq!(format!("{}", config), "memory");
    }

    #[test]
    #[serial]
    fn config_display_sqlite() {
        let config = PersistenceConfig {
            backend: "sqlite".to_string(),
            sqlite_path: "/data/app.db".to_string(),
            postgres_url: None,
        };
        assert_eq!(format!("{}", config), "sqlite(path=/data/app.db)");
    }

    #[test]
    #[serial]
    fn config_display_postgres() {
        let config = PersistenceConfig {
            backend: "postgres".to_string(),
            sqlite_path: "cyberclaw.db".to_string(),
            postgres_url: Some("postgres://secret@host/db".to_string()),
        };
        // URL must not leak into display
        let display = format!("{}", config);
        assert_eq!(display, "postgres(url=****)");
        assert!(!display.contains("secret"));
    }

    #[test]
    #[serial]
    fn config_debug_includes_fields() {
        let config = PersistenceConfig {
            backend: "memory".to_string(),
            sqlite_path: "cyberclaw.db".to_string(),
            postgres_url: None,
        };
        let debug = format!("{:?}", config);
        assert!(debug.contains("memory"));
        assert!(debug.contains("cyberclaw.db"));
    }
}
