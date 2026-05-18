//! 配置管理模块
//!
//! 管理服务器配置,包括监听地址、端口、超时等

use std::net::SocketAddr;
use std::time::Duration;

/// 服务器配置
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// 监听地址
    pub addr: SocketAddr,

    /// 请求超时时间
    pub request_timeout: Duration,

    /// CORS 允许的源
    pub cors_origins: Vec<String>,

    /// 最大请求体大小 (字节)
    pub max_body_size: usize,

    /// 速率限制：每秒允许的请求数
    pub rate_limit_per_second: u64,

    /// 速率限制：允许的突发请求数
    pub rate_limit_burst_size: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: "127.0.0.1:3000".parse().expect("Invalid default address"),
            request_timeout: Duration::from_secs(30),
            cors_origins: vec![
                "http://localhost:3000".to_string(),
                "http://localhost:5173".to_string(),
            ],
            max_body_size: 10 * 1024 * 1024, // 10MB
            // 默认面向 single-operator admin console；冷启动 SPA 会并发触发 8-15 个 API
            // 调用，多 tab + 快速刷新场景 60 burst 很快耗尽，导致 429 大面积出现，
            // 用户感知是"页面打开就是空的"。把限额放宽到对单运营者足够慷慨、
            // 同时仍能挡 DoS（30 req/s 持续 = 2.5 万/天，仍远低于真正 DoS 量级）。
            rate_limit_per_second: 30, // 持续 30 req/s（~6 个 SPA 冷启动 / 秒）
            rate_limit_burst_size: 300, // 突发 300 次（5 个并发 tab × 8 个 API × ~7x 余量）
        }
    }
}

impl ServerConfig {
    fn parse_origins(origins: &str) -> Vec<String> {
        origins
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(ToString::to_string)
            .collect()
    }

    /// 从环境变量创建配置
    pub fn from_env() -> Self {
        let mut config = Self::default();

        // 从环境变量读取监听地址
        if let Ok(addr_str) = std::env::var("CYBERCLAW_ADDR") {
            if let Ok(addr) = addr_str.parse() {
                config.addr = addr;
            }
        }

        // 从环境变量读取超时设置
        if let Ok(timeout_str) = std::env::var("CYBERCLAW_TIMEOUT") {
            if let Ok(timeout_secs) = timeout_str.parse::<u64>() {
                config.request_timeout = Duration::from_secs(timeout_secs);
            }
        }

        // 从环境变量读取 CORS 源
        // 统一契约：优先 ALLOWED_ORIGINS，向后兼容 CYBERCLAW_CORS_ORIGINS。
        if let Ok(origins) = std::env::var("ALLOWED_ORIGINS") {
            let parsed = Self::parse_origins(&origins);
            if !parsed.is_empty() {
                config.cors_origins = parsed;
            } else if let Ok(legacy) = std::env::var("CYBERCLAW_CORS_ORIGINS") {
                let legacy_parsed = Self::parse_origins(&legacy);
                if !legacy_parsed.is_empty() {
                    config.cors_origins = legacy_parsed;
                }
            }
        } else if let Ok(legacy) = std::env::var("CYBERCLAW_CORS_ORIGINS") {
            let legacy_parsed = Self::parse_origins(&legacy);
            if !legacy_parsed.is_empty() {
                config.cors_origins = legacy_parsed;
            }
        }

        // Dev-mode rate-limit upshift — production defaults (30 r/s + 300
        // burst) are too tight for frequent reload / e2e / curl probing on
        // localhost. When ENVIRONMENT=development AND the operator has NOT
        // overridden via env vars, bump to 500/5000. Production deployments
        // (ENVIRONMENT!=development) keep the conservative defaults.
        let env_is_dev = std::env::var("ENVIRONMENT")
            .map(|v| v == "development")
            .unwrap_or(false);
        if env_is_dev {
            if std::env::var("RATE_LIMIT_PER_SECOND").is_err() {
                config.rate_limit_per_second = 500;
            }
            if std::env::var("RATE_LIMIT_BURST_SIZE").is_err() {
                config.rate_limit_burst_size = 5000;
            }
        }

        // 从环境变量读取速率限制配置
        if let Ok(per_second_str) = std::env::var("RATE_LIMIT_PER_SECOND") {
            if let Ok(per_second) = per_second_str.parse::<u64>() {
                config.rate_limit_per_second = per_second;
            }
        }

        if let Ok(burst_size_str) = std::env::var("RATE_LIMIT_BURST_SIZE") {
            if let Ok(burst_size) = burst_size_str.parse::<u32>() {
                config.rate_limit_burst_size = burst_size;
            }
        }

        // 从环境变量读取最大请求体大小
        if let Ok(max_body_str) = std::env::var("MAX_REQUEST_BODY_SIZE") {
            if let Ok(max_body) = max_body_str.parse::<usize>() {
                config.max_body_size = max_body;
            }
        }

        config
    }

    /// 创建自定义配置
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(original) = &self.original {
                unsafe {
                    std::env::set_var(self.key, original);
                }
            } else {
                unsafe {
                    std::env::remove_var(self.key);
                }
            }
        }
    }

    #[test]
    fn test_default_config() {
        let config = ServerConfig::default();
        assert_eq!(config.addr.port(), 3000);
        assert_eq!(config.request_timeout, Duration::from_secs(30));
        assert_eq!(config.max_body_size, 10 * 1024 * 1024);
    }

    #[test]
    fn test_custom_config() {
        let addr: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        let config = ServerConfig::new(addr);
        assert_eq!(config.addr, addr);
    }

    #[test]
    fn test_from_env_reads_allowed_origins_contract() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock poisoned");
        let _cors_guard = EnvVarGuard::set("CYBERCLAW_CORS_ORIGINS", "");
        let _allowed_guard = EnvVarGuard::set(
            "ALLOWED_ORIGINS",
            "https://a.example.com,https://b.example.com",
        );
        let config = ServerConfig::from_env();
        assert_eq!(
            config.cors_origins,
            vec![
                "https://a.example.com".to_string(),
                "https://b.example.com".to_string()
            ]
        );
    }

    #[test]
    fn test_from_env_fallbacks_to_legacy_cors_origins() {
        let _lock = ENV_TEST_LOCK.lock().expect("env test lock poisoned");
        let _allowed_guard = EnvVarGuard::set("ALLOWED_ORIGINS", "");
        let _cors_guard = EnvVarGuard::set("CYBERCLAW_CORS_ORIGINS", "https://legacy.example.com");
        let config = ServerConfig::from_env();
        assert_eq!(
            config.cors_origins,
            vec!["https://legacy.example.com".to_string()]
        );
    }
}
