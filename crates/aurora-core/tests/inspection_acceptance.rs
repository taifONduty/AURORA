use std::{fs, process::Command};

use aurora_core::{
    DecodeError, DomainEvent, EventEnvelope, EventSeq, EventStore, FinishReason,
    InMemoryEventStore, Inspection, InspectionError, JsonlEventStore, ModelOutcome,
    ProjectionError, RecoveryError, RunId, RunLifecycle, RunLimits, SCHEMA_VERSION, StepId,
    StoreError, ToolCallId, ToolEffect, ToolOutcome, ToolRequest, encode_envelope, inspect_jsonl,
    reconstruct, recover_as_interrupted,
};
use serde_json::json;
use tempfile::tempdir;

const INSPECTION_CHILD_PATH: &str = "AURORA_INSPECTION_CHILD_PATH";
const INSPECTION_CHILD_VIEW: &str = "AURORA_INSPECTION_CHILD_VIEW";

fn limits() -> RunLimits {
    RunLimits {
        max_model_steps: 2,
        max_tool_executions: 1,
        model_timeout_ms: 1_000,
        tool_timeout_ms: 1_000,
        shutdown_grace_period_ms: 100,
    }
}

async fn commit(
    store: &mut dyn EventStore,
    run_id: &RunId,
    event: DomainEvent,
) -> Result<(), aurora_core::StoreError> {
    store
        .commit(EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence: EventSeq::new(store.acknowledged().len() as u64 + 1),
            run_id: run_id.clone(),
            observed_at: "2026-01-01T00:00:00Z".to_owned(),
            event,
        })
        .await
}

async fn commit_clean_tool_run(store: &mut dyn EventStore, run_id: &RunId) {
    let request = ToolRequest {
        tool_call_id: ToolCallId::new("call-1"),
        name: "fixture.read".to_owned(),
        arguments: json!({"key": "alpha"}),
    };
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
            outcome: ModelOutcome::ToolRequest(request.clone()),
        },
        DomainEvent::ToolExecutionStarted {
            step_id: StepId::new(1),
            tool_call_id: request.tool_call_id.clone(),
            name: request.name,
            arguments: request.arguments,
            effect: ToolEffect::ReadOnly,
        },
        DomainEvent::ToolCallResolved {
            step_id: StepId::new(1),
            tool_call_id: request.tool_call_id,
            outcome: ToolOutcome::Success {
                value: json!({"value": "fixture"}),
            },
        },
        DomainEvent::ModelRequestStarted {
            step_id: StepId::new(2),
        },
        DomainEvent::ModelRequestFinished {
            step_id: StepId::new(2),
            outcome: ModelOutcome::FinalResponse {
                text: "done".to_owned(),
            },
        },
        DomainEvent::RunFinished {
            reason: FinishReason::Completed,
        },
    ];
    for event in events {
        commit(store, run_id, event)
            .await
            .expect("fixture event commits");
    }
}

#[tokio::test]
async fn acceptance_17_clean_shutdown_reconstructs_without_mutating_jsonl() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("run.jsonl");
    let run_id = RunId::new("run-inspection");
    let mut store = JsonlEventStore::create(&path)
        .await
        .expect("store is created");
    commit_clean_tool_run(&mut store, &run_id).await;
    let expected = reconstruct(store.acknowledged()).expect("live view reconstructs");
    store.close().await.expect("store worker joins");
    let bytes_before = fs::read(&path).expect("log is readable");

    let output = Command::new(std::env::current_exe().expect("test executable is known"))
        .args(["--ignored", "--exact", "fresh_process_inspection_helper"])
        .env(INSPECTION_CHILD_PATH, &path)
        .env(INSPECTION_CHILD_VIEW, format!("{expected:?}"))
        .output()
        .expect("fresh inspection process starts");
    let bytes_after = fs::read(&path).expect("log remains readable");

    assert!(
        output.status.success(),
        "fresh inspection process failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(bytes_after, bytes_before);
}

#[test]
#[ignore = "invoked by acceptance_17 in a separate process"]
fn fresh_process_inspection_helper() {
    let path = std::env::var_os(INSPECTION_CHILD_PATH).expect("child log path is supplied");
    let expected = std::env::var(INSPECTION_CHILD_VIEW).expect("child projection is supplied");
    let bytes_before = fs::read(&path).expect("child reads the log");

    let inspected = inspect_jsonl(&path).expect("child reconstructs the clean log");

    let Inspection::Clean(view) = inspected else {
        panic!("complete log cannot have an incomplete tail");
    };
    assert_eq!(format!("{view:?}"), expected);
    assert_eq!(fs::read(path).expect("child rereads the log"), bytes_before);
}

#[tokio::test]
async fn acceptance_18_interrupted_recovery_appends_only_terminal_interruption() {
    let run_id = RunId::new("run-recovery");
    let request = ToolRequest {
        tool_call_id: ToolCallId::new("call-1"),
        name: "fixture.read".to_owned(),
        arguments: json!({"key": "alpha"}),
    };
    let mut store = InMemoryEventStore::new();
    for event in [
        DomainEvent::RunStarted {
            request: "request".to_owned(),
            limits: limits(),
        },
        DomainEvent::ModelRequestStarted {
            step_id: StepId::new(1),
        },
        DomainEvent::ModelRequestFinished {
            step_id: StepId::new(1),
            outcome: ModelOutcome::ToolRequest(request.clone()),
        },
        DomainEvent::ToolExecutionStarted {
            step_id: StepId::new(1),
            tool_call_id: request.tool_call_id,
            name: request.name,
            arguments: request.arguments,
            effect: ToolEffect::ReadOnly,
        },
    ] {
        commit(&mut store, &run_id, event)
            .await
            .expect("fixture event commits");
    }

    let view = recover_as_interrupted(&mut store, &run_id, "2026-01-01T00:00:01Z")
        .await
        .expect("explicit recovery closes the run");

    assert_eq!(store.acknowledged().len(), 5);
    assert!(matches!(
        store.acknowledged()[4].event,
        DomainEvent::RunFinished {
            reason: FinishReason::Interrupted
        }
    ));
    assert_eq!(view.lifecycle, RunLifecycle::Terminal);
    assert_eq!(view.finish_reason, Some(FinishReason::Interrupted));
    assert!(
        view.pending_operation
            .expect("unknown tool outcome remains visible")
            .execution_started()
    );
}

#[tokio::test]
async fn recovery_empty_history_returns_typed_projection_error_without_mutating_store() {
    let run_id = RunId::new("run-empty-recovery");
    let mut store = InMemoryEventStore::new();
    let before = store.acknowledged().to_vec();

    let error = recover_as_interrupted(&mut store, &run_id, "2026-01-01T00:00:00Z")
        .await
        .expect_err("empty history cannot be recovered");

    assert!(matches!(
        error,
        RecoveryError::Projection(ProjectionError::EmptyHistory)
    ));
    assert_eq!(store.acknowledged(), before);
}

#[tokio::test]
async fn acceptance_19_torn_tail_inspection_returns_diagnostic_and_does_not_write() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("torn.jsonl");
    let run_id = RunId::new("run-torn");
    let mut store = JsonlEventStore::create(&path)
        .await
        .expect("store is created");
    commit(
        &mut store,
        &run_id,
        DomainEvent::RunStarted {
            request: "request".to_owned(),
            limits: limits(),
        },
    )
    .await
    .expect("prefix commits");
    store.close().await.expect("store worker joins");
    let mut bytes = fs::read(&path).expect("prefix is readable");
    let tail_envelope = EventEnvelope {
        schema_version: SCHEMA_VERSION,
        sequence: EventSeq::new(2),
        run_id,
        observed_at: "2026-01-01T00:00:01Z".to_owned(),
        event: DomainEvent::ModelRequestStarted {
            step_id: StepId::new(1),
        },
    };
    let mut tail = encode_envelope(&tail_envelope).expect("tail envelope encodes");
    assert_eq!(tail.pop(), Some(b'\n'));
    bytes.extend(&tail);
    fs::write(&path, &bytes).expect("test creates a torn tail");

    let open_error = JsonlEventStore::open(&path)
        .await
        .expect_err("a writer cannot append after a torn tail");
    assert!(matches!(open_error, StoreError::IncompleteTail));

    let inspected = inspect_jsonl(&path).expect("torn tail exposes its valid prefix");
    let after = fs::read(&path).expect("torn log remains readable");

    match inspected {
        Inspection::IncompleteTail { prefix, tail } => {
            assert_eq!(
                prefix.expect("valid prefix projects").last_sequence.get(),
                1
            );
            assert_eq!(tail, tail_envelope_bytes(&tail_envelope));
        }
        Inspection::Clean(_) => panic!("torn log cannot be reported as clean"),
    }
    assert_eq!(after, bytes);
}

#[tokio::test]
async fn acceptance_20_corrupted_interior_record_produces_no_projection() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("corrupt.jsonl");
    let run_id = RunId::new("run-corrupt");
    let mut store = JsonlEventStore::create(&path)
        .await
        .expect("store is created");
    commit(
        &mut store,
        &run_id,
        DomainEvent::RunStarted {
            request: "request".to_owned(),
            limits: limits(),
        },
    )
    .await
    .expect("prefix commits");
    store.close().await.expect("store worker joins");
    let mut bytes = fs::read(&path).expect("prefix is readable");
    bytes.extend(b"not-json\n");
    fs::write(&path, &bytes).expect("test creates interior corruption");

    let error = inspect_jsonl(&path).expect_err("corruption must reject inspection");

    assert!(matches!(error, InspectionError::Decode(_)));
    assert_eq!(fs::read(&path).expect("log remains readable"), bytes);
}

#[test]
fn semantic_interior_corruption_produces_no_projection() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("semantic-corrupt.jsonl");
    let run_id = RunId::new("run-semantic-corrupt");
    let history = [
        EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence: EventSeq::new(1),
            run_id: run_id.clone(),
            observed_at: "2026-01-01T00:00:00Z".to_owned(),
            event: DomainEvent::RunStarted {
                request: "request".to_owned(),
                limits: limits(),
            },
        },
        EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence: EventSeq::new(3),
            run_id,
            observed_at: "2026-01-01T00:00:01Z".to_owned(),
            event: DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        },
    ];
    let bytes: Vec<u8> = history
        .iter()
        .flat_map(|event| encode_envelope(event).expect("event encodes"))
        .collect();
    fs::write(&path, &bytes).expect("test creates semantic corruption");

    let error = inspect_jsonl(&path).expect_err("semantic corruption must reject inspection");

    assert!(matches!(
        error,
        InspectionError::Decode(DecodeError::MalformedHistory(ProjectionError::Sequence {
            expected: 2,
            actual: 3
        }))
    ));
    assert_eq!(fs::read(&path).expect("log remains readable"), bytes);
}

fn tail_envelope_bytes(envelope: &EventEnvelope) -> Vec<u8> {
    let mut bytes = encode_envelope(envelope).expect("tail envelope encodes");
    assert_eq!(bytes.pop(), Some(b'\n'));
    bytes
}
