use std::{cell::Cell, rc::Rc};

use aurora_core::{
    AuthorizationDecision, Authorizer, DomainEvent, DriverError, EventEnvelope, EventSeq,
    EventStore, FinishReason, FixtureTool, FixtureToolBehavior, InMemoryEventStore, ModelBackend,
    ModelFailure, ModelFuture, ModelInvocation, ModelItem, ModelOutcome, ModelRequestFailure,
    ProjectionError, RunDriver, RunId, RunLifecycle, RunLimits, RunStart, RunView, SCHEMA_VERSION,
    ScriptedModel, ScriptedModelStep, StoreError, StoreFuture, Tool, ToolAuthorization, ToolCallId,
    ToolCatalog, ToolOutcome, ToolRequest, encode_envelope,
};
use serde_json::json;
use tokio::{sync::Notify, task::yield_now};
use tokio_util::sync::CancellationToken;

fn limits() -> RunLimits {
    RunLimits {
        max_model_steps: 3,
        max_tool_executions: 2,
        model_timeout_ms: 1_000,
        tool_timeout_ms: 1_000,
        shutdown_grace_period_ms: 100,
    }
}

fn start(limits: RunLimits) -> RunStart {
    RunStart {
        run_id: RunId::new("run-driver"),
        request: "request".to_owned(),
        limits,
    }
}

fn tool_request(call_id: &str, arguments: serde_json::Value) -> ToolRequest {
    ToolRequest {
        tool_call_id: ToolCallId::new(call_id),
        name: "fixture.read".to_owned(),
        arguments,
    }
}

fn fixture_catalog(behavior: FixtureToolBehavior) -> (ToolCatalog, aurora_core::ActivitySignal) {
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

#[derive(Debug)]
struct CountingAuthorizer {
    decision: AuthorizationDecision,
    calls: Cell<usize>,
}

impl CountingAuthorizer {
    fn new(decision: AuthorizationDecision) -> Self {
        Self {
            decision,
            calls: Cell::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.get()
    }
}

impl Authorizer for CountingAuthorizer {
    fn authorize(&self, _request: &ToolAuthorization<'_>) -> AuthorizationDecision {
        self.calls.set(self.calls.get() + 1);
        self.decision
    }
}

#[derive(Debug)]
struct FaultingStore {
    inner: InMemoryEventStore,
    fail_at_attempt: usize,
    attempts: usize,
    poisoned: bool,
}

impl FaultingStore {
    fn new(fail_at_attempt: usize) -> Self {
        Self {
            inner: InMemoryEventStore::new(),
            fail_at_attempt,
            attempts: 0,
            poisoned: false,
        }
    }
}

impl EventStore for FaultingStore {
    fn commit(&mut self, envelope: EventEnvelope) -> StoreFuture<'_> {
        Box::pin(async move {
            if self.poisoned {
                return Err(StoreError::Poisoned);
            }
            self.attempts += 1;
            if self.attempts == self.fail_at_attempt {
                self.poisoned = true;
                return Err(StoreError::Persistence {
                    message: "injected acknowledgement failure".to_owned(),
                });
            }
            self.inner.commit(envelope).await
        })
    }

    fn acknowledged(&self) -> &[EventEnvelope] {
        self.inner.acknowledged()
    }
}

#[derive(Debug)]
struct CountingStore {
    inner: InMemoryEventStore,
    commit_calls: Rc<Cell<usize>>,
}

impl CountingStore {
    fn new(commit_calls: Rc<Cell<usize>>) -> Self {
        Self {
            inner: InMemoryEventStore::new(),
            commit_calls,
        }
    }
}

impl EventStore for CountingStore {
    fn commit(&mut self, envelope: EventEnvelope) -> StoreFuture<'_> {
        Box::pin(async move {
            self.commit_calls.set(self.commit_calls.get() + 1);
            self.inner.commit(envelope).await
        })
    }

    fn acknowledged(&self) -> &[EventEnvelope] {
        self.inner.acknowledged()
    }
}

#[derive(Debug, Default)]
struct PendingAcknowledgement {
    started: Notify,
    released: Notify,
    started_count: Cell<usize>,
    acknowledged_count: Cell<usize>,
}

impl PendingAcknowledgement {
    async fn wait_until_started(&self) {
        loop {
            let notified = self.started.notified();
            if self.started_count.get() > 0 {
                return;
            }
            notified.await;
        }
    }

    fn release(&self) {
        self.released.notify_one();
    }
}

#[derive(Debug)]
struct PendingAcknowledgementStore {
    acknowledged: Vec<EventEnvelope>,
    acknowledgement: Rc<PendingAcknowledgement>,
}

impl PendingAcknowledgementStore {
    fn new(acknowledgement: Rc<PendingAcknowledgement>) -> Self {
        Self {
            acknowledged: Vec::new(),
            acknowledgement,
        }
    }
}

impl EventStore for PendingAcknowledgementStore {
    fn commit(&mut self, envelope: EventEnvelope) -> StoreFuture<'_> {
        Box::pin(async move {
            if self.acknowledged.is_empty() {
                self.acknowledgement
                    .started_count
                    .set(self.acknowledgement.started_count.get() + 1);
                self.acknowledgement.started.notify_waiters();
                self.acknowledgement.released.notified().await;
            }
            self.acknowledged.push(envelope);
            self.acknowledgement
                .acknowledged_count
                .set(self.acknowledgement.acknowledged_count.get() + 1);
            Ok(())
        })
    }

    fn acknowledged(&self) -> &[EventEnvelope] {
        &self.acknowledged
    }
}

#[derive(Debug)]
struct CountingModel {
    invocation_count: Rc<Cell<usize>>,
}

impl CountingModel {
    fn new(invocation_count: Rc<Cell<usize>>) -> Self {
        Self { invocation_count }
    }
}

impl ModelBackend for CountingModel {
    fn invoke(
        &mut self,
        _input: aurora_core::ModelInput,
        _cancellation: CancellationToken,
    ) -> ModelFuture {
        self.invocation_count.set(self.invocation_count.get() + 1);
        Box::pin(async {
            ModelInvocation::FinalResponse {
                text: "completed after valid timestamp".to_owned(),
            }
        })
    }
}

#[tokio::test]
async fn awaited_commit_withholds_projection_advance_and_runtime_activity_until_acknowledged() {
    let acknowledgement = Rc::new(PendingAcknowledgement::default());
    let mut store = PendingAcknowledgementStore::new(acknowledgement.clone());
    let model_invocations = Rc::new(Cell::new(0));
    let mut model = CountingModel::new(model_invocations.clone());
    let (mut catalog, tool_activity) =
        fixture_catalog(FixtureToolBehavior::Success(json!({"value": "unused"})));
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
    {
        let mut driver = RunDriver::new(
            &mut store,
            &mut model,
            &mut catalog,
            &authorizer,
            &mut timestamp,
        );
        let mut run = Box::pin(driver.run(start(limits()), CancellationToken::new()));
        tokio::select! {
            () = acknowledgement.wait_until_started() => {}
            result = &mut run => panic!("run completed before its first commit was acknowledged: {result:?}"),
        }
        yield_now().await;

        assert_eq!(acknowledgement.started_count.get(), 1);
        assert_eq!(acknowledgement.acknowledged_count.get(), 0);
        assert_eq!(model_invocations.get(), 0);
        assert_eq!(tool_activity.starts(), 0);

        // A cancelled, unacknowledged candidate must leave this driver's
        // projection unchanged so its retry begins from sequence one.
        drop(run);

        acknowledgement.release();
        let view = driver
            .run(start(limits()), CancellationToken::new())
            .await
            .expect("released acknowledgement allows the run to finish");
        assert_eq!(view.lifecycle, RunLifecycle::Terminal);
        assert_eq!(view.last_sequence, EventSeq::new(4));
    }

    assert_eq!(
        store
            .acknowledged()
            .iter()
            .map(|envelope| envelope.sequence)
            .collect::<Vec<_>>(),
        (1..=4).map(EventSeq::new).collect::<Vec<_>>()
    );
    assert_eq!(store.acknowledged().len(), 4);
}

async fn assert_faulting_store_is_poisoned(store: &mut FaultingStore) {
    let prefix_len = store.acknowledged().len();
    let error = store
        .commit(EventEnvelope {
            schema_version: aurora_core::SCHEMA_VERSION,
            sequence: aurora_core::EventSeq::new(prefix_len as u64 + 1),
            run_id: RunId::new("run-driver"),
            observed_at: "2026-01-01T00:00:01Z".to_owned(),
            event: DomainEvent::RunFinished {
                reason: FinishReason::Interrupted,
            },
        })
        .await
        .expect_err("a failed acknowledgement poisons the writer");

    assert!(matches!(error, StoreError::Poisoned));
    assert_eq!(store.acknowledged().len(), prefix_len);
}

#[tokio::test]
async fn new_driver_rejects_an_acknowledged_prefix_before_run_activity() {
    let commit_calls = Rc::new(Cell::new(0));
    let mut store = CountingStore::new(commit_calls.clone());
    store
        .commit(EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence: EventSeq::new(17),
            run_id: RunId::new("opaque-prefix"),
            observed_at: "not-a-timestamp".to_owned(),
            event: DomainEvent::RunFinished {
                reason: FinishReason::Interrupted,
            },
        })
        .await
        .expect("in-memory store acknowledges opaque envelopes");
    let prefix_commit_calls = commit_calls.get();
    let prefix = store.acknowledged().to_vec();
    let prefix_bytes: Vec<u8> = prefix
        .iter()
        .flat_map(|envelope| encode_envelope(envelope).expect("opaque envelope encodes"))
        .collect();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::FinalResponse {
            text: "must not run".to_owned(),
        },
    )]);
    let (mut catalog, tool_activity) =
        fixture_catalog(FixtureToolBehavior::Success(json!({"value": "unused"})));
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let timestamp_calls = Cell::new(0);
    let mut timestamp = || {
        timestamp_calls.set(timestamp_calls.get() + 1);
        "2026-01-01T00:00:00Z".to_owned()
    };

    let error = {
        let mut driver = RunDriver::new(
            &mut store,
            &mut model,
            &mut catalog,
            &authorizer,
            &mut timestamp,
        );
        driver
            .run(start(limits()), CancellationToken::new())
            .await
            .expect_err("a new driver cannot own an acknowledged prefix")
    };

    assert!(matches!(error, DriverError::AcknowledgedHistory));
    assert_eq!(timestamp_calls.get(), 0);
    assert_eq!(model.invocation_count(), 0);
    assert_eq!(tool_activity.starts(), 0);
    assert_eq!(commit_calls.get(), prefix_commit_calls);
    assert_eq!(store.acknowledged(), prefix);
    let after_bytes: Vec<u8> = store
        .acknowledged()
        .iter()
        .flat_map(|envelope| encode_envelope(envelope).expect("opaque envelope encodes"))
        .collect();
    assert_eq!(after_bytes, prefix_bytes);
}

#[tokio::test]
async fn invalid_candidate_does_not_commit_or_advance_the_live_driver_projection() {
    let commit_calls = Rc::new(Cell::new(0));
    let model_invocations = Rc::new(Cell::new(0));
    let mut store = CountingStore::new(commit_calls.clone());
    let mut model = CountingModel::new(model_invocations.clone());
    let mut catalog = ToolCatalog::empty();
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let invalid_timestamp_pending = Cell::new(true);
    let mut timestamp = || {
        if invalid_timestamp_pending.replace(false) {
            "2026-01-01T06:00:00+06:00".to_owned()
        } else {
            "2026-01-01T00:00:00Z".to_owned()
        }
    };
    let mut driver = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    );

    let error = driver
        .run(start(limits()), CancellationToken::new())
        .await
        .expect_err("a non-UTC timestamp is not a valid candidate envelope");

    assert!(matches!(
        error,
        DriverError::Projection(ProjectionError::InvalidTimestamp { sequence: 1 })
    ));
    assert_eq!(commit_calls.get(), 0);
    assert_eq!(model_invocations.get(), 0);

    let view = driver
        .run(start(limits()), CancellationToken::new())
        .await
        .expect("a valid retry uses the untouched sequence-one projection");

    assert_eq!(view.last_sequence, EventSeq::new(4));
    assert_eq!(model_invocations.get(), 1);
    drop(driver);
    assert_eq!(commit_calls.get(), 4);
    assert_eq!(
        store.acknowledged().first().map(|event| event.sequence),
        Some(EventSeq::new(1))
    );
    assert_eq!(store.acknowledged().len(), 4);
}

#[tokio::test]
async fn acceptance_01_final_model_response_without_tools() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::FinalResponse {
            text: "final answer".to_owned(),
        },
    )]);
    let mut catalog = ToolCatalog::empty();
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect("run completes");

    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "run_finished"
        ]
    );
    assert_eq!(view.lifecycle, RunLifecycle::Terminal);
    assert_eq!(view.finish_reason, Some(FinishReason::Completed));
    assert_eq!(view.final_response.as_deref(), Some("final answer"));
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(
        model.inputs()[0].context,
        [ModelItem::UserInput {
            text: "request".to_owned()
        }]
    );
    assert!(model.inputs()[0].tools.is_empty());
    assert_eq!(authorizer.calls(), 0);
}

#[tokio::test]
async fn acceptance_02_one_read_only_tool_call_followed_by_final_response() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![
        ScriptedModelStep::Return(ModelInvocation::ToolRequest(tool_request(
            "call-1",
            json!({"key": "alpha"}),
        ))),
        ScriptedModelStep::Return(ModelInvocation::FinalResponse {
            text: "used fixture result".to_owned(),
        }),
    ]);
    let (mut catalog, tool_activity) = fixture_catalog(FixtureToolBehavior::Success(json!({
        "value": "fixture result"
    })));
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect("run completes");

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
    assert_eq!(view.finish_reason, Some(FinishReason::Completed));
    assert_eq!(tool_activity.starts(), 1);
    assert_eq!(model.invocation_count(), 2);
    assert_eq!(
        model.inputs()[1].context,
        [
            ModelItem::UserInput {
                text: "request".to_owned(),
            },
            ModelItem::ToolRequest {
                tool_call_id: ToolCallId::new("call-1"),
                name: "fixture.read".to_owned(),
                arguments: json!({"key": "alpha"}),
            },
            ModelItem::ToolResult {
                tool_call_id: ToolCallId::new("call-1"),
                outcome: ToolOutcome::Success {
                    value: json!({"value": "fixture result"}),
                },
            },
        ]
    );
    assert_eq!(
        view.model_context.last(),
        Some(&ModelItem::AssistantText {
            text: "used fixture result".to_owned(),
        })
    );
}

#[tokio::test]
async fn acceptance_04_unknown_tool_resolves_without_execution_or_authorization() {
    let mut store = InMemoryEventStore::new();
    let mut request = tool_request("call-1", json!({"key": "alpha"}));
    request.name = "missing.tool".to_owned();
    let mut model = ScriptedModel::new(vec![
        ScriptedModelStep::Return(ModelInvocation::ToolRequest(request)),
        ScriptedModelStep::Return(ModelInvocation::FinalResponse {
            text: "handled".to_owned(),
        }),
    ]);
    let mut catalog = ToolCatalog::empty();
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect("run completes");

    assert_eq!(tool_outcomes(&view), [&ToolOutcome::UnknownTool]);
    assert_eq!(view.finish_reason, Some(FinishReason::Completed));
    assert_eq!(view.final_response.as_deref(), Some("handled"));
    assert_eq!(model.invocation_count(), 2);
    assert_eq!(authorizer.calls(), 0);
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "tool_resolved",
            "model_started",
            "model_finished",
            "run_finished",
        ]
    );
}

#[tokio::test]
async fn acceptance_05_malformed_tool_arguments_resolve_before_authorization() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![
        ScriptedModelStep::Return(ModelInvocation::ToolRequest(tool_request(
            "call-1",
            json!({"key": 7}),
        ))),
        ScriptedModelStep::Return(ModelInvocation::FinalResponse {
            text: "handled".to_owned(),
        }),
    ]);
    let (mut catalog, tool_activity) = fixture_catalog(FixtureToolBehavior::Success(json!(null)));
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect("run completes");

    assert_eq!(tool_outcomes(&view), [&ToolOutcome::InvalidArguments]);
    assert_eq!(authorizer.calls(), 0);
    assert_eq!(tool_activity.starts(), 0);
    assert_eq!(view.finish_reason, Some(FinishReason::Completed));
    assert_eq!(view.final_response.as_deref(), Some("handled"));
    assert_eq!(model.invocation_count(), 2);
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "tool_resolved",
            "model_started",
            "model_finished",
            "run_finished",
        ]
    );
}

#[tokio::test]
async fn acceptance_06_authorization_denial_prevents_tool_execution() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![
        ScriptedModelStep::Return(ModelInvocation::ToolRequest(tool_request(
            "call-1",
            json!({"key": "alpha"}),
        ))),
        ScriptedModelStep::Return(ModelInvocation::FinalResponse {
            text: "handled".to_owned(),
        }),
    ]);
    let (mut catalog, tool_activity) = fixture_catalog(FixtureToolBehavior::Success(json!(null)));
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Deny);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect("run completes");

    assert_eq!(tool_outcomes(&view), [&ToolOutcome::Denied]);
    assert_eq!(authorizer.calls(), 1);
    assert_eq!(tool_activity.starts(), 0);
    assert_eq!(view.finish_reason, Some(FinishReason::Completed));
    assert_eq!(view.final_response.as_deref(), Some("handled"));
    assert_eq!(model.invocation_count(), 2);
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "tool_resolved",
            "model_started",
            "model_finished",
            "run_finished",
        ]
    );
}

#[tokio::test]
async fn acceptance_07_ordinary_tool_failure_is_model_visible_without_retry() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![
        ScriptedModelStep::Return(ModelInvocation::ToolRequest(tool_request(
            "call-1",
            json!({"key": "alpha"}),
        ))),
        ScriptedModelStep::Return(ModelInvocation::FinalResponse {
            text: "handled failure".to_owned(),
        }),
    ]);
    let (mut catalog, tool_activity) = fixture_catalog(FixtureToolBehavior::OrdinaryFailure);
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect("run completes");

    assert_eq!(tool_outcomes(&view), [&ToolOutcome::Failed]);
    assert_eq!(view.finish_reason, Some(FinishReason::Completed));
    assert_eq!(view.final_response.as_deref(), Some("handled failure"));
    assert_eq!(tool_activity.starts(), 1);
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
async fn acceptance_12_ordinary_model_failure_is_terminal_without_retry() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::OrdinaryFailure,
    )]);
    let mut catalog = ToolCatalog::empty();
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect("controlled model failure is a terminal run result");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::Ordinary))
    );
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(store.acknowledged().len(), 4);
}

#[tokio::test]
async fn model_request_failure_commits_the_exact_category_and_stops() {
    let category = ModelRequestFailure::RateLimited;
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::RequestFailure(category),
    )]);
    let (mut catalog, tool_activity) =
        fixture_catalog(FixtureToolBehavior::Success(json!({"value": "unused"})));
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect("provider-neutral failure is a terminal run outcome");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::Request(category)))
    );
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(tool_activity.starts(), 0);
    assert_eq!(
        store.acknowledged()[2].event,
        DomainEvent::ModelRequestFinished {
            step_id: aurora_core::StepId::new(1),
            outcome: ModelOutcome::RequestFailure(category),
        }
    );
    assert_eq!(
        store.acknowledged()[3].event,
        DomainEvent::RunFinished {
            reason: FinishReason::Failed(ModelFailure::Request(category)),
        }
    );
    assert!(
        store
            .acknowledged()
            .iter()
            .all(|envelope| envelope.schema_version == 2)
    );
}

#[tokio::test]
async fn acceptance_13_budget_exhaustion_blocks_the_next_model_step() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::ToolRequest(tool_request("call-1", json!({"key": "alpha"}))),
    )]);
    let (mut catalog, tool_activity) =
        fixture_catalog(FixtureToolBehavior::Success(json!({"value": "ok"})));
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
    let mut constrained = limits();
    constrained.max_model_steps = 1;
    constrained.max_tool_executions = 1;

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(constrained), CancellationToken::new())
    .await
    .expect("budget exhaustion is a terminal run result");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::BudgetExhausted(
            aurora_core::BudgetKind::ModelSteps
        ))
    );
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(tool_activity.starts(), 1);
    assert_eq!(
        event_names(store.acknowledged()).last(),
        Some(&"run_finished")
    );
}

#[tokio::test]
async fn tool_execution_budget_blocks_the_started_event_and_tool_body() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::ToolRequest(tool_request("call-1", json!({"key": "alpha"}))),
    )]);
    let (mut catalog, tool_activity) =
        fixture_catalog(FixtureToolBehavior::Success(json!({"value": "ok"})));
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();
    let mut constrained = limits();
    constrained.max_tool_executions = 0;

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(constrained), CancellationToken::new())
    .await
    .expect("tool budget exhaustion is a terminal run result");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::BudgetExhausted(
            aurora_core::BudgetKind::ToolExecutions
        ))
    );
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(authorizer.calls(), 1);
    assert_eq!(tool_activity.starts(), 0);
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
async fn acceptance_14_persistence_failure_before_tool_effect_blocks_execution() {
    let mut store = FaultingStore::new(4);
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::ToolRequest(tool_request("call-1", json!({"key": "alpha"}))),
    )]);
    let (mut catalog, tool_activity) =
        fixture_catalog(FixtureToolBehavior::Success(json!({"value": "ok"})));
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let error = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect_err("failed acknowledgement stops the driver");

    assert!(error.is_persistence());
    assert_eq!(store.acknowledged().len(), 3);
    assert_eq!(tool_activity.starts(), 0);
    let active = aurora_core::reconstruct(store.acknowledged()).expect("prefix reconstructs");
    assert!(
        !active
            .pending_operation
            .expect("tool request remains unresolved")
            .execution_started()
    );
    assert_faulting_store_is_poisoned(&mut store).await;
}

#[tokio::test]
async fn acceptance_15_persistence_failure_after_tool_effect_blocks_result_use() {
    let mut store = FaultingStore::new(5);
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::ToolRequest(tool_request("call-1", json!({"key": "alpha"}))),
    )]);
    let (mut catalog, tool_activity) =
        fixture_catalog(FixtureToolBehavior::Success(json!({"value": "ok"})));
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let error = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect_err("failed result acknowledgement stops the driver");

    assert!(error.is_persistence());
    assert_eq!(store.acknowledged().len(), 4);
    assert_eq!(tool_activity.starts(), 1);
    assert_eq!(tool_activity.stops(), 1);
    assert_eq!(model.invocation_count(), 1);
    let active = aurora_core::reconstruct(store.acknowledged()).expect("prefix reconstructs");
    assert!(
        active
            .pending_operation
            .expect("started tool remains outcome-unknown")
            .execution_started()
    );
    assert_faulting_store_is_poisoned(&mut store).await;
}

#[tokio::test]
async fn acceptance_16_persistence_failure_at_terminal_success_withholds_success() {
    let mut store = FaultingStore::new(4);
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::FinalResponse {
            text: "must not escape".to_owned(),
        },
    )]);
    let mut catalog = ToolCatalog::empty();
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let error = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect_err("terminal acknowledgement failure withholds success");

    assert!(error.is_persistence());
    assert_eq!(store.acknowledged().len(), 3);
    let active = aurora_core::reconstruct(store.acknowledged()).expect("prefix reconstructs");
    assert_eq!(active.lifecycle, RunLifecycle::Active);
    assert_eq!(active.final_response.as_deref(), Some("must not escape"));
    assert_faulting_store_is_poisoned(&mut store).await;
}

#[tokio::test]
async fn explicit_malformed_model_output_is_a_typed_terminal_failure() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(
        ModelInvocation::MalformedOutput,
    )]);
    let mut catalog = ToolCatalog::empty();
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect("malformed model output is recorded");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::MalformedOutput))
    );
    assert_eq!(model.invocation_count(), 1);
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
}

#[tokio::test]
async fn duplicate_tool_call_identifier_becomes_malformed_model_output() {
    let mut store = InMemoryEventStore::new();
    let repeated = tool_request("call-1", json!({"key": "alpha"}));
    let mut model = ScriptedModel::new(vec![
        ScriptedModelStep::Return(ModelInvocation::ToolRequest(repeated.clone())),
        ScriptedModelStep::Return(ModelInvocation::ToolRequest(repeated)),
    ]);
    let (mut catalog, tool_activity) =
        fixture_catalog(FixtureToolBehavior::Success(json!({"value": "ok"})));
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect("malformed model output terminates the run");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::MalformedOutput))
    );
    assert_eq!(tool_activity.starts(), 1);
    assert_eq!(tool_outcomes(&view).len(), 1);
    assert!(matches!(
        store.acknowledged()[6].event,
        DomainEvent::ModelRequestFinished {
            outcome: aurora_core::ModelOutcome::MalformedOutput,
            ..
        }
    ));
}

#[tokio::test]
async fn backend_result_cannot_forge_user_cancellation() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Return(ModelInvocation::Cancelled)]);
    let mut catalog = ToolCatalog::empty();
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect("a backend protocol violation terminates the run");

    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::Ordinary))
    );
    assert!(matches!(
        store.acknowledged()[2].event,
        DomainEvent::ModelRequestFinished {
            outcome: aurora_core::ModelOutcome::Failed,
            ..
        }
    ));
}

#[tokio::test]
async fn acceptance_22_model_child_panic_is_an_internal_failure_without_retry() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::Panic]);
    let mut catalog = ToolCatalog::empty();
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect("a joined model panic is recorded as a terminal run");

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
            outcome: ModelOutcome::ChildPanicked,
            ..
        }
    ));
    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::ChildPanicked))
    );
    assert_eq!(model.invocation_count(), 1);
    assert!(view.final_response.is_none());
    assert!(tool_outcomes(&view).is_empty());
}

#[tokio::test]
async fn acceptance_23_tool_child_panic_is_internal_and_never_model_visible() {
    let mut store = InMemoryEventStore::new();
    let mut model = ScriptedModel::new(vec![
        ScriptedModelStep::Return(ModelInvocation::ToolRequest(tool_request(
            "call-1",
            json!({"key": "alpha"}),
        ))),
        ScriptedModelStep::Return(ModelInvocation::FinalResponse {
            text: "must not run".to_owned(),
        }),
    ]);
    let (mut catalog, tool_activity) = fixture_catalog(FixtureToolBehavior::Panic);
    let authorizer = CountingAuthorizer::new(AuthorizationDecision::Allow);
    let mut timestamp = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &authorizer,
        &mut timestamp,
    )
    .run(start(limits()), CancellationToken::new())
    .await
    .expect("a joined tool panic is recorded as a terminal run");

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
    assert!(matches!(
        store.acknowledged()[4].event,
        DomainEvent::ToolCallResolved {
            outcome: ToolOutcome::ChildPanicked,
            ..
        }
    ));
    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::ChildPanicked))
    );
    assert_eq!(model.invocation_count(), 1);
    assert_eq!(tool_activity.starts(), 1);
    assert_eq!(tool_activity.stops(), 1);
    assert_eq!(tool_outcomes(&view), [&ToolOutcome::ChildPanicked]);
    assert!(view.final_response.is_none());
}
