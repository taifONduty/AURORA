//! Deterministic runtime contracts for AURORA's first executable slice.

mod codec;
mod driver;
mod event;
mod inspection;
mod model;
mod run;
mod store;
mod tool;

pub use codec::{
    DecodeError, DecodedLog, EncodeError, decode_envelope_line, decode_jsonl, encode_envelope,
};
pub use driver::{
    AuthorizationDecision, Authorizer, DriverError, RunDriver, RunStart, ToolAuthorization,
    UnconfirmedChild,
};
pub use event::{
    DomainEvent, EventEnvelope, ModelOutcome, ProjectionError, SCHEMA_VERSION, ToolOutcome,
    ToolRequest, reconstruct,
};
pub use inspection::{
    Inspection, InspectionError, RecoveryError, inspect_jsonl, recover_as_interrupted,
};
pub use model::{
    ActivitySignal, ModelBackend, ModelFuture, ModelInput, ModelInvocation, ModelItem,
    ScriptedModel, ScriptedModelStep,
};
pub use run::{
    BudgetKind, EventSeq, FinishReason, ModelFailure, ModelRequestFailure, PendingOperation, RunId,
    RunLifecycle, RunLimits, RunView, StepId, ToolCallId, ToolEffect,
};
pub use store::{EventStore, InMemoryEventStore, JsonlEventStore, StoreError, StoreFuture};
pub use tool::{
    CatalogError, FixtureTool, FixtureToolBehavior, Tool, ToolBodyResult, ToolCatalog,
    ToolDefinition, ToolFuture, ValidationError,
};
