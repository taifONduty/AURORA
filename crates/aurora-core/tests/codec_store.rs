use std::{
    fs,
    future::Future,
    task::{Context, Poll, Waker},
};

use aurora_core::{
    BudgetKind, DecodeError, DomainEvent, EventEnvelope, EventSeq, EventStore, FinishReason,
    InMemoryEventStore, JsonlEventStore, ModelFailure, ModelOutcome, ModelRequestFailure, RunId,
    RunLimits, SCHEMA_VERSION, StepId, StoreError, ToolCallId, ToolEffect, ToolOutcome,
    ToolRequest, decode_envelope_line, decode_jsonl, encode_envelope, reconstruct,
};
use proptest::prelude::*;
use tempfile::tempdir;

fn limits() -> RunLimits {
    RunLimits {
        max_model_steps: 2,
        max_tool_executions: 1,
        model_timeout_ms: 1_000,
        tool_timeout_ms: 1_000,
        shutdown_grace_period_ms: 100,
    }
}

fn envelope(sequence: u64, event: DomainEvent) -> EventEnvelope {
    EventEnvelope {
        schema_version: SCHEMA_VERSION,
        sequence: EventSeq::new(sequence),
        run_id: RunId::new("run-codec"),
        observed_at: "2026-01-01T00:00:00Z".to_owned(),
        event,
    }
}

fn commit_event(
    store: &mut dyn EventStore,
    run_id: &RunId,
    observed_at: &str,
    event: DomainEvent,
) -> Result<EventEnvelope, aurora_core::StoreError> {
    let envelope = EventEnvelope {
        schema_version: SCHEMA_VERSION,
        sequence: EventSeq::new(store.acknowledged().len() as u64 + 1),
        run_id: run_id.clone(),
        observed_at: observed_at.to_owned(),
        event,
    };
    complete_immediately(store.commit(envelope.clone()))?;
    Ok(envelope)
}

fn complete_immediately<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("deterministic store acknowledgement must resolve immediately"),
    }
}

fn completed_history(request: String, response: String) -> Vec<EventEnvelope> {
    vec![
        envelope(
            1,
            DomainEvent::RunStarted {
                request,
                limits: limits(),
            },
        ),
        envelope(
            2,
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ),
        envelope(
            3,
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::FinalResponse { text: response },
            },
        ),
        envelope(
            4,
            DomainEvent::RunFinished {
                reason: FinishReason::Completed,
            },
        ),
    ]
}

fn domain_event_strategy() -> impl Strategy<Value = DomainEvent> {
    let text = "[^\\p{C}]{0,64}";
    prop_oneof![
        text.prop_map(|request| DomainEvent::RunStarted {
            request,
            limits: limits(),
        }),
        (1u64..100).prop_map(|step| DomainEvent::ModelRequestStarted {
            step_id: StepId::new(step),
        }),
        (1u64..100, text).prop_map(|(step, value)| DomainEvent::ModelRequestFinished {
            step_id: StepId::new(step),
            outcome: ModelOutcome::FinalResponse { text: value },
        }),
        (1u64..100, text, text).prop_map(|(step, call, name)| {
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(step),
                outcome: ModelOutcome::ToolRequest(ToolRequest {
                    tool_call_id: ToolCallId::new(call),
                    name,
                    arguments: json_object(),
                }),
            }
        }),
        (1u64..100, text, text).prop_map(|(step, call, name)| {
            DomainEvent::ToolExecutionStarted {
                step_id: StepId::new(step),
                tool_call_id: ToolCallId::new(call),
                name,
                arguments: json_object(),
                effect: ToolEffect::ReadOnly,
            }
        }),
        (1u64..100, text).prop_map(|(step, call)| DomainEvent::ToolCallResolved {
            step_id: StepId::new(step),
            tool_call_id: ToolCallId::new(call),
            outcome: ToolOutcome::Success {
                value: serde_json::Value::Null,
            },
        }),
        Just(DomainEvent::ModelRequestFinished {
            step_id: StepId::new(1),
            outcome: ModelOutcome::Failed,
        }),
        Just(DomainEvent::ModelRequestFinished {
            step_id: StepId::new(1),
            outcome: ModelOutcome::MalformedOutput,
        }),
        Just(DomainEvent::ModelRequestFinished {
            step_id: StepId::new(1),
            outcome: ModelOutcome::TimedOut,
        }),
        Just(DomainEvent::ModelRequestFinished {
            step_id: StepId::new(1),
            outcome: ModelOutcome::Cancelled,
        }),
        Just(DomainEvent::ModelRequestFinished {
            step_id: StepId::new(1),
            outcome: ModelOutcome::ChildPanicked,
        }),
        Just(DomainEvent::ModelRequestFinished {
            step_id: StepId::new(1),
            outcome: ModelOutcome::ChildShutdownFailed,
        }),
        model_request_failure_strategy().prop_map(|category| DomainEvent::ModelRequestFinished {
            step_id: StepId::new(1),
            outcome: ModelOutcome::RequestFailure(category),
        }),
        Just(DomainEvent::ToolCallResolved {
            step_id: StepId::new(1),
            tool_call_id: ToolCallId::new("call"),
            outcome: ToolOutcome::UnknownTool,
        }),
        Just(DomainEvent::ToolCallResolved {
            step_id: StepId::new(1),
            tool_call_id: ToolCallId::new("call"),
            outcome: ToolOutcome::InvalidArguments,
        }),
        Just(DomainEvent::ToolCallResolved {
            step_id: StepId::new(1),
            tool_call_id: ToolCallId::new("call"),
            outcome: ToolOutcome::Denied,
        }),
        Just(DomainEvent::ToolCallResolved {
            step_id: StepId::new(1),
            tool_call_id: ToolCallId::new("call"),
            outcome: ToolOutcome::Failed,
        }),
        Just(DomainEvent::ToolCallResolved {
            step_id: StepId::new(1),
            tool_call_id: ToolCallId::new("call"),
            outcome: ToolOutcome::TimedOut,
        }),
        Just(DomainEvent::ToolCallResolved {
            step_id: StepId::new(1),
            tool_call_id: ToolCallId::new("call"),
            outcome: ToolOutcome::Cancelled,
        }),
        Just(DomainEvent::ToolCallResolved {
            step_id: StepId::new(1),
            tool_call_id: ToolCallId::new("call"),
            outcome: ToolOutcome::ChildPanicked,
        }),
        Just(DomainEvent::ToolCallResolved {
            step_id: StepId::new(1),
            tool_call_id: ToolCallId::new("call"),
            outcome: ToolOutcome::ChildShutdownFailed,
        }),
        Just(DomainEvent::RunFinished {
            reason: FinishReason::Completed,
        }),
        Just(DomainEvent::RunFinished {
            reason: FinishReason::Cancelled,
        }),
        Just(DomainEvent::RunFinished {
            reason: FinishReason::BudgetExhausted(BudgetKind::ModelSteps),
        }),
        Just(DomainEvent::RunFinished {
            reason: FinishReason::BudgetExhausted(BudgetKind::ToolExecutions),
        }),
        Just(DomainEvent::RunFinished {
            reason: FinishReason::Failed(ModelFailure::Ordinary),
        }),
        Just(DomainEvent::RunFinished {
            reason: FinishReason::Failed(ModelFailure::Timeout),
        }),
        Just(DomainEvent::RunFinished {
            reason: FinishReason::Failed(ModelFailure::MalformedOutput),
        }),
        Just(DomainEvent::RunFinished {
            reason: FinishReason::Failed(ModelFailure::ChildPanicked),
        }),
        Just(DomainEvent::RunFinished {
            reason: FinishReason::Failed(ModelFailure::ChildShutdown),
        }),
        model_request_failure_strategy().prop_map(|category| DomainEvent::RunFinished {
            reason: FinishReason::Failed(ModelFailure::Request(category)),
        }),
        Just(DomainEvent::RunFinished {
            reason: FinishReason::Interrupted,
        }),
    ]
}

fn json_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

fn model_request_failure_strategy() -> impl Strategy<Value = ModelRequestFailure> {
    prop::sample::select(vec![
        ModelRequestFailure::Authentication,
        ModelRequestFailure::RateLimited,
        ModelRequestFailure::RequestRejected,
        ModelRequestFailure::ServiceUnavailable,
        ModelRequestFailure::Transport,
        ModelRequestFailure::UnsupportedResponse,
    ])
}

fn legal_history(kind: u8, request: String, response: String) -> Vec<EventEnvelope> {
    let mut run_limits = limits();
    if kind == 7 {
        run_limits.max_model_steps = 0;
    } else if kind == 8 {
        run_limits.max_tool_executions = 0;
    }
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
    if kind == 7 {
        history.push(envelope(
            2,
            DomainEvent::RunFinished {
                reason: FinishReason::BudgetExhausted(BudgetKind::ModelSteps),
            },
        ));
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
    if kind == 8 {
        history.push(envelope(
            3,
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::ToolRequest(ToolRequest {
                    tool_call_id: ToolCallId::new("call-budget"),
                    name: "fixture.read".to_owned(),
                    arguments: serde_json::json!({"key": "alpha"}),
                }),
            },
        ));
        history.push(envelope(
            4,
            DomainEvent::RunFinished {
                reason: FinishReason::BudgetExhausted(BudgetKind::ToolExecutions),
            },
        ));
        return history;
    }
    if kind == 9 {
        history.push(envelope(
            3,
            DomainEvent::RunFinished {
                reason: FinishReason::Interrupted,
            },
        ));
        return history;
    }
    if kind == 10 {
        history.push(envelope(
            3,
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::ChildPanicked,
            },
        ));
        history.push(envelope(
            4,
            DomainEvent::RunFinished {
                reason: FinishReason::Failed(ModelFailure::ChildPanicked),
            },
        ));
        return history;
    }
    if kind == 11 {
        let request = ToolRequest {
            tool_call_id: ToolCallId::new("call-panicked"),
            name: "fixture.read".to_owned(),
            arguments: serde_json::json!({"key": "alpha"}),
        };
        history.push(envelope(
            3,
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::ToolRequest(request.clone()),
            },
        ));
        history.push(envelope(
            4,
            DomainEvent::ToolExecutionStarted {
                step_id: StepId::new(1),
                tool_call_id: request.tool_call_id.clone(),
                name: request.name.clone(),
                arguments: request.arguments,
                effect: ToolEffect::ReadOnly,
            },
        ));
        history.push(envelope(
            5,
            DomainEvent::ToolCallResolved {
                step_id: StepId::new(1),
                tool_call_id: request.tool_call_id,
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
    let (outcome, reason) = match kind {
        2 => (
            ModelOutcome::FinalResponse { text: response },
            FinishReason::Completed,
        ),
        3 => (
            ModelOutcome::Failed,
            FinishReason::Failed(ModelFailure::Ordinary),
        ),
        4 => (ModelOutcome::Cancelled, FinishReason::Cancelled),
        5 => (
            ModelOutcome::TimedOut,
            FinishReason::Failed(ModelFailure::Timeout),
        ),
        6 => (
            ModelOutcome::MalformedOutput,
            FinishReason::Failed(ModelFailure::MalformedOutput),
        ),
        _ => unreachable!("property kind is bounded by its strategy"),
    };
    history.push(envelope(
        3,
        DomainEvent::ModelRequestFinished {
            step_id: StepId::new(1),
            outcome,
        },
    ));
    history.push(envelope(4, DomainEvent::RunFinished { reason }));
    history
}

fn append_case(kind: u8, request: String) -> (Vec<DomainEvent>, DomainEvent) {
    let normal_limits = limits();
    let tool_request = ToolRequest {
        tool_call_id: ToolCallId::new("call-prefix"),
        name: "fixture.read".to_owned(),
        arguments: serde_json::json!({"key": "alpha"}),
    };
    let started = DomainEvent::RunStarted {
        request,
        limits: normal_limits.clone(),
    };
    let model_started = DomainEvent::ModelRequestStarted {
        step_id: StepId::new(1),
    };
    let requested_tool = DomainEvent::ModelRequestFinished {
        step_id: StepId::new(1),
        outcome: ModelOutcome::ToolRequest(tool_request.clone()),
    };
    let tool_started = DomainEvent::ToolExecutionStarted {
        step_id: StepId::new(1),
        tool_call_id: tool_request.tool_call_id.clone(),
        name: tool_request.name.clone(),
        arguments: tool_request.arguments.clone(),
        effect: ToolEffect::ReadOnly,
    };
    let tool_resolved = DomainEvent::ToolCallResolved {
        step_id: StepId::new(1),
        tool_call_id: tool_request.tool_call_id,
        outcome: ToolOutcome::Success {
            value: serde_json::json!({"value": "ok"}),
        },
    };

    match kind {
        0 => (Vec::new(), started),
        1 => (vec![started], model_started),
        2 => (
            vec![started, model_started],
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::FinalResponse {
                    text: "done".to_owned(),
                },
            },
        ),
        3 => (
            vec![
                started,
                model_started,
                DomainEvent::ModelRequestFinished {
                    step_id: StepId::new(1),
                    outcome: ModelOutcome::FinalResponse {
                        text: "done".to_owned(),
                    },
                },
            ],
            DomainEvent::RunFinished {
                reason: FinishReason::Completed,
            },
        ),
        4 => {
            let mut exhausted = normal_limits;
            exhausted.max_model_steps = 0;
            (
                vec![DomainEvent::RunStarted {
                    request: "budget".to_owned(),
                    limits: exhausted,
                }],
                DomainEvent::RunFinished {
                    reason: FinishReason::BudgetExhausted(BudgetKind::ModelSteps),
                },
            )
        }
        5 => {
            let mut exhausted = normal_limits;
            exhausted.max_tool_executions = 0;
            (
                vec![
                    DomainEvent::RunStarted {
                        request: "budget".to_owned(),
                        limits: exhausted,
                    },
                    model_started,
                    requested_tool,
                ],
                DomainEvent::RunFinished {
                    reason: FinishReason::BudgetExhausted(BudgetKind::ToolExecutions),
                },
            )
        }
        6 => (vec![started, model_started, requested_tool], tool_started),
        7 => (
            vec![started, model_started, requested_tool, tool_started],
            tool_resolved,
        ),
        8 => (
            vec![
                started,
                model_started,
                requested_tool,
                tool_started,
                tool_resolved,
            ],
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(2),
            },
        ),
        9 => (
            vec![
                started,
                model_started,
                DomainEvent::ModelRequestFinished {
                    step_id: StepId::new(1),
                    outcome: ModelOutcome::Failed,
                },
            ],
            DomainEvent::RunFinished {
                reason: FinishReason::Failed(ModelFailure::Ordinary),
            },
        ),
        _ => unreachable!("property kind is bounded by its strategy"),
    }
}

fn text_strategy(max_length: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..max_length)
        .prop_map(|characters| characters.into_iter().collect())
}

#[test]
fn torn_final_jsonl_record_reports_the_valid_prefix() {
    let history = completed_history("request".to_owned(), "done".to_owned());
    let mut bytes = Vec::new();
    bytes.extend(encode_envelope(&history[0]).expect("first envelope encodes"));
    let mut unterminated = encode_envelope(&history[1]).expect("tail envelope encodes");
    assert_eq!(unterminated.pop(), Some(b'\n'));
    bytes.extend(unterminated);

    let decoded = decode_jsonl(&bytes).expect("a torn tail is a diagnostic, not corruption");

    assert!(decoded.has_incomplete_tail());
    assert_eq!(decoded.envelopes(), &history[..1]);
    assert_eq!(
        decoded.incomplete_tail(),
        Some(
            &bytes[bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .expect("prefix ends")
                + 1..]
        )
    );
}

#[test]
fn corrupted_interior_jsonl_record_rejects_the_log() {
    let history = completed_history("request".to_owned(), "done".to_owned());
    let mut bytes = Vec::new();
    bytes.extend(encode_envelope(&history[0]).expect("first envelope encodes"));
    bytes.extend(b"not-json\n");
    bytes.extend(encode_envelope(&history[1]).expect("second envelope encodes"));

    let error = decode_jsonl(&bytes).expect_err("interior corruption must fail");

    assert!(matches!(error, DecodeError::CorruptRecord { line: 2, .. }));
}

#[test]
fn unknown_schema_version_is_rejected() {
    let mut value = envelope(
        1,
        DomainEvent::RunStarted {
            request: "request".to_owned(),
            limits: limits(),
        },
    );
    value.schema_version = 99;
    let line = serde_json::to_vec(&value).expect("test envelope serializes");

    let error = decode_envelope_line(&line, 1).expect_err("unknown schema must fail");

    assert_eq!(
        error.to_string(),
        "line 1 uses unsupported event schema version 99"
    );
}

#[test]
fn homogeneous_schema_version_1_jsonl_decodes() {
    let mut history = completed_history("request".to_owned(), "done".to_owned());
    for envelope in &mut history {
        envelope.schema_version = 1;
    }
    let encoded: Vec<u8> = history
        .iter()
        .flat_map(|envelope| encode_envelope(envelope).expect("version 1 envelope encodes"))
        .collect();

    let decoded = decode_jsonl(&encoded).expect("homogeneous version 1 history decodes");

    assert_eq!(decoded.envelopes(), history);
}

fn assert_mixed_schema_version_jsonl_is_malformed_history(first: u32, second: u32) {
    let mut history = completed_history("request".to_owned(), "done".to_owned());
    history[0].schema_version = first;
    history[1].schema_version = second;
    let encoded: Vec<u8> = history[..2]
        .iter()
        .flat_map(|envelope| encode_envelope(envelope).expect("test envelope encodes"))
        .collect();

    let error = decode_jsonl(&encoded).expect_err("mixed history must fail");

    assert_eq!(
        error,
        DecodeError::MalformedHistory(aurora_core::ProjectionError::SchemaVersionChanged {
            sequence: 2,
            expected: first,
            actual: second,
        })
    );
}

#[test]
fn mixed_schema_version_1_to_2_jsonl_is_malformed_history() {
    assert_mixed_schema_version_jsonl_is_malformed_history(1, 2);
}

#[test]
fn mixed_schema_version_2_to_1_jsonl_is_malformed_history() {
    assert_mixed_schema_version_jsonl_is_malformed_history(2, 1);
}

fn assert_schema_version_1_event_is_a_codec_violation(event: DomainEvent) {
    let mut event = envelope(3, event);
    event.schema_version = 1;
    let encoded = encode_envelope(&event).expect("test envelope encodes");

    assert_eq!(
        decode_envelope_line(
            encoded.strip_suffix(b"\n").expect("codec appends newline"),
            1,
        )
        .expect_err("version 1 cannot use request failure"),
        DecodeError::SchemaViolation {
            line: 1,
            version: 1,
        }
    );
}

#[test]
fn schema_version_1_model_request_finished_failure_is_a_codec_violation() {
    assert_schema_version_1_event_is_a_codec_violation(DomainEvent::ModelRequestFinished {
        step_id: StepId::new(1),
        outcome: ModelOutcome::RequestFailure(ModelRequestFailure::Authentication),
    });
}

#[test]
fn schema_version_1_run_finished_request_failure_is_a_codec_violation() {
    assert_schema_version_1_event_is_a_codec_violation(DomainEvent::RunFinished {
        reason: FinishReason::Failed(ModelFailure::Request(ModelRequestFailure::Authentication)),
    });
}

#[tokio::test]
async fn jsonl_append_preserves_bytes_and_supplied_envelopes() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("run.jsonl");
    let mut store = JsonlEventStore::create(&path)
        .await
        .expect("store is created");
    let run_id = RunId::new("run-store");

    let first = EventEnvelope {
        schema_version: SCHEMA_VERSION,
        sequence: EventSeq::new(1),
        run_id: run_id.clone(),
        observed_at: "2026-01-01T00:00:00Z".to_owned(),
        event: DomainEvent::RunStarted {
            request: "request".to_owned(),
            limits: limits(),
        },
    };
    store
        .commit(first.clone())
        .await
        .expect("first record commits");
    let first_bytes = fs::read(&path).expect("first prefix is readable");
    let second = EventEnvelope {
        schema_version: SCHEMA_VERSION,
        sequence: EventSeq::new(2),
        run_id,
        observed_at: "2026-01-01T00:00:01Z".to_owned(),
        event: DomainEvent::ModelRequestStarted {
            step_id: StepId::new(1),
        },
    };
    store
        .commit(second.clone())
        .await
        .expect("second record commits");
    let complete_bytes = fs::read(&path).expect("complete log is readable");

    assert_eq!(first.sequence, EventSeq::new(1));
    assert_eq!(second.sequence, EventSeq::new(2));
    assert!(complete_bytes.starts_with(&first_bytes));
    assert_eq!(store.acknowledged(), &[first, second]);
    store.close().await.expect("store worker joins");
}

#[test]
fn in_memory_store_retains_a_supplied_different_run_identifier() {
    let mut store = InMemoryEventStore::new();
    complete_immediately(store.commit(EventEnvelope {
        schema_version: SCHEMA_VERSION,
        sequence: EventSeq::new(1),
        run_id: RunId::new("run-one"),
        observed_at: "2026-01-01T00:00:00Z".to_owned(),
        event: DomainEvent::RunStarted {
            request: "request".to_owned(),
            limits: limits(),
        },
    }))
    .expect("first record commits");

    complete_immediately(store.commit(EventEnvelope {
        schema_version: SCHEMA_VERSION,
        sequence: EventSeq::new(2),
        run_id: RunId::new("run-two"),
        observed_at: "2026-01-01T00:00:01Z".to_owned(),
        event: DomainEvent::ModelRequestStarted {
            step_id: StepId::new(1),
        },
    }))
    .expect("stores retain supplied envelopes without interpreting run identity");

    assert_eq!(store.acknowledged().len(), 2);
}

#[tokio::test]
async fn jsonl_store_allows_only_one_writer_handle() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("exclusive.jsonl");
    let first = JsonlEventStore::create(&path)
        .await
        .expect("first writer owns the log");

    let error = JsonlEventStore::open(&path)
        .await
        .expect_err("a second writer must be refused");

    assert!(matches!(error, StoreError::WriterUnavailable));
    first.close().await.expect("first worker joins");
    JsonlEventStore::open(&path)
        .await
        .expect("the lock is released with its writer")
        .close()
        .await
        .expect("replacement worker joins");
}

proptest! {
    #[test]
    fn request_failure_categories_round_trip_without_provider_detail(
        category in model_request_failure_strategy(),
    ) {
        let outcome = envelope(
            3,
            DomainEvent::ModelRequestFinished {
                step_id: StepId::new(1),
                outcome: ModelOutcome::RequestFailure(category),
            },
        );
        let terminal = envelope(
            4,
            DomainEvent::RunFinished {
                reason: FinishReason::Failed(ModelFailure::Request(category)),
            },
        );

        for candidate in [outcome, terminal] {
            let encoded = encode_envelope(&candidate).expect("envelope encodes");
            let decoded = decode_envelope_line(
                encoded.strip_suffix(b"\n").expect("codec appends newline"),
                1,
            )
            .expect("schema version 2 decodes");
            prop_assert_eq!(decoded, candidate);
        }
    }

    #[test]
    fn phase_1c_compatible_histories_project_equally_at_versions_1_and_2(
        kind in 0_u8..12,
        request in "[a-zA-Z0-9 ]{0,48}",
        response in "[a-zA-Z0-9 ]{0,48}",
    ) {
        let version_2 = legal_history(kind, request, response);
        let mut version_1 = version_2.clone();
        for envelope in &mut version_1 {
            envelope.schema_version = 1;
        }

        prop_assert_eq!(
            reconstruct(&version_1).expect("version 1 history reconstructs"),
            reconstruct(&version_2).expect("version 2 history reconstructs"),
        );
    }

    #[test]
    fn envelope_codec_round_trip_is_canonical(
        request in text_strategy(257),
        model_steps in prop_oneof![Just(0), Just(u32::MAX), any::<u32>()],
        tool_executions in prop_oneof![Just(0), Just(u32::MAX), any::<u32>()],
        model_timeout_ms in prop_oneof![Just(0), Just(u64::MAX), any::<u64>()],
        tool_timeout_ms in prop_oneof![Just(0), Just(u64::MAX), any::<u64>()],
        shutdown_grace_period_ms in prop_oneof![Just(0), Just(u64::MAX), any::<u64>()],
    ) {
        let value = envelope(
            1,
            DomainEvent::RunStarted {
                request,
                limits: RunLimits {
                    max_model_steps: model_steps,
                    max_tool_executions: tool_executions,
                    model_timeout_ms,
                    tool_timeout_ms,
                    shutdown_grace_period_ms,
                },
            },
        );

        let encoded = encode_envelope(&value).expect("generated envelope encodes");
        let decoded = decode_envelope_line(&encoded[..encoded.len() - 1], 1)
            .expect("generated envelope decodes");
        let reencoded = encode_envelope(&decoded).expect("decoded envelope re-encodes");

        prop_assert_eq!(&decoded, &value);
        prop_assert_eq!(reencoded, encoded);
    }

    #[test]
    fn nested_json_and_boundary_identifiers_round_trip_without_normalization(
        run_id in text_strategy(129),
        tool_call_id in text_strategy(129),
        text in text_strategy(129),
        sequence in prop_oneof![Just(1u64), Just(u64::MAX), 1u64..=u64::MAX],
        step in prop_oneof![Just(1u64), Just(u64::MAX), 1u64..=u64::MAX],
        numbers in prop::collection::vec(any::<i64>(), 0..17),
    ) {
        let value = EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence: EventSeq::new(sequence),
            run_id: RunId::new(run_id),
            observed_at: "2026-01-01T00:00:00Z".to_owned(),
            event: DomainEvent::ToolCallResolved {
                step_id: StepId::new(step),
                tool_call_id: ToolCallId::new(tool_call_id),
                outcome: ToolOutcome::Success {
                    value: serde_json::json!({
                        "nested": {
                            "text": text,
                            "numbers": numbers,
                            "enabled": true,
                            "missing": null
                        }
                    }),
                },
            },
        };

        let encoded = encode_envelope(&value).expect("boundary envelope encodes");
        let decoded = decode_envelope_line(&encoded[..encoded.len() - 1], 1)
            .expect("boundary envelope decodes");

        prop_assert_eq!(&decoded, &value);
        prop_assert_eq!(
            encode_envelope(&decoded).expect("boundary envelope re-encodes"),
            encoded
        );
    }

    #[test]
    fn every_domain_event_variant_has_a_stable_envelope_codec(
        event in domain_event_strategy(),
    ) {
        let value = envelope(1, event);
        let encoded = encode_envelope(&value).expect("generated envelope encodes");
        let decoded = decode_envelope_line(&encoded[..encoded.len() - 1], 1)
            .expect("generated envelope decodes");

        prop_assert_eq!(&decoded, &value);
        prop_assert_eq!(
            encode_envelope(&decoded).expect("decoded envelope encodes"),
            encoded
        );
    }

    #[test]
    fn projection_is_deterministic_across_timestamp_changes(
        kind in 0u8..12,
        request in "[^\\p{C}]{0,64}",
        response in "[^\\p{C}]{0,64}",
    ) {
        let history = legal_history(kind, request, response);
        let mut retimestamped = history.clone();
        for event in &mut retimestamped {
            event.observed_at = "2035-12-31T23:59:59Z".to_owned();
        }

        let first = reconstruct(&history).expect("generated history is valid");
        let repeated = reconstruct(&history).expect("repeated reconstruction is valid");
        let changed = reconstruct(&retimestamped).expect("retimestamped history is valid");

        prop_assert_eq!(&first, &repeated);
        prop_assert_eq!(first, changed);
    }

    #[test]
    fn legal_history_codec_round_trip_preserves_its_projection(
        kind in 0u8..12,
        request in text_strategy(129),
        response in text_strategy(129),
    ) {
        let history = legal_history(kind, request, response);
        let encoded: Vec<u8> = history
            .iter()
            .flat_map(|event| encode_envelope(event).expect("history event encodes"))
            .collect();
        let decoded = decode_jsonl(&encoded).expect("generated legal history decodes");
        let decoded_history = decoded.envelopes();

        prop_assert!(!decoded.has_incomplete_tail());
        prop_assert_eq!(
            reconstruct(&history).expect("generated history projects"),
            reconstruct(decoded_history).expect("decoded history projects")
        );
        let reencoded: Vec<u8> = decoded_history
            .iter()
            .flat_map(|event| encode_envelope(event).expect("decoded event re-encodes"))
            .collect();
        prop_assert_eq!(reencoded, encoded);
    }

    #[test]
    fn appending_a_legal_event_preserves_the_serialized_prefix(
        kind in 0u8..10,
        request in "[^\\p{C}]{0,64}",
    ) {
        let run_id = RunId::new("run-prefix");
        let mut store = InMemoryEventStore::new();
        let (prefix_events, next_event) = append_case(kind, request);
        for event in prefix_events {
            commit_event(&mut store, &run_id, "2026-01-01T00:00:00Z", event)
                .expect("generated prefix event commits");
        }
        let prefix: Vec<u8> = store
            .acknowledged()
            .iter()
            .flat_map(|event| encode_envelope(event).expect("prefix encodes"))
            .collect();

        let expected_sequence = store.acknowledged().len() as u64 + 1;
        let appended = commit_event(&mut store, &run_id, "2026-01-01T00:00:01Z", next_event)
            .expect("generated legal next event commits");
        let complete: Vec<u8> = store
            .acknowledged()
            .iter()
            .flat_map(|event| encode_envelope(event).expect("history encodes"))
            .collect();

        prop_assert!(complete.starts_with(&prefix));
        prop_assert_eq!(appended.sequence, EventSeq::new(expected_sequence));
        prop_assert_eq!(appended.run_id, run_id);
    }

    #[test]
    fn store_retains_a_supplied_envelope_without_revalidating_the_transition(
        request in text_strategy(129),
        response in text_strategy(129),
    ) {
        let run_id = RunId::new("run-terminal-prefix");
        let mut store = InMemoryEventStore::new();
        for envelope in completed_history(request, response) {
            commit_event(
                &mut store,
                &run_id,
                &envelope.observed_at,
                envelope.event,
            )
                .expect("generated terminal prefix commits");
        }
        let prefix = store.acknowledged().to_vec();

        let extension = EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence: EventSeq::new(prefix.len() as u64 + 1),
            run_id: run_id.clone(),
            observed_at: "2026-01-01T00:00:01Z".to_owned(),
            event: DomainEvent::ModelRequestStarted {
                step_id: StepId::new(2),
            },
        };
        complete_immediately(store.commit(extension.clone()))
            .expect("stores do not validate domain transitions");

        prop_assert_eq!(&store.acknowledged()[..prefix.len()], prefix.as_slice());
        prop_assert_eq!(store.acknowledged().last(), Some(&extension));
    }
}
