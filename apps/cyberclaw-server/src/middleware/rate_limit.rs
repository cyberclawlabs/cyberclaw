//! 速率限制中间件
//!
//! 使用 tower-governor 实现基于 IP 的速率限制，防止 API 滥用和 DoS 攻击

use axum::http;
use governor::middleware::NoOpMiddleware;
use std::net::IpAddr;
use std::sync::Arc;
use tower_governor::{
    governor::GovernorConfigBuilder, key_extractor::KeyExtractor, GovernorError, GovernorLayer,
};

/// 基于客户端 IP 的 Key 提取器
///
/// 优先从 `x-forwarded-for`、`x-real-ip`、`forwarded` 头提取 IP，
/// 回退到 `ConnectInfo` 对端 IP，若全部失败则使用 `127.0.0.1` 作为默认值
/// 确保速率限制中间件不会因为无法提取 IP 而阻断请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FallbackIpKeyExtractor;

impl KeyExtractor for FallbackIpKeyExtractor {
    type Key = IpAddr;

    fn extract<T>(&self, req: &http::Request<T>) -> Result<Self::Key, GovernorError> {
        use std::net::{Ipv4Addr, SocketAddr};

        let headers = req.headers();

        // 尝试从 x-forwarded-for 提取
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v: &axum::http::HeaderValue| v.to_str().ok())
            .and_then(|s: &str| {
                s.split(',')
                    .find_map(|part| part.trim().parse::<IpAddr>().ok())
            })
        {
            return Ok(ip);
        }

        // 尝试从 x-real-ip 提取
        if let Some(ip) = headers
            .get("x-real-ip")
            .and_then(|v: &axum::http::HeaderValue| v.to_str().ok())
            .and_then(|s: &str| s.parse::<IpAddr>().ok())
        {
            return Ok(ip);
        }

        // 尝试从 ConnectInfo 提取对端 IP
        if let Some(ip) = req
            .extensions()
            .get::<axum::extract::ConnectInfo<SocketAddr>>()
            .map(|addr: &axum::extract::ConnectInfo<SocketAddr>| addr.ip())
        {
            return Ok(ip);
        }

        // 回退：使用 loopback 地址，确保中间件不阻断请求
        Ok(IpAddr::V4(Ipv4Addr::LOCALHOST))
    }
}

/// 创建速率限制层
///
/// # 参数
///
/// * `per_second` - 每秒允许的请求数（补充令牌的时间间隔，单位：秒）
/// * `burst_size` - 允许的突发请求数（令牌桶容量）
///
/// # 返回
///
/// 配置好的 GovernorLayer（基于客户端 IP 地址进行速率限制）
pub fn create_rate_limiter(
    per_second: u64,
    burst_size: u32,
) -> GovernorLayer<FallbackIpKeyExtractor, NoOpMiddleware> {
    let governor_config = GovernorConfigBuilder::default()
        .key_extractor(FallbackIpKeyExtractor)
        .per_second(per_second)
        .burst_size(burst_size)
        .finish()
        .expect("速率限制配置无效：per_second 和 burst_size 必须大于 0");

    GovernorLayer {
        config: Arc::new(governor_config),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_rate_limiter() {
        let _limiter = create_rate_limiter(1, 60);
        // 如果能成功创建，说明配置正确
    }

    #[test]
    fn test_create_rate_limiter_default_params() {
        let _limiter = create_rate_limiter(10, 10);
    }
}
