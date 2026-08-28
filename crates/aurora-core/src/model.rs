use std::{
    collections::VecDeque,
    future::{Future, pending},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::{ModelRequestFailure, ToolCallId, ToolDefinition, ToolOutcome, ToolRequest};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInput {
    pub context: Vec<ModelItem>,
    pub tools: Vec<ToolDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelItem {
    UserInput {
        text: String,
    },
    AssistantText {
        text: String,
    },
    ToolRequest {
        tool_call_id: ToolCallId,
        name: String,
        arguments: serde_json::Value,
    },
    ToolResult {
        tool_call_id: ToolCallId,
        outcome: ToolOutcome,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelInvocation {
    FinalResponse { text: String },
    ToolRequest(ToolRequest),
    RequestFailure(ModelRequestFailure),
    OrdinaryFailure,
    MalformedOutput,
    Cancelled,
}

pub type ModelFuture = Pin<Box<dyn Future<Output = ModelInvocation> + Send + 'static>>;

pub trait ModelBackend {
    /// Creates the owned invocation future without doing blocking work.
    /// Runtime work belongs inside the returned future so the driver can
    /// supervise its deadline, cancellation, and shutdown.
    fn invoke(&mut self, input: ModelInput, cancellation: CancellationToken) -> ModelFuture;
}

#[derive(Clone, Debug)]
pub struct ActivitySignal {
    inner: Arc<ActivityInner>,
}

#[derive(Debug, Default)]
struct ActivityInner {
    starts: AtomicUsize,
    stops: AtomicUsize,
    changed: Notify,
}

impl Default for ActivitySignal {
    fn default() -> Self {
        Self {
            inner: Arc::new(ActivityInner::default()),
        }
    }
}

impl ActivitySignal {
    pub fn starts(&self) -> usize {
        self.inner.starts.load(Ordering::Acquire)
    }

    pub fn stops(&self) -> usize {
        self.inner.stops.load(Ordering::Acquire)
    }

    pub async fn wait_for_starts(&self, expected: usize) {
        loop {
            let changed = self.inner.changed.notified();
            if self.starts() >= expected {
                return;
            }
            changed.await;
        }
    }

    pub async fn wait_for_stops(&self, expected: usize) {
        loop {
            let changed = self.inner.changed.notified();
            if self.stops() >= expected {
                return;
            }
            changed.await;
        }
    }

    pub(crate) fn start_guard(&self) -> ActivityGuard {
        self.inner.starts.fetch_add(1, Ordering::Release);
        self.inner.changed.notify_waiters();
        ActivityGuard {
            activity: self.clone(),
        }
    }
}

pub(crate) struct ActivityGuard {
    activity: ActivitySignal,
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.activity.inner.stops.fetch_add(1, Ordering::Release);
        self.activity.inner.changed.notify_waiters();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptedModelStep {
    Return(ModelInvocation),
    Panic,
    WaitForCancellation,
    IgnoreCancellation,
}

#[derive(Debug)]
pub struct ScriptedModel {
    steps: VecDeque<ScriptedModelStep>,
    inputs: Vec<ModelInput>,
    activity: ActivitySignal,
}

impl ScriptedModel {
    pub fn new(steps: Vec<ScriptedModelStep>) -> Self {
        Self {
            steps: steps.into(),
            inputs: Vec::new(),
            activity: ActivitySignal::default(),
        }
    }

    pub fn invocation_count(&self) -> usize {
        self.inputs.len()
    }

    pub fn inputs(&self) -> &[ModelInput] {
        &self.inputs
    }

    pub fn activity_signal(&self) -> ActivitySignal {
        self.activity.clone()
    }
}

impl ModelBackend for ScriptedModel {
    fn invoke(&mut self, input: ModelInput, cancellation: CancellationToken) -> ModelFuture {
        self.inputs.push(input);
        let activity = self.activity.clone();
        match self.steps.pop_front() {
            Some(ScriptedModelStep::Return(outcome)) => Box::pin(async move {
                let _activity = activity.start_guard();
                outcome
            }),
            Some(ScriptedModelStep::Panic) => Box::pin(async move {
                let _activity = activity.start_guard();
                panic!("scripted model child panic")
            }),
            Some(ScriptedModelStep::WaitForCancellation) => Box::pin(async move {
                let _activity = activity.start_guard();
                cancellation.cancelled().await;
                ModelInvocation::Cancelled
            }),
            Some(ScriptedModelStep::IgnoreCancellation) => Box::pin(async move {
                let _activity = activity.start_guard();
                pending::<ModelInvocation>().await
            }),
            None => Box::pin(async move {
                let _activity = activity.start_guard();
                ModelInvocation::OrdinaryFailure
            }),
        }
    }
}
