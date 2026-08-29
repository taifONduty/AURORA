use crate::{
    InvestigationFailure, InvestigationResult, InvestigationTask, InvestigationTaskId,
    PlanningValidationError, ResearchPlan, ResearchRequest, ResearchStopReason,
};

pub const INVESTIGATION_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvestigationEvent {
    RequestRecorded(ResearchRequest),
    PlanRecorded(ResearchPlan),
    TaskStarted {
        task_id: InvestigationTaskId,
    },
    TaskCompleted {
        task_id: InvestigationTaskId,
        result: InvestigationResult,
    },
    TaskFailed {
        task_id: InvestigationTaskId,
        failure: InvestigationFailure,
    },
    FollowUpRecorded(InvestigationTask),
    ResearchStopped(ResearchStopReason),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvestigationRecord {
    schema_version: u32,
    sequence: u64,
    event: InvestigationEvent,
}

impl InvestigationRecord {
    pub fn new(sequence: u64, event: InvestigationEvent) -> Result<Self, PlanningValidationError> {
        if sequence == 0 {
            return Err(PlanningValidationError::ZeroInvestigationSequence);
        }
        Ok(Self {
            schema_version: INVESTIGATION_SCHEMA_VERSION,
            sequence,
            event,
        })
    }

    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub const fn event(&self) -> &InvestigationEvent {
        &self.event
    }

    pub(crate) fn into_event(self) -> InvestigationEvent {
        self.event
    }
}
