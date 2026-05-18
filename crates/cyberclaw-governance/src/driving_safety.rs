//! 驾驶安全 Platform Plugin。
//!
//! 横切安全策略，在驾驶模式下拦截高风险操作、要求语音确认、
//! 抑制不适合语音朗读的内容（代码块、diff 等）。
//!
//! 本模块不是 Connector，而是治理层的一部分，为语音交互场景
//! 提供安全保障。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 驾驶安全配置。
///
/// 控制驾驶模式下的安全策略参数，包括播报时长限制、
/// 风险确认要求、内容抑制规则等。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrivingSafetyConfig {
    /// 是否启用驾驶安全模式
    pub enabled: bool,
    /// 最大语音播报时长（秒）
    pub max_broadcast_secs: u8,
    /// 需要语音确认的风险级别（如 "High", "Critical"）
    pub require_voice_confirm: Vec<String>,
    /// 确认短语（用户口头确认时匹配）
    pub confirm_phrases: Vec<String>,
    /// 拒绝短语（用户口头拒绝时匹配）
    pub reject_phrases: Vec<String>,
    /// 抑制代码朗读
    pub suppress_code_readout: bool,
    /// 抑制 diff 朗读
    pub suppress_diff_readout: bool,
    /// Medium 风险自动暂停
    pub auto_pause_medium_risk: bool,
}

/// 确认状态机。
///
/// 表示一个需要用户语音确认的操作在其生命周期内的状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConfirmationState {
    /// 空闲，无待确认操作
    Idle,
    /// 等待用户确认
    Pending {
        /// 操作描述
        action_description: String,
        /// 风险级别
        risk: String,
        /// 超时时长（秒）
        timeout_secs: u16,
        /// 发起确认的时间
        asked_at: DateTime<Utc>,
    },
    /// 用户已确认
    Confirmed,
    /// 用户已拒绝
    Rejected,
    /// 确认超时（等同于 Rejected）
    TimedOut,
}

/// 驾驶安全 Plugin。
///
/// 横切安全策略，在驾驶模式下：
/// - 拦截高风险 / 关键操作，要求语音确认
/// - Medium 风险可配置自动暂停
/// - 抑制不适合语音朗读的内容（代码、diff 等）
///
/// # Examples
///
/// ```
/// use cyberclaw_governance::driving_safety::DrivingSafetyPlugin;
///
/// let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
/// assert!(plugin.should_gate("High"));
/// assert!(!plugin.should_gate("Low"));
/// ```
#[derive(Debug, Clone)]
pub struct DrivingSafetyPlugin {
    config: DrivingSafetyConfig,
}

impl DrivingSafetyPlugin {
    /// 使用指定配置创建 Plugin 实例。
    pub fn new(config: DrivingSafetyConfig) -> Self {
        Self { config }
    }

    /// 返回默认驾驶安全配置。
    pub fn default_config() -> DrivingSafetyConfig {
        DrivingSafetyConfig {
            enabled: true,
            max_broadcast_secs: 8,
            require_voice_confirm: vec!["High".to_string(), "Critical".to_string()],
            confirm_phrases: vec!["继续".to_string(), "确认".to_string(), "同意".to_string()],
            reject_phrases: vec!["拒绝".to_string(), "不要".to_string(), "停止".to_string()],
            suppress_code_readout: true,
            suppress_diff_readout: true,
            auto_pause_medium_risk: true,
        }
    }

    /// 获取当前配置的引用。
    pub fn config(&self) -> &DrivingSafetyConfig {
        &self.config
    }

    /// 根据风险级别判断是否应拦截操作。
    ///
    /// 当风险级别在 `require_voice_confirm` 列表中时返回 `true`。
    /// 当 `auto_pause_medium_risk` 启用且风险级别为 "Medium" 时也返回 `true`。
    /// 未启用驾驶安全模式时始终返回 `false`。
    pub fn should_gate(&self, risk_level: &str) -> bool {
        if !self.config.enabled {
            return false;
        }

        // 检查是否在需要确认的风险级别列表中（忽略大小写）
        let risk_lower = risk_level.to_lowercase();
        if self
            .config
            .require_voice_confirm
            .iter()
            .any(|r| r.to_lowercase() == risk_lower)
        {
            return true;
        }

        // Medium 风险自动暂停
        if self.config.auto_pause_medium_risk && risk_lower == "medium" {
            return true;
        }

        false
    }

    /// 为指定操作创建待确认状态。
    ///
    /// 返回 `ConfirmationState::Pending`，超时时长使用
    /// `max_broadcast_secs` 的 2 倍作为默认等待窗口。
    pub fn create_confirmation(&self, action: &str, risk: &str) -> ConfirmationState {
        ConfirmationState::Pending {
            action_description: action.to_string(),
            risk: risk.to_string(),
            timeout_secs: u16::from(self.config.max_broadcast_secs) * 2,
            asked_at: Utc::now(),
        }
    }

    /// 处理用户对待确认操作的口头回复。
    ///
    /// 仅在 `Pending` 状态下生效：
    /// - 匹配确认短语 → `Confirmed`
    /// - 匹配拒绝短语 → `Rejected`
    /// - 均不匹配 → 保持 `Pending`（等待下次输入或超时）
    pub fn process_response(&self, state: &ConfirmationState, response: &str) -> ConfirmationState {
        match state {
            ConfirmationState::Pending { .. } => {
                let resp_lower = response.to_lowercase();

                // 检查确认短语
                if self
                    .config
                    .confirm_phrases
                    .iter()
                    .any(|p| resp_lower.contains(&p.to_lowercase()))
                {
                    return ConfirmationState::Confirmed;
                }

                // 检查拒绝短语
                if self
                    .config
                    .reject_phrases
                    .iter()
                    .any(|p| resp_lower.contains(&p.to_lowercase()))
                {
                    return ConfirmationState::Rejected;
                }

                // 均不匹配，保持原状态
                state.clone()
            }
            // 非 Pending 状态不处理
            _ => state.clone(),
        }
    }

    /// 检查待确认操作是否已超时。
    ///
    /// 仅在 `Pending` 状态下检查：超过 `timeout_secs` 后转为 `TimedOut`。
    /// 其他状态原样返回。
    pub fn check_timeout(&self, state: &ConfirmationState) -> ConfirmationState {
        match state {
            ConfirmationState::Pending {
                timeout_secs,
                asked_at,
                ..
            } => {
                let elapsed = Utc::now().signed_duration_since(*asked_at);
                if elapsed.num_seconds() >= i64::from(*timeout_secs) {
                    ConfirmationState::TimedOut
                } else {
                    state.clone()
                }
            }
            _ => state.clone(),
        }
    }

    /// 检查指定内容类型是否应被抑制（不进行语音朗读）。
    ///
    /// 当前支持的抑制类型：
    /// - `"code"` — 代码块（受 `suppress_code_readout` 控制）
    /// - `"diff"` — diff 内容（受 `suppress_diff_readout` 控制）
    ///
    /// 其他内容类型不抑制。未启用时始终返回 `false`。
    pub fn is_content_suppressed(&self, content_type: &str) -> bool {
        if !self.config.enabled {
            return false;
        }

        match content_type.to_lowercase().as_str() {
            "code" => self.config.suppress_code_readout,
            "diff" => self.config.suppress_diff_readout,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = DrivingSafetyPlugin::default_config();
        assert!(config.enabled);
        assert_eq!(config.max_broadcast_secs, 8);
        assert!(config.suppress_code_readout);
        assert!(config.suppress_diff_readout);
        assert!(config.auto_pause_medium_risk);
        assert_eq!(config.require_voice_confirm.len(), 2);
        assert_eq!(config.confirm_phrases.len(), 3);
        assert_eq!(config.reject_phrases.len(), 3);
    }

    #[test]
    fn should_gate_high_risk() {
        let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
        assert!(plugin.should_gate("High"));
        assert!(plugin.should_gate("Critical"));
    }

    #[test]
    fn should_gate_low_passes() {
        let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
        assert!(!plugin.should_gate("Low"));
    }

    #[test]
    fn should_gate_medium_with_auto_pause() {
        let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
        assert!(plugin.should_gate("Medium"));
    }

    #[test]
    fn should_gate_disabled() {
        let mut config = DrivingSafetyPlugin::default_config();
        config.enabled = false;
        let plugin = DrivingSafetyPlugin::new(config);
        assert!(!plugin.should_gate("High"));
        assert!(!plugin.should_gate("Critical"));
    }

    #[test]
    fn confirmation_pending_to_confirmed() {
        let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
        let state = plugin.create_confirmation("删除文件", "High");
        let result = plugin.process_response(&state, "继续");
        assert!(matches!(result, ConfirmationState::Confirmed));
    }

    #[test]
    fn confirmation_pending_to_rejected() {
        let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
        let state = plugin.create_confirmation("删除文件", "High");
        let result = plugin.process_response(&state, "不要");
        assert!(matches!(result, ConfirmationState::Rejected));
    }

    #[test]
    fn code_content_suppressed() {
        let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
        assert!(plugin.is_content_suppressed("code"));
    }

    #[test]
    fn diff_content_suppressed() {
        let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
        assert!(plugin.is_content_suppressed("diff"));
    }

    #[test]
    fn normal_text_not_suppressed() {
        let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
        assert!(!plugin.is_content_suppressed("text"));
        assert!(!plugin.is_content_suppressed("summary"));
    }
}
