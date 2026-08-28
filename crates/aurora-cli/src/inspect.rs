use std::{fmt::Write, path::Path};

use aurora_core::{Inspection, RunLifecycle, RunView, inspect_jsonl};

use crate::{CommandReport, finish_reason_label};

pub(crate) fn execute(path: &Path) -> CommandReport {
    match inspect_jsonl(path) {
        Ok(Inspection::Clean(view)) => CommandReport::clean(render_view(&view)),
        Ok(Inspection::IncompleteTail { prefix, tail }) => {
            let mut output = prefix.as_ref().map(render_view).unwrap_or_default();
            writeln!(output, "Incomplete tail: {} bytes", tail.len())
                .expect("writing to a String cannot fail");
            CommandReport::failed(
                output,
                "inspection found an incomplete JSONL tail".to_owned(),
            )
        }
        Err(error) => CommandReport::failed(String::new(), format!("inspection failed: {error}")),
    }
}

fn render_view(view: &RunView) -> String {
    let status = match (&view.lifecycle, &view.finish_reason) {
        (RunLifecycle::Active, None) => "active".to_owned(),
        (RunLifecycle::Terminal, Some(reason)) => finish_reason_label(reason),
        _ => "invalid projected state".to_owned(),
    };
    let mut output = format!(
        "Run:             {}\nStatus:          {status}\nEvents:          {}\nModel steps:     {}\nTool executions: {}\n",
        view.run_id,
        view.last_sequence.get(),
        view.model_steps_consumed,
        view.tool_executions_consumed,
    );
    if let Some(response) = &view.final_response {
        output.push_str("\nResponse:\n");
        output.push_str(response);
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use std::fs;

    use aurora_core::{
        BudgetKind, DomainEvent, EventEnvelope, EventSeq, FinishReason, FixtureTool,
        FixtureToolBehavior, ModelFailure, ModelOutcome, ModelRequestFailure, RunId, RunLifecycle,
        RunLimits, RunView, SCHEMA_VERSION, StepId, Tool, ToolRequest, encode_envelope,
    };
    use tempfile::tempdir;

    use super::{execute, render_view};

    #[test]
    fn completed_run_renders_the_summary() {
        let view = RunView {
            run_id: RunId::new("run-inspect"),
            last_sequence: EventSeq::new(4),
            lifecycle: RunLifecycle::Terminal,
            finish_reason: Some(FinishReason::Completed),
            request: "Explain ownership".to_owned(),
            limits: RunLimits {
                max_model_steps: 1,
                max_tool_executions: 0,
                model_timeout_ms: 1_000,
                tool_timeout_ms: 1_000,
                shutdown_grace_period_ms: 100,
            },
            model_steps_consumed: 1,
            tool_executions_consumed: 0,
            pending_operation: None,
            model_context: Vec::new(),
            final_response: Some("Ownership has one clear owner.".to_owned()),
        };

        assert_eq!(
            render_view(&view),
            "Run:             run-inspect\nStatus:          completed\nEvents:          4\nModel steps:     1\nTool executions: 0\n\nResponse:\nOwnership has one clear owner.\n"
        );
    }

    #[test]
    fn active_and_every_terminal_reason_render_clean_reports() {
        let directory = tempdir().expect("temporary directory is created");
        let cases = [
            ("active", started_history(limits(1, 1)), "active", false),
            (
                "completed",
                model_history(
                    ModelOutcome::FinalResponse {
                        text: "completed response".to_owned(),
                    },
                    FinishReason::Completed,
                ),
                "completed",
                true,
            ),
            (
                "cancelled",
                model_history(ModelOutcome::Cancelled, FinishReason::Cancelled),
                "cancelled",
                false,
            ),
            (
                "budget_model",
                budget_model_history(),
                "budget exhausted (model steps)",
                false,
            ),
            (
                "budget_tool",
                budget_tool_history(),
                "budget exhausted (tool executions)",
                false,
            ),
            (
                "ordinary_failure",
                model_history(
                    ModelOutcome::Failed,
                    FinishReason::Failed(ModelFailure::Ordinary),
                ),
                "failed (model failure)",
                false,
            ),
            (
                "authentication_failure",
                request_failure_history(ModelRequestFailure::Authentication),
                "failed (authentication rejected)",
                false,
            ),
            (
                "rate_limited_failure",
                request_failure_history(ModelRequestFailure::RateLimited),
                "failed (rate limited)",
                false,
            ),
            (
                "request_rejected_failure",
                request_failure_history(ModelRequestFailure::RequestRejected),
                "failed (request rejected)",
                false,
            ),
            (
                "service_unavailable_failure",
                request_failure_history(ModelRequestFailure::ServiceUnavailable),
                "failed (service unavailable)",
                false,
            ),
            (
                "transport_failure",
                request_failure_history(ModelRequestFailure::Transport),
                "failed (transport failure)",
                false,
            ),
            (
                "unsupported_response_failure",
                request_failure_history(ModelRequestFailure::UnsupportedResponse),
                "failed (unsupported provider response)",
                false,
            ),
            (
                "timeout_failure",
                model_history(
                    ModelOutcome::TimedOut,
                    FinishReason::Failed(ModelFailure::Timeout),
                ),
                "failed (model timeout)",
                false,
            ),
            (
                "malformed_failure",
                model_history(
                    ModelOutcome::MalformedOutput,
                    FinishReason::Failed(ModelFailure::MalformedOutput),
                ),
                "failed (malformed model output)",
                false,
            ),
            (
                "child_panicked_failure",
                model_history(
                    ModelOutcome::ChildPanicked,
                    FinishReason::Failed(ModelFailure::ChildPanicked),
                ),
                "failed (owned child panicked)",
                false,
            ),
            (
                "child_shutdown_failure",
                model_history(
                    ModelOutcome::ChildShutdownFailed,
                    FinishReason::Failed(ModelFailure::ChildShutdown),
                ),
                "failed (owned child shutdown failed)",
                false,
            ),
            ("interrupted", interrupted_history(), "interrupted", false),
        ];

        for (name, history, status, has_response) in cases {
            let path = directory.path().join(format!("{name}.jsonl"));
            write_history(&path, &history);
            let before = fs::read(&path).expect("log is readable before inspection");

            let report = execute(&path);

            assert_eq!(report.exit_code, 0, "{name}");
            assert!(report.diagnostics.is_empty(), "{name}");
            assert!(
                report
                    .stdout
                    .contains(&format!("Status:          {status}")),
                "{name}"
            );
            assert_eq!(
                report.stdout.contains("Response:\n"),
                has_response,
                "{name}"
            );
            assert_eq!(
                fs::read(&path).expect("log remains readable"),
                before,
                "{name}"
            );
        }
    }

    #[test]
    fn incomplete_tail_reports_the_exact_visible_tail_length_without_writing() {
        let directory = tempdir().expect("temporary directory is created");
        let path = directory.path().join("partial.jsonl");
        let mut bytes = encoded_history(&started_history(limits(1, 1)));
        let tail = b"{\"schema_version\":";
        bytes.extend(tail);
        fs::write(&path, &bytes).expect("partial log is written");

        let report = execute(&path);

        assert_eq!(report.exit_code, 1);
        assert_eq!(
            report.diagnostics,
            ["inspection found an incomplete JSONL tail"]
        );
        assert_eq!(
            report.stdout,
            format!(
                "Run:             run-inspect\nStatus:          active\nEvents:          1\nModel steps:     0\nTool executions: 0\nIncomplete tail: {} bytes\n",
                tail.len()
            )
        );
        assert_eq!(fs::read(&path).expect("log remains readable"), bytes);
    }

    #[test]
    fn inspection_errors_return_failure_reports_without_writing() {
        let directory = tempdir().expect("temporary directory is created");
        let missing = directory.path().join("missing.jsonl");
        assert_failure_without_output(&missing);

        for (name, bytes) in [
            ("empty", Vec::new()),
            ("corrupt", b"not-json\n".to_vec()),
            ("semantic", encoded_history(&semantic_invalid_history())),
        ] {
            let path = directory.path().join(format!("{name}.jsonl"));
            fs::write(&path, &bytes).expect("invalid log is written");
            let before = fs::read(&path).expect("log is readable before inspection");

            assert_failure_without_output(&path);

            assert_eq!(
                fs::read(&path).expect("log remains readable"),
                before,
                "{name}"
            );
        }
    }

    fn assert_failure_without_output(path: &std::path::Path) {
        let report = execute(path);
        assert_eq!(report.exit_code, 1);
        assert!(report.stdout.is_empty());
        assert_eq!(report.diagnostics.len(), 1);
        assert!(report.diagnostics[0].starts_with("inspection failed: "));
    }

    fn limits(max_model_steps: u32, max_tool_executions: u32) -> RunLimits {
        RunLimits {
            max_model_steps,
            max_tool_executions,
            model_timeout_ms: 1_000,
            tool_timeout_ms: 1_000,
            shutdown_grace_period_ms: 100,
        }
    }

    fn started_history(limits: RunLimits) -> Vec<EventEnvelope> {
        vec![envelope(
            1,
            DomainEvent::RunStarted {
                request: "Inspect this run".to_owned(),
                limits,
            },
        )]
    }

    fn model_history(outcome: ModelOutcome, reason: FinishReason) -> Vec<EventEnvelope> {
        let mut history = started_history(limits(1, 1));
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
                    outcome,
                },
            ),
            envelope(4, DomainEvent::RunFinished { reason }),
        ]);
        history
    }

    fn request_failure_history(category: ModelRequestFailure) -> Vec<EventEnvelope> {
        model_history(
            ModelOutcome::RequestFailure(category),
            FinishReason::Failed(ModelFailure::Request(category)),
        )
    }

    fn budget_model_history() -> Vec<EventEnvelope> {
        let mut history = started_history(limits(0, 1));
        history.push(envelope(
            2,
            DomainEvent::RunFinished {
                reason: FinishReason::BudgetExhausted(BudgetKind::ModelSteps),
            },
        ));
        history
    }

    fn budget_tool_history() -> Vec<EventEnvelope> {
        let mut history = started_history(limits(1, 0));
        let fixture = FixtureTool::new("fixture.read", FixtureToolBehavior::OrdinaryFailure);
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
                    outcome: ModelOutcome::ToolRequest(ToolRequest {
                        tool_call_id: aurora_core::ToolCallId::new("call-1"),
                        name: "fixture.read".to_owned(),
                        arguments: fixture.input_schema(),
                    }),
                },
            ),
            envelope(
                4,
                DomainEvent::RunFinished {
                    reason: FinishReason::BudgetExhausted(BudgetKind::ToolExecutions),
                },
            ),
        ]);
        history
    }

    fn interrupted_history() -> Vec<EventEnvelope> {
        let mut history = started_history(limits(1, 1));
        history.push(envelope(
            2,
            DomainEvent::RunFinished {
                reason: FinishReason::Interrupted,
            },
        ));
        history
    }

    fn semantic_invalid_history() -> Vec<EventEnvelope> {
        let mut history = started_history(limits(1, 1));
        history.push(envelope(
            3,
            DomainEvent::ModelRequestStarted {
                step_id: StepId::new(1),
            },
        ));
        history
    }

    fn envelope(sequence: u64, event: DomainEvent) -> EventEnvelope {
        EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence: EventSeq::new(sequence),
            run_id: RunId::new("run-inspect"),
            observed_at: "2026-01-01T00:00:00Z".to_owned(),
            event,
        }
    }

    fn write_history(path: &std::path::Path, history: &[EventEnvelope]) {
        fs::write(path, encoded_history(history)).expect("history is written");
    }

    fn encoded_history(history: &[EventEnvelope]) -> Vec<u8> {
        history
            .iter()
            .flat_map(|envelope| encode_envelope(envelope).expect("event encodes"))
            .collect()
    }
}
