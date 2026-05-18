// tests/helpers/mod.rs
// 测试辅助工具模块

use anyhow::Result;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::fs;
use uuid::Uuid;

pub mod plugin;
pub mod connector;
pub mod scheduler;
pub mod skill;

/// 测试配置
pub struct TestConfig {
    pub temp_dir: TempDir,
    pub test_id: String,
}

impl TestConfig {
    /// 创建新的测试配置
    pub fn new() -> Result<Self> {
        Ok(Self {
            temp_dir: TempDir::new()?,
            test_id: Uuid::new_v4().to_string(),
        })
    }

    /// 获取测试工作目录
    pub fn work_dir(&self) -> &Path {
        self.temp_dir.path()
    }

    /// 创建测试文件
    pub async fn create_file(&self, relative_path: &str, content: &str) -> Result<PathBuf> {
        let file_path = self.temp_dir.path().join(relative_path);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&file_path, content).await?;
        Ok(file_path)
    }

    /// 创建测试目录
    pub async fn create_dir(&self, relative_path: &str) -> Result<PathBuf> {
        let dir_path = self.temp_dir.path().join(relative_path);
        fs::create_dir_all(&dir_path).await?;
        Ok(dir_path)
    }
}

/// 测试运行器
pub struct TestRunner {
    config: TestConfig,
}

impl TestRunner {
    pub fn new() -> Result<Self> {
        Ok(Self {
            config: TestConfig::new()?,
        })
    }

    pub fn config(&self) -> &TestConfig {
        &self.config
    }

    /// 运行异步测试
    pub async fn run<F, Fut>(&self, test: F) -> Result<()>
    where
        F: FnOnce(TestConfig) -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        test(TestConfig::new()?).await
    }
}

/// 等待条件成立
pub async fn wait_for<F>(mut condition: F, timeout: std::time::Duration) -> Result<()>
where
    F: FnMut() -> bool,
{
    let start = std::time::Instant::now();
    while !condition() {
        if start.elapsed() > timeout {
            return Err(anyhow::anyhow!("Timeout waiting for condition"));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Ok(())
}

/// Mock HTTP 服务器
pub mod mock_server {
    use axum::{Router, routing::get, Json};
    use serde_json::json;
    use std::net::SocketAddr;

    pub struct MockHttpServer {
        addr: SocketAddr,
        handle: Option<tokio::task::JoinHandle<()>>,
    }

    impl MockHttpServer {
        /// 启动 Mock 服务器
        pub async fn start() -> anyhow::Result<Self> {
            let app = Router::new()
                .route("/health", get(|| async { Json(json!({"status": "ok"})) }))
                .route("/api/test", get(|| async { Json(json!({"data": "test"})) }));

            let addr: SocketAddr = "127.0.0.1:0".parse()?;
            let listener = tokio::net::TcpListener::bind(addr).await?;
            let addr = listener.local_addr()?;

            let handle = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });

            Ok(Self {
                addr,
                handle: Some(handle),
            })
        }

        pub fn url(&self) -> String {
            format!("http://{}", self.addr)
        }

        pub async fn shutdown(mut self) {
            if let Some(handle) = self.handle.take() {
                handle.abort();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_config_creation() {
        let config = TestConfig::new().unwrap();
        assert!(config.work_dir().exists());
    }

    #[tokio::test]
    async fn test_file_creation() {
        let config = TestConfig::new().unwrap();
        let file_path = config.create_file("test.txt", "content").await.unwrap();
        assert!(file_path.exists());

        let content = fs::read_to_string(file_path).await.unwrap();
        assert_eq!(content, "content");
    }
}