use std::{future::Future, pin::Pin};

use aurora_core::ModelRequestFailure;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ZaiJsonObjectValidationError {
    #[error("JSON-object instructions are blank")]
    BlankInstructions,
    #[error("JSON-object input is blank")]
    BlankInput,
    #[error("JSON-object expected shape must be an object")]
    ExpectedShapeMustBeObject,
    #[error("JSON-object request exceeds the byte limit")]
    RequestTooLarge,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ZaiJsonObjectInvocation {
    Output(Value),
    RequestFailure(ModelRequestFailure),
    MalformedOutput,
    ResponseTooLarge,
    RequestTooLarge,
    Cancelled,
}

pub type ZaiJsonObjectFuture =
    Pin<Box<dyn Future<Output = ZaiJsonObjectInvocation> + Send + 'static>>;

pub struct ZaiJsonObjectRequest {
    instructions: String,
    input: String,
    expected_shape: String,
}

impl std::fmt::Debug for ZaiJsonObjectRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ZaiJsonObjectRequest")
            .field("instructions", &"[REDACTED]")
            .field("input", &"[REDACTED]")
            .field("expected_shape", &"[REDACTED]")
            .finish()
    }
}

impl ZaiJsonObjectRequest {
    pub fn new(
        instructions: impl Into<String>,
        input: impl Into<String>,
        expected_shape: Value,
    ) -> Result<Self, ZaiJsonObjectValidationError> {
        let instructions = instructions.into();
        if instructions.trim().is_empty() {
            return Err(ZaiJsonObjectValidationError::BlankInstructions);
        }
        let input = input.into();
        if input.trim().is_empty() {
            return Err(ZaiJsonObjectValidationError::BlankInput);
        }
        if !expected_shape.is_object() {
            return Err(ZaiJsonObjectValidationError::ExpectedShapeMustBeObject);
        }
        let expected_shape = serde_json::to_string(&expected_shape)
            .map_err(|_| ZaiJsonObjectValidationError::RequestTooLarge)?;
        checked_request_len(instructions.len(), input.len(), expected_shape.len())
            .ok_or(ZaiJsonObjectValidationError::RequestTooLarge)?;
        Ok(Self {
            instructions,
            input,
            expected_shape,
        })
    }

    pub(super) fn into_prompt_parts(self) -> (String, String, String) {
        (self.instructions, self.input, self.expected_shape)
    }
}

fn checked_request_len(first: usize, second: usize, third: usize) -> Option<usize> {
    let total = first.checked_add(second)?.checked_add(third)?;
    (total <= crate::MAX_REQUEST_BYTES).then_some(total)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn json_object_request_requires_nonblank_text_and_object_shape() {
        assert!(matches!(
            ZaiJsonObjectRequest::new("", "input", json!({})),
            Err(ZaiJsonObjectValidationError::BlankInstructions)
        ));
        assert!(matches!(
            ZaiJsonObjectRequest::new("instructions", "", json!({})),
            Err(ZaiJsonObjectValidationError::BlankInput)
        ));
        assert!(matches!(
            ZaiJsonObjectRequest::new("instructions", "input", json!([])),
            Err(ZaiJsonObjectValidationError::ExpectedShapeMustBeObject)
        ));
    }

    #[test]
    fn json_object_request_debug_redacts_prompt_material() {
        let request = ZaiJsonObjectRequest::new(
            "private instructions",
            "private input",
            json!({"private_shape":"value"}),
        )
        .unwrap();
        let debug = format!("{request:?}");
        for private in ["private instructions", "private input", "private_shape"] {
            assert!(!debug.contains(private));
        }
    }

    #[test]
    fn checked_json_object_request_length_rejects_overflow() {
        assert_eq!(checked_request_len(usize::MAX, 1, 0), None);
    }

    #[test]
    fn json_object_request_length_accepts_exact_limit() {
        assert_eq!(
            checked_request_len(crate::MAX_REQUEST_BYTES, 0, 0),
            Some(crate::MAX_REQUEST_BYTES)
        );
    }
}
