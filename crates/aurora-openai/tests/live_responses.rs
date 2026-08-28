use aurora_core::{
    AuthorizationDecision, Authorizer, FinishReason, FixtureTool, FixtureToolBehavior, Inspection,
    JsonlEventStore, RunDriver, RunId, RunLimits, RunStart, Tool, ToolAuthorization, ToolCatalog,
    inspect_jsonl,
};
use aurora_openai::{OpenAiBackend, OpenAiConfig};
use serde_json::json;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct Allow;

impl Authorizer for Allow {
    fn authorize(&self, _request: &ToolAuthorization<'_>) -> AuthorizationDecision {
        AuthorizationDecision::Allow
    }
}

fn credentials() -> (String, String) {
    let api_key =
        std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY is required for ignored live tests");
    let model = std::env::var("AURORA_OPENAI_MODEL")
        .expect("AURORA_OPENAI_MODEL is required for ignored live tests");
    (api_key, model)
}

fn limits() -> RunLimits {
    RunLimits {
        max_model_steps: 3,
        max_tool_executions: 1,
        model_timeout_ms: 120_000,
        tool_timeout_ms: 1_000,
        shutdown_grace_period_ms: 2_000,
    }
}

async fn run_live(request: &str, catalog: &mut ToolCatalog) -> aurora_core::RunView {
    let (api_key, model) = credentials();
    let config = OpenAiConfig::new(api_key, model).expect("live configuration is valid");
    let mut backend = OpenAiBackend::new(config).expect("HTTP client builds");
    let directory = tempdir().expect("temporary directory is created");
    let path = directory.path().join("run.jsonl");
    let mut store = JsonlEventStore::create(&path)
        .await
        .expect("JSONL store is created");
    let mut observed_at = || "2026-08-27T00:00:00Z".to_owned();
    let view = RunDriver::new(&mut store, &mut backend, catalog, &Allow, &mut observed_at)
        .run(
            RunStart {
                run_id: RunId::new("run-live-openai"),
                request: request.to_owned(),
                limits: limits(),
            },
            CancellationToken::new(),
        )
        .await
        .expect("live run returns a durable AURORA outcome");
    store.close().await.expect("JSONL writer joins");
    let reconstructed = match inspect_jsonl(&path).expect("completed log inspects") {
        Inspection::Clean(reconstructed) => reconstructed,
        Inspection::IncompleteTail { .. } => {
            panic!("completed live log has an incomplete tail")
        }
    };
    assert_eq!(reconstructed, view);
    view
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires OPENAI_API_KEY, AURORA_OPENAI_MODEL, network, and credits"]
async fn selected_model_returns_terminal_text() {
    let mut catalog = ToolCatalog::empty();
    let view = run_live(
        "Reply with one short sentence confirming that this request reached the model.",
        &mut catalog,
    )
    .await;

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Completed),
        "selected model returned the durable category {:?}",
        view.finish_reason
    );
    assert!(
        view.final_response
            .as_deref()
            .is_some_and(|text| !text.is_empty())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires OPENAI_API_KEY, AURORA_OPENAI_MODEL, network, and credits"]
async fn selected_model_completes_one_fixture_tool_round_trip() {
    let tools: Vec<Box<dyn Tool>> = vec![Box::new(FixtureTool::new(
        "fixture.read",
        FixtureToolBehavior::Success(json!({
            "key": "alpha",
            "value": "fixture-live-value"
        })),
    ))];
    let mut catalog = ToolCatalog::new(tools).expect("fixture catalog is valid");
    let view = run_live(
        "Use fixture.read exactly once with key alpha. After receiving its result, report the returned value in one short sentence.",
        &mut catalog,
    )
    .await;

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Completed),
        "selected model returned the durable category {:?}",
        view.finish_reason
    );
    assert_eq!(view.tool_executions_consumed, 1);
    assert_eq!(view.model_steps_consumed, 2);
    assert!(
        view.final_response
            .as_deref()
            .is_some_and(|text| !text.is_empty())
    );
}
