use std::{future::Future, pin::Pin};

use aurora_core::ModelRequestFailure;
use reqwest::{
    Client, StatusCode,
    header::{AUTHORIZATION, CONTENT_TYPE},
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    OpenAiConfig,
    translate::{body_independent_failure, classify_http_response},
    wire::{
        MessageContent, MessagePhase, OutputItem, StructuredResponsesRequest, StructuredText,
        StructuredTextFormat, StructuredTextFormatKind,
    },
};

const MAX_STRUCTURED_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_STRUCTURED_REQUEST_BYTES: usize = 4 * 1024 * 1024;

pub struct StructuredOutputRequest {
    name: String,
    instructions: String,
    input: String,
    schema: Value,
}

impl std::fmt::Debug for StructuredOutputRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StructuredOutputRequest")
            .field("name", &"[REDACTED]")
            .field("instructions", &"[REDACTED]")
            .field("input", &"[REDACTED]")
            .field("schema", &"[REDACTED]")
            .finish()
    }
}

impl StructuredOutputRequest {
    pub fn new(
        name: impl Into<String>,
        instructions: impl Into<String>,
        input: impl Into<String>,
        schema: Value,
    ) -> Result<Self, StructuredOutputValidationError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(StructuredOutputValidationError::BlankName);
        }
        let instructions = instructions.into();
        if instructions.trim().is_empty() {
            return Err(StructuredOutputValidationError::BlankInstructions);
        }
        let input = input.into();
        if input.trim().is_empty() {
            return Err(StructuredOutputValidationError::BlankInput);
        }
        if !schema.is_object() {
            return Err(StructuredOutputValidationError::SchemaMustBeObject);
        }
        let schema_bytes = serde_json::to_vec(&schema)
            .map_err(|_| StructuredOutputValidationError::RequestTooLarge)?;
        let raw_bytes = [
            name.len(),
            instructions.len(),
            input.len(),
            schema_bytes.len(),
        ]
        .into_iter()
        .try_fold(0_usize, usize::checked_add)
        .ok_or(StructuredOutputValidationError::RequestTooLarge)?;
        if raw_bytes > MAX_STRUCTURED_REQUEST_BYTES {
            return Err(StructuredOutputValidationError::RequestTooLarge);
        }
        Ok(Self {
            name,
            instructions,
            input,
            schema,
        })
    }

    fn into_wire(self, model: String) -> StructuredResponsesRequest {
        StructuredResponsesRequest {
            model,
            store: false,
            stream: false,
            instructions: self.instructions,
            input: self.input,
            text: StructuredText {
                format: StructuredTextFormat {
                    kind: StructuredTextFormatKind::JsonSchema,
                    name: self.name,
                    strict: true,
                    schema: self.schema,
                },
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StructuredOutputValidationError {
    #[error("structured output schema name is blank")]
    BlankName,
    #[error("structured output instructions are blank")]
    BlankInstructions,
    #[error("structured output input is blank")]
    BlankInput,
    #[error("structured output schema must be a JSON object")]
    SchemaMustBeObject,
    #[error("structured output request exceeds the byte limit")]
    RequestTooLarge,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StructuredOutputInvocation {
    Output(Value),
    RequestFailure(ModelRequestFailure),
    MalformedOutput,
    ResponseTooLarge,
    RequestTooLarge,
    Cancelled,
}

pub type StructuredOutputFuture =
    Pin<Box<dyn Future<Output = StructuredOutputInvocation> + Send + 'static>>;

pub(crate) fn invoke_structured_owned(
    client: Client,
    config: OpenAiConfig,
    request: StructuredOutputRequest,
    cancellation: CancellationToken,
) -> StructuredOutputFuture {
    Box::pin(async move {
        if cancellation.is_cancelled() {
            return StructuredOutputInvocation::Cancelled;
        }
        let request = request.into_wire(config.model);
        let body = serde_json::to_vec(&request);
        if cancellation.is_cancelled() {
            return StructuredOutputInvocation::Cancelled;
        }
        let body = match body {
            Ok(body) if body.len() <= MAX_STRUCTURED_REQUEST_BYTES => body,
            Ok(_) | Err(_) => return StructuredOutputInvocation::RequestTooLarge,
        };
        let pending = client
            .post(config.endpoint)
            .header(AUTHORIZATION, config.api_key.authorization)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .send();
        tokio::pin!(pending);
        let response = tokio::select! {
            biased;
            () = cancellation.cancelled() => return StructuredOutputInvocation::Cancelled,
            result = &mut pending => match result {
                Ok(response) => response,
                Err(_) => return request_failure(ModelRequestFailure::Transport),
            }
        };
        let status = response.status();
        if let Some(category) = body_independent_failure(status) {
            return request_failure(category);
        }
        let body_failure = if status == StatusCode::TOO_MANY_REQUESTS {
            ModelRequestFailure::RateLimited
        } else {
            ModelRequestFailure::Transport
        };
        let body = match read_bounded_body(response, cancellation, body_failure).await {
            Ok(body) => body,
            Err(invocation) => return invocation,
        };
        if !status.is_success() {
            return match classify_http_response(status, &body) {
                aurora_core::ModelInvocation::RequestFailure(category) => request_failure(category),
                _ => request_failure(ModelRequestFailure::UnsupportedResponse),
            };
        }
        classify_completed_structured_response(&body)
    })
}

async fn read_bounded_body(
    mut response: reqwest::Response,
    cancellation: CancellationToken,
    body_failure: ModelRequestFailure,
) -> Result<Vec<u8>, StructuredOutputInvocation> {
    let mut body = Vec::new();
    loop {
        let pending = response.chunk();
        tokio::pin!(pending);
        let chunk = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(StructuredOutputInvocation::Cancelled),
            result = &mut pending => match result {
                Ok(chunk) => chunk,
                Err(_) => return Err(request_failure(body_failure)),
            }
        };
        let Some(chunk) = chunk else {
            return Ok(body);
        };
        if chunk.len() > MAX_STRUCTURED_RESPONSE_BYTES.saturating_sub(body.len()) {
            return Err(StructuredOutputInvocation::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
}

fn classify_completed_structured_response(body: &[u8]) -> StructuredOutputInvocation {
    let Ok(response) = serde_json::from_slice::<crate::wire::ResponsesResponse>(body) else {
        return StructuredOutputInvocation::MalformedOutput;
    };
    if response.status != "completed"
        || response.error.is_some()
        || response.incomplete_details.is_some()
    {
        return match classify_http_response(StatusCode::OK, body) {
            aurora_core::ModelInvocation::RequestFailure(category) => request_failure(category),
            _ => request_failure(ModelRequestFailure::UnsupportedResponse),
        };
    }
    let mut final_text = None;
    for item in response.output {
        match item {
            OutputItem::Reasoning { .. } => {}
            OutputItem::Message {
                role,
                status,
                content,
                phase,
            } => {
                if final_text.is_some()
                    || role != "assistant"
                    || status != "completed"
                    || !matches!(phase, None | Some(MessagePhase::FinalAnswer))
                {
                    return StructuredOutputInvocation::MalformedOutput;
                }
                let mut text = String::new();
                for part in content {
                    match part {
                        MessageContent::OutputText { text: part } => text.push_str(&part),
                        MessageContent::Refusal { .. } => {
                            return StructuredOutputInvocation::MalformedOutput;
                        }
                    }
                }
                if text.is_empty() {
                    return StructuredOutputInvocation::MalformedOutput;
                }
                final_text = Some(text);
            }
            OutputItem::FunctionCall { .. } => return StructuredOutputInvocation::MalformedOutput,
        }
    }
    let Some(text) = final_text else {
        return StructuredOutputInvocation::MalformedOutput;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return StructuredOutputInvocation::MalformedOutput;
    };
    if !value.is_object() {
        return StructuredOutputInvocation::MalformedOutput;
    }
    StructuredOutputInvocation::Output(value)
}

fn request_failure(category: ModelRequestFailure) -> StructuredOutputInvocation {
    StructuredOutputInvocation::RequestFailure(category)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn failed_success_status_envelopes_preserve_recognized_categories_without_payloads() {
        let private_message = "provider-private-message";
        for (code, expected) in [
            ("invalid_api_key", ModelRequestFailure::Authentication),
            ("authentication_error", ModelRequestFailure::Authentication),
            ("rate_limit_exceeded", ModelRequestFailure::RateLimited),
            ("server_error", ModelRequestFailure::ServiceUnavailable),
            (
                "invalid_request_error",
                ModelRequestFailure::RequestRejected,
            ),
            ("insufficient_quota", ModelRequestFailure::RequestRejected),
            ("billing_not_active", ModelRequestFailure::RequestRejected),
        ] {
            let body = serde_json::to_vec(&json!({
                "status": "failed",
                "error": {"code": code, "message": private_message},
                "output": []
            }))
            .unwrap();
            let invocation = classify_completed_structured_response(&body);
            assert_eq!(
                invocation,
                StructuredOutputInvocation::RequestFailure(expected)
            );
            assert!(!format!("{invocation:?}").contains(private_message));
            assert!(!format!("{invocation:?}").contains(code));
        }
    }

    #[test]
    fn malformed_completed_success_remains_malformed_output() {
        let body = serde_json::to_vec(&json!({
            "status": "completed",
            "error": null,
            "output": [{
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [{"type": "output_text", "text": "not JSON"}]
            }]
        }))
        .unwrap();

        assert_eq!(
            classify_completed_structured_response(&body),
            StructuredOutputInvocation::MalformedOutput
        );
    }
}
