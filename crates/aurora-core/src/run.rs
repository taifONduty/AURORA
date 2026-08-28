use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(String);

impl RunId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StepId(u64);

impl StepId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for StepId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(String);

impl ToolCallId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ToolCallId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventSeq(u64);

impl EventSeq {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for EventSeq {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLimits {
    pub max_model_steps: u32,
    pub max_tool_executions: u32,
    pub model_timeout_ms: u64,
    pub tool_timeout_ms: u64,
    pub shutdown_grace_period_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunLifecycle {
    Active,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetKind {
    ModelSteps,
    ToolExecutions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRequestFailure {
    Authentication,
    RateLimited,
    RequestRejected,
    ServiceUnavailable,
    Transport,
    UnsupportedResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFailure {
    Ordinary,
    Request(ModelRequestFailure),
    Timeout,
    MalformedOutput,
    ChildPanicked,
    ChildShutdown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "detail", rename_all = "snake_case")]
pub enum FinishReason {
    Completed,
    Cancelled,
    BudgetExhausted(BudgetKind),
    Failed(ModelFailure),
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEffect {
    ReadOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingOperation {
    Model {
        step_id: StepId,
    },
    Tool {
        step_id: StepId,
        tool_call_id: ToolCallId,
        name: String,
        arguments: serde_json::Value,
        execution_started: bool,
    },
}

impl PendingOperation {
    pub fn execution_started(&self) -> bool {
        matches!(
            self,
            Self::Tool {
                execution_started: true,
                ..
            }
        )
    }

    pub fn tool_call_id(&self) -> Option<&ToolCallId> {
        match self {
            Self::Model { .. } => None,
            Self::Tool { tool_call_id, .. } => Some(tool_call_id),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunView {
    pub run_id: RunId,
    pub last_sequence: EventSeq,
    pub lifecycle: RunLifecycle,
    pub finish_reason: Option<FinishReason>,
    pub request: String,
    pub limits: RunLimits,
    pub model_steps_consumed: u32,
    pub tool_executions_consumed: u32,
    pub pending_operation: Option<PendingOperation>,
    pub model_context: Vec<crate::model::ModelItem>,
    pub final_response: Option<String>,
}
