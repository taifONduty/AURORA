use std::{fs, path::Path};

use crate::event::RunProjection;
use crate::{
    DecodeError, DomainEvent, EventEnvelope, EventStore, FinishReason, ProjectionError, RunId,
    RunLifecycle, RunView, SCHEMA_VERSION, StoreError, decode_jsonl, reconstruct,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inspection {
    Clean(RunView),
    IncompleteTail {
        prefix: Option<RunView>,
        tail: Vec<u8>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum InspectionError {
    #[error("event log could not be read: {message}")]
    Read { message: String },
    #[error("event log could not be decoded: {0}")]
    Decode(#[from] DecodeError),
    #[error("event log could not be reconstructed: {0}")]
    Projection(#[from] ProjectionError),
}

pub fn inspect_jsonl(path: impl AsRef<Path>) -> Result<Inspection, InspectionError> {
    // Inspection is intentionally independent of the writer lifecycle: it
    // reads visible bytes without taking a writer lock, appending, or repairing.
    let bytes = fs::read(path).map_err(|error| InspectionError::Read {
        message: error.to_string(),
    })?;
    let decoded = decode_jsonl(&bytes)?;
    let incomplete_tail = decoded.incomplete_tail().map(<[u8]>::to_vec);
    let prefix = decoded.into_envelopes();

    match incomplete_tail {
        Some(tail) => {
            let prefix = if prefix.is_empty() {
                None
            } else {
                Some(reconstruct(&prefix)?)
            };
            Ok(Inspection::IncompleteTail { prefix, tail })
        }
        None => Ok(Inspection::Clean(reconstruct(&prefix)?)),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    #[error("run is already terminal")]
    AlreadyTerminal,
    #[error("run history could not be reconstructed: {0}")]
    Projection(#[from] ProjectionError),
    #[error("interrupted terminal event could not be committed: {0}")]
    Store(#[from] StoreError),
}

pub async fn recover_as_interrupted(
    store: &mut dyn EventStore,
    run_id: &RunId,
    observed_at: &str,
) -> Result<RunView, RecoveryError> {
    let mut projection = RunProjection::from_history(store.acknowledged())?;
    let view = projection.view().ok_or(ProjectionError::EmptyHistory)?;
    if view.lifecycle == RunLifecycle::Terminal {
        return Err(RecoveryError::AlreadyTerminal);
    }

    // Recovery has no model or tool capability. It can preserve an unknown
    // external outcome, but it cannot infer or replay that outcome.
    let envelope = EventEnvelope {
        schema_version: SCHEMA_VERSION,
        sequence: projection.next_sequence(),
        run_id: run_id.clone(),
        observed_at: observed_at.to_owned(),
        event: DomainEvent::RunFinished {
            reason: FinishReason::Interrupted,
        },
    };
    let prepared = projection.prepare_transition(envelope)?;
    store.commit(prepared.envelope().clone()).await?;
    projection.apply(prepared);
    projection.into_view().map_err(RecoveryError::Projection)
}
