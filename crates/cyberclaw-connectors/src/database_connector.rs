//! Database Connector - Execute SQL operations across multiple database types
//!
//! Provides unified database access capabilities for PostgreSQL, MySQL, and SQLite.
//!
//! ## Capabilities
//!
//! - `db.query`: Execute SELECT queries
//! - `db.execute`: Execute INSERT/UPDATE/DELETE statements
//! - `db.transaction`: Execute statements in a transaction
//! - `db.migrate`: Run database migrations

use crate::types::{
    CapabilityExecutionRequest, CapabilityExecutionResult, Connector,
    ExecutionStatus as ConnectorExecutionStatus,
};
use cyberclaw_core::manifests::{CapabilityContract, ConnectorRuntime};
use cyberclaw_core::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::{AnyPool, Column, Row, TypeInfo, ValueRef};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

/// Database type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseType {
    /// PostgreSQL database
    PostgreSQL,
    /// MySQL database
    MySQL,
    /// SQLite database
    SQLite,
}

impl DatabaseType {
    /// Get the sqlx driver name
    #[allow(dead_code)]
    fn driver_name(&self) -> &'static str {
        match self {
            DatabaseType::PostgreSQL => "postgres",
            DatabaseType::MySQL => "mysql",
            DatabaseType::SQLite => "sqlite",
        }
    }
}

/// Database connection pool wrapper
#[derive(Debug)]
pub struct DatabasePool {
    pool: AnyPool,
    db_type: DatabaseType,
}

impl DatabasePool {
    /// Create a new database pool
    pub async fn new(url: &str, db_type: DatabaseType) -> anyhow::Result<Self> {
        debug!("Creating database pool for {:?}", db_type);
        let pool = AnyPool::connect(url).await?;
        Ok(Self { pool, db_type })
    }

    /// Get a reference to the underlying pool
    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    /// Get the database type
    pub fn db_type(&self) -> DatabaseType {
        self.db_type
    }

    /// Close the pool
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// Database Connector
#[derive(Debug)]
pub struct DatabaseConnector {
    id: ConnectorId,
    pools: Arc<RwLock<HashMap<String, DatabasePool>>>,
    capabilities: Vec<CapabilityContract>,
}

impl DatabaseConnector {
    /// Create a new DatabaseConnector
    pub fn new(connector_id: &str) -> Self {
        let id = ConnectorId::from_string(connector_id.to_string())
            .unwrap_or_else(|_| ConnectorId::from_string("database".to_string()).unwrap());
        let capabilities = Self::build_capabilities();

        Self {
            id,
            pools: Arc::new(RwLock::new(HashMap::new())),
            capabilities,
        }
    }

    /// Register a database connection pool
    pub async fn register_pool(
        &self,
        name: String,
        url: &str,
        db_type: DatabaseType,
    ) -> anyhow::Result<()> {
        info!("Registering database pool '{}' ({:?})", name, db_type);
        let pool = DatabasePool::new(url, db_type).await?;
        self.pools.write().await.insert(name, pool);
        Ok(())
    }

    /// Get a database pool by name
    async fn get_pool(&self, name: &str) -> anyhow::Result<DatabasePool> {
        let pools = self.pools.read().await;
        pools
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("Database pool '{}' not found", name))
            .map(|p| DatabasePool {
                pool: p.pool.clone(),
                db_type: p.db_type,
            })
    }

    /// Build the list of capabilities
    fn build_capabilities() -> Vec<CapabilityContract> {
        vec![
            CapabilityContract {
                id: "db.query".to_string(),
                title: "Database Query".to_string(),
                description: Some("Execute a SELECT query".to_string()),
                input_schema: "DbQueryInput".to_string(),
                output_schema: "DbQueryOutput".to_string(),
                risk: RiskLevel::Low,
                effects: vec![CapabilityEffect::Read],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(30000),
                },
            },
            CapabilityContract {
                id: "db.execute".to_string(),
                title: "Database Execute".to_string(),
                description: Some("Execute INSERT/UPDATE/DELETE statement".to_string()),
                input_schema: "DbExecuteInput".to_string(),
                output_schema: "DbExecuteOutput".to_string(),
                risk: RiskLevel::Medium,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(30000),
                },
            },
            CapabilityContract {
                id: "db.transaction".to_string(),
                title: "Database Transaction".to_string(),
                description: Some("Execute statements in a transaction".to_string()),
                input_schema: "DbTransactionInput".to_string(),
                output_schema: "DbTransactionOutput".to_string(),
                risk: RiskLevel::High,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(60000),
                },
            },
            CapabilityContract {
                id: "db.migrate".to_string(),
                title: "Database Migration".to_string(),
                description: Some("Run database migrations".to_string()),
                input_schema: "DbMigrateInput".to_string(),
                output_schema: "DbMigrateOutput".to_string(),
                risk: RiskLevel::Critical,
                effects: vec![CapabilityEffect::Write],
                placement: None,
                timeouts: CapabilityTimeouts {
                    request_ms: Some(120000),
                },
            },
        ]
    }

    /// Execute a query
    async fn query(&self, input: DbQueryInput) -> anyhow::Result<serde_json::Value> {
        let pool = self.get_pool(&input.database).await?;
        debug!(
            "Executing query on database '{}': {}",
            input.database, input.sql
        );

        let rows: Vec<sqlx::any::AnyRow> = sqlx::query(&input.sql).fetch_all(pool.pool()).await?;

        let results: Vec<HashMap<String, serde_json::Value>> = rows
            .iter()
            .map(row_to_json)
            .collect::<Result<Vec<_>, _>>()?;

        let output = DbQueryOutput {
            rows: results,
            count: rows.len() as u64,
        };

        Ok(serde_json::to_value(output)?)
    }

    /// Execute a statement
    async fn execute(&self, input: DbExecuteInput) -> anyhow::Result<serde_json::Value> {
        let pool = self.get_pool(&input.database).await?;
        debug!(
            "Executing statement on database '{}': {}",
            input.database, input.sql
        );

        let result: sqlx::any::AnyQueryResult =
            sqlx::query(&input.sql).execute(pool.pool()).await?;

        let output = DbExecuteOutput {
            rows_affected: result.rows_affected(),
        };

        Ok(serde_json::to_value(output)?)
    }

    /// Execute a transaction
    async fn transaction(&self, input: DbTransactionInput) -> anyhow::Result<serde_json::Value> {
        let pool = self.get_pool(&input.database).await?;
        debug!(
            "Executing transaction on database '{}' with {} statements",
            input.database,
            input.statements.len()
        );

        let mut tx = pool.pool().begin().await?;

        let mut results = Vec::new();
        for statement in &input.statements {
            let result = sqlx::query(statement).execute(&mut *tx).await?;
            results.push(result.rows_affected());
        }

        tx.commit().await?;

        let output = DbTransactionOutput {
            statements_executed: results.len() as u64,
            rows_affected: results.iter().sum(),
        };

        Ok(serde_json::to_value(output)?)
    }

    /// Execute migrations
    async fn migrate(&self, input: DbMigrateInput) -> anyhow::Result<serde_json::Value> {
        let pool = self.get_pool(&input.database).await?;
        debug!(
            "Executing {} migrations on database '{}'",
            input.migrations.len(),
            input.database
        );

        let mut applied = Vec::new();
        for migration in &input.migrations {
            debug!("Applying migration: {}", migration.name);
            let mut tx = pool.pool().begin().await?;

            for statement in &migration.statements {
                sqlx::query(statement).execute(&mut *tx).await?;
            }

            tx.commit().await?;
            applied.push(migration.name.clone());
        }

        let count = applied.len() as u64;
        let output = DbMigrateOutput {
            migrations_applied: applied,
            count,
        };

        Ok(serde_json::to_value(output)?)
    }
}

#[async_trait::async_trait]
impl Connector for DatabaseConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn runtime(&self) -> ConnectorRuntime {
        ConnectorRuntime::Native
    }

    fn capabilities(&self) -> Vec<CapabilityContract> {
        self.capabilities.clone()
    }

    async fn execute(
        &self,
        request: CapabilityExecutionRequest,
    ) -> anyhow::Result<CapabilityExecutionResult> {
        debug!(
            "DatabaseConnector executing capability {} for execution {}",
            request.capability_id, request.execution_id
        );

        let result = match request.capability_id.as_str() {
            "db.query" => {
                let input: DbQueryInput = serde_json::from_value(request.input.clone())?;
                self.query(input).await
            }
            "db.execute" => {
                let input: DbExecuteInput = serde_json::from_value(request.input.clone())?;
                self.execute(input).await
            }
            "db.transaction" => {
                let input: DbTransactionInput = serde_json::from_value(request.input.clone())?;
                self.transaction(input).await
            }
            "db.migrate" => {
                let input: DbMigrateInput = serde_json::from_value(request.input.clone())?;
                self.migrate(input).await
            }
            _ => {
                let error_msg = format!("Unknown capability: {}", request.capability_id);
                error!("{}", error_msg);
                return Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({
                        "error": error_msg.clone()
                    }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(error_msg),
                    actual_runtime: None,
                });
            }
        };

        match result {
            Ok(output) => {
                info!("Capability {} executed successfully", request.capability_id);
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output,
                    status: ConnectorExecutionStatus::Success,
                    error: None,
                    actual_runtime: None,
                })
            }
            Err(e) => {
                error!("Capability {} failed: {:?}", request.capability_id, e);
                Ok(CapabilityExecutionResult {
                    execution_id: request.execution_id,
                    trace_id: request.trace_id,
                    connector_id: request.connector_id,
                    capability_id: request.capability_id,
                    output: serde_json::json!({
                        "error": e.to_string()
                    }),
                    status: ConnectorExecutionStatus::Failed,
                    error: Some(e.to_string()),
                    actual_runtime: None,
                })
            }
        }
    }
}

/// Input/output schemas for database capabilities

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbQueryInput {
    pub database: String,
    pub sql: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbQueryOutput {
    pub rows: Vec<HashMap<String, serde_json::Value>>,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbExecuteInput {
    pub database: String,
    pub sql: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbExecuteOutput {
    pub rows_affected: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbTransactionInput {
    pub database: String,
    pub statements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbTransactionOutput {
    pub statements_executed: u64,
    pub rows_affected: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbMigration {
    pub name: String,
    pub statements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbMigrateInput {
    pub database: String,
    pub migrations: Vec<DbMigration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbMigrateOutput {
    pub migrations_applied: Vec<String>,
    pub count: u64,
}

/// Convert a sqlx Row to a JSON object
fn row_to_json(row: &sqlx::any::AnyRow) -> anyhow::Result<HashMap<String, serde_json::Value>> {
    let mut map = HashMap::new();

    for (i, column) in row.columns().iter().enumerate() {
        let name = column.name().to_string();
        let value = row_value_to_json(row, i, column.type_info())?;
        map.insert(name, value);
    }

    Ok(map)
}

/// Convert a sqlx value to JSON
fn row_value_to_json(
    row: &sqlx::any::AnyRow,
    index: usize,
    type_info: &sqlx::any::AnyTypeInfo,
) -> anyhow::Result<serde_json::Value> {
    // Check if the value is NULL
    if row.try_get_raw(index)?.is_null() {
        return Ok(serde_json::Value::Null);
    }

    let type_name = type_info.name();

    // Try to match common types - using fallback string conversion
    let value = match type_name {
        "BOOL" | "BOOLEAN" => match row.try_get::<bool, _>(index) {
            Ok(v) => serde_json::Value::Bool(v),
            Err(_) => serde_json::Value::String(format!("<{}>", type_name)),
        },
        "INT2" | "SMALLINT" => match row.try_get::<i16, _>(index) {
            Ok(v) => serde_json::Value::Number(v.into()),
            Err(_) => serde_json::Value::String(format!("<{}>", type_name)),
        },
        "INT4" | "INT" | "INTEGER" => match row.try_get::<i32, _>(index) {
            Ok(v) => serde_json::Value::Number(v.into()),
            Err(_) => serde_json::Value::String(format!("<{}>", type_name)),
        },
        "INT8" | "BIGINT" => match row.try_get::<i64, _>(index) {
            Ok(v) => serde_json::Value::Number(v.into()),
            Err(_) => serde_json::Value::String(format!("<{}>", type_name)),
        },
        "FLOAT4" | "REAL" => match row.try_get::<f32, _>(index) {
            Ok(v) => serde_json::json!(v),
            Err(_) => serde_json::Value::String(format!("<{}>", type_name)),
        },
        "FLOAT8" | "DOUBLE" | "DOUBLE PRECISION" => match row.try_get::<f64, _>(index) {
            Ok(v) => serde_json::json!(v),
            Err(_) => serde_json::Value::String(format!("<{}>", type_name)),
        },
        "TEXT" | "VARCHAR" | "CHAR" | "NVARCHAR" => match row.try_get::<String, _>(index) {
            Ok(v) => serde_json::Value::String(v),
            Err(_) => serde_json::Value::String(format!("<{}>", type_name)),
        },
        _ => {
            // Fallback: try to get as string
            match row.try_get::<String, _>(index) {
                Ok(v) => serde_json::Value::String(v),
                Err(_) => serde_json::Value::String(format!("<{}>", type_name)),
            }
        }
    };

    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_database_connector_creation() {
        let connector = DatabaseConnector::new("test-database");
        assert_eq!(connector.id().as_str(), "test-database");
        assert_eq!(connector.capabilities().len(), 4);
    }

    #[tokio::test]
    async fn test_capabilities_definition() {
        let connector = DatabaseConnector::new("test");
        let caps = connector.capabilities();

        assert!(caps.iter().any(|c| c.id == "db.query"));
        assert!(caps.iter().any(|c| c.id == "db.execute"));
        assert!(caps.iter().any(|c| c.id == "db.transaction"));
        assert!(caps.iter().any(|c| c.id == "db.migrate"));
    }
}
