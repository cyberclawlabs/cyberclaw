/// 统一安全配置模块
///
/// 提供全局安全策略配置，支持风险级别驱动的安全控制
use crate::capability::RiskLevel;
use serde::{Deserialize, Serialize};

/// 最佳努力模式配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BestEffortMode {
    /// 总是使用最佳努力模式（失败不中断执行）
    Always,
    /// 从不使用最佳努力模式（失败立即中断）
    Never,
    /// 基于风险级别决定（低风险使用best-effort，高风险fail-fast）
    RiskBased {
        /// 失败阈值：大于等于此风险级别时fail-fast
        fail_threshold: RiskLevel,
    },
}

/// 溯源失败处理模式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProvenanceFailureMode {
    /// 忽略溯源失败（仅记录警告）
    Ignore,
    /// 溯源失败时中断执行
    FailFast,
    /// 基于风险级别决定
    RiskBased {
        /// 失败阈值：大于等于此风险级别时fail-fast
        fail_threshold: RiskLevel,
    },
}

/// 审计级别
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditLevel {
    /// 不记录审计日志
    None,
    /// 仅记录关键操作
    Critical,
    /// 记录高风险操作
    High,
    /// 记录中等风险操作
    Medium,
    /// 记录所有操作
    All,
}

impl AuditLevel {
    /// 判断是否应该记录指定风险级别的操作
    pub fn should_audit(&self, risk: RiskLevel) -> bool {
        match self {
            AuditLevel::None => false,
            AuditLevel::Critical => matches!(risk, RiskLevel::Critical),
            AuditLevel::High => matches!(risk, RiskLevel::Critical | RiskLevel::High),
            AuditLevel::Medium => matches!(
                risk,
                RiskLevel::Critical | RiskLevel::High | RiskLevel::Medium
            ),
            AuditLevel::All => true,
        }
    }
}

/// 全局安全配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// 最佳努力模式配置
    pub best_effort_mode: BestEffortMode,

    /// 是否要求溯源追踪
    pub require_provenance: bool,

    /// 溯源失败处理模式
    pub provenance_failure_mode: ProvenanceFailureMode,

    /// 审计级别
    pub audit_level: AuditLevel,

    /// 是否记录安全事件
    pub log_security_events: bool,

    /// 关键操作列表（始终fail-fast）
    pub critical_operations: Vec<String>,

    /// 高风险操作列表
    pub high_risk_operations: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            best_effort_mode: BestEffortMode::RiskBased {
                fail_threshold: RiskLevel::High,
            },
            require_provenance: true,
            provenance_failure_mode: ProvenanceFailureMode::RiskBased {
                fail_threshold: RiskLevel::High,
            },
            audit_level: AuditLevel::High,
            log_security_events: true,
            critical_operations: vec![
                "execution.start".to_string(),
                "execution.complete".to_string(),
                "capability.dispatch".to_string(),
                "runtime.select".to_string(),
            ],
            high_risk_operations: vec![
                "memory.compact".to_string(),
                "provenance.finalize".to_string(),
            ],
        }
    }
}

impl SecurityConfig {
    /// 创建严格模式配置（所有操作fail-fast）
    pub fn strict() -> Self {
        Self {
            best_effort_mode: BestEffortMode::Never,
            require_provenance: true,
            provenance_failure_mode: ProvenanceFailureMode::FailFast,
            audit_level: AuditLevel::All,
            log_security_events: true,
            critical_operations: vec![],
            high_risk_operations: vec![],
        }
    }

    /// 创建宽松模式配置（所有操作best-effort）
    pub fn relaxed() -> Self {
        Self {
            best_effort_mode: BestEffortMode::Always,
            require_provenance: false,
            provenance_failure_mode: ProvenanceFailureMode::Ignore,
            audit_level: AuditLevel::Critical,
            log_security_events: true,
            critical_operations: vec![],
            high_risk_operations: vec![],
        }
    }

    /// 判断指定风险级别是否应该使用best-effort模式
    pub fn should_best_effort(&self, risk_level: RiskLevel) -> bool {
        match &self.best_effort_mode {
            BestEffortMode::Always => true,
            BestEffortMode::Never => false,
            BestEffortMode::RiskBased { fail_threshold } => risk_level < *fail_threshold,
        }
    }

    /// 判断指定风险级别是否要求溯源
    pub fn requires_provenance(&self, risk_level: RiskLevel) -> bool {
        if !self.require_provenance {
            return false;
        }

        match &self.provenance_failure_mode {
            ProvenanceFailureMode::Ignore => false,
            ProvenanceFailureMode::FailFast => true,
            ProvenanceFailureMode::RiskBased { fail_threshold } => risk_level >= *fail_threshold,
        }
    }

    /// 判断操作是否为关键操作
    pub fn is_critical_operation(&self, operation: &str) -> bool {
        self.critical_operations.contains(&operation.to_string())
    }

    /// 判断操作是否为高风险操作
    pub fn is_high_risk_operation(&self, operation: &str) -> bool {
        self.high_risk_operations.contains(&operation.to_string())
    }

    /// 判断是否应该记录安全事件
    pub fn should_log_security_event(&self, risk_level: RiskLevel) -> bool {
        self.log_security_events && self.audit_level.should_audit(risk_level)
    }
}

/// 安全配置管理器
#[derive(Debug, Clone)]
pub struct SecurityConfigManager {
    config: SecurityConfig,
}

impl SecurityConfigManager {
    /// 创建新的安全配置管理器
    pub fn new(config: SecurityConfig) -> Self {
        Self { config }
    }

    /// 获取安全配置
    pub fn config(&self) -> &SecurityConfig {
        &self.config
    }

    /// 更新安全配置
    pub fn update_config(&mut self, config: SecurityConfig) {
        self.config = config;
    }
}

impl Default for SecurityConfigManager {
    fn default() -> Self {
        Self::new(SecurityConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SecurityConfig::default();
        assert!(config.require_provenance);
        assert!(config.log_security_events);
        assert!(matches!(config.audit_level, AuditLevel::High));
    }

    #[test]
    fn test_strict_config() {
        let config = SecurityConfig::strict();
        assert!(matches!(config.best_effort_mode, BestEffortMode::Never));
        assert!(matches!(
            config.provenance_failure_mode,
            ProvenanceFailureMode::FailFast
        ));
    }

    #[test]
    fn test_relaxed_config() {
        let config = SecurityConfig::relaxed();
        assert!(matches!(config.best_effort_mode, BestEffortMode::Always));
        assert!(!config.require_provenance);
    }

    #[test]
    fn test_should_best_effort() {
        let config = SecurityConfig::default();
        assert!(config.should_best_effort(RiskLevel::Low));
        assert!(config.should_best_effort(RiskLevel::Medium));
        assert!(!config.should_best_effort(RiskLevel::High));
        assert!(!config.should_best_effort(RiskLevel::Critical));
    }

    #[test]
    fn test_requires_provenance() {
        let config = SecurityConfig::default();
        assert!(!config.requires_provenance(RiskLevel::Low));
        assert!(!config.requires_provenance(RiskLevel::Medium));
        assert!(config.requires_provenance(RiskLevel::High));
        assert!(config.requires_provenance(RiskLevel::Critical));
    }

    #[test]
    fn test_audit_level() {
        let audit_level = AuditLevel::High;
        assert!(audit_level.should_audit(RiskLevel::Critical));
        assert!(audit_level.should_audit(RiskLevel::High));
        assert!(!audit_level.should_audit(RiskLevel::Medium));
        assert!(!audit_level.should_audit(RiskLevel::Low));
    }
}
