use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::event::{event_uses_model_request_failure, is_supported_schema};
use crate::{EventEnvelope, ProjectionError, reconstruct};

#[derive(Debug, thiserror::Error)]
#[error("could not encode event envelope: {message}")]
pub struct EncodeError {
    message: String,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("event log is empty")]
    EmptyLog,
    #[error("line {line} is not a valid event envelope: {message}")]
    CorruptRecord { line: usize, message: String },
    #[error("line {line} uses unsupported event schema version {version}")]
    UnsupportedSchema { line: usize, version: u32 },
    #[error("line {line} uses an event not supported by schema version {version}")]
    SchemaViolation { line: usize, version: u32 },
    #[error("event history is malformed: {0}")]
    MalformedHistory(#[from] ProjectionError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedLog {
    envelopes: Vec<EventEnvelope>,
    incomplete_tail: Option<Vec<u8>>,
}

impl DecodedLog {
    pub fn envelopes(&self) -> &[EventEnvelope] {
        &self.envelopes
    }

    pub fn into_envelopes(self) -> Vec<EventEnvelope> {
        self.envelopes
    }

    pub fn has_incomplete_tail(&self) -> bool {
        self.incomplete_tail.is_some()
    }

    pub fn incomplete_tail(&self) -> Option<&[u8]> {
        self.incomplete_tail.as_deref()
    }
}

pub fn encode_envelope(envelope: &EventEnvelope) -> Result<Vec<u8>, EncodeError> {
    let mut bytes = serde_json::to_vec(envelope).map_err(|error| EncodeError {
        message: error.to_string(),
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn decode_envelope_line(line: &[u8], line_number: usize) -> Result<EventEnvelope, DecodeError> {
    let envelope: EventEnvelope =
        serde_json::from_slice(line).map_err(|error| DecodeError::CorruptRecord {
            line: line_number,
            message: error.to_string(),
        })?;
    if !is_supported_schema(envelope.schema_version) {
        return Err(DecodeError::UnsupportedSchema {
            line: line_number,
            version: envelope.schema_version,
        });
    }
    if envelope.schema_version == 1 && event_uses_model_request_failure(&envelope.event) {
        return Err(DecodeError::SchemaViolation {
            line: line_number,
            version: envelope.schema_version,
        });
    }
    let timestamp = OffsetDateTime::parse(&envelope.observed_at, &Rfc3339);
    if !matches!(timestamp, Ok(value) if value.offset() == UtcOffset::UTC) {
        return Err(DecodeError::CorruptRecord {
            line: line_number,
            message: "observed_at is not an RFC 3339 timestamp".to_owned(),
        });
    }
    Ok(envelope)
}

pub fn decode_jsonl(bytes: &[u8]) -> Result<DecodedLog, DecodeError> {
    if bytes.is_empty() {
        return Err(DecodeError::EmptyLog);
    }

    let (complete, incomplete_tail) = match bytes.iter().rposition(|byte| *byte == b'\n') {
        Some(last_newline) if last_newline + 1 == bytes.len() => (bytes, None),
        Some(last_newline) => (
            &bytes[..=last_newline],
            Some(bytes[last_newline + 1..].to_vec()),
        ),
        None => (&[][..], Some(bytes.to_vec())),
    };

    let mut envelopes = Vec::new();
    let records = complete.strip_suffix(b"\n").unwrap_or(complete);
    if !records.is_empty() {
        for (index, line) in records.split(|byte| *byte == b'\n').enumerate() {
            envelopes.push(decode_envelope_line(line, index + 1)?);
        }
    }

    if !envelopes.is_empty() {
        reconstruct(&envelopes)?;
    }

    Ok(DecodedLog {
        envelopes,
        incomplete_tail,
    })
}
