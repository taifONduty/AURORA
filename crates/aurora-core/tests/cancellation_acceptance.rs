use std::{
    future::pending,
    sync::mpsc,
    time::{Duration, Instant},
};

use aurora_core::{
    AuthorizationDecision, Authorizer, DomainEvent, EventEnvelope, EventStore, FinishReason,
    FixtureTool, FixtureToolBehavior, InMemoryEventStore, ModelBackend, ModelFailure, ModelFuture,
    ModelInput, ModelInvocation, ModelItem, ModelOutcome, RunDriver, RunId, RunLimits, RunStart,
    RunView, ScriptedModel, ScriptedModelStep, StoreFuture, Tool, ToolAuthorization, ToolCallId,
    ToolCatalog, ToolOutcome, ToolRequest,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct BlockingPollModel {
    started: Option<std::sync::mpsc::Sender<()>>,
}

#[derive(Debug)]
struct PanicAfterCancellationModel {
    started: CancellationToken,
}

impl ModelBackend for PanicAfterCancellationModel {
    fn invoke(&mut self, _input: ModelInput, cancellation: CancellationToken) -> ModelFuture {
        let started = self.started.clone();
        Box::pin(async move {
            started.cancel();
            cancellation.cancelled().await;
            panic!("cooperatively cancelled model panic")
        })
    }
}

#[derive(Debug)]
struct PanicAfterCancellationTool;

impl Tool for PanicAfterCancellationTool {
    fn name(&self) -> &str {
        "fixture.read"
    }

    fn description(&self) -> &str {
        "Read one value from the fixture data."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "key": {"type": "string"}
            },
            "required": ["key"]
        })
    }

    fn effect(&self) -> aurora_core::ToolEffect {
        aurora_core::ToolEffect::ReadOnly
    }

    fn validate(&self, _arguments: &serde_json::Value) -> Result<(), aurora_core::ValidationError> {
        Ok(())
    }

    fn execute(
        &mut self,
        _arguments: serde_json::Value,
        cancellation: CancellationToken,
    ) -> aurora_core::ToolFuture {
        Box::pin(async move {
            cancellation.cancelled().await;
            panic!("timed-out tool panic")
        })
    }
}

#[derive(Debug)]
struct PanicOnAbortModel {
    started: CancellationToken,
}

impl ModelBackend for PanicOnAbortModel {
    fn invoke(&mut self, _input: ModelInput, _cancellation: CancellationToken) -> ModelFuture {
        let started = self.started.clone();
        Box::pin(async move {
            let _panic_on_drop = PanicOnDrop;
            started.cancel();
            pending::<ModelInvocation>().await
        })
    }
}

struct PanicOnDrop;

impl Drop for PanicOnDrop {
    fn drop(&mut self) {
        panic!("aborted model cleanup panic")
    }
}

impl ModelBackend for BlockingPollModel {
    fn invoke(&mut self, _input: ModelInput, _cancellation: CancellationToken) -> ModelFuture {
        let started = self.started.take().expect("blocking model runs once");
        Box::pin(async move {
            started.send(()).expect("start observer is present");
            std::thread::sleep(Duration::from_millis(200));
            ModelInvocation::OrdinaryFailure
        })
    }
}

#[derive(Debug)]
struct Allow;

impl Authorizer for Allow {
    fn authorize(&self, _request: &ToolAuthorization<'_>) -> AuthorizationDecision {
        AuthorizationDecision::Allow
    }
}

#[derive(Debug)]
struct CancelOnAuthorize(CancellationToken);

impl Authorizer for CancelOnAuthorize {
    fn authorize(&self, _request: &ToolAuthorization<'_>) -> AuthorizationDecision {
        self.0.cancel();
        AuthorizationDecision::Allow
    }
}

#[derive(Clone, Copy, Debug)]
enum GatedStart {
    ModelRequest,
    ToolExecution,
}

#[derive(Debug)]
struct StartAcknowledgementGateStore {
    inner: InMemoryEventStore,
    gated_start: GatedStart,
    acknowledgement_started: CancellationToken,
    release_acknowledgement: CancellationToken,
}

impl StartAcknowledgementGateStore {
    fn new(
        gated_start: GatedStart,
        acknowledgement_started: CancellationToken,
        release_acknowledgement: CancellationToken,
    ) -> Self {
        Self {
            inner: InMemoryEventStore::new(),
            gated_start,
            acknowledgement_started,
            release_acknowledgement,
        }
    }
}

impl EventStore for StartAcknowledgementGateStore {
    fn commit(&mut self, envelope: EventEnvelope) -> StoreFuture<'_> {
        let gate_this_acknowledgement = matches!(
            (self.gated_start, &envelope.event),
            (
                GatedStart::ModelRequest,
                DomainEvent::ModelRequestStarted { .. }
            ) | (
                GatedStart::ToolExecution,
                DomainEvent::ToolExecutionStarted { .. }
            )
        );
        Box::pin(async move {
            if gate_this_acknowledgement {
                self.acknowledgement_started.cancel();
                self.release_acknowledgement.cancelled().await;
            }
            self.inner.commit(envelope).await
        })
    }

    fn acknowledged(&self) -> &[EventEnvelope] {
        self.inner.acknowledged()
    }
}

#[derive(Debug)]
struct BoundaryProbeTool {
    constructed: Option<mpsc::Sender<()>>,
    started: Option<mpsc::Sender<()>>,
}

impl Tool for BoundaryProbeTool {
    fn name(&self) -> &str {
        "fixture.read"
    }

    fn description(&self) -> &str {
        "Read one value from the fixture data."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "key": {"type": "string"}
            },
            "required": ["key"]
        })
    }

    fn effect(&self) -> aurora_core::ToolEffect {
        aurora_core::ToolEffect::ReadOnly
    }

    fn validate(&self, _arguments: &serde_json::Value) -> Result<(), aurora_core::ValidationError> {
        Ok(())
    }

    fn execute(
        &mut self,
        _arguments: serde_json::Value,
        cancellation: CancellationToken,
    ) -> aurora_core::ToolFuture {
        self.constructed
            .take()
            .expect("probe tool future is constructed once")
            .send(())
            .expect("construction observer remains present");
        let started = self.started.take().expect("probe tool starts once");
        Box::pin(async move {
            started.send(()).expect("start observer remains present");
            cancellation.cancelled().await;
            aurora_core::ToolBodyResult::Cancelled
        })
    }
}

fn limits() -> RunLimits {
    RunLimits {
        max_model_steps: 3,
        max_tool_executions: 2,
        model_timeout_ms: 50,
        tool_timeout_ms: 50,
        shutdown_grace_period_ms: 50,
    }
}

fn start(limits: RunLimits) -> RunStart {
    RunStart {
        run_id: RunId::new("run-cancellation"),
        request: "request".to_owned(),
        limits,
    }
}

fn request() -> ToolRequest {
    ToolRequest {
        tool_call_id: ToolCallId::new("call-1"),
        name: "fixture.read".to_owned(),
        arguments: json!({"key": "alpha"}),
    }
}

fn catalog(behavior: FixtureToolBehavior) -> (ToolCatalog, aurora_core::ActivitySignal) {
    let tool = FixtureTool::new("fixture.read", behavior);
    let activity = tool.activity_signal();
    let tools: Vec<Box<dyn Tool>> = vec![Box::new(tool)];
    (
        ToolCatalog::new(tools).expect("fixture catalog is valid"),
        activity,
    )
}

fn event_names(events: &[EventEnvelope]) -> Vec<&'static str> {
    events
        .iter()
        .map(|envelope| match envelope.event {
            DomainEvent::RunStarted { .. } => "run_started",
            DomainEvent::ModelRequestStarted { .. } => "model_started",
            DomainEvent::ModelRequestFinished { .. } => "model_finished",
            DomainEvent::ToolExecutionStarted { .. } => "tool_started",
            DomainEvent::ToolCallResolved { .. } => "tool_resolved",
            DomainEvent::RunFinished { .. } => "run_finished",
        })
        .collect()
}

fn tool_outcomes(view: &RunView) -> Vec<&ToolOutcome> {
    view.model_context
        .iter()
        .filter_map(|item| match item {
            ModelItem::ToolResult { outcome, .. } => Some(outcome),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn cancellation_during_model_start_acknowledgement_prevents_child_construction() {
    let acknowledgement_started = CancellationToken::new();
    let release_acknowledgement = CancellationToken::new();
    let mut store = StartAcknowledgementGateStore::new(
        GatedStart::ModelRequest,
        acknowledgement_started.clone(),
        release_acknowledgement.clone(),
    );
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::WaitForCancellation]);
    let activity = model.activity_signal();
    let mut tools = ToolCatalog::empty();
    let cancellation = CancellationToken::new();
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = {
        let mut driver = RunDriver::new(&mut store, &mut model, &mut tools, &Allow, &mut timestamp);
        let mut run = Box::pin(driver.run(start(limits()), cancellation.clone()));
        tokio::select! {
            () = acknowledgement_started.cancelled() => {}
            result = &mut run => panic!("run ended before model-start acknowledgement was released: {result:?}"),
        }
        cancellation.cancel();
        release_acknowledgement.cancel();
        run.await
            .expect("acknowledged model start is closed before terminal cancellation")
    };

    assert_eq!(view.finish_reason, Some(FinishReason::Cancelled));
    assert_eq!(model.invocation_count(), 0);
    assert_eq!(activity.starts(), 0);
    assert_eq!(activity.stops(), 0);
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "run_finished"
        ]
    );
    assert!(matches!(
        store.acknowledged()[2].event,
        DomainEvent::ModelRequestFinished {
            outcome: ModelOutcome::Cancelled,
            ..
        }
    ));
}

#[tokio::test]
async fn cancellation_during_tool_start_acknowledgement_prevents_child_construction() {
    let acknowledgement_started = CancellationToken::new();
    let release_acknowledgement = CancellationToken::new();
    let mut store = StartAcknowledgementGateStore::new(
        GatedStart::ToolExecution,
        acknowledgement_started.clone(),
        release_acknowledgement.clone(),
    );
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::ToolRequest(request()),
    )]);
    let (constructed_tx, constructed_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let tools: Vec<Box<dyn Tool>> = vec![Box::new(BoundaryProbeTool {
        constructed: Some(constructed_tx),
        started: Some(started_tx),
    })];
    let mut catalog = ToolCatalog::new(tools).expect("probe catalog is valid");
    let cancellation = CancellationToken::new();
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = {
        let mut driver =
            RunDriver::new(&mut store, &mut model, &mut catalog, &Allow, &mut timestamp);
        let mut run = Box::pin(driver.run(start(limits()), cancellation.clone()));
        tokio::select! {
            () = acknowledgement_started.cancelled() => {}
            result = &mut run => panic!("run ended before tool-start acknowledgement was released: {result:?}"),
        }
        cancellation.cancel();
        release_acknowledgement.cancel();
        run.await
            .expect("acknowledged tool start is resolved before terminal cancellation")
    };

    assert_eq!(view.finish_reason, Some(FinishReason::Cancelled));
    assert!(matches!(
        constructed_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert!(matches!(
        started_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert_eq!(tool_outcomes(&view), [&ToolOutcome::Cancelled]);
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "tool_started",
            "tool_resolved",
            "run_finished",
        ]
    );
}

#[tokio::test]
async fn acceptance_08_tool_timeout_joins_child_before_resolution() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![
        ScriptedModelStep::Return(ModelInvocation::ToolRequest(request())),
        ScriptedModelStep::Return(ModelInvocation::FinalResponse {
            text: "handled timeout".to_owned(),
        }),
    ]);
    let (mut catalog, tool_activity) = catalog(FixtureToolBehavior::WaitForCancellation);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
    let view = tokio::time::timeout(
        Duration::from_secs(2),
        RunDriver::new(&mut store, &mut model, &mut catalog, &Allow, &mut timestamp)
            .run(start(limits()), CancellationToken::new()),
    )
    .await
    .expect("driver must enforce the tool deadline")
    .expect("tool timeout remains a model-visible outcome");

    assert_eq!(view.finish_reason, Some(FinishReason::Completed));
    assert_eq!(view.final_response.as_deref(), Some("handled timeout"));
    assert_eq!(tool_outcomes(&view), [&ToolOutcome::TimedOut]);
    assert_eq!(tool_activity.starts(), 1);
    assert_eq!(tool_activity.stops(), 1);
    assert_eq!(model.invocation_count(), 2);
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "tool_started",
            "tool_resolved",
            "model_started",
            "model_finished",
            "run_finished",
        ]
    );
}

#[tokio::test]
async fn acceptance_09_model_timeout_is_a_distinct_terminal_failure() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::WaitForCancellation]);
    let activity = model.activity_signal();
    let mut catalog = ToolCatalog::empty();
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = tokio::time::timeout(
        Duration::from_secs(2),
        RunDriver::new(&mut store, &mut model, &mut catalog, &Allow, &mut timestamp)
            .run(start(limits()), CancellationToken::new()),
    )
    .await
    .expect("driver must enforce the model deadline")
    .expect("model timeout is recorded as a terminal run");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::Timeout))
    );
    assert_eq!(activity.stops(), 1);
    assert_eq!(activity.starts(), 1);
    assert_eq!(model.invocation_count(), 1);
    assert!(view.final_response.is_none());
    assert!(tool_outcomes(&view).is_empty());
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "run_finished"
        ]
    );
    assert!(matches!(
        store.acknowledged()[2].event,
        DomainEvent::ModelRequestFinished {
            outcome: ModelOutcome::TimedOut,
            ..
        }
    ));
}

#[tokio::test]
async fn acceptance_10_user_cancellation_during_model_execution_joins_child() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::WaitForCancellation]);
    let activity = model.activity_signal();
    let mut catalog = ToolCatalog::empty();
    let cancellation = CancellationToken::new();
    let trigger_token = cancellation.clone();
    let trigger_activity = activity.clone();
    let trigger = tokio::spawn(async move {
        trigger_activity.wait_for_starts(1).await;
        trigger_token.cancel();
    });
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
    let mut generous = limits();
    generous.model_timeout_ms = 1_000;

    let view = RunDriver::new(&mut store, &mut model, &mut catalog, &Allow, &mut timestamp)
        .run(start(generous), cancellation)
        .await
        .expect("user cancellation is a terminal run result");
    trigger.await.expect("cancellation trigger joins");

    assert_eq!(view.finish_reason, Some(FinishReason::Cancelled));
    assert_eq!(activity.starts(), 1);
    assert_eq!(activity.stops(), 1);
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "run_finished"
        ]
    );
    assert!(matches!(
        store.acknowledged()[2].event,
        DomainEvent::ModelRequestFinished {
            outcome: ModelOutcome::Cancelled,
            ..
        }
    ));
}

#[tokio::test]
async fn acceptance_11_user_cancellation_during_tool_execution_is_not_timeout() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::ToolRequest(request()),
    )]);
    let (mut catalog, activity) = catalog(FixtureToolBehavior::WaitForCancellation);
    let cancellation = CancellationToken::new();
    let trigger_token = cancellation.clone();
    let trigger_activity = activity.clone();
    let trigger = tokio::spawn(async move {
        trigger_activity.wait_for_starts(1).await;
        trigger_token.cancel();
    });
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
    let mut generous = limits();
    generous.tool_timeout_ms = 1_000;

    let view = RunDriver::new(&mut store, &mut model, &mut catalog, &Allow, &mut timestamp)
        .run(start(generous), cancellation)
        .await
        .expect("user cancellation is a terminal run result");
    trigger.await.expect("cancellation trigger joins");

    assert_eq!(view.finish_reason, Some(FinishReason::Cancelled));
    assert_eq!(tool_outcomes(&view), [&ToolOutcome::Cancelled]);
    assert_eq!(activity.stops(), 1);
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "tool_started",
            "tool_resolved",
            "run_finished",
        ]
    );
}

#[tokio::test]
async fn user_cancellation_before_tool_start_prevents_new_child_work() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::ToolRequest(request()),
    )]);
    let (mut catalog, activity) = catalog(FixtureToolBehavior::Success(json!({"value": "ok"})));
    let cancellation = CancellationToken::new();
    let authorizer = CancelOnAuthorize(cancellation.clone());
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), cancellation)
    .await
    .expect("cancellation before execution terminates the run");

    assert_eq!(view.finish_reason, Some(FinishReason::Cancelled));
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(activity.starts(), 0);
    assert!(
        !view
            .pending_operation
            .expect("the unstarted request remains inspectable")
            .execution_started()
    );
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "run_finished"
        ]
    );
}

#[tokio::test]
async fn acceptance_21_bounded_child_shutdown_aborts_and_joins_uncooperative_model() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::IgnoreCancellation]);
    let activity = model.activity_signal();
    let mut catalog = ToolCatalog::empty();
    let cancellation = CancellationToken::new();
    let trigger_token = cancellation.clone();
    let trigger_activity = activity.clone();
    let trigger = tokio::spawn(async move {
        trigger_activity.wait_for_starts(1).await;
        trigger_token.cancel();
    });
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
    let mut bounded = limits();
    bounded.model_timeout_ms = 1_000;
    bounded.shutdown_grace_period_ms = 20;

    let view = tokio::time::timeout(
        Duration::from_secs(2),
        RunDriver::new(&mut store, &mut model, &mut catalog, &Allow, &mut timestamp)
            .run(start(bounded), cancellation),
    )
    .await
    .expect("driver shutdown wait must be bounded")
    .expect("shutdown failure is a terminal run result");
    trigger.await.expect("cancellation trigger joins");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::ChildShutdown))
    );
    assert_eq!(activity.starts(), 1);
    assert_eq!(activity.stops(), 1);
    assert!(matches!(
        store.acknowledged()[2].event,
        DomainEvent::ModelRequestFinished {
            outcome: ModelOutcome::ChildShutdownFailed,
            ..
        }
    ));
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "run_finished"
        ]
    );
}

#[tokio::test]
async fn bounded_child_shutdown_aborts_and_joins_uncooperative_tool() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::ToolRequest(request()),
    )]);
    let (mut catalog, activity) = catalog(FixtureToolBehavior::IgnoreCancellation);
    let cancellation = CancellationToken::new();
    let trigger_token = cancellation.clone();
    let trigger_activity = activity.clone();
    let trigger = tokio::spawn(async move {
        trigger_activity.wait_for_starts(1).await;
        trigger_token.cancel();
    });
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
    let mut bounded = limits();
    bounded.tool_timeout_ms = 1_000;
    bounded.shutdown_grace_period_ms = 20;

    let view = tokio::time::timeout(
        Duration::from_secs(2),
        RunDriver::new(&mut store, &mut model, &mut catalog, &Allow, &mut timestamp)
            .run(start(bounded), cancellation),
    )
    .await
    .expect("driver shutdown wait must be bounded")
    .expect("shutdown failure is a terminal run result");
    trigger.await.expect("cancellation trigger joins");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::ChildShutdown))
    );
    assert_eq!(tool_outcomes(&view), [&ToolOutcome::ChildShutdownFailed]);
    assert_eq!(activity.starts(), 1);
    assert_eq!(activity.stops(), 1);
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "tool_started",
            "tool_resolved",
            "run_finished",
        ]
    );
}

#[tokio::test]
async fn model_child_panic_during_cooperative_cancellation_remains_internal_failure() {
    let mut store = InMemoryEventStore::new();
    let started = CancellationToken::new();
    let mut model = PanicAfterCancellationModel {
        started: started.clone(),
    };
    let mut catalog = ToolCatalog::empty();
    let cancellation = CancellationToken::new();
    let trigger_cancellation = cancellation.clone();
    let trigger = tokio::spawn(async move {
        started.cancelled().await;
        trigger_cancellation.cancel();
    });
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
    let mut generous = limits();
    generous.model_timeout_ms = 1_000;

    let view = RunDriver::new(&mut store, &mut model, &mut catalog, &Allow, &mut timestamp)
        .run(start(generous), cancellation)
        .await
        .expect("a panic during cooperative cancellation is durably terminal");
    trigger.await.expect("cancellation trigger joins");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::ChildPanicked))
    );
    assert!(matches!(
        store.acknowledged()[2].event,
        DomainEvent::ModelRequestFinished {
            outcome: ModelOutcome::ChildPanicked,
            ..
        }
    ));
}

#[tokio::test]
async fn tool_child_panic_during_timeout_shutdown_is_not_recorded_as_timeout() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![
        ScriptedModelStep::Return(ModelInvocation::ToolRequest(request())),
        ScriptedModelStep::Return(ModelInvocation::FinalResponse {
            text: "must not run".to_owned(),
        }),
    ]);
    let tools: Vec<Box<dyn Tool>> = vec![Box::new(PanicAfterCancellationTool)];
    let mut catalog = ToolCatalog::new(tools).expect("panic fixture catalog is valid");
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
    let mut short_timeout = limits();
    short_timeout.tool_timeout_ms = 10;

    let view = RunDriver::new(&mut store, &mut model, &mut catalog, &Allow, &mut timestamp)
        .run(start(short_timeout), CancellationToken::new())
        .await
        .expect("a panic during timeout shutdown is durably terminal");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::ChildPanicked))
    );
    assert_eq!(tool_outcomes(&view), [&ToolOutcome::ChildPanicked]);
    assert_eq!(model.invocation_count(), 1);
}

#[tokio::test]
async fn child_panic_observed_after_abort_is_not_child_shutdown_failure() {
    let mut store = InMemoryEventStore::new();
    let started = CancellationToken::new();
    let mut model = PanicOnAbortModel {
        started: started.clone(),
    };
    let mut catalog = ToolCatalog::empty();
    let cancellation = CancellationToken::new();
    let trigger_cancellation = cancellation.clone();
    let trigger = tokio::spawn(async move {
        started.cancelled().await;
        trigger_cancellation.cancel();
    });
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
    let mut bounded = limits();
    bounded.model_timeout_ms = 1_000;
    bounded.shutdown_grace_period_ms = 10;

    let view = RunDriver::new(&mut store, &mut model, &mut catalog, &Allow, &mut timestamp)
        .run(start(bounded), cancellation)
        .await
        .expect("a cleanup panic observed after abort is durably terminal");
    trigger.await.expect("cancellation trigger joins");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::ChildPanicked))
    );
    assert!(matches!(
        store.acknowledged()[2].event,
        DomainEvent::ModelRequestFinished {
            outcome: ModelOutcome::ChildPanicked,
            ..
        }
    ));
}

#[tokio::test]
async fn dropping_the_run_future_aborts_its_owned_child() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::IgnoreCancellation]);
    let activity = model.activity_signal();
    let mut catalog = ToolCatalog::empty();
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
    let mut driver = RunDriver::new(&mut store, &mut model, &mut catalog, &Allow, &mut timestamp);
    let mut running = Box::pin(driver.run(start(limits()), CancellationToken::new()));

    tokio::select! {
        result = &mut running => panic!("run ended unexpectedly: {result:?}"),
        () = activity.wait_for_starts(1) => {}
    }
    drop(running);
    activity.wait_for_stops(1).await;

    assert_eq!(activity.starts(), 1);
    assert_eq!(activity.stops(), 1);
    assert_eq!(store.acknowledged().len(), 2);
}

#[test]
fn unconfirmed_abort_returns_a_typed_error_without_a_terminal_event() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_time()
        .build()
        .expect("test runtime is created");
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let mut store = InMemoryEventStore::new();
    let mut model = BlockingPollModel {
        started: Some(started_tx),
    };
    let mut catalog = ToolCatalog::empty();
    let cancellation = CancellationToken::new();
    let trigger_token = cancellation.clone();
    let trigger = std::thread::spawn(move || {
        started_rx.recv().expect("model start is observed");
        trigger_token.cancel();
    });
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
    let mut bounded = limits();
    bounded.model_timeout_ms = 1_000;
    bounded.shutdown_grace_period_ms = 10;

    let began = Instant::now();
    let result = runtime.block_on(
        RunDriver::new(&mut store, &mut model, &mut catalog, &Allow, &mut timestamp)
            .run(start(bounded), cancellation),
    );
    let returned_after = began.elapsed();
    trigger.join().expect("cancellation trigger joins");

    let error = result.expect_err("an unconfirmed abort cannot produce a terminal run");
    let child = error
        .into_unconfirmed_child()
        .expect("the error retains ownership of the child");
    assert_eq!(store.acknowledged().len(), 2);
    assert!(returned_after < Duration::from_millis(100));
    runtime.block_on(child.join());
}
