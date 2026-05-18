//! Tenant Resource Quota Management
//!
//! 提供租户级资源配额管理功能，包括：
//! - 并发执行数限制
//! - 请求速率限制
//! - 存储配额
//! - API 调用配额
//!
//! 与 PolicyEngine 集成，支持配额超限时的治理决策。

use crate::decision::{GovernanceDecision, ReviewType};
use crate::engine::{EvaluationContext, EvaluationResult, PolicyEngine};
use async_trait::async_trait;
use cyberclaw_core::ids::TenantId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, warn};

/// Tenant resource quota definition
///
/// 定义租户的资源配额限制
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TenantQuota {
    /// 租户 ID
    pub tenant_id: TenantId,

    /// 最大并发执行数 (None = 无限制)
    pub max_concurrent_executions: Option<u32>,

    /// 每分钟最大请求数 (None = 无限制)
    pub requests_per_minute: Option<u32>,

    /// 存储配额（字节）(None = 无限制)
    pub max_storage_bytes: Option<u64>,

    /// 每天最大 API 调用数 (None = 无限制)
    pub api_calls_per_day: Option<u32>,

    /// 配额是否启用
    pub enabled: bool,
}

impl TenantQuota {
    /// Create a new tenant quota
    pub fn new(tenant_id: TenantId) -> Self {
        Self {
            tenant_id,
            max_concurrent_executions: None,
            max_storage_bytes: None,
            requests_per_minute: None,
            api_calls_per_day: None,
            enabled: true,
        }
    }

    /// Create a default quota with reasonable limits
    pub fn default_quota(tenant_id: TenantId) -> Self {
        Self {
            tenant_id,
            max_concurrent_executions: Some(10),
            requests_per_minute: Some(100),
            max_storage_bytes: Some(1_073_741_824), // 1 GB
            api_calls_per_day: Some(10_000),
            enabled: true,
        }
    }

    /// Create an unlimited quota
    pub fn unlimited(tenant_id: TenantId) -> Self {
        Self {
            tenant_id,
            max_concurrent_executions: None,
            requests_per_minute: None,
            max_storage_bytes: None,
            api_calls_per_day: None,
            enabled: true,
        }
    }

    /// Check if a quota limit is defined
    pub fn has_limits(&self) -> bool {
        self.enabled
            && (self.max_concurrent_executions.is_some()
                || self.requests_per_minute.is_some()
                || self.max_storage_bytes.is_some()
                || self.api_calls_per_day.is_some())
    }
}

/// Tenant resource usage tracking
///
/// 跟踪租户的当前资源使用情况
#[derive(Debug, Clone, Default)]
struct TenantUsage {
    /// 当前并发执行数
    concurrent_executions: u32,

    /// 当前分钟内的请求数
    requests_this_minute: u32,

    /// 当前分钟的开始时间戳（秒）
    minute_start: u64,

    /// 当前存储使用量（字节）
    storage_bytes_used: u64,

    /// 今天的 API 调用数
    api_calls_today: u32,

    /// 今天的开始时间戳（秒）
    day_start: u64,
}

impl TenantUsage {
    #[allow(dead_code)]
    fn new() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Self {
            concurrent_executions: 0,
            requests_this_minute: 0,
            minute_start: now,
            storage_bytes_used: 0,
            api_calls_today: 0,
            day_start: now,
        }
    }

    /// Reset minute-based counters if needed
    fn reset_minute_if_needed(&mut self, now: u64) {
        if now - self.minute_start >= 60 {
            self.requests_this_minute = 0;
            self.minute_start = now;
        }
    }

    /// Reset day-based counters if needed
    fn reset_day_if_needed(&mut self, now: u64) {
        if now - self.day_start >= 86400 {
            self.api_calls_today = 0;
            self.day_start = now;
        }
    }
}

/// Quota check result
#[derive(Debug, Clone, PartialEq)]
pub enum QuotaCheckResult {
    /// Quota check passed
    Ok,

    /// Concurrent execution limit exceeded
    ConcurrentLimitExceeded { current: u32, limit: u32 },

    /// Request rate limit exceeded
    RateLimitExceeded { current: u32, limit: u32 },

    /// Storage quota exceeded
    StorageQuotaExceeded { current: u64, limit: u64 },

    /// API call quota exceeded
    ApiQuotaExceeded { current: u32, limit: u32 },

    /// Quota not found for tenant
    QuotaNotFound,

    /// Quota checking disabled
    Disabled,
}

impl QuotaCheckResult {
    pub fn is_ok(&self) -> bool {
        matches!(self, QuotaCheckResult::Ok | QuotaCheckResult::Disabled)
    }

    pub fn to_governance_decision(&self, tenant_id: &TenantId) -> GovernanceDecision {
        match self {
            QuotaCheckResult::Ok | QuotaCheckResult::Disabled => {
                GovernanceDecision::allow("Quota check passed")
            }
            QuotaCheckResult::ConcurrentLimitExceeded { current, limit } => {
                GovernanceDecision::deny(format!(
                    "Tenant {} concurrent execution limit exceeded: {}/{} executions",
                    tenant_id, current, limit
                ))
            }
            QuotaCheckResult::RateLimitExceeded { current, limit } => {
                GovernanceDecision::review_required(
                    format!(
                        "Tenant {} rate limit exceeded: {}/{} requests/min",
                        tenant_id, current, limit
                    ),
                    ReviewType::Approval,
                )
            }
            QuotaCheckResult::StorageQuotaExceeded { current, limit } => {
                GovernanceDecision::deny(format!(
                    "Tenant {} storage quota exceeded: {}/{} bytes",
                    tenant_id, current, limit
                ))
            }
            QuotaCheckResult::ApiQuotaExceeded { current, limit } => {
                GovernanceDecision::review_required(
                    format!(
                        "Tenant {} API quota exceeded: {}/{} calls/day",
                        tenant_id, current, limit
                    ),
                    ReviewType::Approval,
                )
            }
            QuotaCheckResult::QuotaNotFound => GovernanceDecision::allow(format!(
                "No quota defined for tenant {}, allowing access",
                tenant_id
            )),
        }
    }
}

/// Tenant quota manager
///
/// 管理租户配额和使用情况跟踪
pub struct TenantQuotaManager {
    /// Tenant quotas
    quotas: RwLock<HashMap<TenantId, TenantQuota>>,

    /// Tenant usage tracking
    usage: RwLock<HashMap<TenantId, TenantUsage>>,

    /// Default quota for new tenants
    default_quota: TenantQuota,
}

impl TenantQuotaManager {
    /// Create a new tenant quota manager
    pub fn new(default_quota: TenantQuota) -> Self {
        Self {
            quotas: RwLock::new(HashMap::new()),
            usage: RwLock::new(HashMap::new()),
            default_quota,
        }
    }

    /// Create with default quotas
    pub fn with_defaults() -> Self {
        // Use a placeholder tenant ID for the default quota template
        let default_tenant = TenantId::from_string("default".to_string()).unwrap();
        Self::new(TenantQuota::default_quota(default_tenant))
    }

    /// Create with unlimited quotas (permissive mode)
    pub fn unlimited() -> Self {
        let default_tenant = TenantId::from_string("default".to_string()).unwrap();
        Self::new(TenantQuota::unlimited(default_tenant))
    }

    /// Set quota for a tenant
    pub fn set_quota(&self, quota: TenantQuota) {
        let tenant_id = quota.tenant_id.clone();
        self.quotas.write().unwrap().insert(tenant_id, quota);
    }

    /// Get quota for a tenant
    pub fn get_quota(&self, tenant_id: &TenantId) -> Option<TenantQuota> {
        self.quotas.read().unwrap().get(tenant_id).cloned()
    }

    /// Remove quota for a tenant
    pub fn remove_quota(&self, tenant_id: &TenantId) {
        self.quotas.write().unwrap().remove(tenant_id);
        self.usage.write().unwrap().remove(tenant_id);
    }

    /// Check if execution can proceed under quota limits
    pub fn check_execution_quota(&self, tenant_id: &TenantId) -> QuotaCheckResult {
        let quota = match self.get_quota(tenant_id) {
            Some(q) if q.enabled => q,
            Some(_) => return QuotaCheckResult::Disabled,
            None => {
                // Apply default quota
                let mut default = self.default_quota.clone();
                default.tenant_id = tenant_id.clone();
                default
            }
        };

        if !quota.has_limits() {
            return QuotaCheckResult::Ok;
        }

        // Get or create usage tracker
        let mut usage_map = self.usage.write().unwrap();
        let usage = usage_map.entry(tenant_id.clone()).or_default();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Reset time-based counters
        usage.reset_minute_if_needed(now);
        usage.reset_day_if_needed(now);

        // Check concurrent execution limit
        if let Some(limit) = quota.max_concurrent_executions {
            if usage.concurrent_executions >= limit {
                return QuotaCheckResult::ConcurrentLimitExceeded {
                    current: usage.concurrent_executions,
                    limit,
                };
            }
        }

        // Check rate limit
        if let Some(limit) = quota.requests_per_minute {
            if usage.requests_this_minute >= limit {
                return QuotaCheckResult::RateLimitExceeded {
                    current: usage.requests_this_minute,
                    limit,
                };
            }
        }

        // Check storage quota
        if let Some(limit) = quota.max_storage_bytes {
            if usage.storage_bytes_used >= limit {
                return QuotaCheckResult::StorageQuotaExceeded {
                    current: usage.storage_bytes_used,
                    limit,
                };
            }
        }

        // Check API call quota
        if let Some(limit) = quota.api_calls_per_day {
            if usage.api_calls_today >= limit {
                return QuotaCheckResult::ApiQuotaExceeded {
                    current: usage.api_calls_today,
                    limit,
                };
            }
        }

        QuotaCheckResult::Ok
    }

    /// Increment concurrent execution count
    pub fn start_execution(&self, tenant_id: &TenantId) {
        let mut usage_map = self.usage.write().unwrap();
        let usage = usage_map.entry(tenant_id.clone()).or_default();

        usage.concurrent_executions += 1;
        usage.requests_this_minute += 1;
        usage.api_calls_today += 1;

        debug!(
            "Started execution for tenant {}: {} concurrent",
            tenant_id, usage.concurrent_executions
        );
    }

    /// Decrement concurrent execution count
    pub fn end_execution(&self, tenant_id: &TenantId) {
        let mut usage_map = self.usage.write().unwrap();
        if let Some(usage) = usage_map.get_mut(tenant_id) {
            if usage.concurrent_executions > 0 {
                usage.concurrent_executions -= 1;
                debug!(
                    "Ended execution for tenant {}: {} concurrent",
                    tenant_id, usage.concurrent_executions
                );
            }
        }
    }

    /// Update storage usage
    pub fn update_storage_usage(&self, tenant_id: &TenantId, bytes: u64) {
        let mut usage_map = self.usage.write().unwrap();
        let usage = usage_map.entry(tenant_id.clone()).or_default();

        usage.storage_bytes_used = bytes;
    }

    /// Get current usage for a tenant
    pub fn get_usage(&self, tenant_id: &TenantId) -> Option<TenantUsageSnapshot> {
        self.usage
            .read()
            .unwrap()
            .get(tenant_id)
            .map(|usage| TenantUsageSnapshot {
                tenant_id: tenant_id.clone(),
                concurrent_executions: usage.concurrent_executions,
                requests_this_minute: usage.requests_this_minute,
                storage_bytes_used: usage.storage_bytes_used,
                api_calls_today: usage.api_calls_today,
            })
    }

    /// Get all quotas
    pub fn list_quotas(&self) -> Vec<TenantQuota> {
        self.quotas.read().unwrap().values().cloned().collect()
    }
}

/// Snapshot of tenant usage for reporting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantUsageSnapshot {
    pub tenant_id: TenantId,
    pub concurrent_executions: u32,
    pub requests_this_minute: u32,
    pub storage_bytes_used: u64,
    pub api_calls_today: u32,
}

/// Quota policy engine wrapper
///
/// 将配额检查集成到 PolicyEngine trait 中
pub struct QuotaPolicyEngine {
    quota_manager: Arc<TenantQuotaManager>,
}

impl QuotaPolicyEngine {
    pub fn new(quota_manager: Arc<TenantQuotaManager>) -> Self {
        Self { quota_manager }
    }
}

#[async_trait]
impl PolicyEngine for QuotaPolicyEngine {
    async fn evaluate_capability(
        &self,
        context: EvaluationContext,
    ) -> anyhow::Result<EvaluationResult> {
        let tenant_id = match &context.actor.tenant_id {
            Some(tid) => tid,
            None => {
                if matches!(
                    context.actor.actor_type,
                    cyberclaw_core::identity::ActorType::System
                ) {
                    return Ok(EvaluationResult {
                        decision: GovernanceDecision::allow(
                            "No tenant context, quota check skipped for system actor",
                        ),
                        evaluated_risk: context.capability.risk,
                        context_info: vec!["Quota check skipped for system actor".to_string()],
                    });
                }

                return Ok(EvaluationResult {
                    decision: GovernanceDecision::deny(
                        "Missing tenant context for non-system actor",
                    ),
                    evaluated_risk: context.capability.risk,
                    context_info: vec!["Missing tenant context for non-system actor".to_string()],
                });
            }
        };

        // Check quota
        let check_result = self.quota_manager.check_execution_quota(tenant_id);

        let decision = check_result.to_governance_decision(tenant_id);

        let context_info = vec![
            format!("Tenant quota checked for: {}", tenant_id),
            format!("Quota check result: {:?}", check_result),
        ];

        if !check_result.is_ok() {
            warn!(
                "Quota check failed for tenant {}: {:?}",
                tenant_id, check_result
            );
        }

        Ok(EvaluationResult {
            decision,
            evaluated_risk: context.capability.risk,
            context_info,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::prelude::*;

    fn create_test_tenant() -> TenantId {
        TenantId::from_string("test-tenant".to_string()).unwrap()
    }

    fn create_test_capability() -> CapabilityRef {
        CapabilityRef {
            id: CapabilityId::from_string("test.capability".to_string()).unwrap(),
            connector_id: ConnectorId::from_string("test-connector".to_string()).unwrap(),
            risk: RiskLevel::Medium,
            effects: vec![CapabilityEffect::Execute],
            placement: None,
        }
    }

    fn create_test_actor(tenant_id: TenantId) -> ActorRef {
        ActorRef {
            id: ActorId::from_string("test-actor".to_string()).unwrap(),
            actor_type: ActorType::Agent,
            tenant_id: Some(tenant_id),
            home_node_id: None,
            display_name: "Test Actor".to_string(),
        }
    }

    fn create_test_actor_without_tenant(actor_type: ActorType) -> ActorRef {
        ActorRef {
            id: ActorId::from_string("test-actor-no-tenant".to_string()).unwrap(),
            actor_type,
            tenant_id: None,
            home_node_id: None,
            display_name: "Test Actor Without Tenant".to_string(),
        }
    }

    #[test]
    fn test_tenant_quota_creation() {
        let tenant = create_test_tenant();
        let quota = TenantQuota::default_quota(tenant.clone());

        assert_eq!(quota.tenant_id, tenant);
        assert_eq!(quota.max_concurrent_executions, Some(10));
        assert_eq!(quota.requests_per_minute, Some(100));
        assert!(quota.enabled);
    }

    #[test]
    fn test_unlimited_quota() {
        let tenant = create_test_tenant();
        let quota = TenantQuota::unlimited(tenant.clone());

        assert_eq!(quota.tenant_id, tenant);
        assert!(quota.max_concurrent_executions.is_none());
        assert!(quota.requests_per_minute.is_none());
        assert!(!quota.has_limits());
    }

    #[test]
    fn test_quota_manager_set_get() {
        let manager = TenantQuotaManager::with_defaults();
        let tenant = create_test_tenant();
        let quota = TenantQuota::default_quota(tenant.clone());

        manager.set_quota(quota.clone());

        let retrieved = manager.get_quota(&tenant).unwrap();
        assert_eq!(retrieved.tenant_id, tenant);
        assert_eq!(retrieved.max_concurrent_executions, Some(10));
    }

    #[test]
    fn test_concurrent_execution_limit() {
        let manager = TenantQuotaManager::with_defaults();
        let tenant = create_test_tenant();

        // Set low limit for testing
        let mut quota = TenantQuota::default_quota(tenant.clone());
        quota.max_concurrent_executions = Some(2);
        manager.set_quota(quota);

        // First execution should succeed
        let result1 = manager.check_execution_quota(&tenant);
        assert!(result1.is_ok());
        manager.start_execution(&tenant);

        // Second execution should succeed
        let result2 = manager.check_execution_quota(&tenant);
        assert!(result2.is_ok());
        manager.start_execution(&tenant);

        // Third execution should fail
        let result3 = manager.check_execution_quota(&tenant);
        assert!(matches!(
            result3,
            QuotaCheckResult::ConcurrentLimitExceeded { .. }
        ));

        // End one execution
        manager.end_execution(&tenant);

        // Now should succeed again
        let result4 = manager.check_execution_quota(&tenant);
        assert!(result4.is_ok());
    }

    #[test]
    fn test_storage_quota() {
        let manager = TenantQuotaManager::with_defaults();
        let tenant = create_test_tenant();

        let mut quota = TenantQuota::default_quota(tenant.clone());
        quota.max_storage_bytes = Some(1000); // 1KB limit
        manager.set_quota(quota);

        // Under limit should succeed
        manager.update_storage_usage(&tenant, 500);
        let result1 = manager.check_execution_quota(&tenant);
        assert!(result1.is_ok());

        // Over limit should fail
        manager.update_storage_usage(&tenant, 1500);
        let result2 = manager.check_execution_quota(&tenant);
        assert!(matches!(
            result2,
            QuotaCheckResult::StorageQuotaExceeded { .. }
        ));
    }

    #[test]
    fn test_quota_not_found_allows_access() {
        let manager = TenantQuotaManager::unlimited();
        let tenant = create_test_tenant();

        // No quota set, should allow
        let result = manager.check_execution_quota(&tenant);
        assert!(result.is_ok());
    }

    #[test]
    fn test_quota_check_result_to_decision() {
        let tenant = create_test_tenant();

        let ok_result = QuotaCheckResult::Ok;
        let decision = ok_result.to_governance_decision(&tenant);
        assert!(decision.is_allow());

        let limit_exceeded = QuotaCheckResult::ConcurrentLimitExceeded {
            current: 10,
            limit: 10,
        };
        let decision = limit_exceeded.to_governance_decision(&tenant);
        assert!(decision.is_deny());

        let rate_exceeded = QuotaCheckResult::RateLimitExceeded {
            current: 100,
            limit: 100,
        };
        let decision = rate_exceeded.to_governance_decision(&tenant);
        assert!(decision.is_review_required());
    }

    #[tokio::test]
    async fn test_quota_policy_engine() {
        let manager = Arc::new(TenantQuotaManager::with_defaults());
        let engine = QuotaPolicyEngine::new(manager.clone());

        let tenant = create_test_tenant();
        let mut quota = TenantQuota::default_quota(tenant.clone());
        quota.max_concurrent_executions = Some(1);
        manager.set_quota(quota);

        let capability = create_test_capability();
        let actor = create_test_actor(tenant.clone());

        let context = EvaluationContext {
            capability,
            actor,
            execution_id: ExecutionId::new(),
            reason: Some("Test".to_string()),
        };

        // First evaluation should allow
        let result1 = engine.evaluate_capability(context.clone()).await.unwrap();
        assert!(result1.decision.is_allow());
        manager.start_execution(&tenant);

        // Second evaluation should deny (concurrent limit)
        let result2 = engine.evaluate_capability(context).await.unwrap();
        assert!(result2.decision.is_deny());
    }

    #[tokio::test]
    async fn test_quota_policy_engine_denies_non_system_actor_without_tenant_context() {
        let manager = Arc::new(TenantQuotaManager::with_defaults());
        let engine = QuotaPolicyEngine::new(manager);

        let context = EvaluationContext {
            capability: create_test_capability(),
            actor: create_test_actor_without_tenant(ActorType::Connector),
            execution_id: ExecutionId::new(),
            reason: Some("Webhook capability execution".to_string()),
        };

        let result = engine.evaluate_capability(context).await.unwrap();

        assert!(result.decision.is_deny());
        assert!(result
            .context_info
            .iter()
            .any(|info| info.contains("Missing tenant context")));
    }

    #[tokio::test]
    async fn test_quota_policy_engine_allows_system_actor_without_tenant_context() {
        let manager = Arc::new(TenantQuotaManager::with_defaults());
        let engine = QuotaPolicyEngine::new(manager);

        let context = EvaluationContext {
            capability: create_test_capability(),
            actor: create_test_actor_without_tenant(ActorType::System),
            execution_id: ExecutionId::new(),
            reason: Some("System capability execution".to_string()),
        };

        let result = engine.evaluate_capability(context).await.unwrap();

        assert!(result.decision.is_allow());
        assert!(result
            .context_info
            .iter()
            .any(|info| info.contains("system actor")));
    }

    #[test]
    fn test_get_usage_snapshot() {
        let manager = TenantQuotaManager::with_defaults();
        let tenant = create_test_tenant();

        manager.start_execution(&tenant);
        manager.start_execution(&tenant);
        manager.update_storage_usage(&tenant, 500);

        let snapshot = manager.get_usage(&tenant).unwrap();
        assert_eq!(snapshot.concurrent_executions, 2);
        assert_eq!(snapshot.storage_bytes_used, 500);
        assert_eq!(snapshot.requests_this_minute, 2);
        assert_eq!(snapshot.api_calls_today, 2);
    }

    #[test]
    fn test_list_quotas() {
        let manager = TenantQuotaManager::with_defaults();

        let tenant1 = TenantId::from_string("tenant-1".to_string()).unwrap();
        let tenant2 = TenantId::from_string("tenant-2".to_string()).unwrap();

        manager.set_quota(TenantQuota::default_quota(tenant1));
        manager.set_quota(TenantQuota::default_quota(tenant2));

        let quotas = manager.list_quotas();
        assert_eq!(quotas.len(), 2);
    }
}
