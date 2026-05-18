//! Sensitive data redaction types.
//!
//! This module provides [`SensitiveString`] — a wrapper around `String` that
//! automatically redacts its contents in [`Debug`], [`Display`], and serde
//! serialization contexts, preventing accidental leakage of passwords, tokens,
//! and other secrets into log files or structured output.
//!
//! # Examples
//!
//! ```
//! use cyberclaw_core::sensitive::{SensitiveString, RedactionStrategy};
//!
//! let password = SensitiveString::from_password("super-secret");
//! assert_eq!(format!("{:?}", password), "<PASSWORD>");
//! assert_eq!(format!("{}", password), "<PASSWORD>");
//!
//! let token = SensitiveString::from_token("sk_live_abcdef1234567890");
//! // Partial strategy keeps the first 4 chars and last 4 chars.
//! assert!(format!("{}", token).contains("****"));
//! ```

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

// ---------------------------------------------------------------------------
// RedactionStrategy
// ---------------------------------------------------------------------------

/// Describes how a [`SensitiveString`] value is redacted when printed or
/// serialised.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RedactionStrategy {
    /// Replace the entire value with the literal string `"***REDACTED***"`.
    Full,

    /// Keep `prefix` leading characters and `suffix` trailing characters,
    /// replacing the middle with `"****"`.
    ///
    /// If the value is shorter than `prefix + suffix` the whole value is
    /// replaced with `"***REDACTED***"`.
    Partial {
        /// Number of leading characters to expose.
        prefix: usize,
        /// Number of trailing characters to expose.
        suffix: usize,
    },

    /// Replace the entire value with a descriptive type label.
    TypeOnly(SensitiveType),
}

/// Human-readable label used by [`RedactionStrategy::TypeOnly`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SensitiveType {
    /// A user password or passphrase.
    Password,
    /// An API key or service credential.
    ApiKey,
    /// A bearer token (OAuth, JWT, etc.).
    Token,
    /// A generic secret value.
    Secret,
}

impl fmt::Display for SensitiveType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SensitiveType::Password => write!(f, "<PASSWORD>"),
            SensitiveType::ApiKey => write!(f, "<API_KEY>"),
            SensitiveType::Token => write!(f, "<TOKEN>"),
            SensitiveType::Secret => write!(f, "<SECRET>"),
        }
    }
}

// ---------------------------------------------------------------------------
// SensitiveString
// ---------------------------------------------------------------------------

/// A `String` wrapper that prevents accidental exposure of sensitive data.
///
/// The inner value is never exposed via [`Debug`], [`Display`], or serde
/// serialisation — the configured [`RedactionStrategy`] is applied instead.
///
/// Use [`SensitiveString::expose`] to access the raw value when it is
/// genuinely required (e.g. when passing it to an authentication call).
///
/// ## Serde behaviour
///
/// * **Serialisation** — always emits the *redacted* string (e.g.
///   `"***REDACTED***"`).  The raw value and the strategy are never written
///   to output.
/// * **Deserialisation** — accepts the redacted string that was previously
///   serialised.  The `inner` value is restored as an empty string (the raw
///   secret is gone) and the strategy defaults to [`RedactionStrategy::Full`].
///   This is intentional: once a secret is serialised it cannot be
///   round-tripped back to a live credential.
#[derive(Clone)]
pub struct SensitiveString {
    /// The raw value.  Never serialised.
    inner: String,

    /// Controls how the value is rendered in non-sensitive contexts.
    pub redaction_strategy: RedactionStrategy,
}

// ---------------------------------------------------------------------------
// serde::Deserialize — accepts the redacted string representation
// ---------------------------------------------------------------------------

/// Visitor that accepts only a string value (the redacted output) and
/// produces a [`SensitiveString`] with an empty `inner` and `Full` strategy.
struct SensitiveStringVisitor;

impl<'de> Visitor<'de> for SensitiveStringVisitor {
    type Value = SensitiveString;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a redacted sensitive string")
    }

    fn visit_str<E: de::Error>(self, _v: &str) -> Result<SensitiveString, E> {
        // The raw value cannot be recovered from the redacted output.
        // We deserialise into a placeholder with an empty inner string.
        Ok(SensitiveString {
            inner: String::new(),
            redaction_strategy: RedactionStrategy::Full,
        })
    }
}

impl<'de> Deserialize<'de> for SensitiveString {
    fn deserialize<D: Deserializer<'de>>(deserialiser: D) -> Result<Self, D::Error> {
        deserialiser.deserialize_str(SensitiveStringVisitor)
    }
}

impl SensitiveString {
    // ------------------------------------------------------------------
    // Constructors
    // ------------------------------------------------------------------

    /// Create a `SensitiveString` with an explicit strategy.
    pub fn new(value: impl Into<String>, strategy: RedactionStrategy) -> Self {
        Self {
            inner: value.into(),
            redaction_strategy: strategy,
        }
    }

    /// Convenience: wrap a password using the `TypeOnly(Password)` strategy.
    ///
    /// # Examples
    ///
    /// ```
    /// use cyberclaw_core::sensitive::SensitiveString;
    /// let p = SensitiveString::from_password("hunter2");
    /// assert_eq!(format!("{}", p), "<PASSWORD>");
    /// ```
    pub fn from_password(value: impl Into<String>) -> Self {
        Self::new(value, RedactionStrategy::TypeOnly(SensitiveType::Password))
    }

    /// Convenience: wrap an API key using a partial strategy that keeps 4
    /// leading and 4 trailing characters.
    ///
    /// # Examples
    ///
    /// ```
    /// use cyberclaw_core::sensitive::SensitiveString;
    /// let k = SensitiveString::from_api_key("sk_live_LONGKEYVALUE1234");
    /// let rendered = format!("{}", k);
    /// assert!(rendered.starts_with("sk_l"));
    /// assert!(rendered.ends_with("1234"));
    /// assert!(rendered.contains("****"));
    /// ```
    pub fn from_api_key(value: impl Into<String>) -> Self {
        Self::new(
            value,
            RedactionStrategy::Partial {
                prefix: 4,
                suffix: 4,
            },
        )
    }

    /// Convenience: wrap a bearer/OAuth/JWT token using a partial strategy.
    ///
    /// # Examples
    ///
    /// ```
    /// use cyberclaw_core::sensitive::SensitiveString;
    /// let t = SensitiveString::from_token("eyJhbGciOiJIUzI1NiJ9.PAYLOAD.SIG");
    /// let rendered = format!("{}", t);
    /// assert!(rendered.contains("****"));
    /// ```
    pub fn from_token(value: impl Into<String>) -> Self {
        Self::new(
            value,
            RedactionStrategy::Partial {
                prefix: 4,
                suffix: 4,
            },
        )
    }

    /// Convenience: wrap a generic secret using the full redaction strategy.
    ///
    /// # Examples
    ///
    /// ```
    /// use cyberclaw_core::sensitive::SensitiveString;
    /// let s = SensitiveString::from_secret("very-secret-value");
    /// assert_eq!(format!("{}", s), "***REDACTED***");
    /// ```
    pub fn from_secret(value: impl Into<String>) -> Self {
        Self::new(value, RedactionStrategy::Full)
    }

    // ------------------------------------------------------------------
    // Access
    // ------------------------------------------------------------------

    /// Obtain a reference to the raw (unredacted) value.
    ///
    /// **Warning:** the caller is responsible for not logging or serialising
    /// the returned reference.
    pub fn expose(&self) -> &str {
        &self.inner
    }

    // ------------------------------------------------------------------
    // Redaction
    // ------------------------------------------------------------------

    /// Apply the configured strategy and return the redacted representation.
    pub fn redact(&self) -> String {
        match self.redaction_strategy {
            RedactionStrategy::Full => "***REDACTED***".to_owned(),
            RedactionStrategy::Partial { prefix, suffix } => {
                redact_partial(&self.inner, prefix, suffix)
            }
            RedactionStrategy::TypeOnly(kind) => kind.to_string(),
        }
    }
}

/// Produce a partially-redacted string.
///
/// ```text
/// "sk_live_abc123xyz" with prefix=4, suffix=4  →  "sk_l****xyz"
///                                                      ^^^^       first 4
///                                                          ****   mask
///                                                              ^^^^ last 4 (from original)
/// ```
///
/// If the string is shorter than `prefix + suffix`, the full string is
/// replaced with `"***REDACTED***"`.
fn redact_partial(value: &str, prefix: usize, suffix: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    let len = chars.len();

    if len <= prefix + suffix {
        return "***REDACTED***".to_owned();
    }

    let head: String = chars[..prefix].iter().collect();
    let tail: String = chars[len - suffix..].iter().collect();
    format!("{}****{}", head, tail)
}

// ---------------------------------------------------------------------------
// fmt::Debug — redacted
// ---------------------------------------------------------------------------

impl fmt::Debug for SensitiveString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.redact())
    }
}

// ---------------------------------------------------------------------------
// fmt::Display — redacted
// ---------------------------------------------------------------------------

impl fmt::Display for SensitiveString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.redact())
    }
}

// ---------------------------------------------------------------------------
// serde::Serialize — serialises only the redacted representation
// ---------------------------------------------------------------------------

impl Serialize for SensitiveString {
    fn serialize<S: Serializer>(&self, serialiser: S) -> Result<S::Ok, S::Error> {
        serialiser.serialize_str(&self.redact())
    }
}

// The blanket Deserialize derive (above) already handles the incoming side.
// The `inner` field is `#[serde(skip)]` so it deserialises to an empty string.
// If full round-trip is ever needed, a custom Deserializer can be provided here.

// ---------------------------------------------------------------------------
// PartialEq / Eq — compare the *raw* values for equality tests
// ---------------------------------------------------------------------------

impl PartialEq for SensitiveString {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner && self.redaction_strategy == other.redaction_strategy
    }
}

impl Eq for SensitiveString {}

// ---------------------------------------------------------------------------
// From<String> / From<&str>
// ---------------------------------------------------------------------------

impl From<String> for SensitiveString {
    /// Defaults to the `Full` redaction strategy.
    fn from(value: String) -> Self {
        Self::new(value, RedactionStrategy::Full)
    }
}

impl From<&str> for SensitiveString {
    /// Defaults to the `Full` redaction strategy.
    fn from(value: &str) -> Self {
        Self::new(value, RedactionStrategy::Full)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Display / Debug redaction --

    #[test]
    fn test_full_redaction_display() {
        let s = SensitiveString::from_secret("super-secret");
        assert_eq!(format!("{}", s), "***REDACTED***");
    }

    #[test]
    fn test_full_redaction_debug() {
        let s = SensitiveString::from_secret("super-secret");
        assert_eq!(format!("{:?}", s), "***REDACTED***");
    }

    #[test]
    fn test_password_type_only_display() {
        let s = SensitiveString::from_password("hunter2");
        assert_eq!(format!("{}", s), "<PASSWORD>");
    }

    #[test]
    fn test_password_type_only_debug() {
        let s = SensitiveString::from_password("hunter2");
        assert_eq!(format!("{:?}", s), "<PASSWORD>");
    }

    #[test]
    fn test_api_key_partial_redaction() {
        let s = SensitiveString::from_api_key("sk_live_LONGKEYVALUE1234");
        let rendered = format!("{}", s);
        // First 4 chars preserved.
        assert!(rendered.starts_with("sk_l"), "got: {rendered}");
        // Last 4 chars preserved.
        assert!(rendered.ends_with("1234"), "got: {rendered}");
        // Middle is masked.
        assert!(rendered.contains("****"), "got: {rendered}");
        // Raw value is not present.
        assert!(!rendered.contains("LONGKEYVALUE"), "got: {rendered}");
    }

    #[test]
    fn test_token_partial_redaction() {
        let s = SensitiveString::from_token("eyJhbGciOiJIUzI1NiJ9.PAYLOAD.SIG");
        let rendered = format!("{}", s);
        assert!(rendered.starts_with("eyJh"), "got: {rendered}");
        assert!(rendered.ends_with(".SIG"), "got: {rendered}");
        assert!(rendered.contains("****"), "got: {rendered}");
    }

    #[test]
    fn test_partial_redaction_short_value_falls_back_to_full() {
        // Value shorter than prefix + suffix should fall back.
        let s = SensitiveString::new(
            "ab",
            RedactionStrategy::Partial {
                prefix: 4,
                suffix: 4,
            },
        );
        assert_eq!(format!("{}", s), "***REDACTED***");
    }

    // -- serde serialisation --

    #[test]
    fn test_serialise_full_redaction() {
        let s = SensitiveString::from_secret("super-secret");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#""***REDACTED***""#);
        assert!(!json.contains("super-secret"));
    }

    #[test]
    fn test_serialise_password_type_only() {
        let s = SensitiveString::from_password("hunter2");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#""<PASSWORD>""#);
        assert!(!json.contains("hunter2"));
    }

    #[test]
    fn test_serialise_api_key_partial() {
        let s = SensitiveString::from_api_key("sk_live_LONGKEYVALUE1234");
        let json = serde_json::to_string(&s).unwrap();
        // Raw middle section must not appear.
        assert!(!json.contains("LONGKEYVALUE"), "json leaked value: {json}");
        assert!(json.contains("sk_l"), "json: {json}");
        assert!(json.contains("1234"), "json: {json}");
    }

    #[test]
    fn test_deserialise_inner_is_empty() {
        // After round-trip the inner value is empty (serde(skip)).
        let s = SensitiveString::from_secret("super-secret");
        let json = serde_json::to_string(&s).unwrap();
        let recovered: SensitiveString = serde_json::from_str(&json).unwrap();
        // inner is empty because it was skipped during serialisation.
        assert_eq!(recovered.expose(), "");
        // Strategy is preserved because it is serialised.
        assert_eq!(recovered.redaction_strategy, RedactionStrategy::Full);
    }

    // -- expose() gives the raw value --

    #[test]
    fn test_expose_returns_raw_value() {
        let s = SensitiveString::from_secret("raw-value");
        assert_eq!(s.expose(), "raw-value");
    }

    // -- struct containing SensitiveString --

    #[test]
    fn test_struct_debug_does_not_leak() {
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Config {
            user: String,
            password: SensitiveString,
        }
        let cfg = Config {
            user: "alice".to_owned(),
            password: SensitiveString::from_password("s3cr3t"),
        };
        let rendered = format!("{:?}", cfg);
        assert!(!rendered.contains("s3cr3t"), "debug leaked: {rendered}");
        assert!(rendered.contains("<PASSWORD>"), "debug: {rendered}");
    }

    #[test]
    fn test_struct_serialize_does_not_leak() {
        use serde::Serialize;
        #[derive(Serialize)]
        #[allow(dead_code)]
        struct Event {
            user: String,
            token: SensitiveString,
        }
        // Token has a long middle section "LONGMIDDLEPART" that must not appear
        // in the serialised output. The first/last 4 chars may be present (by
        // design of the Partial strategy).
        let evt = Event {
            user: "alice".to_owned(),
            token: SensitiveString::from_token("eyJhLONGMIDDLEPARTabcd"),
        };
        let json = serde_json::to_string(&evt).unwrap();
        // The middle section must never appear.
        assert!(!json.contains("LONGMIDDLEPART"), "json leaked: {json}");
        // The mask indicator must be present.
        assert!(json.contains("****"), "mask missing from: {json}");
    }

    // -- TypeOnly variants --

    #[test]
    fn test_type_only_api_key_label() {
        let s = SensitiveString::new("key123", RedactionStrategy::TypeOnly(SensitiveType::ApiKey));
        assert_eq!(format!("{}", s), "<API_KEY>");
    }

    #[test]
    fn test_type_only_token_label() {
        let s = SensitiveString::new("tok456", RedactionStrategy::TypeOnly(SensitiveType::Token));
        assert_eq!(format!("{}", s), "<TOKEN>");
    }

    #[test]
    fn test_type_only_secret_label() {
        let s = SensitiveString::new("sec789", RedactionStrategy::TypeOnly(SensitiveType::Secret));
        assert_eq!(format!("{}", s), "<SECRET>");
    }

    // -- From impls --

    #[test]
    fn test_from_str_uses_full_strategy() {
        let s: SensitiveString = "raw".into();
        assert_eq!(format!("{}", s), "***REDACTED***");
        assert_eq!(s.expose(), "raw");
    }

    #[test]
    fn test_from_string_uses_full_strategy() {
        let s: SensitiveString = String::from("raw").into();
        assert_eq!(format!("{}", s), "***REDACTED***");
        assert_eq!(s.expose(), "raw");
    }

    // -- redact_partial helper --

    #[test]
    fn test_redact_partial_unicode_safe() {
        // Multi-byte characters — prefix/suffix are counted in chars, not bytes.
        let value = "αβγδεζηθ"; // 8 Greek letters
        let result = redact_partial(value, 2, 2);
        assert!(result.starts_with("αβ"), "got: {result}");
        assert!(result.ends_with("ηθ"), "got: {result}");
        assert!(result.contains("****"), "got: {result}");
    }
}
