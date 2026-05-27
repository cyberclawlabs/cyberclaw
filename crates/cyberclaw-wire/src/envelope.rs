//! Envelope encoder/decoder for the typed SSE wire format.
//!
//! The wire envelope is:
//!
//! ```json
//! {"v": 1, "type": "<variant_snake_case>", "data": {<variant_fields>}}
//! ```
//!
//! `Done` is the only special-case: it serialises to the literal `[DONE]`
//! sentinel and the decoder recognises that sentinel as `Frame::Done`.

use crate::{Frame, DONE_SENTINEL, MAX_FRAME_BYTES, PROTOCOL_VERSION};
use serde_json::Value;

/// Errors that can occur while encoding a [`Frame`] for the wire.
#[derive(Debug)]
pub enum EncodeError {
    /// `serde_json` failed to serialise the frame.
    Serde(serde_json::Error),
    /// The serialised envelope exceeded [`MAX_FRAME_BYTES`].
    FrameTooLarge {
        /// Actual byte length of the serialised envelope.
        actual: usize,
        /// Maximum allowed byte length.
        limit: usize,
    },
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serde(e) => write!(f, "wire encode serde error: {e}"),
            Self::FrameTooLarge { actual, limit } => {
                write!(
                    f,
                    "wire frame too large: {actual} bytes exceeds {limit}-byte limit"
                )
            }
        }
    }
}

impl std::error::Error for EncodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serde(e) => Some(e),
            Self::FrameTooLarge { .. } => None,
        }
    }
}

impl From<serde_json::Error> for EncodeError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serde(value)
    }
}

/// Errors that can occur while decoding an SSE `data:` body into a [`Frame`].
#[derive(Debug)]
pub enum DecodeError {
    /// Body was not valid JSON (and was not the `[DONE]` sentinel).
    InvalidJson(serde_json::Error),
    /// JSON did not match the envelope shape (missing `v` / `type`).
    InvalidEnvelope(String),
    /// The envelope `v` field did not match [`PROTOCOL_VERSION`].
    VersionMismatch {
        /// Version reported by the sender.
        sender: u64,
        /// Version this decoder supports.
        supported: u8,
    },
    /// `type` matched no known variant, or `data` failed inner deserialization.
    UnknownVariant(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(e) => write!(f, "wire decode JSON error: {e}"),
            Self::InvalidEnvelope(s) => write!(f, "wire decode invalid envelope: {s}"),
            Self::VersionMismatch { sender, supported } => write!(
                f,
                "wire protocol version {sender} not supported (decoder supports v{supported})"
            ),
            Self::UnknownVariant(s) => write!(f, "wire decode unknown variant: {s}"),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJson(e) => Some(e),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for DecodeError {
    fn from(value: serde_json::Error) -> Self {
        Self::InvalidJson(value)
    }
}

/// Serialise a [`Frame`] into the body of an SSE `data:` line.
///
/// [`Frame::Done`] is encoded as the literal [`DONE_SENTINEL`] (no envelope).
/// All other variants are wrapped in `{"v":1,"type":"…","data":{…}}`.
pub fn to_sse_data(frame: &Frame) -> Result<String, EncodeError> {
    if matches!(frame, Frame::Done) {
        return Ok(DONE_SENTINEL.to_string());
    }

    // Serialise the frame to a tagged JSON value first so we can splice the
    // `v` field into the envelope. This avoids defining a parallel "wire
    // envelope" type for every variant.
    let tagged = serde_json::to_value(frame)?;
    let (variant_type, data) = match &tagged {
        Value::Object(map) => {
            let ty = map
                .get("type")
                .and_then(|t| t.as_str())
                .ok_or_else(|| {
                    EncodeError::Serde(serde::ser::Error::custom(
                        "frame serialised without `type` tag",
                    ))
                })?
                .to_string();
            let data = map.get("data").cloned().unwrap_or(Value::Null);
            (ty, data)
        }
        _ => {
            return Err(EncodeError::Serde(serde::ser::Error::custom(
                "frame serialised to non-object value",
            )))
        }
    };

    let envelope = serde_json::json!({
        "v": PROTOCOL_VERSION,
        "type": variant_type,
        "data": data,
    });
    let encoded = serde_json::to_string(&envelope)?;

    if encoded.len() > MAX_FRAME_BYTES {
        return Err(EncodeError::FrameTooLarge {
            actual: encoded.len(),
            limit: MAX_FRAME_BYTES,
        });
    }

    Ok(encoded)
}

/// Parse a single SSE `data:` body into a [`Frame`].
///
/// The literal [`DONE_SENTINEL`] (after trimming) decodes to [`Frame::Done`].
/// All other inputs MUST be a versioned envelope.
pub fn from_sse_data(data: &str) -> Result<Frame, DecodeError> {
    let data = data.trim();
    if data == DONE_SENTINEL {
        return Ok(Frame::Done);
    }

    let value: Value = serde_json::from_str(data)?;
    let obj = value
        .as_object()
        .ok_or_else(|| DecodeError::InvalidEnvelope("payload is not a JSON object".to_string()))?;

    // Version check — missing `v` is also a mismatch (legacy frames go through
    // the legacy parser, not this decoder).
    let sender_version = obj
        .get("v")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| DecodeError::InvalidEnvelope("missing `v` field".to_string()))?;
    if sender_version != u64::from(PROTOCOL_VERSION) {
        return Err(DecodeError::VersionMismatch {
            sender: sender_version,
            supported: PROTOCOL_VERSION,
        });
    }

    let variant_type = obj
        .get("type")
        .and_then(|t| t.as_str())
        .ok_or_else(|| DecodeError::InvalidEnvelope("missing `type` field".to_string()))?
        .to_string();
    let data = obj.get("data").cloned().unwrap_or(Value::Null);

    // Reconstruct the serde-tagged shape (`{"type":"…","data":{…}}`) and let
    // serde do the variant dispatch.
    let tagged = serde_json::json!({
        "type": variant_type,
        "data": data,
    });
    serde_json::from_value::<Frame>(tagged).map_err(|e| DecodeError::UnknownVariant(e.to_string()))
}
