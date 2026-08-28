use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub(crate) struct ResponsesRequest {
    pub(crate) model: String,
    pub(crate) store: bool,
    pub(crate) stream: bool,
    pub(crate) parallel_tool_calls: bool,
    pub(crate) input: Vec<InputItem>,
    pub(crate) tools: Vec<FunctionTool>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum InputItem {
    Message {
        role: MessageRole,
        content: Vec<InputContent>,
    },
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    FunctionCallOutput {
        call_id: String,
        output: String,
    },
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MessageRole {
    User,
    Assistant,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum InputContent {
    InputText { text: String },
}

#[derive(Serialize)]
pub(crate) struct FunctionTool {
    #[serde(rename = "type")]
    pub(crate) kind: FunctionToolKind,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) parameters: serde_json::Value,
    pub(crate) strict: bool,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FunctionToolKind {
    Function,
}

#[derive(Deserialize)]
pub(crate) struct ResponsesResponse {
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) output: Vec<OutputItem>,
    #[serde(default)]
    pub(crate) error: Option<ApiError>,
    #[serde(default)]
    pub(crate) incomplete_details: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MessagePhase {
    Commentary,
    FinalAnswer,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum OutputItem {
    Message {
        role: String,
        status: String,
        content: Vec<MessageContent>,
        #[serde(default)]
        phase: Option<MessagePhase>,
    },
    #[allow(
        dead_code,
        reason = "direct-final classification rejects tool continuations as a whole"
    )]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
        status: String,
    },
    Reasoning {
        #[serde(flatten)]
        _fields: BTreeMap<String, serde_json::Value>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum MessageContent {
    OutputText {
        text: String,
    },
    Refusal {
        #[serde(default)]
        _refusal: String,
    },
}

#[derive(Deserialize)]
pub(crate) struct ApiError {
    #[serde(default)]
    pub(crate) code: Option<String>,
    #[serde(default, rename = "message")]
    pub(crate) _message: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ApiErrorEnvelope {
    pub(crate) error: ApiError,
}
