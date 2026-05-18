//! 驾驶安全 Plugin 集成测试。

use cyberclaw_governance::driving_safety::{
    ConfirmationState, DrivingSafetyConfig, DrivingSafetyPlugin,
};

// =========================================================================
// Config 测试 (3)
// =========================================================================

#[test]
fn default_config_has_expected_values() {
    let config = DrivingSafetyPlugin::default_config();
    assert!(config.enabled);
    assert_eq!(config.max_broadcast_secs, 8);
    assert!(config.suppress_code_readout);
    assert!(config.suppress_diff_readout);
    assert!(config.auto_pause_medium_risk);
    assert_eq!(config.require_voice_confirm, vec!["High", "Critical"]);
    assert_eq!(config.confirm_phrases, vec!["继续", "确认", "同意"]);
    assert_eq!(config.reject_phrases, vec!["拒绝", "不要", "停止"]);
}

#[test]
fn custom_config_overrides() {
    let config = DrivingSafetyConfig {
        enabled: false,
        max_broadcast_secs: 15,
        require_voice_confirm: vec!["Critical".to_string()],
        confirm_phrases: vec!["yes".to_string()],
        reject_phrases: vec!["no".to_string()],
        suppress_code_readout: false,
        suppress_diff_readout: false,
        auto_pause_medium_risk: false,
    };
    let plugin = DrivingSafetyPlugin::new(config);
    let c = plugin.config();
    assert!(!c.enabled);
    assert_eq!(c.max_broadcast_secs, 15);
    assert_eq!(c.require_voice_confirm.len(), 1);
    assert!(!c.suppress_code_readout);
}

#[test]
fn config_edge_case_empty_phrases() {
    let config = DrivingSafetyConfig {
        enabled: true,
        max_broadcast_secs: 0,
        require_voice_confirm: vec![],
        confirm_phrases: vec![],
        reject_phrases: vec![],
        suppress_code_readout: false,
        suppress_diff_readout: false,
        auto_pause_medium_risk: false,
    };
    let plugin = DrivingSafetyPlugin::new(config);
    // 空的 require_voice_confirm 且 auto_pause 关闭 → 不拦截任何级别
    assert!(!plugin.should_gate("High"));
    assert!(!plugin.should_gate("Critical"));
    assert!(!plugin.should_gate("Medium"));

    // 空的 confirm/reject phrases → process_response 保持 Pending
    let state = plugin.create_confirmation("test", "High");
    let result = plugin.process_response(&state, "继续");
    assert!(matches!(result, ConfirmationState::Pending { .. }));
}

// =========================================================================
// should_gate 测试 (3)
// =========================================================================

#[test]
fn should_gate_high_risk() {
    let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
    assert!(plugin.should_gate("High"), "High 风险应被拦截");
    assert!(plugin.should_gate("Critical"), "Critical 风险应被拦截");
}

#[test]
fn should_gate_medium_with_auto_pause() {
    let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
    assert!(
        plugin.should_gate("Medium"),
        "auto_pause_medium_risk=true 时 Medium 应被拦截"
    );

    // 关闭 auto_pause 后 Medium 不再拦截
    let mut config = DrivingSafetyPlugin::default_config();
    config.auto_pause_medium_risk = false;
    let plugin2 = DrivingSafetyPlugin::new(config);
    assert!(
        !plugin2.should_gate("Medium"),
        "auto_pause_medium_risk=false 时 Medium 不应被拦截"
    );
}

#[test]
fn should_gate_low_passes() {
    let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
    assert!(!plugin.should_gate("Low"), "Low 风险不应被拦截");
    assert!(!plugin.should_gate("Unknown"), "未知风险级别不应被拦截");
}

// =========================================================================
// 确认流程测试 (3)
// =========================================================================

#[test]
fn confirmation_pending_to_confirmed() {
    let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
    let state = plugin.create_confirmation("删除 3 个文件", "High");

    // 验证 Pending 状态
    match &state {
        ConfirmationState::Pending {
            action_description,
            risk,
            timeout_secs,
            ..
        } => {
            assert_eq!(action_description, "删除 3 个文件");
            assert_eq!(risk, "High");
            assert_eq!(*timeout_secs, 16); // max_broadcast_secs(8) * 2
        }
        other => panic!("expected Pending, got {:?}", other),
    }

    // 确认 → Confirmed
    let result = plugin.process_response(&state, "继续执行吧");
    assert!(matches!(result, ConfirmationState::Confirmed));

    // 其他确认短语也生效
    let result2 = plugin.process_response(&state, "我同意");
    assert!(matches!(result2, ConfirmationState::Confirmed));
}

#[test]
fn confirmation_pending_to_rejected() {
    let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
    let state = plugin.create_confirmation("部署到生产", "Critical");

    let result = plugin.process_response(&state, "不要执行");
    assert!(matches!(result, ConfirmationState::Rejected));

    let result2 = plugin.process_response(&state, "停止");
    assert!(matches!(result2, ConfirmationState::Rejected));
}

#[test]
fn confirmation_pending_to_timed_out() {
    let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());

    // 构造一个已经过期的 Pending 状态
    let expired_state = ConfirmationState::Pending {
        action_description: "测试操作".to_string(),
        risk: "High".to_string(),
        timeout_secs: 0, // 立即超时
        asked_at: chrono::Utc::now() - chrono::Duration::seconds(10),
    };

    let result = plugin.check_timeout(&expired_state);
    assert!(
        matches!(result, ConfirmationState::TimedOut),
        "过期的 Pending 应转为 TimedOut"
    );

    // 未过期的 Pending 保持原样
    let fresh_state = plugin.create_confirmation("新操作", "High");
    let result2 = plugin.check_timeout(&fresh_state);
    assert!(
        matches!(result2, ConfirmationState::Pending { .. }),
        "未过期的 Pending 应保持不变"
    );

    // 非 Pending 状态不受影响
    let idle = ConfirmationState::Idle;
    let result3 = plugin.check_timeout(&idle);
    assert!(matches!(result3, ConfirmationState::Idle));
}

// =========================================================================
// 内容抑制测试 (3)
// =========================================================================

#[test]
fn code_content_suppressed() {
    let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
    assert!(plugin.is_content_suppressed("code"), "代码内容应被抑制");
    // 大小写不敏感
    assert!(plugin.is_content_suppressed("Code"));
    assert!(plugin.is_content_suppressed("CODE"));
}

#[test]
fn diff_content_suppressed() {
    let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
    assert!(plugin.is_content_suppressed("diff"), "diff 内容应被抑制");
    assert!(plugin.is_content_suppressed("Diff"));
}

#[test]
fn normal_text_not_suppressed() {
    let plugin = DrivingSafetyPlugin::new(DrivingSafetyPlugin::default_config());
    assert!(!plugin.is_content_suppressed("text"), "普通文本不应被抑制");
    assert!(!plugin.is_content_suppressed("summary"));
    assert!(!plugin.is_content_suppressed("error"));
    assert!(!plugin.is_content_suppressed(""));
}
