//! Semantic LLM error classification with recovery hints.
//!
//! Replaces the 2-way `LlmErrorKind` (Transient / NonTransient) with a
//! 16-variant `LlmFailoverReason` enum.  Each variant carries recovery hints
//! that the retry / failover loop can act on directly instead of re-inspecting
//! the raw error.
//!
//! # Design
//!
//! Classification is a two-pass pipeline:
//!
//! 1. **Body pattern matching** — regex patterns over the error message body
//!    (more specific than status codes; handles provider quirks where billing
//!    exhaustion arrives as HTTP 402 *or* 429 *or* 400 with a distinguishing
//!    body string).
//! 2. **HTTP status code fallback** — maps well-known status codes to reasons
//!    when no body pattern matches.
//!
//! Ported from `hermes-agent/agent/error_classifier.py`.

use once_cell::sync::Lazy;
use regex::RegexSet;

// ---------------------------------------------------------------------------
// LlmFailoverReason enum
// ---------------------------------------------------------------------------

/// Semantic reason an LLM API call failed, with associated recovery hints.
///
/// Use [`classify_llm_error`] to obtain a reason from an HTTP status code and
/// raw body string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LlmFailoverReason {
    /// Credit / billing exhaustion — rotate credential immediately.
    Billing,
    /// RPM / TPM rate limit — retry with backoff, optionally rotate credential.
    RateLimit,
    /// Input too long for model context window — compress context.
    ContextOverflow,
    /// Attachment / image too large — shrink and retry.
    ImageTooLarge,
    /// Model name unknown or not available at this provider — fallback.
    ModelNotFound,
    /// 401 with invalid API key — rotate credential or fallback.
    AuthInvalid,
    /// 401 with expired token — rotate credential or fallback.
    AuthExpired,
    /// 403 permission denied — fallback to another provider.
    PermissionDenied,
    /// Organisation-level quota exhausted — rotate credential.
    QuotaExceeded,
    /// 503 service unavailable — retry with backoff, fallback if repeated.
    ServiceUnavailable,
    /// 504 / read timeout — retry with backoff.
    Timeout,
    /// Generic 5xx internal server error — retry / fallback.
    InternalError,
    /// 400 bad request that does not match a more specific reason.
    BadRequest,
    /// Safety / content filter refusal.
    ContentFilter,
    /// Anthropic thinking-block signature invalid — retry after stripping.
    ThinkingSignature,
    /// Unclassifiable — retry with backoff.
    Unknown,
}

impl LlmFailoverReason {
    /// Whether the caller should attempt context compression before retrying.
    ///
    /// True only for [`ContextOverflow`][Self::ContextOverflow].
    pub fn should_compress(self) -> bool {
        matches!(self, Self::ContextOverflow)
    }

    /// Whether the caller should rotate to a different API credential.
    ///
    /// True for billing exhaustion, quota, and invalid / expired auth.
    pub fn should_rotate_credential(self) -> bool {
        matches!(
            self,
            Self::Billing | Self::QuotaExceeded | Self::AuthInvalid | Self::AuthExpired
        )
    }

    /// Whether the caller should fall over to the next provider in the chain.
    ///
    /// True for model-not-found, service unavailable, and generic internal errors.
    pub fn should_fallback(self) -> bool {
        matches!(
            self,
            Self::ModelNotFound | Self::ServiceUnavailable | Self::InternalError
        )
    }

    /// Whether the caller should retry (possibly after a backoff delay).
    ///
    /// True for transient conditions: rate limit, timeout, service temporarily
    /// unavailable, and generic internal errors.
    pub fn should_retry(self) -> bool {
        matches!(
            self,
            Self::RateLimit | Self::Timeout | Self::ServiceUnavailable | Self::InternalError
        )
    }
}

// ---------------------------------------------------------------------------
// Regex pattern sets
// ---------------------------------------------------------------------------

/// Body patterns that confirm billing / credit exhaustion.
static BILLING_PATTERNS: Lazy<RegexSet> = Lazy::new(|| {
    RegexSet::new([
        r"(?i)insufficient.{0,10}credits?",
        r"(?i)insufficient.{0,10}quota",
        r"(?i)insufficient.{0,10}balance",
        r"(?i)credit.{0,10}balance",
        r"(?i)credits? have been exhausted",
        r"(?i)top up your credits?",
        r"(?i)payment.{0,10}required",
        r"(?i)billing.{0,5}hard.{0,5}limit",
        r"(?i)exceeded your current quota",
        r"(?i)account is deactivated",
        r"(?i)plan does not include",
    ])
    .expect("BILLING_PATTERNS regex set is valid")
});

/// Body patterns that indicate a transient rate limit (not billing).
static RATE_LIMIT_PATTERNS: Lazy<RegexSet> = Lazy::new(|| {
    RegexSet::new([
        r"(?i)rate.{0,5}limit",
        r"(?i)too many requests",
        r"(?i)throttled",
        r"(?i)requests per minute",
        r"(?i)tokens per minute",
        r"(?i)requests per day",
        r"(?i)try again in",
        r"(?i)please retry after",
        r"(?i)resource.{0,5}exhausted",
        r"(?i)throttlingexception",
        r"(?i)too many concurrent requests",
        r"(?i)servicequotaexceededexception",
    ])
    .expect("RATE_LIMIT_PATTERNS regex set is valid")
});

/// Body patterns that indicate context / token length overflow.
static CONTEXT_OVERFLOW_PATTERNS: Lazy<RegexSet> = Lazy::new(|| {
    RegexSet::new([
        r"(?i)context.{0,10}length",
        r"(?i)context.{0,10}size",
        r"(?i)maximum.{0,10}context",
        r"(?i)token.{0,10}limit",
        r"(?i)too many tokens",
        r"(?i)reduce the length",
        r"(?i)exceeds the limit",
        r"(?i)context.{0,10}window",
        r"(?i)prompt is too long",
        r"(?i)prompt exceeds max length",
        r"(?i)maximum number of tokens",
        r"(?i)exceeds the max_model_len",
        r"(?i)max_model_len",
        r"(?i)input is too long",
        r"(?i)maximum model length",
        r"(?i)context length exceeded",
        r"(?i)context_length_exceeded",
    ])
    .expect("CONTEXT_OVERFLOW_PATTERNS regex set is valid")
});

/// Body patterns that indicate an image attachment is too large.
static IMAGE_TOO_LARGE_PATTERNS: Lazy<RegexSet> = Lazy::new(|| {
    RegexSet::new([
        r"(?i)image exceeds",
        r"(?i)image too large",
        r"(?i)image_too_large",
        r"(?i)image size exceeds",
    ])
    .expect("IMAGE_TOO_LARGE_PATTERNS regex set is valid")
});

/// Body patterns that indicate the model name is unknown.
static MODEL_NOT_FOUND_PATTERNS: Lazy<RegexSet> = Lazy::new(|| {
    RegexSet::new([
        r"(?i)is not a valid model",
        r"(?i)invalid model",
        r"(?i)model not found",
        r"(?i)model_not_found",
        r"(?i)no such model",
        r"(?i)unknown model",
        r"(?i)unsupported model",
    ])
    .expect("MODEL_NOT_FOUND_PATTERNS regex set is valid")
});

/// Body patterns that indicate a content / safety filter refusal.
static CONTENT_FILTER_PATTERNS: Lazy<RegexSet> = Lazy::new(|| {
    RegexSet::new([
        r"(?i)content.{0,10}filter",
        r"(?i)safety.{0,10}refusal",
        r"(?i)content_filter",
        r"(?i)content policy",
        r"(?i)violates our usage policy",
    ])
    .expect("CONTENT_FILTER_PATTERNS regex set is valid")
});

/// Body patterns that indicate an Anthropic thinking-block signature error.
static THINKING_SIGNATURE_PATTERNS: Lazy<RegexSet> = Lazy::new(|| {
    RegexSet::new([r"(?i)thinking.*signature", r"(?i)signature.*thinking"])
        .expect("THINKING_SIGNATURE_PATTERNS regex set is valid")
});

// ---------------------------------------------------------------------------
// classify_llm_error
// ---------------------------------------------------------------------------

/// Classify an LLM API error into a [`LlmFailoverReason`].
///
/// The pipeline is:
/// 1. Body pattern matching (most specific — handles provider-specific quirks)
/// 2. HTTP status code fallback
///
/// # Arguments
///
/// * `status` — Optional HTTP status code from the API response.
/// * `body` — Raw error message / body text from the API response.  May be
///   the JSON body serialised to a string, or just the error message field.
///   Case-insensitive pattern matching is applied.
///
/// # Examples
///
/// ```
/// use cyberclaw_llm::failover_reason::{classify_llm_error, LlmFailoverReason};
///
/// // Body-based classification takes priority over status code
/// let reason = classify_llm_error(Some(429), "insufficient credits");
/// assert_eq!(reason, LlmFailoverReason::Billing);
///
/// // Status-code fallback when no body pattern matches
/// let reason = classify_llm_error(Some(401), "");
/// assert_eq!(reason, LlmFailoverReason::AuthInvalid);
/// ```
pub fn classify_llm_error(status: Option<u16>, body: &str) -> LlmFailoverReason {
    // ── 1. Body pattern matching (priority-ordered) ──────────────────────

    // ThinkingSignature check requires both keywords — do it first (400-specific
    // Anthropic quirk that would otherwise fall into BadRequest).
    if THINKING_SIGNATURE_PATTERNS.is_match(body) {
        return LlmFailoverReason::ThinkingSignature;
    }

    // Image-too-large before context overflow: both can have "exceeds" and
    // image shrink is cheaper than context compression.
    if IMAGE_TOO_LARGE_PATTERNS.is_match(body) {
        return LlmFailoverReason::ImageTooLarge;
    }

    if CONTEXT_OVERFLOW_PATTERNS.is_match(body) {
        return LlmFailoverReason::ContextOverflow;
    }

    // Billing before rate-limit: some providers put billing errors on 429.
    if BILLING_PATTERNS.is_match(body) {
        return LlmFailoverReason::Billing;
    }

    if RATE_LIMIT_PATTERNS.is_match(body) {
        return LlmFailoverReason::RateLimit;
    }

    if MODEL_NOT_FOUND_PATTERNS.is_match(body) {
        return LlmFailoverReason::ModelNotFound;
    }

    if CONTENT_FILTER_PATTERNS.is_match(body) {
        return LlmFailoverReason::ContentFilter;
    }

    // ── 2. HTTP status code fallback ────────────────────────────────────

    match status {
        Some(401) => LlmFailoverReason::AuthInvalid,
        Some(402) => LlmFailoverReason::Billing,
        Some(403) => LlmFailoverReason::PermissionDenied,
        Some(404) => LlmFailoverReason::ModelNotFound,
        Some(413) => LlmFailoverReason::ImageTooLarge,
        Some(429) => LlmFailoverReason::RateLimit,
        Some(503) => LlmFailoverReason::ServiceUnavailable,
        Some(504) => LlmFailoverReason::Timeout,
        Some(500..=599) => LlmFailoverReason::InternalError,
        Some(400) => LlmFailoverReason::BadRequest,
        _ => LlmFailoverReason::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── Status code classification ──────────────────────────────────────

    #[test]
    fn test_classify_status_401_gives_auth_invalid() {
        let reason = classify_llm_error(Some(401), "");
        assert_eq!(reason, LlmFailoverReason::AuthInvalid);
    }

    #[test]
    fn test_classify_status_429_gives_rate_limit() {
        let reason = classify_llm_error(Some(429), "rate limit exceeded");
        assert_eq!(reason, LlmFailoverReason::RateLimit);
    }

    #[test]
    fn test_classify_status_503_gives_service_unavailable() {
        let reason = classify_llm_error(Some(503), "");
        assert_eq!(reason, LlmFailoverReason::ServiceUnavailable);
    }

    #[test]
    fn test_classify_status_500_gives_internal_error() {
        let reason = classify_llm_error(Some(500), "");
        assert_eq!(reason, LlmFailoverReason::InternalError);
    }

    #[test]
    fn test_classify_status_504_gives_timeout() {
        let reason = classify_llm_error(Some(504), "");
        assert_eq!(reason, LlmFailoverReason::Timeout);
    }

    #[test]
    fn test_classify_status_400_gives_bad_request() {
        let reason = classify_llm_error(Some(400), "");
        assert_eq!(reason, LlmFailoverReason::BadRequest);
    }

    #[test]
    fn test_classify_status_none_gives_unknown() {
        let reason = classify_llm_error(None, "");
        assert_eq!(reason, LlmFailoverReason::Unknown);
    }

    // ── Body pattern classification ─────────────────────────────────────

    #[test]
    fn test_classify_body_credit_gives_billing() {
        let reason = classify_llm_error(Some(429), "insufficient credits for this request");
        assert_eq!(reason, LlmFailoverReason::Billing);
    }

    #[test]
    fn test_classify_body_context_length_exceeded() {
        let reason = classify_llm_error(Some(400), "context_length_exceeded: prompt too long");
        assert_eq!(reason, LlmFailoverReason::ContextOverflow);
    }

    #[test]
    fn test_classify_body_context_overflow_gives_context_overflow() {
        let reason = classify_llm_error(Some(400), "This model's maximum context length is 4096");
        assert_eq!(reason, LlmFailoverReason::ContextOverflow);
    }

    #[test]
    fn test_classify_body_rate_limit_pattern() {
        let reason = classify_llm_error(None, "rate limit exceeded, try again later");
        assert_eq!(reason, LlmFailoverReason::RateLimit);
    }

    #[test]
    fn test_classify_body_model_not_found() {
        let reason = classify_llm_error(Some(404), "model not found: gpt-99");
        assert_eq!(reason, LlmFailoverReason::ModelNotFound);
    }

    #[test]
    fn test_classify_body_content_filter() {
        let reason = classify_llm_error(Some(400), "content_filter: request was blocked");
        assert_eq!(reason, LlmFailoverReason::ContentFilter);
    }

    #[test]
    fn test_classify_body_thinking_signature() {
        let reason = classify_llm_error(Some(400), "thinking block signature is invalid");
        assert_eq!(reason, LlmFailoverReason::ThinkingSignature);
    }

    #[test]
    fn test_classify_body_image_too_large() {
        let reason = classify_llm_error(Some(400), "image exceeds 5 MB maximum");
        assert_eq!(reason, LlmFailoverReason::ImageTooLarge);
    }

    #[test]
    fn test_classify_body_billing_overrides_status_429() {
        // Provider returns 429 but body says "billing hard limit" — Billing wins.
        let reason = classify_llm_error(Some(429), "You have hit the billing hard limit");
        assert_eq!(reason, LlmFailoverReason::Billing);
    }

    // ── Recovery hint methods ───────────────────────────────────────────

    #[test]
    fn test_should_compress_only_for_context_overflow() {
        assert!(LlmFailoverReason::ContextOverflow.should_compress());
        for reason in [
            LlmFailoverReason::Billing,
            LlmFailoverReason::RateLimit,
            LlmFailoverReason::AuthInvalid,
            LlmFailoverReason::ServiceUnavailable,
            LlmFailoverReason::Unknown,
        ] {
            assert!(!reason.should_compress(), "{reason:?} should not compress");
        }
    }

    #[test]
    fn test_should_rotate_credential_covers_billing_quota_auth() {
        for reason in [
            LlmFailoverReason::Billing,
            LlmFailoverReason::QuotaExceeded,
            LlmFailoverReason::AuthInvalid,
            LlmFailoverReason::AuthExpired,
        ] {
            assert!(
                reason.should_rotate_credential(),
                "{reason:?} should rotate credential"
            );
        }
        for reason in [
            LlmFailoverReason::RateLimit,
            LlmFailoverReason::ContextOverflow,
            LlmFailoverReason::ServiceUnavailable,
            LlmFailoverReason::Unknown,
        ] {
            assert!(
                !reason.should_rotate_credential(),
                "{reason:?} should NOT rotate credential"
            );
        }
    }

    #[test]
    fn test_should_retry_for_transient_variants() {
        for reason in [
            LlmFailoverReason::RateLimit,
            LlmFailoverReason::Timeout,
            LlmFailoverReason::ServiceUnavailable,
            LlmFailoverReason::InternalError,
        ] {
            assert!(reason.should_retry(), "{reason:?} should retry");
        }
        for reason in [
            LlmFailoverReason::Billing,
            LlmFailoverReason::AuthInvalid,
            LlmFailoverReason::ContextOverflow,
            LlmFailoverReason::Unknown,
        ] {
            assert!(!reason.should_retry(), "{reason:?} should NOT retry");
        }
    }

    #[test]
    fn test_should_fallback_for_model_service_internal() {
        for reason in [
            LlmFailoverReason::ModelNotFound,
            LlmFailoverReason::ServiceUnavailable,
            LlmFailoverReason::InternalError,
        ] {
            assert!(reason.should_fallback(), "{reason:?} should fallback");
        }
        for reason in [
            LlmFailoverReason::Billing,
            LlmFailoverReason::RateLimit,
            LlmFailoverReason::ContextOverflow,
            LlmFailoverReason::Unknown,
        ] {
            assert!(!reason.should_fallback(), "{reason:?} should NOT fallback");
        }
    }

    // ── Case-insensitive matching ───────────────────────────────────────

    #[test]
    fn test_patterns_are_case_insensitive() {
        assert_eq!(
            classify_llm_error(None, "INSUFFICIENT CREDITS"),
            LlmFailoverReason::Billing
        );
        assert_eq!(
            classify_llm_error(None, "CONTEXT LENGTH EXCEEDED"),
            LlmFailoverReason::ContextOverflow
        );
        assert_eq!(
            classify_llm_error(None, "Rate Limit Exceeded"),
            LlmFailoverReason::RateLimit
        );
    }

    // ── 5xx catch-all ───────────────────────────────────────────────────

    #[test]
    fn test_5xx_range_gives_internal_error() {
        for status in [501u16, 502, 507, 599] {
            let reason = classify_llm_error(Some(status), "");
            assert_eq!(
                reason,
                LlmFailoverReason::InternalError,
                "status {status} should map to InternalError"
            );
        }
    }
}
