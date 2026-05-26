//! Unit tests for the typed wire protocol.

use super::*;
use serde_json::json;

// ---------------------------------------------------------------------------
// Round-trip tests (F.1 — 15 cases)
// ---------------------------------------------------------------------------

fn round_trip(frame: Frame) -> Frame {
    let encoded = frame.to_sse_data().expect("encode should succeed");
    Frame::from_sse_data(&encoded).expect("decode should succeed")
}

#[test]
fn round_trip_token() {
    let f = Frame::Token { content: "Hello, 世界".to_string() };
    assert_eq!(round_trip(f.clone()), f);
}

#[test]
fn round_trip_tool_start() {
    let f = Frame::ToolStart {
        tool: "fs.read".to_string(),
        args: json!({"path": "/tmp/foo.txt"}),
    };
    assert_eq!(round_trip(f.clone()), f);
}

#[test]
fn round_trip_tool_progress() {
    let f = Frame::ToolProgress {
        tool: "long_runner".to_string(),
        percent: Some(42),
        message: Some("halfway there".to_string()),
    };
    assert_eq!(round_trip(f.clone()), f);
}

#[test]
fn round_trip_tool_complete_ok() {
    let f = Frame::ToolComplete {
        tool: "fs.read".to_string(),
        ok: true,
        preview: "file contents...".to_string(),
        duration_ms: 42,
    };
    assert_eq!(round_trip(f.clone()), f);
}

#[test]
fn round_trip_tool_complete_failure() {
    let f = Frame::ToolComplete {
        tool: "fs.read".to_string(),
        ok: false,
        preview: "permission denied".to_string(),
        duration_ms: 7,
    };
    assert_eq!(round_trip(f.clone()), f);
}

#[test]
fn round_trip_approval_pending() {
    let f = Frame::ApprovalPending {
        tool: "shell.run".to_string(),
        reason: Some("risky capability".to_string()),
    };
    assert_eq!(round_trip(f.clone()), f);
}

#[test]
fn round_trip_approval_granted() {
    let f = Frame::ApprovalGranted { tool: "shell.run".to_string() };
    assert_eq!(round_trip(f.clone()), f);
}

#[test]
fn round_trip_approval_denied() {
    let f = Frame::ApprovalDenied {
        tool: "shell.run".to_string(),
        reason: Some("policy denied".to_string()),
    };
    assert_eq!(round_trip(f.clone()), f);
}

#[test]
fn round_trip_error_all_kinds() {
    let kinds = [
        ErrorKind::Billing,
        ErrorKind::RateLimit,
        ErrorKind::ContextOverflow,
        ErrorKind::ImageTooLarge,
        ErrorKind::ModelNotFound,
        ErrorKind::AuthInvalid,
        ErrorKind::AuthExpired,
        ErrorKind::PermissionDenied,
        ErrorKind::QuotaExceeded,
        ErrorKind::ServiceUnavailable,
        ErrorKind::Timeout,
        ErrorKind::InternalError,
        ErrorKind::BadRequest,
        ErrorKind::ContentFilter,
        ErrorKind::ThinkingSignature,
        ErrorKind::InvalidRequest,
        ErrorKind::GovernanceDenied,
        ErrorKind::Unknown,
    ];
    for kind in kinds {
        let f = Frame::Error {
            message: format!("error of kind {kind:?}"),
            kind: kind.clone(),
        };
        let rt = round_trip(f.clone());
        assert_eq!(rt, f, "{kind:?} did not round-trip");
    }
}

#[test]
fn round_trip_usage() {
    let f = Frame::Usage {
        model: "claude-3-5-sonnet".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        cache_read_tokens: 10,
        cache_write_tokens: 5,
    };
    assert_eq!(round_trip(f.clone()), f);
}

#[test]
fn round_trip_rate_limit() {
    let f = Frame::RateLimit {
        provider: "anthropic".to_string(),
        requests_limit: Some(1000),
        requests_remaining: Some(999),
        tokens_limit: Some(40_000),
        tokens_remaining: Some(39_000),
        requests_reset_secs: Some(60.0),
        tokens_reset_secs: Some(30.5),
    };
    assert_eq!(round_trip(f.clone()), f);
}

#[test]
fn round_trip_heartbeat() {
    let f = Frame::Heartbeat { elapsed_secs: 15 };
    assert_eq!(round_trip(f.clone()), f);
}

#[test]
fn done_sentinel_format() {
    let encoded = Frame::Done.to_sse_data().unwrap();
    assert_eq!(encoded, "[DONE]");
}

#[test]
fn done_parse_from_sentinel() {
    assert_eq!(Frame::from_sse_data("[DONE]").unwrap(), Frame::Done);
    // Whitespace tolerated.
    assert_eq!(Frame::from_sse_data("  [DONE]  ").unwrap(), Frame::Done);
}

#[test]
fn version_envelope_present() {
    let encoded = Frame::Token { content: "x".to_string() }
        .to_sse_data()
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(v.get("v").and_then(|n| n.as_u64()), Some(1));
    assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("token"));
    assert!(v.get("data").is_some());
}

#[test]
fn frame_size_limit_enforced() {
    // Build a Token frame whose serialised envelope exceeds MAX_FRAME_BYTES.
    let huge = "X".repeat(MAX_FRAME_BYTES + 100);
    let f = Frame::Token { content: huge };
    match f.to_sse_data() {
        Err(EncodeError::FrameTooLarge { actual, limit }) => {
            assert_eq!(limit, MAX_FRAME_BYTES);
            assert!(actual > MAX_FRAME_BYTES);
        }
        other => panic!("expected FrameTooLarge, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Compat tests (F.2 — 5 cases)
// ---------------------------------------------------------------------------

#[test]
fn unknown_type_returns_error() {
    let payload = r#"{"v":1,"type":"nope_not_a_real_variant","data":{}}"#;
    match Frame::from_sse_data(payload) {
        Err(DecodeError::UnknownVariant(_)) => {}
        other => panic!("expected UnknownVariant, got {other:?}"),
    }
}

#[test]
fn version_mismatch_returns_error() {
    let payload = r#"{"v":2,"type":"token","data":{"content":"hi"}}"#;
    match Frame::from_sse_data(payload) {
        Err(DecodeError::VersionMismatch { sender: 2, supported: 1 }) => {}
        other => panic!("expected VersionMismatch, got {other:?}"),
    }
}

#[test]
fn missing_version_returns_error() {
    let payload = r#"{"type":"token","data":{"content":"hi"}}"#;
    match Frame::from_sse_data(payload) {
        Err(DecodeError::InvalidEnvelope(_)) => {}
        other => panic!("expected InvalidEnvelope, got {other:?}"),
    }
}

#[test]
fn legacy_token_frame_fallback() {
    // The legacy OpenAI-shaped `{"choices":[{"delta":{"content":"…"}}]}`
    // frame is NOT a valid v1 envelope (no `v` field). The wire decoder
    // returns InvalidEnvelope; the CLI is responsible for falling back to
    // the legacy parser when this happens.
    let legacy = r#"{"choices":[{"delta":{"content":"Hello"}}]}"#;
    assert!(matches!(
        Frame::from_sse_data(legacy),
        Err(DecodeError::InvalidEnvelope(_))
    ));
}

#[test]
fn extra_fields_ignored() {
    // Extra envelope fields (`v_minor`) and extra data fields are tolerated:
    // serde's default `deny_unknown_fields` is OFF for our enum.
    let payload = r#"{"v":1,"v_minor":3,"type":"token","data":{"content":"hi","future_field":42}}"#;
    let frame = Frame::from_sse_data(payload).expect("extra fields should be tolerated");
    assert_eq!(frame, Frame::Token { content: "hi".to_string() });
}

// ---------------------------------------------------------------------------
// Encoded JSON shape (spec B.2 examples)
// ---------------------------------------------------------------------------

#[test]
fn encoded_token_matches_spec_shape() {
    let encoded = Frame::Token { content: "Hello".to_string() }
        .to_sse_data()
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(parsed["v"], json!(1));
    assert_eq!(parsed["type"], json!("token"));
    assert_eq!(parsed["data"]["content"], json!("Hello"));
}

#[test]
fn encoded_error_matches_spec_shape() {
    let encoded = Frame::Error {
        message: "insufficient credits".to_string(),
        kind: ErrorKind::Billing,
    }
    .to_sse_data()
    .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(parsed["v"], json!(1));
    assert_eq!(parsed["type"], json!("error"));
    assert_eq!(parsed["data"]["message"], json!("insufficient credits"));
    assert_eq!(parsed["data"]["kind"], json!("billing"));
}

#[test]
fn encoded_heartbeat_matches_spec_shape() {
    let encoded = Frame::Heartbeat { elapsed_secs: 15 }
        .to_sse_data()
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(parsed["v"], json!(1));
    assert_eq!(parsed["type"], json!("heartbeat"));
    assert_eq!(parsed["data"]["elapsed_secs"], json!(15));
}
