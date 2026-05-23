//! Rate limit header parser for LLM provider responses.
//!
//! Captures `x-ratelimit-*` headers from HTTP responses and exposes them
//! as a typed [`RateLimitSnapshot`] for display in the `/usage` slash command.
//!
//! # Supported headers
//!
//! The following headers are parsed (present on Anthropic, OpenAI, and
//! OpenAI-compatible providers that follow the same convention):
//!
//! | Header | Field |
//! |--------|-------|
//! | `x-ratelimit-limit-requests` | RPM cap |
//! | `x-ratelimit-remaining-requests` | Requests left in minute window |
//! | `x-ratelimit-limit-tokens` | TPM cap |
//! | `x-ratelimit-remaining-tokens` | Tokens left in minute window |
//! | `x-ratelimit-reset-requests` | Seconds until minute request window resets |
//! | `x-ratelimit-reset-tokens` | Seconds until minute token window resets |
//!
//! All fields are `Option` — if the provider does not return the header,
//! the field is `None` and display falls back to "not reported".

use chrono::{DateTime, Utc};
use reqwest::header::HeaderMap;

/// A snapshot of rate-limit state captured from a single LLM response.
///
/// All numeric fields are `Option<u64>`. If the provider did not return the
/// corresponding header the field is `None`.
#[derive(Debug, Clone)]
pub struct RateLimitSnapshot {
    /// Maximum requests per minute (RPM) allowed by the provider.
    pub requests_limit: Option<u64>,
    /// Requests remaining in the current minute window.
    pub requests_remaining: Option<u64>,
    /// Maximum tokens per minute (TPM) allowed by the provider.
    pub tokens_limit: Option<u64>,
    /// Tokens remaining in the current minute window.
    pub tokens_remaining: Option<u64>,
    /// Seconds until the request-rate window resets (raw string from header).
    pub requests_reset_secs: Option<f64>,
    /// Seconds until the token-rate window resets (raw string from header).
    pub tokens_reset_secs: Option<f64>,
    /// Wall-clock time when this snapshot was captured.
    pub captured_at: DateTime<Utc>,
    /// Provider name ("anthropic", "openai", "generic", …).
    pub provider: String,
}

impl RateLimitSnapshot {
    /// Parse `x-ratelimit-*` headers from an HTTP response.
    ///
    /// Returns `None` when none of the relevant headers are present (i.e. the
    /// provider does not expose rate-limit information). Returns `Some` even
    /// if only a subset of headers is present — absent headers become `None`
    /// fields inside the snapshot.
    pub fn from_headers(headers: &HeaderMap, provider: &str) -> Option<Self> {
        let requests_limit = parse_header_u64(headers, "x-ratelimit-limit-requests");
        let requests_remaining = parse_header_u64(headers, "x-ratelimit-remaining-requests");
        let tokens_limit = parse_header_u64(headers, "x-ratelimit-limit-tokens");
        let tokens_remaining = parse_header_u64(headers, "x-ratelimit-remaining-tokens");
        let requests_reset_secs = parse_header_reset_secs(headers, "x-ratelimit-reset-requests");
        let tokens_reset_secs = parse_header_reset_secs(headers, "x-ratelimit-reset-tokens");

        // Return None only when absolutely no rate-limit header was found.
        if requests_limit.is_none()
            && requests_remaining.is_none()
            && tokens_limit.is_none()
            && tokens_remaining.is_none()
            && requests_reset_secs.is_none()
            && tokens_reset_secs.is_none()
        {
            return None;
        }

        Some(Self {
            requests_limit,
            requests_remaining,
            tokens_limit,
            tokens_remaining,
            requests_reset_secs,
            tokens_reset_secs,
            captured_at: Utc::now(),
            provider: provider.to_string(),
        })
    }

    /// Percentage of the request quota that has been consumed (0.0 – 100.0).
    ///
    /// Returns `None` when `requests_limit` is absent or zero.
    pub fn usage_percent_requests(&self) -> Option<f64> {
        let limit = self.requests_limit?;
        if limit == 0 {
            return None;
        }
        let remaining = self.requests_remaining.unwrap_or(0);
        let used = limit.saturating_sub(remaining);
        Some((used as f64 / limit as f64) * 100.0)
    }

    /// Percentage of the token quota that has been consumed (0.0 – 100.0).
    ///
    /// Returns `None` when `tokens_limit` is absent or zero.
    pub fn usage_percent_tokens(&self) -> Option<f64> {
        let limit = self.tokens_limit?;
        if limit == 0 {
            return None;
        }
        let remaining = self.tokens_remaining.unwrap_or(0);
        let used = limit.saturating_sub(remaining);
        Some((used as f64 / limit as f64) * 100.0)
    }

    /// Approximate seconds elapsed since this snapshot was captured.
    pub fn age_secs(&self) -> f64 {
        (Utc::now() - self.captured_at).num_milliseconds().max(0) as f64 / 1000.0
    }

    /// Estimated seconds remaining until the request window resets,
    /// adjusted for time elapsed since the snapshot was captured.
    pub fn requests_reset_remaining_secs(&self) -> Option<f64> {
        let raw = self.requests_reset_secs?;
        Some((raw - self.age_secs()).max(0.0))
    }

    /// Estimated seconds remaining until the token window resets,
    /// adjusted for time elapsed since the snapshot was captured.
    pub fn tokens_reset_remaining_secs(&self) -> Option<f64> {
        let raw = self.tokens_reset_secs?;
        Some((raw - self.age_secs()).max(0.0))
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse a header value as `u64`. Returns `None` if the header is absent or
/// if the value cannot be parsed as an integer.
fn parse_header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    let val = headers.get(name)?.to_str().ok()?;
    val.trim().parse::<u64>().ok()
}

/// Parse a reset-time header.
///
/// Providers use two formats:
/// - A plain integer/float number of seconds: `"30"`, `"29.4"`
/// - A duration string like `"30s"`, `"1m30s"`, `"2m"`
///
/// We normalise everything to seconds as `f64`. Returns `None` when the header
/// is absent or unparseable.
fn parse_header_reset_secs(headers: &HeaderMap, name: &str) -> Option<f64> {
    let raw = headers.get(name)?.to_str().ok()?;
    let raw = raw.trim();
    parse_duration_str(raw)
}

/// Convert a duration string to seconds.
///
/// Accepted formats:
/// - `"30"` or `"30.5"` — plain seconds
/// - `"30s"` — seconds suffix
/// - `"1m30s"` or `"1m"` — minutes and optional seconds
/// - `"1h30m20s"` — hours, minutes, seconds
fn parse_duration_str(s: &str) -> Option<f64> {
    // Fast path: plain numeric (no suffix).
    if let Ok(v) = s.parse::<f64>() {
        return Some(v);
    }

    // Duration string parser: walk left-to-right accumulating digits, then
    // consume a unit character.
    let mut total: f64 = 0.0;
    let mut num_buf = String::new();
    let mut matched_any = false;

    for ch in s.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            num_buf.push(ch);
        } else {
            let n: f64 = num_buf.parse().unwrap_or(0.0);
            num_buf.clear();
            match ch {
                'h' => {
                    total += n * 3600.0;
                    matched_any = true;
                }
                'm' => {
                    total += n * 60.0;
                    matched_any = true;
                }
                's' => {
                    total += n;
                    matched_any = true;
                }
                _ => return None, // unknown suffix — bail
            }
        }
    }

    // Trailing digits without a unit — treat as seconds (some providers omit
    // the 's' suffix).
    if !num_buf.is_empty() {
        if let Ok(n) = num_buf.parse::<f64>() {
            total += n;
            matched_any = true;
        }
    }

    if matched_any {
        Some(total)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    fn make_headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    // -------------------------------------------------------------------------
    // Parse Anthropic-style headers
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_anthropic_style_headers() {
        let headers = make_headers(&[
            ("x-ratelimit-limit-requests", "1000"),
            ("x-ratelimit-remaining-requests", "766"),
            ("x-ratelimit-limit-tokens", "40000"),
            ("x-ratelimit-remaining-tokens", "27700"),
            ("x-ratelimit-reset-requests", "45s"),
            ("x-ratelimit-reset-tokens", "30s"),
        ]);

        let snap = RateLimitSnapshot::from_headers(&headers, "anthropic")
            .expect("should parse Anthropic headers");

        assert_eq!(snap.provider, "anthropic");
        assert_eq!(snap.requests_limit, Some(1000));
        assert_eq!(snap.requests_remaining, Some(766));
        assert_eq!(snap.tokens_limit, Some(40000));
        assert_eq!(snap.tokens_remaining, Some(27700));
        assert_eq!(snap.requests_reset_secs, Some(45.0));
        assert_eq!(snap.tokens_reset_secs, Some(30.0));
    }

    // -------------------------------------------------------------------------
    // Parse OpenAI-style headers (plain integer seconds)
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_openai_style_headers() {
        let headers = make_headers(&[
            ("x-ratelimit-limit-requests", "500"),
            ("x-ratelimit-remaining-requests", "499"),
            ("x-ratelimit-limit-tokens", "150000"),
            ("x-ratelimit-remaining-tokens", "149850"),
            ("x-ratelimit-reset-requests", "120"),
            ("x-ratelimit-reset-tokens", "60"),
        ]);

        let snap = RateLimitSnapshot::from_headers(&headers, "openai")
            .expect("should parse OpenAI headers");

        assert_eq!(snap.provider, "openai");
        assert_eq!(snap.requests_limit, Some(500));
        assert_eq!(snap.requests_remaining, Some(499));
        assert_eq!(snap.tokens_limit, Some(150000));
        assert_eq!(snap.tokens_remaining, Some(149850));
        assert_eq!(snap.requests_reset_secs, Some(120.0));
        assert_eq!(snap.tokens_reset_secs, Some(60.0));
    }

    // -------------------------------------------------------------------------
    // Handle missing headers — returns None
    // -------------------------------------------------------------------------

    #[test]
    fn test_missing_headers_returns_none() {
        let headers = HeaderMap::new();
        let snap = RateLimitSnapshot::from_headers(&headers, "generic");
        assert!(snap.is_none(), "empty headers should yield None");
    }

    // -------------------------------------------------------------------------
    // Partial headers — Some snapshot with missing fields as None
    // -------------------------------------------------------------------------

    #[test]
    fn test_partial_headers_returns_some_with_nones() {
        let headers = make_headers(&[("x-ratelimit-limit-requests", "100")]);

        let snap = RateLimitSnapshot::from_headers(&headers, "openai")
            .expect("partial headers should still produce a snapshot");

        assert_eq!(snap.requests_limit, Some(100));
        assert!(snap.requests_remaining.is_none());
        assert!(snap.tokens_limit.is_none());
        assert!(snap.tokens_remaining.is_none());
    }

    // -------------------------------------------------------------------------
    // usage_percent_requests / usage_percent_tokens calculations
    // -------------------------------------------------------------------------

    #[test]
    fn test_usage_percent_requests() {
        let headers = make_headers(&[
            ("x-ratelimit-limit-requests", "1000"),
            ("x-ratelimit-remaining-requests", "750"),
        ]);
        let snap = RateLimitSnapshot::from_headers(&headers, "anthropic").unwrap();
        let pct = snap.usage_percent_requests().expect("should compute pct");
        // used = 1000 - 750 = 250; pct = 25.0
        assert!((pct - 25.0).abs() < 0.001, "expected ~25%, got {pct}");
    }

    #[test]
    fn test_usage_percent_tokens() {
        let headers = make_headers(&[
            ("x-ratelimit-limit-tokens", "40000"),
            ("x-ratelimit-remaining-tokens", "28000"),
        ]);
        let snap = RateLimitSnapshot::from_headers(&headers, "anthropic").unwrap();
        let pct = snap.usage_percent_tokens().expect("should compute pct");
        // used = 40000 - 28000 = 12000; pct = 30.0
        assert!((pct - 30.0).abs() < 0.001, "expected ~30%, got {pct}");
    }

    #[test]
    fn test_usage_percent_no_limit_returns_none() {
        let snap = RateLimitSnapshot {
            requests_limit: None,
            requests_remaining: Some(500),
            tokens_limit: Some(0), // zero limit → None
            tokens_remaining: Some(0),
            requests_reset_secs: None,
            tokens_reset_secs: None,
            captured_at: Utc::now(),
            provider: "test".to_string(),
        };
        assert!(snap.usage_percent_requests().is_none());
        assert!(snap.usage_percent_tokens().is_none());
    }

    // -------------------------------------------------------------------------
    // Duration string parsing
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_duration_plain_int() {
        assert_eq!(parse_duration_str("30"), Some(30.0));
    }

    #[test]
    fn test_parse_duration_plain_float() {
        assert_eq!(parse_duration_str("29.5"), Some(29.5));
    }

    #[test]
    fn test_parse_duration_seconds_suffix() {
        assert_eq!(parse_duration_str("45s"), Some(45.0));
    }

    #[test]
    fn test_parse_duration_minutes_only() {
        assert_eq!(parse_duration_str("2m"), Some(120.0));
    }

    #[test]
    fn test_parse_duration_minutes_and_seconds() {
        assert_eq!(parse_duration_str("1m30s"), Some(90.0));
    }

    #[test]
    fn test_parse_duration_hours_minutes_seconds() {
        assert_eq!(parse_duration_str("1h2m3s"), Some(3723.0));
    }

    #[test]
    fn test_parse_duration_unknown_returns_none() {
        assert!(parse_duration_str("bad_value").is_none());
    }
}
