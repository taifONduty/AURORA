use std::{
    ffi::OsString,
    future::Future,
    io,
    path::{Path, PathBuf},
};

use aurora_core::{
    AuthorizationDecision, Authorizer, FinishReason, JsonlEventStore, ModelBackend, RunDriver,
    RunId, RunLifecycle, RunLimits, RunStart, RunView, ToolAuthorization, ToolCatalog,
};
use aurora_openai::{OpenAiBackend, OpenAiConfig};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{CommandReport, EXIT_CANCELLED, EXIT_FAILURE, EXIT_OK, finish_reason_label};

#[derive(Debug)]
struct OperatorEnvironment {
    api_key: String,
    model: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InterruptObservation {
    CtrlCObserved,
    WatcherShutdown,
}

enum RunOutcome {
    Durable(Box<RunView>),
    DriverFailed(String),
}

type WatcherOutcome = Result<InterruptObservation, String>;
type CloseOutcome = Result<(), String>;

#[derive(Debug)]
struct DenyTools;

impl Authorizer for DenyTools {
    fn authorize(&self, _request: &ToolAuthorization<'_>) -> AuthorizationDecision {
        AuthorizationDecision::Deny
    }
}

pub(crate) async fn execute(prompt: String, output: PathBuf) -> CommandReport {
    let environment = match read_operator_environment(|name| std::env::var_os(name)) {
        Ok(environment) => environment,
        Err(error) => return CommandReport::configuration(error),
    };
    let config = match OpenAiConfig::new(environment.api_key, environment.model) {
        Ok(config) => config,
        Err(error) => {
            return CommandReport::configuration(format!(
                "OpenAI configuration is invalid: {error}"
            ));
        }
    };
    let mut backend = match OpenAiBackend::new(config) {
        Ok(backend) => backend,
        Err(error) => {
            return CommandReport::failed(
                String::new(),
                format!("OpenAI backend could not be created: {error}"),
            );
        }
    };

    execute_with_backend(
        prompt,
        output,
        &mut backend,
        run_limits(),
        current_timestamp,
        tokio::signal::ctrl_c(),
    )
    .await
}

fn read_operator_environment<F>(mut lookup: F) -> Result<OperatorEnvironment, String>
where
    F: FnMut(&str) -> Option<OsString>,
{
    let api_key = read_environment_value("OPENAI_API_KEY", &mut lookup)?;
    let model = read_environment_value("AURORA_OPENAI_MODEL", &mut lookup)?;
    Ok(OperatorEnvironment { api_key, model })
}

fn read_environment_value<F>(name: &str, lookup: &mut F) -> Result<String, String>
where
    F: FnMut(&str) -> Option<OsString>,
{
    let value = lookup(name).ok_or_else(|| format!("{name} is not set"))?;
    let value = value
        .into_string()
        .map_err(|_| format!("{name} is not valid Unicode"))?;
    if value.is_empty() {
        return Err(format!("{name} is empty"));
    }
    Ok(value)
}

fn run_limits() -> RunLimits {
    RunLimits {
        max_model_steps: 1,
        max_tool_executions: 0,
        model_timeout_ms: 120_000,
        tool_timeout_ms: 1_000,
        shutdown_grace_period_ms: 2_000,
    }
}

fn resolve_output_path(path: &Path, current: &Path) -> PathBuf {
    match path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(_) => path.to_owned(),
        None => current.join(path),
    }
}

fn new_run_id() -> RunId {
    RunId::new(Uuid::new_v4().to_string())
}

fn current_timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("UTC timestamps can be formatted as RFC 3339")
}

async fn watch_interrupt<S>(
    signal: S,
    run_token: CancellationToken,
    shutdown: CancellationToken,
) -> Result<InterruptObservation, String>
where
    S: Future<Output = io::Result<()>>,
{
    tokio::pin!(signal);
    tokio::select! {
        result = &mut signal => match result {
            Ok(()) => {
                run_token.cancel();
                Ok(InterruptObservation::CtrlCObserved)
            }
            Err(error) => {
                run_token.cancel();
                Err(format!("Ctrl-C watcher failed: {error}"))
            }
        },
        () = shutdown.cancelled() => Ok(InterruptObservation::WatcherShutdown),
    }
}

async fn execute_with_backend<B, S, C>(
    prompt: String,
    output: PathBuf,
    backend: &mut B,
    limits: RunLimits,
    mut observed_at: C,
    signal: S,
) -> CommandReport
where
    B: ModelBackend,
    S: Future<Output = io::Result<()>> + Send + 'static,
    C: FnMut() -> String,
{
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => {
            return CommandReport::failed(
                String::new(),
                format!("current directory could not be read: {error}"),
            );
        }
    };
    let output = resolve_output_path(&output, &current);
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return CommandReport::failed(
            String::new(),
            format!("run output directory could not be created: {error}"),
        );
    }
    let mut store = match JsonlEventStore::create(&output).await {
        Ok(store) => store,
        Err(error) => {
            return CommandReport::failed(
                String::new(),
                format!("run log could not be created: {error}"),
            );
        }
    };

    let run_token = CancellationToken::new();
    let watcher_shutdown = CancellationToken::new();
    let watcher = tokio::spawn(watch_interrupt(
        signal,
        run_token.clone(),
        watcher_shutdown.clone(),
    ));
    let mut catalog = ToolCatalog::empty();
    let authorizer = DenyTools;
    let driver_result = {
        let mut driver = RunDriver::new(
            &mut store,
            backend,
            &mut catalog,
            &authorizer,
            &mut observed_at,
        );
        driver
            .run(
                RunStart {
                    run_id: new_run_id(),
                    request: prompt,
                    limits,
                },
                run_token,
            )
            .await
    };

    watcher_shutdown.cancel();
    let watcher_outcome = match watcher.await {
        Ok(outcome) => outcome,
        Err(_) => Err("Ctrl-C watcher task failed".to_owned()),
    };
    let run_outcome = match driver_result {
        Ok(view) => RunOutcome::Durable(Box::new(view)),
        Err(error) => {
            let diagnostic = format!("run driver failed: {error}");
            if let Some(child) = error.into_unconfirmed_child() {
                child.join().await;
            }
            RunOutcome::DriverFailed(diagnostic)
        }
    };
    let close_outcome = store
        .close()
        .await
        .map_err(|error| format!("event log close failed: {error}"));

    classify(run_outcome, watcher_outcome, close_outcome)
}

fn classify(
    run_outcome: RunOutcome,
    watcher_outcome: WatcherOutcome,
    close_outcome: CloseOutcome,
) -> CommandReport {
    match run_outcome {
        RunOutcome::DriverFailed(driver) => {
            let mut diagnostics = vec![driver];
            append_failures(&mut diagnostics, watcher_outcome, close_outcome);
            CommandReport::with_diagnostics(EXIT_FAILURE, String::new(), diagnostics)
        }
        RunOutcome::Durable(view) => classify_durable(*view, watcher_outcome, close_outcome),
    }
}

fn classify_durable(
    view: RunView,
    watcher_outcome: WatcherOutcome,
    close_outcome: CloseOutcome,
) -> CommandReport {
    if view.lifecycle != RunLifecycle::Terminal {
        let mut diagnostics = vec!["run failed: driver returned an active run".to_owned()];
        append_failures(&mut diagnostics, watcher_outcome, close_outcome);
        return CommandReport::with_diagnostics(EXIT_FAILURE, String::new(), diagnostics);
    }

    match view.finish_reason.as_ref() {
        Some(FinishReason::Completed) => {
            let mut diagnostics = Vec::new();
            append_failures(&mut diagnostics, watcher_outcome, close_outcome);
            let Some(response) = view.final_response else {
                diagnostics.insert(0, "completed run has no final response".to_owned());
                return CommandReport::with_diagnostics(EXIT_FAILURE, String::new(), diagnostics);
            };
            if diagnostics.is_empty() {
                CommandReport::with_diagnostics(EXIT_OK, format!("{response}\n"), diagnostics)
            } else {
                CommandReport::with_diagnostics(EXIT_FAILURE, String::new(), diagnostics)
            }
        }
        Some(FinishReason::Cancelled) => classify_cancelled(watcher_outcome, close_outcome),
        Some(reason) => {
            let mut diagnostics = vec![format!("run failed: {}", finish_reason_label(reason))];
            append_failures(&mut diagnostics, watcher_outcome, close_outcome);
            CommandReport::with_diagnostics(EXIT_FAILURE, String::new(), diagnostics)
        }
        None => {
            let mut diagnostics = vec!["run failed: terminal reason is missing".to_owned()];
            append_failures(&mut diagnostics, watcher_outcome, close_outcome);
            CommandReport::with_diagnostics(EXIT_FAILURE, String::new(), diagnostics)
        }
    }
}

fn classify_cancelled(
    watcher_outcome: WatcherOutcome,
    close_outcome: CloseOutcome,
) -> CommandReport {
    match watcher_outcome {
        Ok(InterruptObservation::CtrlCObserved) => match close_outcome {
            Ok(()) => CommandReport::with_diagnostics(
                EXIT_CANCELLED,
                String::new(),
                vec!["run cancelled by Ctrl-C".to_owned()],
            ),
            Err(close) => CommandReport::with_diagnostics(
                EXIT_FAILURE,
                String::new(),
                vec!["run cancelled by Ctrl-C".to_owned(), close],
            ),
        },
        Ok(InterruptObservation::WatcherShutdown) => {
            let mut diagnostics = vec!["run failed: cancelled".to_owned()];
            if let Err(close) = close_outcome {
                diagnostics.push(close);
            }
            CommandReport::with_diagnostics(EXIT_FAILURE, String::new(), diagnostics)
        }
        Err(watcher) => {
            let mut diagnostics = vec![watcher];
            if let Err(close) = close_outcome {
                diagnostics.push(close);
            }
            CommandReport::with_diagnostics(EXIT_FAILURE, String::new(), diagnostics)
        }
    }
}

fn append_failures(
    diagnostics: &mut Vec<String>,
    watcher_outcome: WatcherOutcome,
    close_outcome: CloseOutcome,
) {
    if let Err(watcher) = watcher_outcome {
        diagnostics.push(watcher);
    }
    if let Err(close) = close_outcome {
        diagnostics.push(close);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        future::{pending, ready},
        io,
        path::{Path, PathBuf},
        time::Duration,
    };

    use aurora_core::{
        BudgetKind, EventSeq, FinishReason, Inspection, JsonlEventStore, ModelFailure,
        ModelInvocation, ModelRequestFailure, RunId, RunLifecycle, RunView, ScriptedModel,
        ScriptedModelStep, inspect_jsonl,
    };
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::CommandReport;

    const OBSERVED_AT: &str = "2026-08-28T00:00:00Z";

    #[test]
    fn missing_api_key_is_a_local_configuration_error() {
        let error = read_operator_environment(|name| {
            (name == "AURORA_OPENAI_MODEL").then(|| OsString::from("gpt-test"))
        })
        .expect_err("missing key is rejected locally");
        assert_eq!(error, "OPENAI_API_KEY is not set");
    }

    #[test]
    fn missing_model_is_a_local_configuration_error() {
        let error = read_operator_environment(|name| {
            (name == "OPENAI_API_KEY").then(|| OsString::from("test-key"))
        })
        .expect_err("missing model is rejected locally");
        assert_eq!(error, "AURORA_OPENAI_MODEL is not set");
    }

    #[test]
    fn empty_operator_environment_values_are_rejected() {
        for (empty_name, expected) in [
            ("OPENAI_API_KEY", "OPENAI_API_KEY is empty"),
            ("AURORA_OPENAI_MODEL", "AURORA_OPENAI_MODEL is empty"),
        ] {
            let error = read_operator_environment(|name| {
                if name == empty_name {
                    Some(OsString::new())
                } else {
                    Some(OsString::from("configured"))
                }
            })
            .expect_err("empty values are rejected locally");
            assert_eq!(error, expected);
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_text_operator_environment_values_are_rejected() {
        use std::os::unix::ffi::OsStringExt;

        for (invalid_name, expected) in [
            ("OPENAI_API_KEY", "OPENAI_API_KEY is not valid Unicode"),
            (
                "AURORA_OPENAI_MODEL",
                "AURORA_OPENAI_MODEL is not valid Unicode",
            ),
        ] {
            let error = read_operator_environment(|name| {
                if name == invalid_name {
                    Some(OsString::from_vec(vec![0xff]))
                } else {
                    Some(OsString::from("configured"))
                }
            })
            .expect_err("non-text values are rejected locally");
            assert_eq!(error, expected);
        }
    }

    #[test]
    fn operator_environment_accepts_only_the_required_values() {
        let environment = read_operator_environment(|name| match name {
            "OPENAI_API_KEY" => Some(OsString::from("test-key")),
            "AURORA_OPENAI_MODEL" => Some(OsString::from("gpt-test")),
            _ => None,
        })
        .expect("required values are accepted");
        assert_eq!(environment.api_key, "test-key");
        assert_eq!(environment.model, "gpt-test");
    }

    #[test]
    fn environment_reader_accepts_process_lookup() {
        let result = read_operator_environment(|name| std::env::var_os(name));
        assert!(result.is_err());
    }

    #[test]
    fn bare_output_resolves_under_the_current_directory() {
        let current = Path::new("/tmp/aurora-current");
        assert_eq!(
            resolve_output_path(Path::new("run.jsonl"), current),
            current.join("run.jsonl")
        );
    }

    #[test]
    fn output_with_a_parent_remains_operator_relative() {
        assert_eq!(
            resolve_output_path(Path::new("runs/run.jsonl"), Path::new("/tmp/ignored")),
            PathBuf::from("runs/run.jsonl")
        );
    }

    #[test]
    fn run_identity_is_not_derived_from_the_observation_time() {
        let run_id = new_run_id();
        assert_ne!(run_id.as_str(), OBSERVED_AT);
        assert!(uuid::Uuid::parse_str(run_id.as_str()).is_ok());
    }

    #[tokio::test]
    async fn watcher_shutdown_never_cancels_the_run() {
        let run_token = CancellationToken::new();
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        let observation = tokio::time::timeout(
            Duration::from_secs(1),
            watch_interrupt(pending::<io::Result<()>>(), run_token.clone(), shutdown),
        )
        .await
        .expect("watcher shutdown is prompt")
        .expect("watcher shutdown succeeds");
        assert_eq!(observation, InterruptObservation::WatcherShutdown);
        assert!(!run_token.is_cancelled());
    }

    #[tokio::test]
    async fn observed_ctrl_c_cancels_the_existing_run_token() {
        let run_token = CancellationToken::new();
        let observation =
            watch_interrupt(ready(Ok(())), run_token.clone(), CancellationToken::new())
                .await
                .expect("simulated Ctrl-C succeeds");
        assert_eq!(observation, InterruptObservation::CtrlCObserved);
        assert!(run_token.is_cancelled());
    }

    #[tokio::test]
    async fn completed_run_closes_and_reconstructs_without_waiting_for_a_signal() {
        let temporary = tempdir().expect("temporary directory is created");
        let output = temporary.path().join("completed.jsonl");
        let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
            ModelInvocation::FinalResponse {
                text: "Ownership has one clear owner.".to_owned(),
            },
        )]);
        let report = tokio::time::timeout(
            Duration::from_secs(1),
            execute_with_backend(
                "Explain ownership".to_owned(),
                output.clone(),
                &mut model,
                run_limits(),
                fixed_clock,
                pending::<io::Result<()>>(),
            ),
        )
        .await
        .expect("normal completion does not wait for Ctrl-C");
        assert_eq!(
            report,
            CommandReport::with_diagnostics(
                0,
                "Ownership has one clear owner.\n".to_owned(),
                Vec::new(),
            )
        );
        assert_clean_reason(&output, FinishReason::Completed);
        JsonlEventStore::open(&output)
            .await
            .expect("closed log can be reopened")
            .close()
            .await
            .expect("reopened log closes");
    }

    #[tokio::test]
    async fn ctrl_c_keeps_the_driver_awaited_until_durable_cancellation() {
        let temporary = tempdir().expect("temporary directory is created");
        let output = temporary.path().join("cancelled.jsonl");
        let mut model = ScriptedModel::new(vec![ScriptedModelStep::WaitForCancellation]);
        let report = tokio::time::timeout(
            Duration::from_secs(1),
            execute_with_backend(
                "Wait".to_owned(),
                output.clone(),
                &mut model,
                run_limits(),
                fixed_clock,
                ready(Ok(())),
            ),
        )
        .await
        .expect("cancelled run reaches quiescent termination");
        assert_eq!(report.exit_code, 130);
        assert_eq!(report.diagnostics, ["run cancelled by Ctrl-C"]);
        assert_clean_reason(&output, FinishReason::Cancelled);
    }

    #[tokio::test]
    async fn ordinary_model_failure_is_durable_and_unsuccessful() {
        let temporary = tempdir().expect("temporary directory is created");
        let output = temporary.path().join("failed.jsonl");
        let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
            ModelInvocation::OrdinaryFailure,
        )]);
        let report = execute_with_backend(
            "Fail".to_owned(),
            output.clone(),
            &mut model,
            run_limits(),
            fixed_clock,
            pending::<io::Result<()>>(),
        )
        .await;
        assert_eq!(report.exit_code, 1);
        assert_eq!(report.stdout, "");
        assert_eq!(report.diagnostics, ["run failed: failed (model failure)"]);
        assert_clean_reason(&output, FinishReason::Failed(ModelFailure::Ordinary));
    }

    #[tokio::test]
    async fn existing_output_is_unchanged_and_model_is_not_invoked() {
        let temporary = tempdir().expect("temporary directory is created");
        let output = temporary.path().join("existing.jsonl");
        let sentinel = b"keep these exact bytes";
        std::fs::write(&output, sentinel).expect("sentinel is written");
        let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
            ModelInvocation::FinalResponse {
                text: "must not run".to_owned(),
            },
        )]);
        let report = execute_with_backend(
            "Do not run".to_owned(),
            output.clone(),
            &mut model,
            run_limits(),
            fixed_clock,
            pending::<io::Result<()>>(),
        )
        .await;
        assert_eq!(report.exit_code, 1);
        assert_eq!(report.stdout, "");
        assert_eq!(model.invocation_count(), 0);
        assert_eq!(
            std::fs::read(output).expect("sentinel is readable"),
            sentinel
        );
    }

    #[tokio::test]
    async fn timed_out_uncooperative_model_is_quiescent_before_store_close() {
        let temporary = tempdir().expect("temporary directory is created");
        let output = temporary.path().join("timeout.jsonl");
        let mut model = ScriptedModel::new(vec![ScriptedModelStep::IgnoreCancellation]);
        let activity = model.activity_signal();
        let mut limits = run_limits();
        limits.model_timeout_ms = 1;
        limits.shutdown_grace_period_ms = 1;
        let report = tokio::time::timeout(
            Duration::from_secs(1),
            execute_with_backend(
                "Wait forever".to_owned(),
                output.clone(),
                &mut model,
                limits,
                fixed_clock,
                pending::<io::Result<()>>(),
            ),
        )
        .await
        .expect("owned child shutdown remains bounded");
        assert_eq!(report.exit_code, 1);
        assert_eq!(activity.starts(), 1);
        assert_eq!(activity.stops(), 1);
        assert_clean_reason(&output, FinishReason::Failed(ModelFailure::ChildShutdown));
    }

    #[tokio::test]
    async fn signal_failure_cancels_closes_and_reports_the_watcher() {
        let temporary = tempdir().expect("temporary directory is created");
        let output = temporary.path().join("signal-error.jsonl");
        let mut model = ScriptedModel::new(vec![ScriptedModelStep::WaitForCancellation]);
        let report = execute_with_backend(
            "Wait".to_owned(),
            output.clone(),
            &mut model,
            run_limits(),
            fixed_clock,
            ready(Err(io::Error::other("fixture signal"))),
        )
        .await;
        assert_eq!(report.exit_code, 1);
        assert_eq!(
            report.diagnostics,
            ["Ctrl-C watcher failed: fixture signal"]
        );
        assert_clean_reason(&output, FinishReason::Cancelled);
        JsonlEventStore::open(&output)
            .await
            .expect("signal failure still releases the writer")
            .close()
            .await
            .expect("reopened log closes");
    }

    #[tokio::test]
    async fn diagnostics_do_not_repeat_operator_or_provider_text() {
        let temporary = tempdir().expect("temporary directory is created");
        let output = temporary.path().join("neutral.jsonl");
        let api_key = "supplied-api-key";
        let provider_body = "sensitive-provider-body";
        let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
            ModelInvocation::RequestFailure(ModelRequestFailure::Authentication),
        )]);
        let report = execute_with_backend(
            format!("{api_key} {provider_body}"),
            output,
            &mut model,
            run_limits(),
            fixed_clock,
            pending::<io::Result<()>>(),
        )
        .await;
        let diagnostics = report.diagnostics.join("\n");
        assert!(!diagnostics.contains(api_key));
        assert!(!diagnostics.contains(provider_body));
        assert_eq!(diagnostics, "run failed: failed (authentication rejected)");
    }

    #[test]
    fn classifier_enforces_run_watcher_and_close_precedence() {
        let completed = terminal_view(FinishReason::Completed, Some("answer"));
        assert_eq!(
            classify(
                RunOutcome::Durable(Box::new(completed.clone())),
                Ok(InterruptObservation::WatcherShutdown),
                Ok(())
            ),
            report(0, "answer\n", &[])
        );
        assert_eq!(
            classify(
                RunOutcome::Durable(Box::new(completed.clone())),
                Err("watcher failed".to_owned()),
                Ok(())
            ),
            report(1, "", &["watcher failed"])
        );
        assert_eq!(
            classify(
                RunOutcome::Durable(Box::new(completed)),
                Ok(InterruptObservation::WatcherShutdown),
                Err("close failed".to_owned())
            ),
            report(1, "", &["close failed"])
        );
        assert_eq!(
            classify(
                RunOutcome::DriverFailed("driver failed".to_owned()),
                Ok(InterruptObservation::WatcherShutdown),
                Ok(())
            ),
            report(1, "", &["driver failed"])
        );
        assert_eq!(
            classify(
                RunOutcome::DriverFailed("driver failed".to_owned()),
                Err("watcher failed".to_owned()),
                Err("close failed".to_owned())
            ),
            report(1, "", &["driver failed", "watcher failed", "close failed"])
        );
    }

    #[test]
    fn classifier_requires_observed_ctrl_c_and_successful_close_for_130() {
        let cancelled = terminal_view(FinishReason::Cancelled, None);
        assert_eq!(
            classify(
                RunOutcome::Durable(Box::new(cancelled.clone())),
                Ok(InterruptObservation::CtrlCObserved),
                Ok(())
            ),
            report(130, "", &["run cancelled by Ctrl-C"])
        );
        assert_eq!(
            classify(
                RunOutcome::Durable(Box::new(cancelled.clone())),
                Ok(InterruptObservation::WatcherShutdown),
                Ok(())
            ),
            report(1, "", &["run failed: cancelled"])
        );
        assert_eq!(
            classify(
                RunOutcome::Durable(Box::new(cancelled)),
                Ok(InterruptObservation::CtrlCObserved),
                Err("close failed".to_owned())
            ),
            report(1, "", &["run cancelled by Ctrl-C", "close failed"])
        );
    }

    #[test]
    fn classifier_reports_durable_failure_and_budget_reasons() {
        assert_eq!(
            classify(
                RunOutcome::Durable(Box::new(terminal_view(
                    FinishReason::Failed(ModelFailure::Request(
                        ModelRequestFailure::Authentication
                    )),
                    None
                ))),
                Ok(InterruptObservation::WatcherShutdown),
                Ok(())
            ),
            report(1, "", &["run failed: failed (authentication rejected)"])
        );
        assert_eq!(
            classify(
                RunOutcome::Durable(Box::new(terminal_view(
                    FinishReason::BudgetExhausted(BudgetKind::ModelSteps),
                    None
                ))),
                Ok(InterruptObservation::WatcherShutdown),
                Ok(())
            ),
            report(1, "", &["run failed: budget exhausted (model steps)"])
        );
    }

    fn fixed_clock() -> String {
        OBSERVED_AT.to_owned()
    }

    fn report(exit_code: u8, stdout: &str, diagnostics: &[&str]) -> CommandReport {
        CommandReport::with_diagnostics(
            exit_code,
            stdout.to_owned(),
            diagnostics.iter().map(|line| (*line).to_owned()).collect(),
        )
    }

    fn terminal_view(reason: FinishReason, response: Option<&str>) -> RunView {
        RunView {
            run_id: RunId::new("run-classifier"),
            last_sequence: EventSeq::new(4),
            lifecycle: RunLifecycle::Terminal,
            finish_reason: Some(reason),
            request: "request".to_owned(),
            limits: run_limits(),
            model_steps_consumed: 1,
            tool_executions_consumed: 0,
            pending_operation: None,
            model_context: Vec::new(),
            final_response: response.map(str::to_owned),
        }
    }

    fn assert_clean_reason(path: &Path, expected: FinishReason) {
        let inspection = inspect_jsonl(path).expect("run log is inspectable");
        let Inspection::Clean(view) = inspection else {
            panic!("run log has an incomplete tail");
        };
        assert_eq!(view.lifecycle, RunLifecycle::Terminal);
        assert_eq!(view.finish_reason, Some(expected));
    }
}
