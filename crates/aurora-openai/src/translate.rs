use aurora_core::{ModelInput, ModelInvocation, ModelItem, ModelRequestFailure, ToolRequest};
use reqwest::StatusCode;

use crate::wire::{
    ApiErrorEnvelope, FunctionTool, FunctionToolKind, InputContent, InputItem, MessageContent,
    MessagePhase, MessageRole, OutputItem, ResponsesRequest, ResponsesResponse,
};

pub(crate) fn request_from_model_input(
    model: String,
    input: ModelInput,
) -> Result<ResponsesRequest, ModelRequestFailure> {
    let context = input
        .context
        .into_iter()
        .map(input_item)
        .collect::<Result<Vec<_>, _>>()?;
    let tools = input
        .tools
        .into_iter()
        .map(|definition| FunctionTool {
            kind: FunctionToolKind::Function,
            name: definition.name,
            description: definition.description,
            parameters: definition.input_schema,
            strict: false,
        })
        .collect();
    Ok(ResponsesRequest {
        model,
        store: false,
        stream: false,
        parallel_tool_calls: false,
        input: context,
        tools,
    })
}

fn input_item(item: ModelItem) -> Result<InputItem, ModelRequestFailure> {
    match item {
        ModelItem::UserInput { text } => Ok(InputItem::Message {
            role: MessageRole::User,
            content: vec![InputContent::InputText { text }],
        }),
        ModelItem::AssistantText { text } => Ok(InputItem::Message {
            role: MessageRole::Assistant,
            content: vec![InputContent::InputText { text }],
        }),
        ModelItem::ToolRequest {
            tool_call_id,
            name,
            arguments,
        } => serde_json::to_string(&arguments)
            .map(|arguments| InputItem::FunctionCall {
                call_id: tool_call_id.as_str().to_owned(),
                name,
                arguments,
            })
            .map_err(|_| ModelRequestFailure::UnsupportedResponse),
        ModelItem::ToolResult {
            tool_call_id,
            outcome,
        } => serde_json::to_string(&outcome)
            .map(|output| InputItem::FunctionCallOutput {
                call_id: tool_call_id.as_str().to_owned(),
                output,
            })
            .map_err(|_| ModelRequestFailure::UnsupportedResponse),
    }
}

pub(crate) fn classify_http_response(status: StatusCode, body: &[u8]) -> ModelInvocation {
    if let Some(category) = body_independent_failure(status) {
        return request_failure(category);
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        let code = api_error_code(body);
        let category = code
            .as_deref()
            .filter(|code| is_quota_or_billing(code))
            .map_or(ModelRequestFailure::RateLimited, |_| {
                ModelRequestFailure::RequestRejected
            });
        return request_failure(category);
    }
    let Ok(response) = serde_json::from_slice::<ResponsesResponse>(body) else {
        return request_failure(ModelRequestFailure::UnsupportedResponse);
    };
    if response.status != "completed" {
        return response
            .error
            .as_ref()
            .and_then(|error| error.code.as_deref())
            .and_then(classify_api_error_code)
            .map_or_else(
                || request_failure(ModelRequestFailure::UnsupportedResponse),
                request_failure,
            );
    }
    if response.error.is_some() || response.incomplete_details.is_some() {
        return request_failure(ModelRequestFailure::UnsupportedResponse);
    }
    normalize_completed_output(response.output)
}

pub(crate) fn body_independent_failure(status: StatusCode) -> Option<ModelRequestFailure> {
    if status.is_success() || status == StatusCode::TOO_MANY_REQUESTS {
        return None;
    }
    if status.is_redirection() {
        return Some(ModelRequestFailure::RequestRejected);
    }
    if status == StatusCode::UNAUTHORIZED {
        return Some(ModelRequestFailure::Authentication);
    }
    if status.is_client_error() {
        return Some(ModelRequestFailure::RequestRejected);
    }
    if status.is_server_error() {
        return Some(ModelRequestFailure::ServiceUnavailable);
    }
    Some(ModelRequestFailure::UnsupportedResponse)
}

fn normalize_completed_output(output: Vec<OutputItem>) -> ModelInvocation {
    let mut messages = Vec::new();
    let mut calls = Vec::new();
    let mut saw_reasoning = false;

    for item in output {
        match item {
            OutputItem::Message {
                role,
                status,
                content,
                phase,
            } => messages.push((role, status, content, phase)),
            OutputItem::FunctionCall {
                call_id,
                name,
                arguments,
                status,
            } => calls.push((call_id, name, arguments, status)),
            OutputItem::Reasoning { .. } => saw_reasoning = true,
        }
    }

    match (messages.as_slice(), calls.as_slice()) {
        ([(role, status, content, phase)], []) => {
            normalize_assistant_message(role, status, content, phase.as_ref())
        }
        ([], [(call_id, name, arguments, status)]) if !saw_reasoning => {
            normalize_function_call(call_id, name, arguments, status)
        }
        _ => request_failure(ModelRequestFailure::UnsupportedResponse),
    }
}

fn normalize_assistant_message(
    role: &str,
    status: &str,
    content: &[MessageContent],
    phase: Option<&MessagePhase>,
) -> ModelInvocation {
    if role != "assistant"
        || status != "completed"
        || !matches!(phase, None | Some(MessagePhase::FinalAnswer))
    {
        return request_failure(ModelRequestFailure::UnsupportedResponse);
    }
    let mut text = String::new();
    for part in content {
        match part {
            MessageContent::OutputText { text: part } => text.push_str(part),
            MessageContent::Refusal { .. } => {
                return request_failure(ModelRequestFailure::UnsupportedResponse);
            }
        }
    }
    if text.is_empty() {
        request_failure(ModelRequestFailure::UnsupportedResponse)
    } else {
        ModelInvocation::FinalResponse { text }
    }
}

fn normalize_function_call(
    call_id: &str,
    name: &str,
    arguments: &str,
    status: &str,
) -> ModelInvocation {
    if call_id.is_empty() || name.is_empty() || status != "completed" {
        return request_failure(ModelRequestFailure::UnsupportedResponse);
    }
    let Ok(arguments) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return request_failure(ModelRequestFailure::UnsupportedResponse);
    };
    if !arguments.is_object() {
        return request_failure(ModelRequestFailure::UnsupportedResponse);
    }
    ModelInvocation::ToolRequest(ToolRequest {
        tool_call_id: aurora_core::ToolCallId::new(call_id),
        name: name.to_owned(),
        arguments,
    })
}

fn api_error_code(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<ApiErrorEnvelope>(body)
        .ok()?
        .error
        .code
}

fn classify_api_error_code(code: &str) -> Option<ModelRequestFailure> {
    if is_quota_or_billing(code) {
        return Some(ModelRequestFailure::RequestRejected);
    }
    match code {
        "invalid_api_key" | "authentication_error" => Some(ModelRequestFailure::Authentication),
        "rate_limit_exceeded" => Some(ModelRequestFailure::RateLimited),
        "server_error" => Some(ModelRequestFailure::ServiceUnavailable),
        "invalid_request_error" => Some(ModelRequestFailure::RequestRejected),
        _ => None,
    }
}

fn is_quota_or_billing(code: &str) -> bool {
    let code = code.to_ascii_lowercase();
    code.contains("quota") || code.contains("billing")
}

fn request_failure(category: ModelRequestFailure) -> ModelInvocation {
    ModelInvocation::RequestFailure(category)
}

#[cfg(test)]
mod tests {
    use aurora_core::{
        ModelInput, ModelInvocation, ModelItem, ModelRequestFailure, ToolCallId, ToolDefinition,
        ToolEffect, ToolOutcome, ToolRequest,
    };
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;

    fn definition() -> ToolDefinition {
        ToolDefinition {
            name: "fixture.read".to_owned(),
            description: "Read one value from the fixture data.".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "key": {"type": "string"}
                },
                "required": ["key"]
            }),
            effect: ToolEffect::ReadOnly,
        }
    }

    fn classify(status: u16, value: serde_json::Value) -> ModelInvocation {
        classify_http_response(
            StatusCode::from_u16(status).expect("test status is valid"),
            &serde_json::to_vec(&value).expect("test response serializes"),
        )
    }

    #[test]
    fn direct_final_request_is_explicitly_stateless_and_non_streaming() {
        let request = request_from_model_input(
            "fixture-model".to_owned(),
            ModelInput {
                context: vec![ModelItem::UserInput {
                    text: "answer".to_owned(),
                }],
                tools: vec![definition()],
            },
        )
        .expect("normalized input serializes");

        assert_eq!(
            serde_json::to_value(request).expect("wire request serializes"),
            json!({
                "model": "fixture-model",
                "store": false,
                "stream": false,
                "parallel_tool_calls": false,
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [{
                        "type": "input_text",
                        "text": "answer"
                    }]
                }],
                "tools": [{
                    "type": "function",
                    "name": "fixture.read",
                    "description": "Read one value from the fixture data.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "key": {"type": "string"}
                        },
                        "required": ["key"]
                    },
                    "strict": false
                }]
            })
        );
    }

    #[test]
    fn second_request_rebuilds_only_normalized_committed_context() {
        let request = request_from_model_input(
            "fixture-model".to_owned(),
            ModelInput {
                context: vec![
                    ModelItem::UserInput {
                        text: "look up alpha".to_owned(),
                    },
                    ModelItem::ToolRequest {
                        tool_call_id: ToolCallId::new("call-1"),
                        name: "fixture.read".to_owned(),
                        arguments: json!({"key": "alpha"}),
                    },
                    ModelItem::ToolResult {
                        tool_call_id: ToolCallId::new("call-1"),
                        outcome: ToolOutcome::Success {
                            value: json!({"value": "fixture"}),
                        },
                    },
                ],
                tools: vec![definition()],
            },
        )
        .expect("normalized input serializes");
        let value = serde_json::to_value(request).expect("wire request serializes");

        assert_eq!(
            value["input"],
            json!([
                {
                    "type": "message",
                    "role": "user",
                    "content": [{
                        "type": "input_text",
                        "text": "look up alpha"
                    }]
                },
                {
                    "type": "function_call",
                    "call_id": "call-1",
                    "name": "fixture.read",
                    "arguments": "{\"key\":\"alpha\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call-1",
                    "output": "{\"type\":\"success\",\"detail\":{\"value\":{\"value\":\"fixture\"}}}"
                }
            ])
        );
        let object = value.as_object().expect("request is an object");
        for forbidden in [
            "previous_response_id",
            "conversation",
            "reasoning",
            "include",
            "prompt_cache_key",
        ] {
            assert!(!object.contains_key(forbidden));
        }
    }

    #[test]
    fn prior_assistant_text_becomes_an_assistant_input_message() {
        let request = request_from_model_input(
            "fixture-model".to_owned(),
            ModelInput {
                context: vec![ModelItem::AssistantText {
                    text: "prior answer".to_owned(),
                }],
                tools: Vec::new(),
            },
        )
        .expect("normalized input serializes");

        assert_eq!(
            serde_json::to_value(request).expect("wire request serializes")["input"],
            json!([{
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "input_text",
                    "text": "prior answer"
                }]
            }])
        );
    }

    #[test]
    fn completed_assistant_parts_form_one_non_empty_final_response() {
        assert_eq!(
            classify(
                200,
                json!({
                    "status": "completed",
                    "error": null,
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "status": "completed",
                        "phase": "final_answer",
                        "content": [
                            {"type": "output_text", "text": "hel"},
                            {"type": "output_text", "text": "lo"}
                        ]
                    }]
                }),
            ),
            ModelInvocation::FinalResponse {
                text: "hello".to_owned(),
            }
        );
    }

    #[test]
    fn commentary_phase_is_not_a_terminal_assistant_message() {
        assert_eq!(
            classify(
                200,
                json!({
                    "status": "completed",
                    "error": null,
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "status": "completed",
                        "phase": "commentary",
                        "content": [{"type": "output_text", "text": "working"}]
                    }]
                }),
            ),
            ModelInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse)
        );
    }

    #[test]
    fn unknown_message_phase_is_unsupported() {
        assert_eq!(
            classify(
                200,
                json!({
                    "status": "completed",
                    "error": null,
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "status": "completed",
                        "phase": "provider_private_phase",
                        "content": [{"type": "output_text", "text": "done"}]
                    }]
                }),
            ),
            ModelInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse)
        );
    }

    #[test]
    fn malformed_message_phase_is_unsupported() {
        assert_eq!(
            classify(
                200,
                json!({
                    "status": "completed",
                    "error": null,
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "status": "completed",
                        "phase": {"provider": "private"},
                        "content": [{"type": "output_text", "text": "done"}]
                    }]
                }),
            ),
            ModelInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse)
        );
    }

    #[test]
    fn completed_response_with_incomplete_details_is_unsupported() {
        assert_eq!(
            classify(
                200,
                json!({
                    "status": "completed",
                    "error": null,
                    "incomplete_details": {"reason": "max_output_tokens"},
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "status": "completed",
                        "phase": "final_answer",
                        "content": [{"type": "output_text", "text": "truncated"}]
                    }]
                }),
            ),
            ModelInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse)
        );
    }

    #[test]
    fn one_completed_function_call_normalizes_to_one_tool_request() {
        assert_eq!(
            classify(
                200,
                json!({
                    "status": "completed",
                    "error": null,
                    "output": [{
                        "type": "function_call",
                        "call_id": "call-1",
                        "name": "fixture.read",
                        "arguments": "{\"key\":\"alpha\"}",
                        "status": "completed"
                    }]
                })
            ),
            ModelInvocation::ToolRequest(ToolRequest {
                tool_call_id: aurora_core::ToolCallId::new("call-1"),
                name: "fixture.read".to_owned(),
                arguments: json!({"key": "alpha"}),
            })
        );
    }

    #[test]
    fn terminal_reasoning_may_be_discarded_after_whole_output_is_known() {
        for output in [
            json!([
                {"type": "reasoning", "id": "reasoning-private"},
                {
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "phase": "final_answer",
                    "content": [{"type": "output_text", "text": "done"}]
                }
            ]),
            json!([
                {
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "phase": "final_answer",
                    "content": [{"type": "output_text", "text": "done"}]
                },
                {"type": "reasoning", "id": "reasoning-private"}
            ]),
        ] {
            assert_eq!(
                classify(
                    200,
                    json!({
                        "status": "completed",
                        "error": null,
                        "output": output
                    })
                ),
                ModelInvocation::FinalResponse {
                    text: "done".to_owned(),
                }
            );
        }
    }

    #[test]
    fn unsupported_completed_shapes_fail_explicitly() {
        let function = json!({
            "type": "function_call",
            "call_id": "call-1",
            "name": "fixture.read",
            "arguments": "{\"key\":\"alpha\"}",
            "status": "completed"
        });
        let message = json!({
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": "done"}]
        });
        let cases = [
            json!([function.clone(), function.clone()]),
            json!([message.clone(), function.clone()]),
            json!([function.clone(), message.clone()]),
            json!([
                {"type": "reasoning", "id": "private"},
                function.clone()
            ]),
            json!([{
                "type": "function_call",
                "call_id": "",
                "name": "fixture.read",
                "arguments": "{\"key\":\"alpha\"}",
                "status": "completed"
            }]),
            json!([{
                "type": "function_call",
                "call_id": "call-1",
                "name": "",
                "arguments": "{\"key\":\"alpha\"}",
                "status": "completed"
            }]),
            json!([{
                "type": "function_call",
                "call_id": "call-1",
                "name": "fixture.read",
                "arguments": "not-json",
                "status": "completed"
            }]),
            json!([{
                "type": "function_call",
                "call_id": "call-1",
                "name": "fixture.read",
                "arguments": "[1,2,3]",
                "status": "completed"
            }]),
            json!([{
                "type": "function_call",
                "call_id": "call-1",
                "name": "fixture.read",
                "arguments": "{\"key\":\"alpha\"}",
                "status": "in_progress"
            }]),
            json!([message.clone(), message]),
            json!([{
                "type": "unknown_output_item",
                "value": "private"
            }]),
            json!([{
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "unknown_content",
                    "value": "private"
                }]
            }]),
        ];

        for output in cases {
            assert_eq!(
                classify(
                    200,
                    json!({
                        "status": "completed",
                        "error": null,
                        "output": output
                    })
                ),
                ModelInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse)
            );
        }
    }

    proptest! {
        #[test]
        fn output_text_parts_are_combined_in_provider_order(
            parts in prop::collection::vec("[a-zA-Z0-9 ]{1,16}", 1..8),
        ) {
            let expected = parts.concat();
            let content = parts
                .into_iter()
                .map(|text| json!({"type": "output_text", "text": text}))
                .collect::<Vec<_>>();
            let invocation = classify(
                200,
                json!({
                    "status": "completed",
                    "error": null,
                    "output": [{
                        "type": "message",
                        "role": "assistant",
                        "status": "completed",
                        "content": content
                    }]
                }),
            );

            prop_assert_eq!(
                invocation,
                ModelInvocation::FinalResponse { text: expected }
            );
        }
    }

    #[test]
    fn empty_ambiguous_or_refusal_outputs_are_unsupported() {
        for output in [
            json!([]),
            json!([{
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [{"type": "output_text", "text": ""}]
            }]),
            json!([{
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [{"type": "refusal", "refusal": "no"}]
            }]),
        ] {
            assert_eq!(
                classify(
                    200,
                    json!({
                        "status": "completed",
                        "error": null,
                        "output": output
                    })
                ),
                ModelInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse)
            );
        }
    }

    #[test]
    fn http_and_api_error_categories_are_provider_neutral() {
        let cases = [
            (
                401,
                json!({"error": {"code": "invalid_api_key", "message": "secret"}}),
                ModelRequestFailure::Authentication,
            ),
            (
                429,
                json!({"error": {"code": "insufficient_quota", "message": "secret"}}),
                ModelRequestFailure::RequestRejected,
            ),
            (
                429,
                json!({"error": {"code": "billing_hard_limit_reached", "message": "secret"}}),
                ModelRequestFailure::RequestRejected,
            ),
            (
                429,
                json!({"error": {"code": "rate_limit_exceeded", "message": "secret"}}),
                ModelRequestFailure::RateLimited,
            ),
            (
                302,
                json!({"error": {"code": "redirect", "message": "secret"}}),
                ModelRequestFailure::RequestRejected,
            ),
            (
                400,
                json!({"error": {"code": "invalid_request_error", "message": "secret"}}),
                ModelRequestFailure::RequestRejected,
            ),
            (
                503,
                json!({"error": {"code": "server_error", "message": "secret"}}),
                ModelRequestFailure::ServiceUnavailable,
            ),
        ];

        for (status, body, expected) in cases {
            let invocation = classify(status, body);
            assert_eq!(invocation, ModelInvocation::RequestFailure(expected));
            assert!(!format!("{invocation:?}").contains("secret"));
        }
    }

    #[test]
    fn non_completed_objects_use_only_recognized_error_codes() {
        let cases = [
            ("invalid_api_key", ModelRequestFailure::Authentication),
            ("authentication_error", ModelRequestFailure::Authentication),
            ("insufficient_quota", ModelRequestFailure::RequestRejected),
            ("billing_not_active", ModelRequestFailure::RequestRejected),
            ("rate_limit_exceeded", ModelRequestFailure::RateLimited),
            ("server_error", ModelRequestFailure::ServiceUnavailable),
            (
                "invalid_request_error",
                ModelRequestFailure::RequestRejected,
            ),
            (
                "unknown_provider_code",
                ModelRequestFailure::UnsupportedResponse,
            ),
        ];
        for (code, expected) in cases {
            let invocation = classify(
                200,
                json!({
                    "status": "failed",
                    "output": [],
                    "error": {"code": code, "message": "provider-private"}
                }),
            );
            assert_eq!(invocation, ModelInvocation::RequestFailure(expected));
            assert!(!format!("{invocation:?}").contains("provider-private"));
        }
        assert_eq!(
            classify(
                200,
                json!({
                    "status": "incomplete",
                    "output": [],
                    "error": null,
                    "incomplete_details": {"reason": "max_output_tokens"}
                })
            ),
            ModelInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse)
        );
    }

    #[test]
    fn invalid_success_bytes_are_unsupported_without_payload_detail() {
        let invocation = classify_http_response(StatusCode::OK, b"provider secret");
        assert_eq!(
            invocation,
            ModelInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse)
        );
        assert!(!format!("{invocation:?}").contains("provider secret"));
    }
}
