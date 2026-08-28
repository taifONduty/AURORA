use std::{fs, process::Command};

use aurora_core::{
    DomainEvent, EventEnvelope, EventSeq, FinishReason, ModelFailure, ModelOutcome, RunId,
    RunLimits, SCHEMA_VERSION, StepId, encode_envelope,
};
use tempfile::tempdir;

fn aurora() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aurora"))
}

fn output_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn no_subcommand_is_a_usage_error() {
    let output = aurora().output().expect("aurora starts");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(output_text(&output.stderr).contains("Usage: aurora <COMMAND>"));
}

#[test]
fn run_without_prompt_is_a_usage_error() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("missing-prompt.jsonl");
    let output = aurora()
        .args(["run", "--output"])
        .arg(&path)
        .output()
        .expect("aurora starts");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(output_text(&output.stderr).contains("--prompt <PROMPT>"));
    assert!(!path.exists());
}

#[test]
fn missing_api_key_is_a_configuration_error_without_creating_output() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("missing-key.jsonl");
    let model_secret = "model-value-that-must-not-be-echoed";
    let output = aurora()
        .args(["run", "--prompt", "Explain ownership", "--output"])
        .arg(&path)
        .env_remove("OPENAI_API_KEY")
        .env("AURORA_OPENAI_MODEL", model_secret)
        .output()
        .expect("aurora starts");
    let stderr = output_text(&output.stderr);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(stderr.contains("OPENAI_API_KEY is not set"));
    assert!(!stderr.contains(model_secret));
    assert!(!path.exists());
}

#[test]
fn missing_model_is_a_configuration_error_without_revealing_the_api_key() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("missing-model.jsonl");
    let api_key_secret = "api-key-that-must-not-be-echoed";
    let output = aurora()
        .args(["run", "--prompt", "Explain ownership", "--output"])
        .arg(&path)
        .env("OPENAI_API_KEY", api_key_secret)
        .env_remove("AURORA_OPENAI_MODEL")
        .output()
        .expect("aurora starts");
    let stderr = output_text(&output.stderr);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(stderr.contains("AURORA_OPENAI_MODEL is not set"));
    assert!(!stderr.contains(api_key_secret));
    assert!(!path.exists());
}

#[test]
fn inspect_rejects_json_output() {
    let output = aurora()
        .args(["inspect", "run.jsonl", "--json"])
        .output()
        .expect("aurora starts");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(output_text(&output.stderr).contains("unexpected argument '--json'"));
}

#[test]
fn inspect_reports_a_clean_active_log_without_changing_it() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("active.jsonl");
    let bytes = encoded_history(&active_history());
    fs::write(&path, &bytes).expect("active log is written");

    let output = aurora()
        .arg("inspect")
        .arg(&path)
        .output()
        .expect("aurora starts");
    let stdout = output_text(&output.stdout);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert!(stdout.contains("Run:             run-cli-acceptance"));
    assert!(stdout.contains("Status:          active"));
    assert!(stdout.contains("Events:          1"));
    assert!(stdout.contains("Model steps:     0"));
    assert!(stdout.contains("Tool executions: 0"));
    assert_eq!(fs::read(&path).expect("active log remains readable"), bytes);
}

#[test]
fn inspect_reports_a_clean_failed_log_as_a_valid_run() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("failed.jsonl");
    let bytes = encoded_history(&failed_history());
    fs::write(&path, &bytes).expect("failed log is written");

    let output = aurora()
        .arg("inspect")
        .arg(&path)
        .output()
        .expect("aurora starts");
    let stdout = output_text(&output.stdout);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert!(stdout.contains("Run:             run-cli-acceptance"));
    assert!(stdout.contains("Status:          failed (model failure)"));
    assert!(stdout.contains("Events:          4"));
    assert!(stdout.contains("Model steps:     1"));
    assert!(stdout.contains("Tool executions: 0"));
    assert_eq!(fs::read(&path).expect("failed log remains readable"), bytes);
}

#[test]
fn inspect_reports_an_incomplete_tail_and_preserves_the_valid_prefix() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("incomplete.jsonl");
    let mut bytes = encoded_history(&active_history());
    let tail = b"{\"schema_version\":";
    bytes.extend_from_slice(tail);
    fs::write(&path, &bytes).expect("incomplete log is written");

    let output = aurora()
        .arg("inspect")
        .arg(&path)
        .output()
        .expect("aurora starts");
    let stdout = output_text(&output.stdout);
    let stderr = output_text(&output.stderr);

    assert_eq!(output.status.code(), Some(1));
    assert!(stdout.contains("Status:          active"));
    assert!(stdout.contains(&format!("Incomplete tail: {} bytes", tail.len())));
    assert!(stderr.contains("inspection found an incomplete JSONL tail"));
    assert_eq!(
        fs::read(&path).expect("incomplete log remains readable"),
        bytes
    );
}

#[test]
fn inspect_rejects_a_corrupt_log_without_changing_it() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("corrupt.jsonl");
    let bytes = b"not-json\n";
    fs::write(&path, bytes).expect("corrupt log is written");

    let output = aurora()
        .arg("inspect")
        .arg(&path)
        .output()
        .expect("aurora starts");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(output_text(&output.stderr).contains("inspection failed:"));
    assert_eq!(
        fs::read(&path).expect("corrupt log remains readable"),
        bytes
    );
}

#[test]
fn run_never_overwrites_an_existing_output() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("existing.jsonl");
    let sentinel = b"operator-owned bytes";
    fs::write(&path, sentinel).expect("sentinel file is written");

    let output = aurora()
        .args(["run", "--prompt", "Explain ownership", "--output"])
        .arg(&path)
        .env("OPENAI_API_KEY", "test-key")
        .env("AURORA_OPENAI_MODEL", "test-model")
        .output()
        .expect("aurora starts");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(output_text(&output.stderr).contains("run log could not be created:"));
    assert_eq!(
        fs::read(&path).expect("existing output remains readable"),
        sentinel
    );
}

#[test]
#[ignore = "requires OPENAI_API_KEY, AURORA_OPENAI_MODEL, network, and credits"]
fn configured_run_creates_an_inspectable_log() {
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("live.jsonl");
    let run = aurora()
        .args([
            "run",
            "--prompt",
            "Reply with one short sentence about Rust ownership.",
            "--output",
        ])
        .arg(&path)
        .output()
        .expect("aurora starts");

    assert_eq!(
        run.status.code(),
        Some(0),
        "run stderr: {}",
        output_text(&run.stderr)
    );
    assert!(!run.stdout.is_empty());

    let inspection = aurora()
        .arg("inspect")
        .arg(&path)
        .output()
        .expect("aurora starts");

    assert_eq!(
        inspection.status.code(),
        Some(0),
        "inspect stderr: {}",
        output_text(&inspection.stderr)
    );
    assert!(
        output_text(&inspection.stdout).contains("Status:          completed"),
        "inspect stdout: {}",
        output_text(&inspection.stdout)
    );
}

fn active_history() -> Vec<EventEnvelope> {
    vec![envelope(
        1,
        DomainEvent::RunStarted {
            request: "Inspect this run".to_owned(),
            limits: RunLimits {
                max_model_steps: 1,
                max_tool_executions: 0,
                model_timeout_ms: 1_000,
                tool_timeout_ms: 1_000,
                shutdown_grace_period_ms: 100,
            },
        },
    )]
}

fn failed_history() -> Vec<EventEnvelope> {
    let mut history = active_history();
    history.extend([
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
                outcome: ModelOutcome::Failed,
            },
        ),
        envelope(
            4,
            DomainEvent::RunFinished {
                reason: FinishReason::Failed(ModelFailure::Ordinary),
            },
        ),
    ]);
    history
}

fn envelope(sequence: u64, event: DomainEvent) -> EventEnvelope {
    EventEnvelope {
        schema_version: SCHEMA_VERSION,
        sequence: EventSeq::new(sequence),
        run_id: RunId::new("run-cli-acceptance"),
        observed_at: "2026-08-29T00:00:00Z".to_owned(),
        event,
    }
}

fn encoded_history(history: &[EventEnvelope]) -> Vec<u8> {
    history
        .iter()
        .flat_map(|event| encode_envelope(event).expect("event encodes"))
        .collect()
}
