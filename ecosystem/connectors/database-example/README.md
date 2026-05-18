# Database Connector Example

The Database Connector provides unified database access across PostgreSQL, MySQL, and SQLite.

## Features

- **Multi-database support**: PostgreSQL, MySQL, SQLite
- **Connection pooling**: Efficient connection management
- **Transaction support**: ACID-compliant transactions
- **Migration support**: Database schema migrations
- **Type-safe operations**: Strong typing with Rust

## Capabilities

| Capability | Risk Level | Description |
|------------|-----------|-------------|
| `db.query` | Low | Execute SELECT queries |
| `db.execute` | Medium | Execute INSERT/UPDATE/DELETE |
| `db.transaction` | High | Execute transactional statements |
| `db.migrate` | Critical | Run database migrations |

## Quick Start

### 1. Setup Databases

**PostgreSQL:**
```bash
docker run -d \
  --name postgres-test \
  -p 5432:5432 \
  -e POSTGRES_PASSWORD=test \
  postgres:15
```

**MySQL:**
```bash
docker run -d \
  --name mysql-test \
  -p 3306:3306 \
  -e MYSQL_ROOT_PASSWORD=test \
  mysql:8
```

**SQLite:**
```bash
# No setup needed - uses in-memory or file-based database
```

### 2. Register Database Pools

```rust
use cyberclaw_connectors::{DatabaseConnector, DatabaseType};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let connector = Arc::new(DatabaseConnector::new("database"));

    // PostgreSQL
    connector.register_pool(
        "postgres".to_string(),
        "postgres://postgres:test@localhost:5432/test",
        DatabaseType::PostgreSQL,
    ).await?;

    // MySQL
    connector.register_pool(
        "mysql".to_string(),
        "mysql://root:test@localhost:3306/test",
        DatabaseType::MySQL,
    ).await?;

    // SQLite
    connector.register_pool(
        "sqlite".to_string(),
        "sqlite::memory:",
        DatabaseType::SQLite,
    ).await?;

    Ok(())
}
```

### 3. Execute Queries

```rust
use cyberclaw_connectors::{Connector, DbQueryInput};
use cyberclaw_core::prelude::*;

async fn query_users(connector: &DatabaseConnector) -> anyhow::Result<()> {
    let input = DbQueryInput {
        database: "postgres".to_string(),
        sql: "SELECT id, name FROM users".to_string(),
    };

    let request = CapabilityExecutionRequest {
        execution_id: ExecutionId::new(),
        trace_id: "trace-001".to_string(),
        actor: ActorRef::system(),
        workspace: WorkspaceRef::default(),
        connector_id: ConnectorId::from_string("database".to_string())?,
        capability_id: CapabilityId::new("db.query"),
        input: serde_json::to_value(&input)?,
    };

    let result = connector.execute(request).await?;
    println!("Query result: {:?}", result.output);

    Ok(())
}
```

### 4. Execute Statements

```rust
use cyberclaw_connectors::DbExecuteInput;

async fn insert_user(connector: &DatabaseConnector) -> anyhow::Result<()> {
    let input = DbExecuteInput {
        database: "postgres".to_string(),
        sql: "INSERT INTO users (name, email) VALUES ('Alice', 'alice@example.com')".to_string(),
    };

    let request = create_request("db.execute", &input)?;
    let result = connector.execute(request).await?;

    let output: DbExecuteOutput = serde_json::from_value(result.output)?;
    println!("Rows affected: {}", output.rows_affected);

    Ok(())
}
```

### 5. Run Transactions

```rust
use cyberclaw_connectors::DbTransactionInput;

async fn transfer_funds(connector: &DatabaseConnector) -> anyhow::Result<()> {
    let input = DbTransactionInput {
        database: "postgres".to_string(),
        statements: vec![
            "UPDATE accounts SET balance = balance - 100 WHERE id = 1".to_string(),
            "UPDATE accounts SET balance = balance + 100 WHERE id = 2".to_string(),
            "INSERT INTO transfers (from_id, to_id, amount) VALUES (1, 2, 100)".to_string(),
        ],
    };

    let request = create_request("db.transaction", &input)?;
    let result = connector.execute(request).await?;

    let output: DbTransactionOutput = serde_json::from_value(result.output)?;
    println!("Statements executed: {}", output.statements_executed);
    println!("Rows affected: {}", output.rows_affected);

    Ok(())
}
```

### 6. Run Migrations

```rust
use cyberclaw_connectors::{DbMigrateInput, DbMigration};

async fn run_migrations(connector: &DatabaseConnector) -> anyhow::Result<()> {
    let input = DbMigrateInput {
        database: "postgres".to_string(),
        migrations: vec![
            DbMigration {
                name: "001_create_users".to_string(),
                statements: vec![
                    "CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT, email TEXT UNIQUE)".to_string(),
                    "CREATE INDEX idx_users_email ON users(email)".to_string(),
                ],
            },
            DbMigration {
                name: "002_add_timestamps".to_string(),
                statements: vec![
                    "ALTER TABLE users ADD COLUMN created_at TIMESTAMP DEFAULT NOW()".to_string(),
                ],
            },
        ],
    };

    let request = create_request("db.migrate", &input)?;
    let result = connector.execute(request).await?;

    let output: DbMigrateOutput = serde_json::from_value(result.output)?;
    println!("Migrations applied: {:?}", output.migrations_applied);

    Ok(())
}
```

## Running Tests

### SQLite Tests (No Setup Required)

```bash
cargo test --test database_connector_tests
```

### PostgreSQL Tests

```bash
# Start PostgreSQL
docker run -d -p 5432:5432 -e POSTGRES_PASSWORD=test postgres:15

# Run tests
POSTGRES_URL=postgres://postgres:test@localhost:5432/postgres \
cargo test --test database_connector_tests -- --ignored test_postgres
```

### MySQL Tests

```bash
# Start MySQL
docker run -d -p 3306:3306 -e MYSQL_ROOT_PASSWORD=test mysql:8

# Wait for MySQL to be ready
sleep 10

# Create test database
docker exec -it $(docker ps -q -f ancestor=mysql:8) \
  mysql -uroot -ptest -e "CREATE DATABASE IF NOT EXISTS test;"

# Run tests
MYSQL_URL=mysql://root:test@localhost:3306/test \
cargo test --test database_connector_tests -- --ignored test_mysql
```

### Run All Tests

```bash
# Start all databases
docker-compose up -d

# Run all tests
POSTGRES_URL=postgres://postgres:test@localhost:5432/postgres \
MYSQL_URL=mysql://root:test@localhost:3306/test \
cargo test --test database_connector_tests -- --ignored
```

## Configuration

See `database-config.yaml` for detailed configuration examples.

## Security Considerations

1. **Connection Strings**: Never commit credentials to version control
   - Use environment variables
   - Use secrets management (AWS Secrets Manager, Vault)

2. **SQL Injection**: Always use parameterized queries when possible
   - The connector does not currently support parameter binding
   - Ensure input validation at the application layer

3. **Permissions**: Grant minimal database permissions
   - Read-only users for queries
   - Restricted users for write operations
   - DBA users only for migrations

4. **Audit Logging**: Enable audit logging for compliance
   - Log all database operations
   - Track slow queries
   - Monitor connection pool usage

## Troubleshooting

### Connection Refused

```
Error: Failed to connect to database
```

**Solution**: Ensure database is running and accessible
```bash
# PostgreSQL
docker ps | grep postgres
psql -h localhost -U postgres -d test

# MySQL
docker ps | grep mysql
mysql -h 127.0.0.1 -u root -ptest test
```

### Pool Exhausted

```
Error: Connection pool exhausted
```

**Solution**: Increase pool size or reduce connection lifetime
```yaml
pool:
  max_connections: 20  # Increase from default
```

### Slow Queries

```
Warning: Query took 5000ms
```

**Solution**:
1. Add database indexes
2. Optimize query structure
3. Use connection pooling effectively
4. Monitor with `EXPLAIN ANALYZE`

## Advanced Usage

### Custom Type Mapping

The connector automatically maps database types to JSON:

| Database Type | JSON Type |
|---------------|-----------|
| INTEGER, BIGINT | Number |
| REAL, DOUBLE | Number |
| TEXT, VARCHAR | String |
| BOOLEAN | Boolean |
| JSON, JSONB | Object |
| TIMESTAMP | String (ISO 8601) |
| UUID | String |

### Connection Pool Tuning

```rust
// For high-concurrency workloads
connector.register_pool_with_config(
    "postgres".to_string(),
    "postgres://...",
    DatabaseType::PostgreSQL,
    PoolConfig {
        max_connections: 50,
        min_connections: 10,
        connection_timeout: Duration::from_secs(5),
        idle_timeout: Duration::from_secs(300),
    }
).await?;
```

## License

Apache 2.0
