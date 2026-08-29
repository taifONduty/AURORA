use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    BlockedReason, CodecError, INVESTIGATION_SCHEMA_VERSION, IdentityError, InvestigationEvent,
    InvestigationFailure, InvestigationRecord, InvestigationResult, InvestigationTask,
    InvestigationTaskId, PlanningValidationError, ResearchGap, ResearchPlan, ResearchRequest,
    ResearchStopReason, TaskOrigin, decode_record, encode_record,
};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InvestigationCodecError {
    #[error("investigation record could not be encoded")]
    Encoding,
    #[error("investigation record JSON is malformed")]
    MalformedJson,
    #[error("unsupported investigation schema version {0}")]
    UnsupportedSchema(u32),
    #[error("investigation record identity is invalid: {0}")]
    InvalidIdentity(#[from] IdentityError),
    #[error("investigation record is invalid: {0}")]
    InvalidRecord(#[from] PlanningValidationError),
    #[error("investigation result contains an invalid research record: {0}")]
    InvalidResearchRecord(CodecError),
}

#[derive(Serialize, Deserialize)]
struct WireRecord {
    schema_version: u32,
    sequence: u64,
    event: WireEvent,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireEvent {
    RequestRecorded { request: WireRequest },
    PlanRecorded { plan: WirePlan },
    TaskStarted { task_id: String },
    TaskCompleted { task_id: String, result: WireResult },
    TaskFailed { task_id: String, failure: String },
    FollowUpRecorded { task: WireTask },
    ResearchStopped { reason: WireStopReason },
}

#[derive(Serialize, Deserialize)]
struct WireRequest {
    question: String,
}

#[derive(Serialize, Deserialize)]
struct WirePlan {
    tasks: Vec<WireTask>,
}

#[derive(Serialize, Deserialize)]
struct WireTask {
    id: String,
    objective: String,
    origin: WireTaskOrigin,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireTaskOrigin {
    Initial,
    FollowUp { parent_task_id: String, gap: String },
}

#[derive(Serialize, Deserialize)]
struct WireResult {
    research_records: Vec<Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireStopReason {
    OperatorStopped,
    BudgetExhausted,
    Blocked { reason: String },
}

pub fn encode_investigation_record(
    record: &InvestigationRecord,
) -> Result<Vec<u8>, InvestigationCodecError> {
    let wire = WireRecord::try_from(record)?;
    serde_json::to_vec(&wire).map_err(|_| InvestigationCodecError::Encoding)
}

pub fn decode_investigation_record(
    bytes: &[u8],
) -> Result<InvestigationRecord, InvestigationCodecError> {
    let wire: WireRecord =
        serde_json::from_slice(bytes).map_err(|_| InvestigationCodecError::MalformedJson)?;
    if wire.schema_version != INVESTIGATION_SCHEMA_VERSION {
        return Err(InvestigationCodecError::UnsupportedSchema(
            wire.schema_version,
        ));
    }
    InvestigationRecord::new(wire.sequence, InvestigationEvent::try_from(wire.event)?)
        .map_err(InvestigationCodecError::InvalidRecord)
}

impl TryFrom<&InvestigationRecord> for WireRecord {
    type Error = InvestigationCodecError;

    fn try_from(record: &InvestigationRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            schema_version: record.schema_version(),
            sequence: record.sequence(),
            event: WireEvent::try_from(record.event())?,
        })
    }
}

impl TryFrom<&InvestigationEvent> for WireEvent {
    type Error = InvestigationCodecError;

    fn try_from(event: &InvestigationEvent) -> Result<Self, Self::Error> {
        match event {
            InvestigationEvent::RequestRecorded(request) => Ok(Self::RequestRecorded {
                request: WireRequest {
                    question: request.question().to_owned(),
                },
            }),
            InvestigationEvent::PlanRecorded(plan) => Ok(Self::PlanRecorded {
                plan: WirePlan {
                    tasks: plan.tasks().iter().map(WireTask::from).collect(),
                },
            }),
            InvestigationEvent::TaskStarted { task_id } => Ok(Self::TaskStarted {
                task_id: task_id.to_string(),
            }),
            InvestigationEvent::TaskCompleted { task_id, result } => Ok(Self::TaskCompleted {
                task_id: task_id.to_string(),
                result: WireResult::try_from(result)?,
            }),
            InvestigationEvent::TaskFailed { task_id, failure } => Ok(Self::TaskFailed {
                task_id: task_id.to_string(),
                failure: failure.as_str().to_owned(),
            }),
            InvestigationEvent::FollowUpRecorded(task) => Ok(Self::FollowUpRecorded {
                task: WireTask::from(task),
            }),
            InvestigationEvent::ResearchStopped(reason) => Ok(Self::ResearchStopped {
                reason: WireStopReason::from(reason),
            }),
        }
    }
}

impl From<&InvestigationTask> for WireTask {
    fn from(task: &InvestigationTask) -> Self {
        Self {
            id: task.id().to_string(),
            objective: task.objective().to_owned(),
            origin: WireTaskOrigin::from(task.origin()),
        }
    }
}

impl From<&TaskOrigin> for WireTaskOrigin {
    fn from(origin: &TaskOrigin) -> Self {
        match origin {
            TaskOrigin::Initial => Self::Initial,
            TaskOrigin::FollowUp {
                parent_task_id,
                gap,
            } => Self::FollowUp {
                parent_task_id: parent_task_id.to_string(),
                gap: gap.as_str().to_owned(),
            },
        }
    }
}

impl TryFrom<&InvestigationResult> for WireResult {
    type Error = InvestigationCodecError;

    fn try_from(result: &InvestigationResult) -> Result<Self, Self::Error> {
        let research_records = result
            .research_records()
            .iter()
            .map(|record| {
                let bytes = encode_record(record)
                    .map_err(InvestigationCodecError::InvalidResearchRecord)?;
                serde_json::from_slice(&bytes).map_err(|_| InvestigationCodecError::Encoding)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { research_records })
    }
}

impl From<&ResearchStopReason> for WireStopReason {
    fn from(reason: &ResearchStopReason) -> Self {
        match reason {
            ResearchStopReason::OperatorStopped => Self::OperatorStopped,
            ResearchStopReason::BudgetExhausted => Self::BudgetExhausted,
            ResearchStopReason::Blocked(reason) => Self::Blocked {
                reason: reason.as_str().to_owned(),
            },
        }
    }
}

impl TryFrom<WireEvent> for InvestigationEvent {
    type Error = InvestigationCodecError;

    fn try_from(event: WireEvent) -> Result<Self, Self::Error> {
        match event {
            WireEvent::RequestRecorded { request } => Ok(Self::RequestRecorded(
                ResearchRequest::new(request.question)?,
            )),
            WireEvent::PlanRecorded { plan } => {
                let tasks = plan
                    .tasks
                    .into_iter()
                    .map(InvestigationTask::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self::PlanRecorded(ResearchPlan::new(tasks)?))
            }
            WireEvent::TaskStarted { task_id } => Ok(Self::TaskStarted {
                task_id: InvestigationTaskId::from_str(&task_id)?,
            }),
            WireEvent::TaskCompleted { task_id, result } => Ok(Self::TaskCompleted {
                task_id: InvestigationTaskId::from_str(&task_id)?,
                result: InvestigationResult::try_from(result)?,
            }),
            WireEvent::TaskFailed { task_id, failure } => Ok(Self::TaskFailed {
                task_id: InvestigationTaskId::from_str(&task_id)?,
                failure: InvestigationFailure::new(failure)?,
            }),
            WireEvent::FollowUpRecorded { task } => {
                Ok(Self::FollowUpRecorded(InvestigationTask::try_from(task)?))
            }
            WireEvent::ResearchStopped { reason } => {
                Ok(Self::ResearchStopped(ResearchStopReason::try_from(reason)?))
            }
        }
    }
}

impl TryFrom<WireTask> for InvestigationTask {
    type Error = InvestigationCodecError;

    fn try_from(task: WireTask) -> Result<Self, Self::Error> {
        let id = InvestigationTaskId::from_str(&task.id)?;
        match task.origin {
            WireTaskOrigin::Initial => Ok(Self::initial(id, task.objective)?),
            WireTaskOrigin::FollowUp {
                parent_task_id,
                gap,
            } => Ok(Self::follow_up(
                id,
                InvestigationTaskId::from_str(&parent_task_id)?,
                task.objective,
                ResearchGap::new(gap)?,
            )?),
        }
    }
}

impl TryFrom<WireResult> for InvestigationResult {
    type Error = InvestigationCodecError;

    fn try_from(result: WireResult) -> Result<Self, Self::Error> {
        let research_records = result
            .research_records
            .into_iter()
            .map(|record| {
                let bytes = serde_json::to_vec(&record)
                    .map_err(|_| InvestigationCodecError::MalformedJson)?;
                decode_record(&bytes).map_err(InvestigationCodecError::InvalidResearchRecord)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self::new(research_records))
    }
}

impl TryFrom<WireStopReason> for ResearchStopReason {
    type Error = InvestigationCodecError;

    fn try_from(reason: WireStopReason) -> Result<Self, Self::Error> {
        match reason {
            WireStopReason::OperatorStopped => Ok(Self::OperatorStopped),
            WireStopReason::BudgetExhausted => Ok(Self::BudgetExhausted),
            WireStopReason::Blocked { reason } => Ok(Self::Blocked(BlockedReason::new(reason)?)),
        }
    }
}
