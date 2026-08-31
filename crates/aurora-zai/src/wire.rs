use serde::{Deserialize, Serialize, de::IgnoredAny};
use serde_json::Value;

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Serialize)]
pub(super) struct ChatMessage {
    pub(super) role: MessageRole,
    pub(super) content: String,
}

#[derive(Serialize)]
pub(super) struct Thinking {
    #[serde(rename = "type")]
    pub(super) kind: ThinkingKind,
    pub(super) clear_thinking: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum ThinkingKind {
    Enabled,
}

#[derive(Serialize)]
pub(super) struct ResponseFormat {
    #[serde(rename = "type")]
    pub(super) kind: ResponseFormatKind,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ResponseFormatKind {
    JsonObject,
}

#[derive(Serialize)]
pub(super) struct ChatCompletionRequest {
    pub(super) model: &'static str,
    pub(super) messages: Vec<ChatMessage>,
    pub(super) thinking: Thinking,
    pub(super) reasoning_effort: crate::ReasoningEffort,
    pub(super) max_tokens: u32,
    pub(super) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) response_format: Option<ResponseFormat>,
}

#[derive(Deserialize)]
pub(super) struct ChatCompletionResponse {
    pub(super) choices: Vec<Choice>,
}

#[derive(Deserialize)]
pub(super) struct Choice {
    pub(super) message: AssistantMessage,
    pub(super) finish_reason: String,
}

#[derive(Deserialize)]
pub(super) struct AssistantMessage {
    pub(super) role: Option<String>,
    pub(super) content: Option<String>,
    #[serde(default)]
    pub(super) tool_calls: Option<Vec<Value>>,
    #[serde(default, rename = "reasoning_content")]
    pub(super) _reasoning_content: Option<IgnoredAny>,
}
