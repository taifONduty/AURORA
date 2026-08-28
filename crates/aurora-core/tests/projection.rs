use aurora_core::{
    DomainEvent, EventEnvelope, EventSeq, FinishReason, ModelFailure, ModelItem, ModelOutcome,
    ModelRequestFailure, ProjectionError, RunId, RunLifecycle, RunLimits, SCHEMA_VERSION, StepId,
    ToolCallId, ToolEffect, ToolOutcome, ToolRequest, reconstruct,
};
use serde_json::json;

fn limits() -> RunLimits {
    RunLimits {
        max_model_steps: 2,
        max_tool_executions: 1,
        model_timeout_ms: 1_000,
        tool_timeout_ms: 1_000,
        shutdown_grace_period_ms: 100,
    }
}

fn envelope(sequence: u64, observed_at: &str, event: DomainEvent) -> EventEnvelope {
    EventEnvelope {
        schema_version: SCHEMA_VERSION,
        sequence: EventSeq::new(sequence),
        run_id: RunId::new("run-projection"),
        observed_at: observed_at.to_owned(),
        event,
    }
}

fn envelope_at_version(schema_version: u32, sequence: u64, event: DomainEvent) -> EventEnvelope {
    EventEnvelope {
        schema_version,
        sequence: EventSeq::new(sequence),
        run_id: RunId::new("run-projection"),
        observed_at: "2026-01-01T00:00:00Z".to_owned(),
        event,
    }
}

#[test]
fn final_response_history_reconstructs_as_completed() {
    let history = vec![
        envelope(
            1,
            "2026-01-01T00:00:00Z",
            DomainEvent::RunStarted {
                request: "summarize the fixture".to_owned(),
                limits: limits(),
            },
        ),
        envelope(
            2,
            "2026-01-01T00:00:01Z",
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
        envelope(
            3,
            "2026-01-01T00:00:02Z",
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::FinalResponse {
                    text: "fixture summary".to_owned(),
                },
            },
        ),
        envelope(
            4,
            "2026-01-01T00:00:03Z",
            DomainEvent::RunFinished {
                reason: FinishReason::Completed,
            },
        ),
    ];

    let view = reconstruct(&history).expect("history is valid");

    assert_eq!(view.lifecycle, RunLifecycle::Terminal);
    assert_eq!(view.finish_reason, Some(FinishReason::Completed));
    assert_eq!(view.final_response.as_deref(), Some("fixture summary"));
    assert_eq!(view.model_steps_consumed, 1);
    assert_eq!(view.tool_executions_consumed, 0);
    assert!(view.pending_operation.is_none());
    assert_eq!(
        view.model_context,
        [
            ModelItem::UserInput {
                text: "summarize the fixture".to_owned(),
            },
            ModelItem::AssistantText {
                text: "fixture summary".to_owned(),
            },
        ]
    );
}

#[test]
fn reconstruction_ignores_observational_timestamps() {
    let events = [
        DomainEvent::RunStarted {
            request: "request".to_owned(),
            limits: limits(),
        },
        DomainEvent::ModelRequestStarted {
            step_id: StepId::new(1),
        },
        DomainEvent::ModelRequestFinished {
            step_id: StepId::new(1),
            outcome: ModelOutcome::FinalResponse {
                text: "done".to_owned(),
            },
        },
        DomainEvent::RunFinished {
            reason: FinishReason::Completed,
        },
    ];
    let first: Vec<_> = events
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, event)| envelope(index as u64 + 1, "2026-01-01T00:00:00Z", event))
        .collect();
    let second: Vec<_> = events
        .into_iter()
        .enumerate()
        .map(|(index, event)| envelope(index as u64 + 1, "2030-12-31T23:59:59Z", event))
        .collect();

    assert_eq!(
        reconstruct(&first).expect("first history is valid"),
        reconstruct(&second).expect("second history is valid")
    );
}

#[test]
fn reconstruction_rejects_a_sequence_gap() {
    let history = vec![
        envelope(
            1,
            "2026-01-01T00:00:00Z",
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope(
            3,
            "2026-01-01T00:00:01Z",
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
    ];

    let error = reconstruct(&history).expect_err("sequence gap must fail");

    assert_eq!(error.to_string(), "expected event sequence 2, found 3");
}

#[test]
fn interrupted_started_tool_remains_outcome_unknown() {
    let request = ToolRequest {
        tool_call_id: ToolCallId::new("call-1"),
        name: "fixture.read".to_owned(),
        arguments: json!({"key": "value"}),
    };
    let history = vec![
        envelope(
            1,
            "2026-01-01T00:00:00Z",
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope(
            2,
            "2026-01-01T00:00:01Z",
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
        envelope(
            3,
            "2026-01-01T00:00:02Z",
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::ToolRequest(request.clone()),
            },
        ),
        envelope(
            4,
            "2026-01-01T00:00:03Z",
            DomainEvent::ToolExecutionStarted {
                step_id: StepId::new(1),
                tool_call_id: request.tool_call_id.clone(),
                name: request.name.clone(),
                arguments: request.arguments.clone(),
                effect: ToolEffect::ReadOnly,
            },
        ),
        envelope(
            5,
            "2026-01-01T00:00:04Z",
            DomainEvent::RunFinished {
                reason: FinishReason::Interrupted,
            },
        ),
    ];

    let view = reconstruct(&history).expect("interrupted history is valid");
    let pending = view
        .pending_operation
        .expect("the unresolved tool must remain visible");

    assert!(pending.execution_started());
    assert_eq!(pending.tool_call_id(), Some(&ToolCallId::new("call-1")));
    assert_eq!(view.finish_reason, Some(FinishReason::Interrupted));
    assert!(
        !view
            .model_context
            .iter()
            .any(|item| matches!(item, ModelItem::ToolResult { .. }))
    );
}

#[test]
fn a_terminal_history_rejects_later_events() {
    let history = vec![
        envelope(
            1,
            "2026-01-01T00:00:00Z",
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope(
            2,
            "2026-01-01T00:00:01Z",
            DomainEvent::RunFinished {
                reason: FinishReason::Cancelled,
            },
        ),
        envelope(
            3,
            "2026-01-01T00:00:02Z",
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
    ];

    let error = reconstruct(&history).expect_err("terminal history cannot continue");

    assert_eq!(
        error.to_string(),
        "event 3 appears after the run became terminal"
    );
}

#[test]
fn unstarted_tool_rejects_an_execution_outcome() {
    let request = ToolRequest {
        tool_call_id: ToolCallId::new("call-1"),
        name: "fixture.read".to_owned(),
        arguments: json!({"key": "value"}),
    };
    let history = vec![
        envelope(
            1,
            "2026-01-01T00:00:00Z",
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope(
            2,
            "2026-01-01T00:00:01Z",
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
        envelope(
            3,
            "2026-01-01T00:00:02Z",
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::ToolRequest(request),
            },
        ),
        envelope(
            4,
            "2026-01-01T00:00:03Z",
            DomainEvent::ToolCallResolved {
                step_id: StepId::new(1),
                tool_call_id: ToolCallId::new("call-1"),
                outcome: ToolOutcome::Success {
                    value: json!({"value": "result"}),
                },
            },
        ),
    ];

    let error = reconstruct(&history).expect_err("success requires execution start");

    assert_eq!(
        error.to_string(),
        "tool call call-1 has an execution outcome without an execution-start event"
    );
}

#[test]
fn model_step_budget_cannot_close_an_unresolved_tool_request() {
    let history = vec![
        envelope(
            1,
            "2026-01-01T00:00:00Z",
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope(
            2,
            "2026-01-01T00:00:01Z",
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
        envelope(
            3,
            "2026-01-01T00:00:02Z",
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::ToolRequest(ToolRequest {
                    tool_call_id: ToolCallId::new("call-1"),
                    name: "fixture.read".to_owned(),
                    arguments: json!({"key": "alpha"}),
                }),
            },
        ),
        envelope(
            4,
            "2026-01-01T00:00:03Z",
            DomainEvent::RunFinished {
                reason: FinishReason::BudgetExhausted(aurora_core::BudgetKind::ModelSteps),
            },
        ),
    ];

    let error = reconstruct(&history).expect_err("the exhausted budget must match the boundary");

    assert_eq!(
        error.to_string(),
        "event 4 violates run ordering: RunFinished reason does not match the preceding run state"
    );
}

#[test]
fn budget_exhaustion_requires_the_recorded_limit_to_be_reached() {
    let history = vec![
        envelope(
            1,
            "2026-01-01T00:00:00Z",
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope(
            2,
            "2026-01-01T00:00:01Z",
            DomainEvent::RunFinished {
                reason: FinishReason::BudgetExhausted(aurora_core::BudgetKind::ModelSteps),
            },
        ),
    ];

    assert!(reconstruct(&history).is_err());
}

#[test]
fn history_cannot_start_a_model_step_past_its_recorded_limit() {
    let mut no_model_steps = limits();
    no_model_steps.max_model_steps = 0;
    let history = vec![
        envelope(
            1,
            "2026-01-01T00:00:00Z",
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: no_model_steps,
            },
        ),
        envelope(
            2,
            "2026-01-01T00:00:01Z",
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
    ];

    assert!(reconstruct(&history).is_err());
}

#[test]
fn unnormalized_tool_request_is_a_malformed_history() {
    let history = vec![
        envelope(
            1,
            "2026-01-01T00:00:00Z",
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope(
            2,
            "2026-01-01T00:00:01Z",
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
        envelope(
            3,
            "2026-01-01T00:00:02Z",
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::ToolRequest(ToolRequest {
                    tool_call_id: ToolCallId::new("call-1"),
                    name: String::new(),
                    arguments: json!(["not", "an", "object"]),
                }),
            },
        ),
    ];

    assert!(reconstruct(&history).is_err());
}

#[test]
fn observational_timestamp_must_use_utc_offset() {
    let history = vec![envelope(
        1,
        "2026-01-01T06:00:00+06:00",
        DomainEvent::RunStarted {
            request: "request".to_owned(),
            limits: limits(),
        },
    )];

    assert!(reconstruct(&history).is_err());
}

#[test]
fn model_child_panic_history_requires_the_internal_panic_finish_reason() {
    let history = vec![
        envelope(
            1,
            "2026-01-01T00:00:00Z",
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope(
            2,
            "2026-01-01T00:00:01Z",
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
        envelope(
            3,
            "2026-01-01T00:00:02Z",
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::ChildPanicked,
            },
        ),
        envelope(
            4,
            "2026-01-01T00:00:03Z",
            DomainEvent::RunFinished {
                reason: FinishReason::Failed(ModelFailure::ChildPanicked),
            },
        ),
    ];

    let view = reconstruct(&history).expect("model panic history is valid");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::ChildPanicked))
    );
    assert!(view.final_response.is_none());
    assert!(view.pending_operation.is_none());
}

#[test]
fn model_child_panic_history_rejects_a_mismatched_failure_cause() {
    let history = vec![
        envelope(
            1,
            "2026-01-01T00:00:00Z",
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope(
            2,
            "2026-01-01T00:00:01Z",
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
        envelope(
            3,
            "2026-01-01T00:00:02Z",
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::ChildPanicked,
            },
        ),
        envelope(
            4,
            "2026-01-01T00:00:03Z",
            DomainEvent::RunFinished {
                reason: FinishReason::Failed(ModelFailure::Ordinary),
            },
        ),
    ];

    let error = reconstruct(&history).expect_err("mismatched model panic cause must fail");

    assert_eq!(
        error,
        ProjectionError::InvalidTransition {
            sequence: 4,
            message: "RunFinished reason does not match the preceding run state".to_owned(),
        }
    );
}

#[test]
fn tool_child_panic_history_is_terminal_and_retains_the_typed_resolution() {
    let request = ToolRequest {
        tool_call_id: ToolCallId::new("call-panicked"),
        name: "fixture.read".to_owned(),
        arguments: json!({"key": "alpha"}),
    };
    let history = vec![
        envelope(
            1,
            "2026-01-01T00:00:00Z",
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope(
            2,
            "2026-01-01T00:00:01Z",
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
        envelope(
            3,
            "2026-01-01T00:00:02Z",
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::ToolRequest(request.clone()),
            },
        ),
        envelope(
            4,
            "2026-01-01T00:00:03Z",
            DomainEvent::ToolExecutionStarted {
                step_id: StepId::new(1),
                tool_call_id: request.tool_call_id.clone(),
                name: request.name.clone(),
                arguments: request.arguments,
                effect: ToolEffect::ReadOnly,
            },
        ),
        envelope(
            5,
            "2026-01-01T00:00:04Z",
            DomainEvent::ToolCallResolved {
                step_id: StepId::new(1),
                tool_call_id: request.tool_call_id,
                outcome: ToolOutcome::ChildPanicked,
            },
        ),
        envelope(
            6,
            "2026-01-01T00:00:05Z",
            DomainEvent::RunFinished {
                reason: FinishReason::Failed(ModelFailure::ChildPanicked),
            },
        ),
    ];

    let view = reconstruct(&history).expect("tool panic history is valid");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::ChildPanicked))
    );
    assert!(matches!(
        view.model_context.last(),
        Some(ModelItem::ToolResult {
            outcome: ToolOutcome::ChildPanicked,
            ..
        })
    ));
    assert!(view.pending_operation.is_none());
}

#[test]
fn tool_child_panic_history_rejects_a_mismatched_failure_cause() {
    let request = ToolRequest {
        tool_call_id: ToolCallId::new("call-panicked"),
        name: "fixture.read".to_owned(),
        arguments: json!({"key": "alpha"}),
    };
    let history = vec![
        envelope(
            1,
            "2026-01-01T00:00:00Z",
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope(
            2,
            "2026-01-01T00:00:01Z",
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
        envelope(
            3,
            "2026-01-01T00:00:02Z",
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::ToolRequest(request.clone()),
            },
        ),
        envelope(
            4,
            "2026-01-01T00:00:03Z",
            DomainEvent::ToolExecutionStarted {
                step_id: StepId::new(1),
                tool_call_id: request.tool_call_id.clone(),
                name: request.name.clone(),
                arguments: request.arguments,
                effect: ToolEffect::ReadOnly,
            },
        ),
        envelope(
            5,
            "2026-01-01T00:00:04Z",
            DomainEvent::ToolCallResolved {
                step_id: StepId::new(1),
                tool_call_id: request.tool_call_id,
                outcome: ToolOutcome::ChildPanicked,
            },
        ),
        envelope(
            6,
            "2026-01-01T00:00:05Z",
            DomainEvent::RunFinished {
                reason: FinishReason::Failed(ModelFailure::Ordinary),
            },
        ),
    ];

    let error = reconstruct(&history).expect_err("mismatched tool panic cause must fail");

    assert_eq!(
        error,
        ProjectionError::InvalidTransition {
            sequence: 6,
            message: "RunFinished reason does not match the preceding run state".to_owned(),
        }
    );
}

#[test]
fn request_failure_requires_the_same_terminal_category() {
    let history = vec![
        envelope_at_version(
            2,
            1,
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope_at_version(
            2,
            2,
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
        envelope_at_version(
            2,
            3,
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::RequestFailure(ModelRequestFailure::RateLimited),
            },
        ),
        envelope_at_version(
            2,
            4,
            DomainEvent::RunFinished {
                reason: FinishReason::Failed(ModelFailure::Request(
                    ModelRequestFailure::ServiceUnavailable,
                )),
            },
        ),
    ];

    let error = reconstruct(&history).expect_err("failure categories must match");
    assert_eq!(
        error,
        ProjectionError::InvalidTransition {
            sequence: 4,
            message: "RunFinished reason does not match the preceding run state".to_owned(),
        }
    );
}

#[test]
fn homogeneous_phase_1c_history_remains_readable() {
    let history = vec![
        envelope_at_version(
            1,
            1,
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope_at_version(
            1,
            2,
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
        envelope_at_version(
            1,
            3,
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::FinalResponse {
                    text: "done".to_owned(),
                },
            },
        ),
        envelope_at_version(
            1,
            4,
            DomainEvent::RunFinished {
                reason: FinishReason::Completed,
            },
        ),
    ];

    assert_eq!(
        reconstruct(&history)
            .expect("homogeneous schema version 1 is supported")
            .final_response
            .as_deref(),
        Some("done")
    );
}

fn assert_mixed_schema_versions_are_rejected(first: u32, second: u32) {
    let history = vec![
        envelope_at_version(
            first,
            1,
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope_at_version(
            second,
            2,
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
    ];

    assert_eq!(
        reconstruct(&history).expect_err("a history cannot change schema"),
        ProjectionError::SchemaVersionChanged {
            sequence: 2,
            expected: first,
            actual: second,
        }
    );
}

#[test]
fn mixed_schema_version_1_to_2_is_rejected() {
    assert_mixed_schema_versions_are_rejected(1, 2);
}

#[test]
fn mixed_schema_version_2_to_1_is_rejected() {
    assert_mixed_schema_versions_are_rejected(2, 1);
}

#[test]
fn schema_version_1_rejects_model_request_finished_failure() {
    let history = vec![
        envelope_at_version(
            1,
            1,
            DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        ),
        envelope_at_version(
            1,
            2,
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
        envelope_at_version(
            1,
            3,
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::RequestFailure(ModelRequestFailure::Authentication),
            },
        ),
    ];

    assert_eq!(
        reconstruct(&history).expect_err("version 1 cannot contain Phase 1D failures"),
        ProjectionError::InvalidTransition {
            sequence: 3,
            message: "schema version 1 cannot contain model request failures".to_owned(),
        }
    );
}

#[test]
fn schema_version_1_rejects_run_finished_request_failure() {
    let history = vec![envelope_at_version(
        1,
        1,
        DomainEvent::RunFinished {
            reason: FinishReason::Failed(ModelFailure::Request(
                ModelRequestFailure::Authentication,
            )),
        },
    )];

    assert_eq!(
        reconstruct(&history).expect_err("version 1 cannot contain Phase 1D failures"),
        ProjectionError::InvalidTransition {
            sequence: 1,
            message: "schema version 1 cannot contain model request failures".to_owned(),
        }
    );
}
