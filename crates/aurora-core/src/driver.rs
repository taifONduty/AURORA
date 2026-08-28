use std::{future::Future, pin::Pin, time::Duration};

use tokio::{
    runtime::Handle,
    task::{AbortHandle, JoinHandle},
};
use tokio_util::sync::CancellationToken;

use crate::event::RunProjection;
use crate::{
    BudgetKind, DomainEvent, EventEnvelope, EventStore, FinishReason, ModelBackend, ModelFailure,
    ModelInput, ModelInvocation, ModelOutcome, ProjectionError, RunId, RunLimits, RunView,
    SCHEMA_VERSION, StepId, StoreError, ToolBodyResult, ToolCallId, ToolCatalog, ToolEffect,
    ToolOutcome, ToolRequest,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorizationDecision {
    Allow,
    Deny,
}

#[derive(Debug)]
pub struct ToolAuthorization<'a> {
    pub tool_call_id: &'a ToolCallId,
    pub name: &'a str,
    pub arguments: &'a serde_json::Value,
    pub effect: ToolEffect,
}

pub trait Authorizer {
    fn authorize(&self, request: &ToolAuthorization<'_>) -> AuthorizationDecision;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunStart {
    pub run_id: RunId,
    pub request: String,
    pub limits: RunLimits,
}

#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("a run driver requires an empty acknowledged event store")]
    AcknowledgedHistory,
    #[error("run persistence failed: {0}")]
    Persistence(#[from] StoreError),
    #[error("committed run history became malformed: {0}")]
    Projection(#[from] ProjectionError),
    #[error("owned child termination could not be confirmed within the shutdown bound")]
    ChildShutdownUnconfirmed(UnconfirmedChild),
}

impl DriverError {
    pub fn is_persistence(&self) -> bool {
        matches!(self, Self::Persistence(_))
    }

    pub fn into_unconfirmed_child(self) -> Option<UnconfirmedChild> {
        match self {
            Self::ChildShutdownUnconfirmed(child) => Some(child),
            _ => None,
        }
    }
}

pub struct RunDriver<'a> {
    store: &'a mut dyn EventStore,
    model: &'a mut dyn ModelBackend,
    catalog: &'a mut ToolCatalog,
    authorizer: &'a dyn Authorizer,
    observed_at: &'a mut dyn FnMut() -> String,
    projection: RunProjection,
}

impl<'a> RunDriver<'a> {
    pub fn new(
        store: &'a mut dyn EventStore,
        model: &'a mut dyn ModelBackend,
        catalog: &'a mut ToolCatalog,
        authorizer: &'a dyn Authorizer,
        observed_at: &'a mut dyn FnMut() -> String,
    ) -> Self {
        Self {
            store,
            model,
            catalog,
            authorizer,
            observed_at,
            projection: RunProjection::empty(),
        }
    }

    pub async fn run(
        &mut self,
        start: RunStart,
        user_cancellation: CancellationToken,
    ) -> Result<RunView, DriverError> {
        if !self.store.acknowledged().is_empty() {
            return Err(DriverError::AcknowledgedHistory);
        }
        let run_id = start.run_id;
        let mut view = self
            .commit(
                &run_id,
                DomainEvent::RunStarted {
                    request: start.request,
                    limits: start.limits,
                },
            )
            .await?;

        loop {
            if user_cancellation.is_cancelled() {
                return self.finish(&run_id, FinishReason::Cancelled).await;
            }
            if view.model_steps_consumed >= view.limits.max_model_steps {
                return self
                    .finish(
                        &run_id,
                        FinishReason::BudgetExhausted(BudgetKind::ModelSteps),
                    )
                    .await;
            }

            let step_id = StepId::new(u64::from(view.model_steps_consumed) + 1);
            view = self
                .commit(&run_id, DomainEvent::ModelRequestStarted { step_id })
                .await?;

            if user_cancellation.is_cancelled() {
                self.commit(
                    &run_id,
                    DomainEvent::ModelRequestFinished {
                        step_id,
                        outcome: ModelOutcome::Cancelled,
                    },
                )
                .await?;
                return self.finish(&run_id, FinishReason::Cancelled).await;
            }

            let model_input = ModelInput {
                context: view.model_context.clone(),
                tools: self.catalog.definitions(),
            };
            let child_cancellation = CancellationToken::new();
            let model_future = self.model.invoke(model_input, child_cancellation.clone());
            let child_exit = await_owned_child(
                model_future,
                &user_cancellation,
                child_cancellation,
                view.limits.model_timeout_ms,
                view.limits.shutdown_grace_period_ms,
            )
            .await
            .map_err(DriverError::ChildShutdownUnconfirmed)?;
            let (outcome, next) = match child_exit {
                ChildExit::Completed(invocation) => {
                    normalize_model_invocation(&self.projection, invocation)
                }
                ChildExit::Panicked => (
                    ModelOutcome::ChildPanicked,
                    AfterModel::Fail(ModelFailure::ChildPanicked),
                ),
                ChildExit::UserCancelled => (ModelOutcome::Cancelled, AfterModel::Cancel),
                ChildExit::TimedOut => (
                    ModelOutcome::TimedOut,
                    AfterModel::Fail(ModelFailure::Timeout),
                ),
                ChildExit::ShutdownFailed => (
                    ModelOutcome::ChildShutdownFailed,
                    AfterModel::Fail(ModelFailure::ChildShutdown),
                ),
            };
            view = self
                .commit(
                    &run_id,
                    DomainEvent::ModelRequestFinished { step_id, outcome },
                )
                .await?;

            match next {
                AfterModel::Complete => return self.finish(&run_id, FinishReason::Completed).await,
                AfterModel::Fail(failure) => {
                    return self.finish(&run_id, FinishReason::Failed(failure)).await;
                }
                AfterModel::Cancel => return self.finish(&run_id, FinishReason::Cancelled).await,
                AfterModel::UseTool(request) => {
                    match self
                        .resolve_tool(&run_id, &view, step_id, request, &user_cancellation)
                        .await?
                    {
                        AfterTool::Continue => {
                            view = self
                                .projection
                                .view()
                                .expect("a committed tool result retains the run")
                                .clone();
                        }
                        AfterTool::Cancel => {
                            return self.finish(&run_id, FinishReason::Cancelled).await;
                        }
                        AfterTool::Fail(failure) => {
                            return self.finish(&run_id, FinishReason::Failed(failure)).await;
                        }
                        AfterTool::BudgetExhausted => {
                            return self
                                .finish(
                                    &run_id,
                                    FinishReason::BudgetExhausted(BudgetKind::ToolExecutions),
                                )
                                .await;
                        }
                    }
                }
            }
        }
    }

    async fn resolve_tool(
        &mut self,
        run_id: &RunId,
        view: &RunView,
        step_id: StepId,
        request: ToolRequest,
        user_cancellation: &CancellationToken,
    ) -> Result<AfterTool, DriverError> {
        if user_cancellation.is_cancelled() {
            return Ok(AfterTool::Cancel);
        }
        let Some(effect) = self.catalog.effect(&request.name) else {
            self.commit(
                run_id,
                DomainEvent::ToolCallResolved {
                    step_id,
                    tool_call_id: request.tool_call_id,
                    outcome: ToolOutcome::UnknownTool,
                },
            )
            .await?;
            return Ok(AfterTool::Continue);
        };

        let arguments_are_valid = {
            let tool = self
                .catalog
                .get_mut(&request.name)
                .expect("resolved tool remains registered during a run");
            tool.validate(&request.arguments).is_ok()
        };
        if !arguments_are_valid {
            self.commit(
                run_id,
                DomainEvent::ToolCallResolved {
                    step_id,
                    tool_call_id: request.tool_call_id,
                    outcome: ToolOutcome::InvalidArguments,
                },
            )
            .await?;
            return Ok(AfterTool::Continue);
        }

        if self.authorizer.authorize(&ToolAuthorization {
            tool_call_id: &request.tool_call_id,
            name: &request.name,
            arguments: &request.arguments,
            effect,
        }) == AuthorizationDecision::Deny
        {
            self.commit(
                run_id,
                DomainEvent::ToolCallResolved {
                    step_id,
                    tool_call_id: request.tool_call_id,
                    outcome: ToolOutcome::Denied,
                },
            )
            .await?;
            return Ok(AfterTool::Continue);
        }

        if view.tool_executions_consumed >= view.limits.max_tool_executions {
            return Ok(AfterTool::BudgetExhausted);
        }
        if user_cancellation.is_cancelled() {
            return Ok(AfterTool::Cancel);
        }

        self.commit(
            run_id,
            DomainEvent::ToolExecutionStarted {
                step_id,
                tool_call_id: request.tool_call_id.clone(),
                name: request.name.clone(),
                arguments: request.arguments.clone(),
                effect,
            },
        )
        .await?;

        if user_cancellation.is_cancelled() {
            self.commit(
                run_id,
                DomainEvent::ToolCallResolved {
                    step_id,
                    tool_call_id: request.tool_call_id,
                    outcome: ToolOutcome::Cancelled,
                },
            )
            .await?;
            return Ok(AfterTool::Cancel);
        }

        let child_cancellation = CancellationToken::new();
        let tool = self
            .catalog
            .get_mut(&request.name)
            .expect("registered tool remains present for the duration of a run");
        let tool_future = tool.execute(request.arguments, child_cancellation.clone());
        let child_exit = await_owned_child(
            tool_future,
            user_cancellation,
            child_cancellation,
            view.limits.tool_timeout_ms,
            view.limits.shutdown_grace_period_ms,
        )
        .await
        .map_err(DriverError::ChildShutdownUnconfirmed)?;
        let (outcome, after) = match child_exit {
            ChildExit::Completed(ToolBodyResult::Success(value)) => {
                (ToolOutcome::Success { value }, AfterTool::Continue)
            }
            ChildExit::Completed(ToolBodyResult::Failed) => {
                (ToolOutcome::Failed, AfterTool::Continue)
            }
            ChildExit::Completed(ToolBodyResult::Cancelled) => {
                (ToolOutcome::Failed, AfterTool::Continue)
            }
            ChildExit::UserCancelled => (ToolOutcome::Cancelled, AfterTool::Cancel),
            ChildExit::TimedOut => (ToolOutcome::TimedOut, AfterTool::Continue),
            ChildExit::Panicked => (
                ToolOutcome::ChildPanicked,
                AfterTool::Fail(ModelFailure::ChildPanicked),
            ),
            ChildExit::ShutdownFailed => (
                ToolOutcome::ChildShutdownFailed,
                AfterTool::Fail(ModelFailure::ChildShutdown),
            ),
        };
        self.commit(
            run_id,
            DomainEvent::ToolCallResolved {
                step_id,
                tool_call_id: request.tool_call_id,
                outcome,
            },
        )
        .await?;

        Ok(after)
    }

    async fn finish(
        &mut self,
        run_id: &RunId,
        reason: FinishReason,
    ) -> Result<RunView, DriverError> {
        self.commit(run_id, DomainEvent::RunFinished { reason })
            .await
    }

    async fn commit(&mut self, run_id: &RunId, event: DomainEvent) -> Result<RunView, DriverError> {
        let envelope = EventEnvelope {
            schema_version: SCHEMA_VERSION,
            sequence: self.projection.next_sequence(),
            run_id: run_id.clone(),
            observed_at: (self.observed_at)(),
            event,
        };
        let prepared = self.projection.prepare_transition(envelope)?;
        self.store.commit(prepared.envelope().clone()).await?;
        self.projection.apply(prepared);
        Ok(self
            .projection
            .view()
            .expect("a committed transition creates or retains the run")
            .clone())
    }
}

enum AfterModel {
    Complete,
    Fail(ModelFailure),
    Cancel,
    UseTool(ToolRequest),
}

fn normalize_model_invocation(
    projection: &RunProjection,
    invocation: ModelInvocation,
) -> (ModelOutcome, AfterModel) {
    match invocation {
        ModelInvocation::FinalResponse { text } => {
            (ModelOutcome::FinalResponse { text }, AfterModel::Complete)
        }
        ModelInvocation::ToolRequest(request) => {
            let duplicate = projection.contains_tool_call(&request.tool_call_id);
            if duplicate
                || request.tool_call_id.as_str().is_empty()
                || request.name.is_empty()
                || !request.arguments.is_object()
            {
                (
                    ModelOutcome::MalformedOutput,
                    AfterModel::Fail(ModelFailure::MalformedOutput),
                )
            } else {
                (
                    ModelOutcome::ToolRequest(request.clone()),
                    AfterModel::UseTool(request),
                )
            }
        }
        ModelInvocation::RequestFailure(category) => (
            ModelOutcome::RequestFailure(category),
            AfterModel::Fail(ModelFailure::Request(category)),
        ),
        ModelInvocation::OrdinaryFailure => (
            ModelOutcome::Failed,
            AfterModel::Fail(ModelFailure::Ordinary),
        ),
        ModelInvocation::MalformedOutput => (
            ModelOutcome::MalformedOutput,
            AfterModel::Fail(ModelFailure::MalformedOutput),
        ),
        // Cancellation is assigned by the driver after observing its own
        // cancellation token. A backend result cannot impersonate the caller.
        ModelInvocation::Cancelled => (
            ModelOutcome::Failed,
            AfterModel::Fail(ModelFailure::Ordinary),
        ),
    }
}

enum AfterTool {
    Continue,
    Cancel,
    Fail(ModelFailure),
    BudgetExhausted,
}

enum ChildExit<T> {
    Completed(T),
    Panicked,
    UserCancelled,
    TimedOut,
    ShutdownFailed,
}

async fn await_owned_child<F, T>(
    future: F,
    user_cancellation: &CancellationToken,
    child_cancellation: CancellationToken,
    timeout_ms: u64,
    shutdown_grace_period_ms: u64,
) -> Result<ChildExit<T>, UnconfirmedChild>
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let mut child = OwnedChild::new(tokio::spawn(future));
    let deadline = tokio::time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(deadline);

    tokio::select! {
        biased;
        result = child.handle_mut() => {
            child.clear();
            return Ok(joined_result(result));
        },
        () = user_cancellation.cancelled() => {}
        () = &mut deadline => {
            return stop_child(
                child,
                child_cancellation,
                shutdown_grace_period_ms,
                ChildExit::TimedOut,
            ).await;
        }
    }

    stop_child(
        child,
        child_cancellation,
        shutdown_grace_period_ms,
        ChildExit::UserCancelled,
    )
    .await
}

fn joined_result<T>(result: Result<T, tokio::task::JoinError>) -> ChildExit<T> {
    match result {
        Ok(output) => ChildExit::Completed(output),
        Err(error) => child_exit_from_join_error(error),
    }
}

fn child_exit_from_join_error<T>(error: tokio::task::JoinError) -> ChildExit<T> {
    if error.is_panic() {
        ChildExit::Panicked
    } else {
        ChildExit::ShutdownFailed
    }
}

async fn stop_child<T>(
    mut child: OwnedChild<T>,
    child_cancellation: CancellationToken,
    shutdown_grace_period_ms: u64,
    stopped: ChildExit<T>,
) -> Result<ChildExit<T>, UnconfirmedChild>
where
    T: Send + 'static,
{
    child_cancellation.cancel();
    match tokio::time::timeout(
        Duration::from_millis(shutdown_grace_period_ms),
        child.handle_mut(),
    )
    .await
    {
        Ok(result) => {
            child.clear();
            match result {
                Err(error) => Ok(child_exit_from_join_error(error)),
                Ok(_) => Ok(stopped),
            }
        }
        Err(_) => {
            child.abort();
            match tokio::time::timeout(
                Duration::from_millis(shutdown_grace_period_ms),
                child.handle_mut(),
            )
            .await
            {
                Ok(Err(error)) => {
                    child.clear();
                    Ok(child_exit_from_join_error(error))
                }
                Ok(Ok(_)) => {
                    child.clear();
                    Ok(ChildExit::ShutdownFailed)
                }
                Err(_) => Err(child.into_unconfirmed()),
            }
        }
    }
}

struct OwnedChild<T: Send + 'static> {
    handle: Option<JoinHandle<T>>,
    runtime: Handle,
}

impl<T: Send + 'static> OwnedChild<T> {
    fn new(handle: JoinHandle<T>) -> Self {
        Self {
            handle: Some(handle),
            runtime: Handle::current(),
        }
    }

    fn handle_mut(&mut self) -> &mut JoinHandle<T> {
        self.handle
            .as_mut()
            .expect("owned child handle is present while it is awaited")
    }

    fn abort(&self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }

    fn clear(&mut self) {
        self.handle = None;
    }
}

impl<T: Send + 'static> OwnedChild<T> {
    fn into_unconfirmed(mut self) -> UnconfirmedChild {
        let handle = self
            .handle
            .take()
            .expect("unconfirmed child retains its join handle");
        let abort = handle.abort_handle();
        let runtime = self.runtime.clone();
        let join = Box::pin(async move {
            let _ = handle.await;
        });
        UnconfirmedChild {
            abort,
            join: Some(join),
            runtime,
        }
    }
}

impl<T: Send + 'static> Drop for OwnedChild<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            // Dropping the outer run future cannot await cleanup. Transfer the
            // join handle to a one-shot runtime task so the model or tool task
            // is still owned until Tokio confirms that it stopped.
            std::mem::drop(self.runtime.spawn(async move {
                let _ = handle.await;
            }));
        }
    }
}

#[must_use = "join the retained child before treating shutdown as confirmed"]
pub struct UnconfirmedChild {
    abort: AbortHandle,
    join: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    runtime: Handle,
}

impl UnconfirmedChild {
    pub async fn join(mut self) {
        if let Some(join) = self.join.as_mut() {
            join.as_mut().await;
            self.join = None;
        }
    }
}

impl std::fmt::Debug for UnconfirmedChild {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UnconfirmedChild")
            .finish_non_exhaustive()
    }
}

impl Drop for UnconfirmedChild {
    fn drop(&mut self) {
        self.abort.abort();
        if let Some(join) = self.join.take() {
            std::mem::drop(self.runtime.spawn(join));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dropping_unconfirmed_child_still_observes_its_join_result() {
        let joined = CancellationToken::new();
        let joined_after_wait = joined.clone();
        let handle = tokio::spawn(std::future::pending::<()>());
        let child = UnconfirmedChild {
            abort: handle.abort_handle(),
            join: Some(Box::pin(async move {
                let _ = handle.await;
                joined_after_wait.cancel();
            })),
            runtime: Handle::current(),
        };

        drop(child);

        tokio::time::timeout(Duration::from_secs(1), joined.cancelled())
            .await
            .expect("dropping the public owner still observes child termination");
    }

    #[tokio::test]
    async fn dropping_pending_unconfirmed_child_join_future_still_observes_termination() {
        let joined = CancellationToken::new();
        let joined_after_wait = joined.clone();
        let handle = tokio::spawn(std::future::pending::<()>());
        let child = UnconfirmedChild {
            abort: handle.abort_handle(),
            join: Some(Box::pin(async move {
                let _ = handle.await;
                joined_after_wait.cancel();
            })),
            runtime: Handle::current(),
        };

        let mut public_join = Box::pin(child.join());
        std::future::poll_fn(|context| {
            assert!(matches!(
                public_join.as_mut().poll(context),
                std::task::Poll::Pending
            ));
            std::task::Poll::Ready(())
        })
        .await;
        drop(public_join);

        tokio::time::timeout(Duration::from_secs(1), joined.cancelled())
            .await
            .expect("dropping a pending public join still transfers child cleanup");
    }

    #[tokio::test]
    async fn unexpected_non_panic_join_cancellation_is_child_shutdown_failure() {
        let child = tokio::spawn(std::future::pending::<()>());
        child.abort();

        assert!(matches!(
            joined_result(child.await),
            ChildExit::ShutdownFailed
        ));
    }
}
