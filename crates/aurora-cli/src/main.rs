mod inspect;

use std::{path::PathBuf, process::ExitCode};

use aurora_core::{BudgetKind, FinishReason, ModelFailure, ModelRequestFailure};
use clap::{Parser, Subcommand};

const EXIT_OK: u8 = 0;
const EXIT_FAILURE: u8 = 1;

#[derive(Debug, Parser)]
#[command(name = "aurora", version)]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Inspect { path: PathBuf },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CommandReport {
    pub(crate) exit_code: u8,
    pub(crate) stdout: String,
    pub(crate) diagnostics: Vec<String>,
}

impl CommandReport {
    pub(crate) fn clean(stdout: String) -> Self {
        Self {
            exit_code: EXIT_OK,
            stdout,
            diagnostics: Vec::new(),
        }
    }

    pub(crate) fn failed(stdout: String, diagnostic: String) -> Self {
        Self {
            exit_code: EXIT_FAILURE,
            stdout,
            diagnostics: vec![diagnostic],
        }
    }

    fn emit(self) -> ExitCode {
        if !self.stdout.is_empty() {
            print!("{}", self.stdout);
        }
        for diagnostic in self.diagnostics {
            eprintln!("{diagnostic}");
        }
        ExitCode::from(self.exit_code)
    }
}

pub(crate) fn finish_reason_label(reason: &FinishReason) -> String {
    match reason {
        FinishReason::Completed => "completed".to_owned(),
        FinishReason::Cancelled => "cancelled".to_owned(),
        FinishReason::BudgetExhausted(kind) => format!(
            "budget exhausted ({})",
            match kind {
                BudgetKind::ModelSteps => "model steps",
                BudgetKind::ToolExecutions => "tool executions",
            }
        ),
        FinishReason::Failed(failure) => format!(
            "failed ({})",
            match failure {
                ModelFailure::Ordinary => "model failure",
                ModelFailure::Request(category) => match category {
                    ModelRequestFailure::Authentication => "authentication rejected",
                    ModelRequestFailure::RateLimited => "rate limited",
                    ModelRequestFailure::RequestRejected => "request rejected",
                    ModelRequestFailure::ServiceUnavailable => "service unavailable",
                    ModelRequestFailure::Transport => "transport failure",
                    ModelRequestFailure::UnsupportedResponse => "unsupported provider response",
                },
                ModelFailure::Timeout => "model timeout",
                ModelFailure::MalformedOutput => "malformed model output",
                ModelFailure::ChildPanicked => "owned child panicked",
                ModelFailure::ChildShutdown => "owned child shutdown failed",
            }
        ),
        FinishReason::Interrupted => "interrupted".to_owned(),
    }
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    match arguments.command {
        Command::Inspect { path } => inspect::execute(&path),
    }
    .emit()
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Arguments, Command};

    #[test]
    fn inspect_accepts_exactly_one_path() {
        let arguments = Arguments::try_parse_from(["aurora", "inspect", "run.jsonl"])
            .expect("inspect command parses");
        assert!(matches!(
            arguments.command,
            Command::Inspect { path } if path.as_path() == std::path::Path::new("run.jsonl")
        ));
    }

    #[test]
    fn inspect_rejects_json_output() {
        let error = Arguments::try_parse_from(["aurora", "inspect", "run.jsonl", "--json"])
            .expect_err("Phase 1E has no JSON output flag");
        assert_eq!(error.exit_code(), 2);
    }
}
