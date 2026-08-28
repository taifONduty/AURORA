use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::ModelItem;
use crate::run::{
    BudgetKind, EventSeq, FinishReason, ModelFailure, ModelRequestFailure, PendingOperation, RunId,
    RunLifecycle, RunLimits, RunView, StepId, ToolCallId, ToolEffect,
};

const PHASE_1C_SCHEMA_VERSION: u32 = 1;
pub const SCHEMA_VERSION: u32 = 2;

pub(crate) fn is_supported_schema(version: u32) -> bool {
    matches!(version, PHASE_1C_SCHEMA_VERSION | SCHEMA_VERSION)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRequest {
    pub tool_call_id: ToolCallId,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "detail", rename_all = "snake_case")]
pub enum ModelOutcome {
    FinalResponse { text: String },
    ToolRequest(ToolRequest),
    RequestFailure(ModelRequestFailure),
    Failed,
    MalformedOutput,
    TimedOut,
    Cancelled,
    ChildPanicked,
    ChildShutdownFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "detail", rename_all = "snake_case")]
pub enum ToolOutcome {
    Success { value: serde_json::Value },
    UnknownTool,
    InvalidArguments,
    Denied,
    Failed,
    TimedOut,
    Cancelled,
    ChildPanicked,
    ChildShutdownFailed,
}

impl ToolOutcome {
    fn requires_execution(&self) -> bool {
        matches!(
            self,
            Self::Success { .. }
                | Self::Failed
                | Self::TimedOut
                | Self::Cancelled
                | Self::ChildPanicked
                | Self::ChildShutdownFailed
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomainEvent {
    RunStarted {
        request: String,
        limits: RunLimits,
    },
    ModelRequestStarted {
        step_id: StepId,
    },
    ModelRequestFinished {
        step_id: StepId,
        outcome: ModelOutcome,
    },
    ToolExecutionStarted {
        step_id: StepId,
        tool_call_id: ToolCallId,
        name: String,
        arguments: serde_json::Value,
        effect: ToolEffect,
    },
    ToolCallResolved {
        step_id: StepId,
        tool_call_id: ToolCallId,
        outcome: ToolOutcome,
    },
    RunFinished {
        reason: FinishReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub schema_version: u32,
    pub sequence: EventSeq,
    pub run_id: RunId,
    pub observed_at: String,
    pub event: DomainEvent,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProjectionError {
    #[error("event history is empty")]
    EmptyHistory,
    #[error("unsupported event schema version {0}")]
    UnsupportedSchema(u32),
    #[error("event {sequence} changes schema version from {expected} to {actual}")]
    SchemaVersionChanged {
        sequence: u64,
        expected: u32,
        actual: u32,
    },
    #[error("expected event sequence {expected}, found {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("event {sequence} belongs to run {actual}, expected {expected}")]
    RunMismatch {
        sequence: u64,
        expected: RunId,
        actual: RunId,
    },
    #[error("event {sequence} has an invalid observational timestamp")]
    InvalidTimestamp { sequence: u64 },
    #[error("event {sequence} appears after the run became terminal")]
    AfterTerminal { sequence: u64 },
    #[error("event {sequence} violates run ordering: {message}")]
    InvalidTransition { sequence: u64, message: String },
    #[error("step identifier {0} is reused")]
    DuplicateStep(StepId),
    #[error("tool-call identifier {0} is reused")]
    DuplicateToolCall(ToolCallId),
    #[error("tool call {0} has an execution outcome without an execution-start event")]
    OutcomeWithoutExecution(ToolCallId),
    #[error("tool call {0} has a non-execution outcome after execution started")]
    NonExecutionOutcomeAfterStart(ToolCallId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExpectedFinish {
    Completed,
    Cancelled,
    Failed(ModelFailure),
}

pub(crate) struct RunProjection {
    view: Option<RunView>,
    schema_version: Option<u32>,
    seen_steps: HashSet<StepId>,
    seen_tool_calls: HashSet<ToolCallId>,
    expected_finish: Option<ExpectedFinish>,
}

#[derive(Debug)]
pub(crate) struct PreparedTransition {
    prior_sequence: u64,
    envelope: EventEnvelope,
}

impl PreparedTransition {
    pub(crate) fn envelope(&self) -> &EventEnvelope {
        &self.envelope
    }
}

pub fn reconstruct(history: &[EventEnvelope]) -> Result<RunView, ProjectionError> {
    let mut projection = RunProjection::empty();
    for envelope in history.iter().cloned() {
        let prepared = projection.prepare_transition(envelope)?;
        projection.apply(prepared);
    }
    projection.into_view()
}

fn validate_envelope(
    envelope: &EventEnvelope,
    expected_sequence: u64,
    run_id: &RunId,
    expected_schema_version: Option<u32>,
) -> Result<(), ProjectionError> {
    if !is_supported_schema(envelope.schema_version) {
        return Err(ProjectionError::UnsupportedSchema(envelope.schema_version));
    }
    if let Some(expected) = expected_schema_version
        && envelope.schema_version != expected
    {
        return Err(ProjectionError::SchemaVersionChanged {
            sequence: envelope.sequence.get(),
            expected,
            actual: envelope.schema_version,
        });
    }
    if envelope.schema_version == PHASE_1C_SCHEMA_VERSION
        && event_uses_model_request_failure(&envelope.event)
    {
        return Err(invalid_transition(
            envelope.sequence.get(),
            "schema version 1 cannot contain model request failures",
        ));
    }
    if envelope.sequence.get() != expected_sequence {
        return Err(ProjectionError::Sequence {
            expected: expected_sequence,
            actual: envelope.sequence.get(),
        });
    }
    if &envelope.run_id != run_id {
        return Err(ProjectionError::RunMismatch {
            sequence: envelope.sequence.get(),
            expected: run_id.clone(),
            actual: envelope.run_id.clone(),
        });
    }
    let timestamp = OffsetDateTime::parse(&envelope.observed_at, &Rfc3339);
    if !matches!(timestamp, Ok(value) if value.offset() == UtcOffset::UTC) {
        return Err(ProjectionError::InvalidTimestamp {
            sequence: envelope.sequence.get(),
        });
    }
    Ok(())
}

pub(crate) fn event_uses_model_request_failure(event: &DomainEvent) -> bool {
    matches!(
        event,
        DomainEvent::ModelRequestFinished {
            outcome: ModelOutcome::RequestFailure(_),
            ..
        } | DomainEvent::RunFinished {
            reason: FinishReason::Failed(ModelFailure::Request(_)),
        }
    )
}

impl RunProjection {
    pub(crate) fn empty() -> Self {
        Self {
            view: None,
            schema_version: None,
            seen_steps: HashSet::new(),
            seen_tool_calls: HashSet::new(),
            expected_finish: None,
        }
    }

    pub(crate) fn from_history(history: &[EventEnvelope]) -> Result<Self, ProjectionError> {
        let mut projection = Self::empty();
        for envelope in history.iter().cloned() {
            let prepared = projection.prepare_transition(envelope)?;
            projection.apply(prepared);
        }
        Ok(projection)
    }

    pub(crate) fn view(&self) -> Option<&RunView> {
        self.view.as_ref()
    }

    pub(crate) fn into_view(self) -> Result<RunView, ProjectionError> {
        self.view.ok_or(ProjectionError::EmptyHistory)
    }

    pub(crate) fn next_sequence(&self) -> EventSeq {
        EventSeq::new(
            self.view
                .as_ref()
                .map_or(1, |view| view.last_sequence.get() + 1),
        )
    }

    pub(crate) fn contains_tool_call(&self, tool_call_id: &ToolCallId) -> bool {
        self.seen_tool_calls.contains(tool_call_id)
    }

    pub(crate) fn prepare_transition(
        &self,
        envelope: EventEnvelope,
    ) -> Result<PreparedTransition, ProjectionError> {
        let prior_sequence = self
            .view
            .as_ref()
            .map_or(0, |view| view.last_sequence.get());
        let expected_run_id = self
            .view
            .as_ref()
            .map_or(&envelope.run_id, |view| &view.run_id);
        validate_envelope(
            &envelope,
            prior_sequence + 1,
            expected_run_id,
            self.schema_version,
        )?;

        match &self.view {
            None => {
                if !matches!(envelope.event, DomainEvent::RunStarted { .. }) {
                    return Err(invalid_transition(
                        envelope.sequence.get(),
                        "the first event must be RunStarted",
                    ));
                }
            }
            Some(view) => {
                if view.lifecycle == RunLifecycle::Terminal {
                    return Err(ProjectionError::AfterTerminal {
                        sequence: envelope.sequence.get(),
                    });
                }
                self.validate_event(&envelope)?;
            }
        }

        Ok(PreparedTransition {
            prior_sequence,
            envelope,
        })
    }

    fn validate_event(&self, envelope: &EventEnvelope) -> Result<(), ProjectionError> {
        let sequence = envelope.sequence.get();
        match &envelope.event {
            DomainEvent::RunStarted { .. } => Err(invalid_transition(
                sequence,
                "RunStarted may appear only once",
            )),
            DomainEvent::ModelRequestStarted { step_id } => {
                self.validate_model_start(sequence, *step_id)
            }
            DomainEvent::ModelRequestFinished { step_id, outcome } => {
                self.validate_model_finish(sequence, *step_id, outcome)
            }
            DomainEvent::ToolExecutionStarted {
                step_id,
                tool_call_id,
                name,
                arguments,
                effect,
            } => {
                self.validate_tool_start(sequence, *step_id, tool_call_id, name, arguments, *effect)
            }
            DomainEvent::ToolCallResolved {
                step_id,
                tool_call_id,
                outcome,
            } => self.validate_tool_resolution(sequence, *step_id, tool_call_id, outcome),
            DomainEvent::RunFinished { reason } => self.validate_run_finish(sequence, reason),
        }
    }

    fn validate_model_start(&self, sequence: u64, step_id: StepId) -> Result<(), ProjectionError> {
        let view = self.view.as_ref().expect("validation requires a run");
        if view.pending_operation.is_some() || self.expected_finish.is_some() {
            return Err(invalid_transition(
                sequence,
                "a model request cannot start while another transition is unresolved",
            ));
        }
        if step_id.get() == 0 || self.seen_steps.contains(&step_id) {
            return Err(ProjectionError::DuplicateStep(step_id));
        }
        if view.model_steps_consumed >= view.limits.max_model_steps {
            return Err(invalid_transition(
                sequence,
                "ModelRequestStarted exceeds the recorded model-step limit",
            ));
        }
        Ok(())
    }

    fn validate_model_finish(
        &self,
        sequence: u64,
        step_id: StepId,
        outcome: &ModelOutcome,
    ) -> Result<(), ProjectionError> {
        let view = self.view.as_ref().expect("validation requires a run");
        match &view.pending_operation {
            Some(PendingOperation::Model { step_id: open_step }) if *open_step == step_id => {}
            _ => {
                return Err(invalid_transition(
                    sequence,
                    "ModelRequestFinished must close the open model step",
                ));
            }
        }

        match outcome {
            ModelOutcome::FinalResponse { .. }
            | ModelOutcome::RequestFailure(_)
            | ModelOutcome::Failed
            | ModelOutcome::MalformedOutput
            | ModelOutcome::TimedOut
            | ModelOutcome::Cancelled
            | ModelOutcome::ChildPanicked
            | ModelOutcome::ChildShutdownFailed => {}
            ModelOutcome::ToolRequest(request) => {
                if request.tool_call_id.as_str().is_empty() {
                    return Err(ProjectionError::DuplicateToolCall(
                        request.tool_call_id.clone(),
                    ));
                }
                if request.name.is_empty() || !request.arguments.is_object() {
                    return Err(invalid_transition(
                        sequence,
                        "model tool request is not normalized",
                    ));
                }
                if self.seen_tool_calls.contains(&request.tool_call_id) {
                    return Err(ProjectionError::DuplicateToolCall(
                        request.tool_call_id.clone(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_tool_start(
        &self,
        sequence: u64,
        step_id: StepId,
        tool_call_id: &ToolCallId,
        name: &str,
        arguments: &serde_json::Value,
        effect: ToolEffect,
    ) -> Result<(), ProjectionError> {
        let view = self.view.as_ref().expect("validation requires a run");
        let Some(PendingOperation::Tool {
            step_id: requested_step,
            tool_call_id: requested_call,
            name: requested_name,
            arguments: requested_arguments,
            execution_started,
        }) = &view.pending_operation
        else {
            return Err(invalid_transition(
                sequence,
                "ToolExecutionStarted must refer to an unresolved tool request",
            ));
        };
        if *requested_step != step_id
            || requested_call != tool_call_id
            || requested_name != name
            || requested_arguments != arguments
            || *execution_started
            || effect != ToolEffect::ReadOnly
        {
            return Err(invalid_transition(
                sequence,
                "ToolExecutionStarted does not match the unresolved tool request",
            ));
        }
        if view.tool_executions_consumed >= view.limits.max_tool_executions {
            return Err(invalid_transition(
                sequence,
                "ToolExecutionStarted exceeds the recorded tool-execution limit",
            ));
        }
        Ok(())
    }

    fn validate_tool_resolution(
        &self,
        sequence: u64,
        step_id: StepId,
        tool_call_id: &ToolCallId,
        outcome: &ToolOutcome,
    ) -> Result<(), ProjectionError> {
        let view = self.view.as_ref().expect("validation requires a run");
        let Some(PendingOperation::Tool {
            step_id: requested_step,
            tool_call_id: requested_call,
            execution_started,
            ..
        }) = &view.pending_operation
        else {
            return Err(invalid_transition(
                sequence,
                "ToolCallResolved must refer to an unresolved tool request",
            ));
        };
        if *requested_step != step_id || requested_call != tool_call_id {
            return Err(invalid_transition(
                sequence,
                "ToolCallResolved does not match the unresolved tool request",
            ));
        }
        if outcome.requires_execution() && !execution_started {
            return Err(ProjectionError::OutcomeWithoutExecution(
                tool_call_id.clone(),
            ));
        }
        if !outcome.requires_execution() && *execution_started {
            return Err(ProjectionError::NonExecutionOutcomeAfterStart(
                tool_call_id.clone(),
            ));
        }

        Ok(())
    }

    fn validate_run_finish(
        &self,
        sequence: u64,
        reason: &FinishReason,
    ) -> Result<(), ProjectionError> {
        let view = self.view.as_ref().expect("validation requires a run");
        let valid = match reason {
            FinishReason::Interrupted => true,
            FinishReason::Completed => {
                view.pending_operation.is_none()
                    && self.expected_finish == Some(ExpectedFinish::Completed)
            }
            FinishReason::Cancelled => {
                let no_live_operation = match &view.pending_operation {
                    None => true,
                    Some(PendingOperation::Tool {
                        execution_started, ..
                    }) => !execution_started,
                    Some(PendingOperation::Model { .. }) => false,
                };
                no_live_operation
                    && (self.expected_finish == Some(ExpectedFinish::Cancelled)
                        || self.expected_finish.is_none())
            }
            FinishReason::Failed(failure) => {
                view.pending_operation.is_none()
                    && self.expected_finish == Some(ExpectedFinish::Failed(*failure))
            }
            FinishReason::BudgetExhausted(BudgetKind::ModelSteps) => {
                self.expected_finish.is_none()
                    && view.pending_operation.is_none()
                    && view.model_steps_consumed >= view.limits.max_model_steps
            }
            FinishReason::BudgetExhausted(BudgetKind::ToolExecutions) => {
                self.expected_finish.is_none()
                    && view.tool_executions_consumed >= view.limits.max_tool_executions
                    && matches!(
                        view.pending_operation,
                        Some(PendingOperation::Tool {
                            execution_started: false,
                            ..
                        })
                    )
            }
        };
        if !valid {
            return Err(invalid_transition(
                sequence,
                "RunFinished reason does not match the preceding run state",
            ));
        }
        Ok(())
    }

    pub(crate) fn apply(&mut self, prepared: PreparedTransition) {
        let PreparedTransition {
            prior_sequence,
            envelope,
        } = prepared;
        debug_assert_eq!(
            prior_sequence,
            self.view
                .as_ref()
                .map_or(0, |view| view.last_sequence.get())
        );
        let sequence = envelope.sequence;

        match self.schema_version {
            None => self.schema_version = Some(envelope.schema_version),
            Some(version) => debug_assert_eq!(version, envelope.schema_version),
        }

        match envelope.event {
            DomainEvent::RunStarted { request, limits } => {
                self.view = Some(RunView {
                    run_id: envelope.run_id,
                    last_sequence: sequence,
                    lifecycle: RunLifecycle::Active,
                    finish_reason: None,
                    request: request.clone(),
                    limits,
                    model_steps_consumed: 0,
                    tool_executions_consumed: 0,
                    pending_operation: None,
                    model_context: vec![ModelItem::UserInput { text: request }],
                    final_response: None,
                });
                return;
            }
            DomainEvent::ModelRequestStarted { step_id } => {
                let view = self.view.as_mut().expect("prepared run exists");
                self.seen_steps.insert(step_id);
                view.model_steps_consumed += 1;
                view.pending_operation = Some(PendingOperation::Model { step_id });
            }
            DomainEvent::ModelRequestFinished { step_id, outcome } => {
                let view = self.view.as_mut().expect("prepared run exists");
                view.pending_operation = None;
                match outcome {
                    ModelOutcome::FinalResponse { text } => {
                        view.final_response = Some(text.clone());
                        view.model_context.push(ModelItem::AssistantText { text });
                        self.expected_finish = Some(ExpectedFinish::Completed);
                    }
                    ModelOutcome::ToolRequest(request) => {
                        self.seen_tool_calls.insert(request.tool_call_id.clone());
                        view.model_context.push(ModelItem::ToolRequest {
                            tool_call_id: request.tool_call_id.clone(),
                            name: request.name.clone(),
                            arguments: request.arguments.clone(),
                        });
                        view.pending_operation = Some(PendingOperation::Tool {
                            step_id,
                            tool_call_id: request.tool_call_id,
                            name: request.name,
                            arguments: request.arguments,
                            execution_started: false,
                        });
                    }
                    ModelOutcome::RequestFailure(category) => {
                        self.expected_finish =
                            Some(ExpectedFinish::Failed(ModelFailure::Request(category)));
                    }
                    ModelOutcome::Failed => {
                        self.expected_finish = Some(ExpectedFinish::Failed(ModelFailure::Ordinary));
                    }
                    ModelOutcome::MalformedOutput => {
                        self.expected_finish =
                            Some(ExpectedFinish::Failed(ModelFailure::MalformedOutput));
                    }
                    ModelOutcome::TimedOut => {
                        self.expected_finish = Some(ExpectedFinish::Failed(ModelFailure::Timeout));
                    }
                    ModelOutcome::Cancelled => {
                        self.expected_finish = Some(ExpectedFinish::Cancelled);
                    }
                    ModelOutcome::ChildPanicked => {
                        self.expected_finish =
                            Some(ExpectedFinish::Failed(ModelFailure::ChildPanicked));
                    }
                    ModelOutcome::ChildShutdownFailed => {
                        self.expected_finish =
                            Some(ExpectedFinish::Failed(ModelFailure::ChildShutdown));
                    }
                }
            }
            DomainEvent::ToolExecutionStarted { .. } => {
                let view = self.view.as_mut().expect("prepared run exists");
                let Some(PendingOperation::Tool {
                    execution_started, ..
                }) = &mut view.pending_operation
                else {
                    unreachable!("prepared tool start has a pending request");
                };
                *execution_started = true;
                view.tool_executions_consumed += 1;
            }
            DomainEvent::ToolCallResolved {
                tool_call_id,
                outcome,
                ..
            } => {
                let view = self.view.as_mut().expect("prepared run exists");
                view.model_context.push(ModelItem::ToolResult {
                    tool_call_id,
                    outcome: outcome.clone(),
                });
                view.pending_operation = None;
                match outcome {
                    ToolOutcome::Cancelled => {
                        self.expected_finish = Some(ExpectedFinish::Cancelled);
                    }
                    ToolOutcome::ChildPanicked => {
                        self.expected_finish =
                            Some(ExpectedFinish::Failed(ModelFailure::ChildPanicked));
                    }
                    ToolOutcome::ChildShutdownFailed => {
                        self.expected_finish =
                            Some(ExpectedFinish::Failed(ModelFailure::ChildShutdown));
                    }
                    _ => {}
                }
            }
            DomainEvent::RunFinished { reason } => {
                let view = self.view.as_mut().expect("prepared run exists");
                view.lifecycle = RunLifecycle::Terminal;
                view.finish_reason = Some(reason);
            }
        }

        self.view
            .as_mut()
            .expect("prepared run exists")
            .last_sequence = sequence;
    }
}

fn invalid_transition(sequence: u64, message: impl Into<String>) -> ProjectionError {
    ProjectionError::InvalidTransition {
        sequence,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn limits() -> RunLimits {
        RunLimits {
            max_model_steps: 1,
            max_tool_executions: 0,
            model_timeout_ms: 100,
            tool_timeout_ms: 100,
            shutdown_grace_period_ms: 10,
        }
    }

    fn envelope(sequence: u64, event: DomainEvent) -> EventEnvelope {
        EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence: EventSeq::new(sequence),
            run_id: RunId::new("run-incremental"),
            observed_at: "2026-01-01T00:00:00Z".to_owned(),
            event,
        }
    }

    fn generated_history(kind: u8, request: String, final_text: String) -> Vec<EventEnvelope> {
        let mut run_limits = limits();
        run_limits.max_model_steps = 2;
        run_limits.max_tool_executions = 1;
        let mut history = vec![envelope(
            1,
            DomainEvent::RunStarted {
                request,
                limits: run_limits,
            },
        )];
        if kind == 0 {
            return history;
        }

        history.push(envelope(
            2,
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ));
        if kind == 1 {
            return history;
        }

        let terminal_model_outcome = match kind {
            2 | 3 => Some((
                ModelOutcome::FinalResponse {
                    text: final_text.clone(),
                },
                FinishReason::Completed,
            )),
            4 => Some((
                ModelOutcome::Failed,
                FinishReason::Failed(ModelFailure::Ordinary),
            )),
            5 => Some((ModelOutcome::Cancelled, FinishReason::Cancelled)),
            6 => Some((
                ModelOutcome::TimedOut,
                FinishReason::Failed(ModelFailure::Timeout),
            )),
            7 => Some((
                ModelOutcome::ChildPanicked,
                FinishReason::Failed(ModelFailure::ChildPanicked),
            )),
            _ => None,
        };
        if let Some((outcome, reason)) = terminal_model_outcome {
            history.push(envelope(
                3,
                DomainEvent::ModelRequestFinished {
                    step_id: StepId::new(1),
                    outcome,
                },
            ));
            if kind != 2 {
                history.push(envelope(4, DomainEvent::RunFinished { reason }));
            }
            return history;
        }
        if kind == 8 {
            history.push(envelope(
                3,
                DomainEvent::RunFinished {
                    reason: FinishReason::Interrupted,
                },
            ));
            return history;
        }

        let tool_request = ToolRequest {
            tool_call_id: ToolCallId::new("generated-call"),
            name: "fixture.read".to_owned(),
            arguments: serde_json::json!({"key": "alpha"}),
        };
        history.push(envelope(
            3,
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::ToolRequest(tool_request.clone()),
            },
        ));
        if kind == 9 {
            return history;
        }
        history.push(envelope(
            4,
            DomainEvent::ToolExecutionStarted {
                step_id: StepId::new(1),
                tool_call_id: tool_request.tool_call_id.clone(),
                name: tool_request.name,
                arguments: tool_request.arguments,
                effect: ToolEffect::ReadOnly,
            },
        ));
        if kind == 10 {
            return history;
        }
        if kind == 15 {
            history.push(envelope(
                5,
                DomainEvent::RunFinished {
                    reason: FinishReason::Interrupted,
                },
            ));
            return history;
        }
        if kind == 14 {
            history.push(envelope(
                5,
                DomainEvent::ToolCallResolved {
                    step_id: StepId::new(1),
                    tool_call_id: tool_request.tool_call_id,
                    outcome: ToolOutcome::ChildPanicked,
                },
            ));
            history.push(envelope(
                6,
                DomainEvent::RunFinished {
                    reason: FinishReason::Failed(ModelFailure::ChildPanicked),
                },
            ));
            return history;
        }

        history.push(envelope(
            5,
            DomainEvent::ToolCallResolved {
                step_id: StepId::new(1),
                tool_call_id: tool_request.tool_call_id,
                outcome: ToolOutcome::Success {
                    value: serde_json::json!({"value": "fixture"}),
                },
            },
        ));
        if kind == 11 {
            return history;
        }
        history.push(envelope(
            6,
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(2),
            },
        ));
        if kind == 12 {
            return history;
        }
        history.push(envelope(
            7,
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(2),
                outcome: ModelOutcome::FinalResponse { text: final_text },
            },
        ));
        history.push(envelope(
            8,
            DomainEvent::RunFinished {
                reason: FinishReason::Completed,
            },
        ));
        history
    }

    #[test]
    fn rejected_candidate_does_not_change_the_live_projection() {
        let mut projection = RunProjection::empty();
        let started = envelope(
            1,
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        );
        let prepared = projection
            .prepare_transition(started)
            .expect("first event is valid");
        projection.apply(prepared);
        let before = projection.view().expect("run exists").clone();

        let error = projection
            .prepare_transition(envelope(
                3,
                DomainEvent::ModelRequestStarted {
                    step_id: StepId::new(1),
                },
            ))
            .expect_err("sequence gap is rejected");

        assert!(matches!(
            error,
            ProjectionError::Sequence {
                expected: 2,
                actual: 3
            }
        ));
        assert_eq!(projection.view(), Some(&before));
    }

    proptest! {
        #[test]
        fn incremental_projection_matches_every_cold_prefix(
            kind in 0u8..16,
            request in "[a-zA-Z0-9 ]{0,48}",
            final_text in "[a-zA-Z0-9 ]{0,48}",
        ) {
            let history = generated_history(kind, request, final_text);
            let mut projection = RunProjection::empty();

            for (index, candidate) in history.iter().cloned().enumerate() {
                let prepared = projection
                    .prepare_transition(candidate)
                    .expect("generated transition is valid");
                projection.apply(prepared);
                let cold = reconstruct(&history[..=index]).expect("prefix reconstructs");
                prop_assert_eq!(projection.view(), Some(&cold));
            }
        }
    }
}
