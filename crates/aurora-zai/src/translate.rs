use aurora_core::{ModelInput, ModelInvocation, ModelItem, ModelRequestFailure};

use crate::{
    GLM_5_3, MAX_OUTPUT_TOKENS, ReasoningEffort, ZaiJsonObjectInvocation, ZaiJsonObjectRequest,
    wire::{
        ChatCompletionRequest, ChatCompletionResponse, ChatMessage, MessageRole, ResponseFormat,
        ResponseFormatKind, Thinking, ThinkingKind,
    },
};

pub(super) fn plain_request(
    input: ModelInput,
    effort: ReasoningEffort,
) -> Result<ChatCompletionRequest, ModelRequestFailure> {
    if !input.tools.is_empty() {
        return Err(ModelRequestFailure::RequestRejected);
    }
    let messages = input
        .context
        .into_iter()
        .map(|item| match item {
            ModelItem::UserInput { text } => Ok(ChatMessage {
                role: MessageRole::User,
                content: text,
            }),
            ModelItem::AssistantText { text } => Ok(ChatMessage {
                role: MessageRole::Assistant,
                content: text,
            }),
            ModelItem::ToolRequest { .. } | ModelItem::ToolResult { .. } => {
                Err(ModelRequestFailure::RequestRejected)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ChatCompletionRequest {
        model: GLM_5_3,
        messages,
        thinking: Thinking {
            kind: ThinkingKind::Enabled,
            clear_thinking: true,
        },
        reasoning_effort: effort,
        max_tokens: MAX_OUTPUT_TOKENS,
        stream: false,
        response_format: None,
    })
}

pub(super) fn json_object_request(
    request: ZaiJsonObjectRequest,
    effort: ReasoningEffort,
) -> ChatCompletionRequest {
    let (instructions, input, expected_shape) = request.into_prompt_parts();
    let system = format!(
        "{}\n\nExpected top-level JSON object shape:\n{}",
        instructions, expected_shape
    );
    ChatCompletionRequest {
        model: GLM_5_3,
        messages: vec![
            ChatMessage {
                role: MessageRole::System,
                content: system,
            },
            ChatMessage {
                role: MessageRole::User,
                content: input,
            },
        ],
        thinking: Thinking {
            kind: ThinkingKind::Enabled,
            clear_thinking: true,
        },
        reasoning_effort: effort,
        max_tokens: MAX_OUTPUT_TOKENS,
        stream: false,
        response_format: Some(ResponseFormat {
            kind: ResponseFormatKind::JsonObject,
        }),
    }
}

pub(super) fn classify_plain_response(body: &[u8]) -> ModelInvocation {
    let Ok(response) = serde_json::from_slice::<ChatCompletionResponse>(body) else {
        return ModelInvocation::MalformedOutput;
    };
    let [choice] = response.choices.as_slice() else {
        return ModelInvocation::MalformedOutput;
    };
    if choice
        .message
        .tool_calls
        .as_ref()
        .is_some_and(|calls| !calls.is_empty())
    {
        return ModelInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse);
    }
    if choice.finish_reason != "stop" || choice.message.role.as_deref() != Some("assistant") {
        return ModelInvocation::MalformedOutput;
    }
    let Some(content) = choice.message.content.as_ref() else {
        return ModelInvocation::MalformedOutput;
    };
    if content.trim().is_empty() {
        return ModelInvocation::MalformedOutput;
    }
    ModelInvocation::FinalResponse {
        text: content.clone(),
    }
}

pub(super) fn classify_json_object_response(body: &[u8]) -> ZaiJsonObjectInvocation {
    let Ok(response) = serde_json::from_slice::<ChatCompletionResponse>(body) else {
        return ZaiJsonObjectInvocation::MalformedOutput;
    };
    let [choice] = response.choices.as_slice() else {
        return ZaiJsonObjectInvocation::MalformedOutput;
    };
    if choice
        .message
        .tool_calls
        .as_ref()
        .is_some_and(|calls| !calls.is_empty())
    {
        return ZaiJsonObjectInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse);
    }
    if choice.finish_reason != "stop" || choice.message.role.as_deref() != Some("assistant") {
        return ZaiJsonObjectInvocation::MalformedOutput;
    }
    let Some(content) = choice.message.content.as_ref() else {
        return ZaiJsonObjectInvocation::MalformedOutput;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return ZaiJsonObjectInvocation::MalformedOutput;
    };
    if !value.is_object() {
        return ZaiJsonObjectInvocation::MalformedOutput;
    }
    ZaiJsonObjectInvocation::Output(value)
}

#[cfg(test)]
mod tests {
    use aurora_core::{
        ModelInput, ModelInvocation, ModelItem, ModelRequestFailure, ToolCallId, ToolDefinition,
        ToolEffect, ToolOutcome,
    };
    use serde_json::json;

    use super::*;

    #[test]
    fn normalized_visible_context_serializes_in_order_and_clears_thinking() {
        let request = plain_request(
            ModelInput {
                context: vec![
                    ModelItem::UserInput {
                        text: "question".into(),
                    },
                    ModelItem::AssistantText {
                        text: "answer".into(),
                    },
                ],
                tools: Vec::new(),
            },
            crate::ReasoningEffort::High,
        )
        .expect("visible context is supported");
        let value = serde_json::to_value(request).expect("request serializes");
        assert_eq!(value["model"], "glm-5.3");
        assert_eq!(
            value["messages"][0],
            json!({"role":"user","content":"question"})
        );
        assert_eq!(
            value["messages"][1],
            json!({"role":"assistant","content":"answer"})
        );
        assert_eq!(
            value["thinking"],
            json!({"type":"enabled","clear_thinking":true})
        );
        assert_eq!(value["reasoning_effort"], "high");
        assert_eq!(value["max_tokens"], 4096);
        assert_eq!(value["stream"], false);
        assert!(value.get("response_format").is_none());
        assert!(!value.to_string().contains("reasoning_content"));
    }

    #[test]
    fn every_reasoning_effort_serializes_the_exact_plain_wire_request() {
        for (effort, serialized) in [
            (crate::ReasoningEffort::Low, "low"),
            (crate::ReasoningEffort::High, "high"),
            (crate::ReasoningEffort::Max, "max"),
        ] {
            let request = plain_request(
                ModelInput {
                    context: vec![ModelItem::UserInput {
                        text: "question".into(),
                    }],
                    tools: Vec::new(),
                },
                effort,
            )
            .expect("plain visible input is supported");
            assert_eq!(
                serde_json::to_value(request).expect("request serializes"),
                json!({
                    "model":"glm-5.3",
                    "messages":[{"role":"user","content":"question"}],
                    "thinking":{"type":"enabled","clear_thinking":true},
                    "reasoning_effort":serialized,
                    "max_tokens":4096,
                    "stream":false
                })
            );
        }
    }

    #[test]
    fn json_object_request_serializes_system_shape_and_json_mode() {
        let request = json_object_request(
            ZaiJsonObjectRequest::new(
                "Return the required object.",
                "fixture input",
                json!({"status":"string"}),
            )
            .unwrap(),
            crate::ReasoningEffort::Low,
        );
        let value = serde_json::to_value(request).expect("request serializes");
        assert_eq!(value["model"], "glm-5.3");
        assert_eq!(
            value["messages"][0],
            json!({
                "role":"system",
                "content":"Return the required object.\n\nExpected top-level JSON object shape:\n{\"status\":\"string\"}"
            })
        );
        assert_eq!(
            value["messages"][1],
            json!({"role":"user","content":"fixture input"})
        );
        assert_eq!(
            value["thinking"],
            json!({"type":"enabled","clear_thinking":true})
        );
        assert_eq!(value["reasoning_effort"], "low");
        assert_eq!(value["max_tokens"], 4096);
        assert_eq!(value["stream"], false);
        assert_eq!(value["response_format"], json!({"type":"json_object"}));
    }

    #[test]
    fn any_tool_surface_is_rejected_before_wire_construction() {
        let cases = [
            ModelInput {
                context: Vec::new(),
                tools: vec![ToolDefinition {
                    name: "search".into(),
                    description: "Search fixture content.".into(),
                    input_schema: json!({"type":"object"}),
                    effect: ToolEffect::ReadOnly,
                }],
            },
            ModelInput {
                context: vec![ModelItem::ToolRequest {
                    tool_call_id: ToolCallId::new("call-1"),
                    name: "search".into(),
                    arguments: json!({}),
                }],
                tools: Vec::new(),
            },
            ModelInput {
                context: vec![ModelItem::ToolResult {
                    tool_call_id: ToolCallId::new("call-1"),
                    outcome: ToolOutcome::Success {
                        value: json!({"result":"fixture"}),
                    },
                }],
                tools: Vec::new(),
            },
        ];
        for input in cases {
            assert!(matches!(
                plain_request(input, crate::ReasoningEffort::Low),
                Err(ModelRequestFailure::RequestRejected)
            ));
        }
    }

    fn response(message: serde_json::Value, finish_reason: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({"choices":[{"message":message,"finish_reason":finish_reason}]}))
            .unwrap()
    }

    #[test]
    fn visible_answer_is_returned_and_private_reasoning_is_ignored() {
        let body = response(
            json!({"role":"assistant","content":"visible answer","reasoning_content":"private chain","tool_calls":null}),
            "stop",
        );
        assert_eq!(
            classify_plain_response(&body),
            ModelInvocation::FinalResponse {
                text: "visible answer".into()
            }
        );
        assert!(!format!("{:?}", classify_plain_response(&body)).contains("private chain"));
    }

    #[test]
    fn reasoning_without_visible_answer_is_malformed() {
        let body = response(
            json!({"role":"assistant","content":null,"reasoning_content":"private chain"}),
            "stop",
        );
        assert_eq!(
            classify_plain_response(&body),
            ModelInvocation::MalformedOutput
        );
    }

    #[test]
    fn blank_content_is_malformed() {
        assert_eq!(
            classify_plain_response(&response(
                json!({"role":"assistant","content":"  "}),
                "stop"
            )),
            ModelInvocation::MalformedOutput
        );
    }

    #[test]
    fn multiple_choices_are_malformed() {
        let body = serde_json::to_vec(&json!({"choices":[{"message":{"role":"assistant","content":"a"},"finish_reason":"stop"},{"message":{"role":"assistant","content":"b"},"finish_reason":"stop"}]})).unwrap();
        assert_eq!(
            classify_plain_response(&body),
            ModelInvocation::MalformedOutput
        );
    }

    #[test]
    fn non_assistant_role_is_malformed() {
        assert_eq!(
            classify_plain_response(&response(json!({"role":"user","content":"a"}), "stop")),
            ModelInvocation::MalformedOutput
        );
    }

    #[test]
    fn non_stop_finish_is_malformed() {
        assert_eq!(
            classify_plain_response(&response(
                json!({"role":"assistant","content":"a"}),
                "length"
            )),
            ModelInvocation::MalformedOutput
        );
    }

    #[test]
    fn unexpected_finish_is_malformed() {
        assert_eq!(
            classify_plain_response(&response(
                json!({"role":"assistant","content":"a"}),
                "other"
            )),
            ModelInvocation::MalformedOutput
        );
    }

    #[test]
    fn tool_calls_are_unsupported() {
        assert_eq!(
            classify_plain_response(&response(
                json!({"role":"assistant","content":"a","tool_calls":[{"id":"x"}]}),
                "stop"
            )),
            ModelInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse)
        );
    }

    #[test]
    fn malformed_json_is_malformed() {
        assert_eq!(
            classify_plain_response(b"not json"),
            ModelInvocation::MalformedOutput
        );
    }

    #[test]
    fn json_object_classification_requires_one_stopped_assistant_choice() {
        let malformed = [
            serde_json::to_vec(&json!({"choices":[]})).unwrap(),
            serde_json::to_vec(&json!({"choices":[{"message":{"role":"assistant","content":"{}"},"finish_reason":"stop"},{"message":{"role":"assistant","content":"{}"},"finish_reason":"stop"}]})).unwrap(),
            response(json!({"role":"user","content":"{}"}), "stop"),
            response(json!({"role":"assistant","content":"{}"}), "length"),
            response(json!({"role":"assistant","content":null}), "stop"),
        ];
        for body in malformed {
            assert_eq!(
                classify_json_object_response(&body),
                ZaiJsonObjectInvocation::MalformedOutput
            );
        }
    }

    #[test]
    fn json_object_classification_rejects_non_objects_and_invalid_json() {
        for content in ["not json", "[]", "true", "42", "\"text\"", ""] {
            assert_eq!(
                classify_json_object_response(&response(
                    json!({"role":"assistant","content":content}),
                    "stop"
                )),
                ZaiJsonObjectInvocation::MalformedOutput
            );
        }
    }

    #[test]
    fn json_object_tool_calls_are_unsupported() {
        assert_eq!(
            classify_json_object_response(&response(
                json!({"role":"assistant","content":"{}","tool_calls":[{"id":"x"}]}),
                "stop"
            )),
            ZaiJsonObjectInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse)
        );
    }

    #[test]
    fn json_object_classification_does_not_validate_prompted_shape() {
        assert_eq!(
            classify_json_object_response(&response(
                json!({"role":"assistant","content":"{\"different\":1}"}),
                "stop"
            )),
            ZaiJsonObjectInvocation::Output(json!({"different":1}))
        );
    }

    proptest::proptest! {
        #[test]
        fn private_reasoning_never_changes_the_visible_invocation(reasoning in ".{0,2048}") {
            let body = serde_json::to_vec(&json!({"choices":[{"message":{"role":"assistant","content":"visible","reasoning_content":reasoning},"finish_reason":"stop"}]})).unwrap();
            proptest::prop_assert_eq!(classify_plain_response(&body), ModelInvocation::FinalResponse { text: "visible".into() });
        }


        #[test]
        fn private_reasoning_does_not_change_json_object_output(reasoning in ".{0,2048}") {
            let body = serde_json::to_vec(&json!({"choices":[{"message":{"role":"assistant","content":"{\"visible\":true}","reasoning_content":reasoning},"finish_reason":"stop"}]})).unwrap();
            proptest::prop_assert_eq!(classify_json_object_response(&body), ZaiJsonObjectInvocation::Output(json!({"visible":true})));
        }
    }
}
