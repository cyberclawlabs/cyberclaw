// tests/integration/e2e/mod.rs
// E2E测试套件框架

use anyhow::Result;
use std::time::Duration;

pub mod platform_e2e;
pub mod plugin_e2e;
pub mod connector_e2e;
pub mod scheduler_e2e;
pub mod skill_e2e;
pub mod workflow_e2e;

/// E2E 测试配置
pub struct E2ETestConfig {
    pub timeout: Duration,
    pub parallel: bool,
    pub cleanup: bool,
}

impl Default for E2ETestConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60),
            parallel: false,
            cleanup: true,
        }
    }
}

/// E2E 测试运行器
pub struct E2ETestRunner {
    config: E2ETestConfig,
}

impl E2ETestRunner {
    pub fn new(config: E2ETestConfig) -> Self {
        Self { config }
    }

    /// 运行完整的 E2E 测试套件
    pub async fn run_all(&self) -> Result<TestReport> {
        let mut report = TestReport::new();

        // 运行各个模块的 E2E 测试
        report.merge(self.run_platform_tests().await?);
        report.merge(self.run_plugin_tests().await?);
        report.merge(self.run_connector_tests().await?);
        report.merge(self.run_scheduler_tests().await?);
        report.merge(self.run_skill_tests().await?);
        report.merge(self.run_workflow_tests().await?);

        Ok(report)
    }

    async fn run_platform_tests(&self) -> Result<TestReport> {
        platform_e2e::run_tests(&self.config).await
    }

    async fn run_plugin_tests(&self) -> Result<TestReport> {
        plugin_e2e::run_tests(&self.config).await
    }

    async fn run_connector_tests(&self) -> Result<TestReport> {
        connector_e2e::run_tests(&self.config).await
    }

    async fn run_scheduler_tests(&self) -> Result<TestReport> {
        scheduler_e2e::run_tests(&self.config).await
    }

    async fn run_skill_tests(&self) -> Result<TestReport> {
        skill_e2e::run_tests(&self.config).await
    }

    async fn run_workflow_tests(&self) -> Result<TestReport> {
        workflow_e2e::run_tests(&self.config).await
    }
}

/// 测试报告
#[derive(Debug, Clone)]
pub struct TestReport {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub duration: Duration,
    pub failures: Vec<TestFailure>,
}

impl TestReport {
    pub fn new() -> Self {
        Self {
            total: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
            duration: Duration::from_secs(0),
            failures: Vec::new(),
        }
    }

    pub fn merge(&mut self, other: TestReport) {
        self.total += other.total;
        self.passed += other.passed;
        self.failed += other.failed;
        self.skipped += other.skipped;
        self.duration += other.duration;
        self.failures.extend(other.failures);
    }

    pub fn success_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        (self.passed as f64 / self.total as f64) * 100.0
    }

    pub fn print_summary(&self) {
        println!("\n=== E2E Test Report ===");
        println!("Total:   {} tests", self.total);
        println!("Passed:  {} tests", self.passed);
        println!("Failed:  {} tests", self.failed);
        println!("Skipped: {} tests", self.skipped);
        println!("Success Rate: {:.2}%", self.success_rate());
        println!("Duration: {:?}", self.duration);

        if !self.failures.is_empty() {
            println!("\n=== Failures ===");
            for failure in &self.failures {
                println!("\n[{}] {}", failure.test_name, failure.message);
                if let Some(stack) = &failure.stack_trace {
                    println!("{}", stack);
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TestFailure {
    pub test_name: String,
    pub message: String,
    pub stack_trace: Option<String>,
}

/// 测试场景
pub struct TestScenario {
    pub name: String,
    pub description: String,
    pub steps: Vec<TestStep>,
}

pub struct TestStep {
    pub description: String,
    pub action: Box<dyn Fn() -> Result<()>>,
    pub assertion: Box<dyn Fn() -> Result<()>>,
}

/// E2E 测试宏
#[macro_export]
macro_rules! e2e_test {
    ($name:ident, $description:expr, $body:block) => {
        #[tokio::test]
        async fn $name() {
            println!("Running E2E test: {}", $description);
            let start = std::time::Instant::now();

            let result = async { $body }.await;

            let duration = start.elapsed();

            match result {
                Ok(_) => println!("✓ {} ({}ms)", $description, duration.as_millis()),
                Err(e) => {
                    println!("✗ {} ({}ms)", $description, duration.as_millis());
                    println!("  Error: {}", e);
                    panic!("Test failed: {}", e);
                }
            }
        }
    };
}

/// 测试工具函数
pub mod utils {
    use super::*;
    use tokio::time::{sleep, timeout};

    /// 等待条件满足
    pub async fn wait_until<F>(
        condition: F,
        timeout_duration: Duration,
        check_interval: Duration,
    ) -> Result<()>
    where
        F: Fn() -> bool,
    {
        let start = std::time::Instant::now();

        loop {
            if condition() {
                return Ok(());
            }

            if start.elapsed() > timeout_duration {
                return Err(anyhow::anyhow!("Timeout waiting for condition"));
            }

            sleep(check_interval).await;
        }
    }

    /// 重试函数
    pub async fn retry<F, T>(
        mut action: F,
        max_attempts: usize,
        delay: Duration,
    ) -> Result<T>
    where
        F: FnMut() -> Result<T>,
    {
        for attempt in 1..=max_attempts {
            match action() {
                Ok(result) => return Ok(result),
                Err(e) if attempt == max_attempts => return Err(e),
                Err(_) => {
                    println!("Attempt {} failed, retrying...", attempt);
                    sleep(delay).await;
                }
            }
        }

        unreachable!()
    }

    /// 并行执行测试
    pub async fn run_parallel<F, Fut>(
        tests: Vec<(&str, F)>,
    ) -> Vec<Result<()>>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        let mut handles = Vec::new();

        for (name, test) in tests {
            let name = name.to_string();
            let handle = tokio::spawn(async move {
                println!("Starting parallel test: {}", name);
                test().await
            });
            handles.push(handle);
        }

        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(Err(anyhow::anyhow!("Task panicked: {}", e))),
            }
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_report_merge() {
        let mut report1 = TestReport::new();
        report1.total = 10;
        report1.passed = 8;
        report1.failed = 2;

        let mut report2 = TestReport::new();
        report2.total = 5;
        report2.passed = 4;
        report2.failed = 1;

        report1.merge(report2);

        assert_eq!(report1.total, 15);
        assert_eq!(report1.passed, 12);
        assert_eq!(report1.failed, 3);
        assert_eq!(report1.success_rate(), 80.0);
    }

    e2e_test!(test_macro_example, "Example E2E test using macro", {
        // 测试步骤
        let result = 1 + 1;
        assert_eq!(result, 2);
        Ok::<(), anyhow::Error>(())
    });
}