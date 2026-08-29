use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    IdentifiedResearchGap, IdentityError, InvestigationCodecError, PlanningValidationError,
    RESEARCH_CONTROL_SCHEMA_VERSION, ResearchControlEvent, ResearchControlLimits,
    ResearchControlRecord, ResearchControlValidationError, ResearchFailure, ResearchGap,
    ResearchGapCause, ResearchGapId, VerificationCodecError, VerificationId,
    decode_investigation_record, decode_verification_record, encode_investigation_record,
    encode_verification_record,
};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResearchControlCodecError {
    #[error("research control record could not be encoded")]
    Encoding,
    #[error("research control record JSON is malformed")]
    MalformedJson,
    #[error("unsupported research control schema version {0}")]
    UnsupportedSchema(u32),
    #[error("research control identity is invalid: {0}")]
    InvalidIdentity(#[from] IdentityError),
    #[error("research control record is invalid: {0}")]
    InvalidRecord(#[from] ResearchControlValidationError),
    #[error("research gap is invalid: {0}")]
    InvalidGap(PlanningValidationError),
    #[error("research control contains an invalid investigation record: {0}")]
    InvalidInvestigationRecord(InvestigationCodecError),
    #[error("research control contains an invalid verification record: {0}")]
    InvalidVerificationRecord(VerificationCodecError),
}

#[derive(Serialize, Deserialize)]
struct WireRecord {
    schema_version: u32,
    sequence: u64,
    event: WireEvent,
}

#[derive(Deserialize)]
struct WireHeader {
    schema_version: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireEvent {
    LimitsRecorded {
        limits: WireLimits,
    },
    InvestigationAdvanced {
        record: Value,
    },
    VerificationRecorded {
        record: Value,
    },
    GapIdentified {
        gap: WireGap,
    },
    GapFollowUpRecorded {
        gap_id: String,
        record: Value,
    },
    GapResolved {
        gap_id: String,
        verification_id: String,
    },
    ResearchCompleted,
    ResearchFailed {
        failure: String,
    },
}

#[derive(Serialize, Deserialize)]
struct WireLimits {
    max_follow_up_tasks: u32,
}

#[derive(Serialize, Deserialize)]
struct WireGap {
    id: String,
    cause: WireCause,
    description: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireCause {
    Verification { verification_id: String },
    InvestigationFailure { task_id: String },
}

pub fn encode_research_control_record(
    record: &ResearchControlRecord,
) -> Result<Vec<u8>, ResearchControlCodecError> {
    let wire = WireRecord::try_from(record)?;
    serde_json::to_vec(&wire).map_err(|_| ResearchControlCodecError::Encoding)
}

pub fn decode_research_control_record(
    bytes: &[u8],
) -> Result<ResearchControlRecord, ResearchControlCodecError> {
    let header: WireHeader =
        serde_json::from_slice(bytes).map_err(|_| ResearchControlCodecError::MalformedJson)?;
    if header.schema_version != RESEARCH_CONTROL_SCHEMA_VERSION {
        return Err(ResearchControlCodecError::UnsupportedSchema(
            header.schema_version,
        ));
    }
    let wire: WireRecord =
        serde_json::from_slice(bytes).map_err(|_| ResearchControlCodecError::MalformedJson)?;
    ResearchControlRecord::new(wire.sequence, ResearchControlEvent::try_from(wire.event)?)
        .map_err(ResearchControlCodecError::InvalidRecord)
}

impl TryFrom<&ResearchControlRecord> for WireRecord {
    type Error = ResearchControlCodecError;

    fn try_from(record: &ResearchControlRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            schema_version: record.schema_version(),
            sequence: record.sequence(),
            event: WireEvent::try_from(record.event())?,
        })
    }
}

impl TryFrom<&ResearchControlEvent> for WireEvent {
    type Error = ResearchControlCodecError;

    fn try_from(event: &ResearchControlEvent) -> Result<Self, Self::Error> {
        match event {
            ResearchControlEvent::LimitsRecorded(limits) => Ok(Self::LimitsRecorded {
                limits: WireLimits {
                    max_follow_up_tasks: limits.max_follow_up_tasks(),
                },
            }),
            ResearchControlEvent::InvestigationAdvanced(record) => {
                Ok(Self::InvestigationAdvanced {
                    record: encode_investigation_value(record)?,
                })
            }
            ResearchControlEvent::VerificationRecorded(record) => Ok(Self::VerificationRecorded {
                record: encode_verification_value(record)?,
            }),
            ResearchControlEvent::GapIdentified(gap) => Ok(Self::GapIdentified {
                gap: WireGap::from(gap),
            }),
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id,
                investigation_record,
            } => Ok(Self::GapFollowUpRecorded {
                gap_id: gap_id.to_string(),
                record: encode_investigation_value(investigation_record)?,
            }),
            ResearchControlEvent::GapResolved {
                gap_id,
                verification_id,
            } => Ok(Self::GapResolved {
                gap_id: gap_id.to_string(),
                verification_id: verification_id.to_string(),
            }),
            ResearchControlEvent::ResearchCompleted => Ok(Self::ResearchCompleted),
            ResearchControlEvent::ResearchFailed(failure) => Ok(Self::ResearchFailed {
                failure: failure.as_str().to_owned(),
            }),
        }
    }
}

impl From<&IdentifiedResearchGap> for WireGap {
    fn from(gap: &IdentifiedResearchGap) -> Self {
        Self {
            id: gap.id().to_string(),
            cause: WireCause::from(gap.cause()),
            description: gap.description().as_str().to_owned(),
        }
    }
}

impl From<&ResearchGapCause> for WireCause {
    fn from(cause: &ResearchGapCause) -> Self {
        match cause {
            ResearchGapCause::Verification(id) => Self::Verification {
                verification_id: id.to_string(),
            },
            ResearchGapCause::InvestigationFailure(id) => Self::InvestigationFailure {
                task_id: id.to_string(),
            },
        }
    }
}

impl TryFrom<WireEvent> for ResearchControlEvent {
    type Error = ResearchControlCodecError;

    fn try_from(event: WireEvent) -> Result<Self, Self::Error> {
        match event {
            WireEvent::LimitsRecorded { limits } => Ok(Self::LimitsRecorded(
                ResearchControlLimits::new(limits.max_follow_up_tasks),
            )),
            WireEvent::InvestigationAdvanced { record } => Ok(Self::InvestigationAdvanced(
                decode_investigation_value(record)?,
            )),
            WireEvent::VerificationRecorded { record } => Ok(Self::VerificationRecorded(
                decode_verification_value(record)?,
            )),
            WireEvent::GapIdentified { gap } => {
                Ok(Self::GapIdentified(IdentifiedResearchGap::try_from(gap)?))
            }
            WireEvent::GapFollowUpRecorded { gap_id, record } => Ok(Self::GapFollowUpRecorded {
                gap_id: ResearchGapId::from_str(&gap_id)?,
                investigation_record: decode_investigation_value(record)?,
            }),
            WireEvent::GapResolved {
                gap_id,
                verification_id,
            } => Ok(Self::GapResolved {
                gap_id: ResearchGapId::from_str(&gap_id)?,
                verification_id: VerificationId::from_str(&verification_id)?,
            }),
            WireEvent::ResearchCompleted => Ok(Self::ResearchCompleted),
            WireEvent::ResearchFailed { failure } => {
                Ok(Self::ResearchFailed(ResearchFailure::new(failure)?))
            }
        }
    }
}

impl TryFrom<WireGap> for IdentifiedResearchGap {
    type Error = ResearchControlCodecError;

    fn try_from(gap: WireGap) -> Result<Self, Self::Error> {
        Ok(Self::new(
            ResearchGapId::from_str(&gap.id)?,
            ResearchGapCause::try_from(gap.cause)?,
            ResearchGap::new(gap.description).map_err(ResearchControlCodecError::InvalidGap)?,
        ))
    }
}

impl TryFrom<WireCause> for ResearchGapCause {
    type Error = ResearchControlCodecError;

    fn try_from(cause: WireCause) -> Result<Self, Self::Error> {
        match cause {
            WireCause::Verification { verification_id } => Ok(Self::Verification(
                VerificationId::from_str(&verification_id)?,
            )),
            WireCause::InvestigationFailure { task_id } => Ok(Self::InvestigationFailure(
                crate::InvestigationTaskId::from_str(&task_id)?,
            )),
        }
    }
}

fn encode_investigation_value(
    record: &crate::InvestigationRecord,
) -> Result<Value, ResearchControlCodecError> {
    let bytes = encode_investigation_record(record)
        .map_err(ResearchControlCodecError::InvalidInvestigationRecord)?;
    serde_json::from_slice(&bytes).map_err(|_| ResearchControlCodecError::Encoding)
}

fn decode_investigation_value(
    value: Value,
) -> Result<crate::InvestigationRecord, ResearchControlCodecError> {
    let bytes = serde_json::to_vec(&value).map_err(|_| ResearchControlCodecError::MalformedJson)?;
    decode_investigation_record(&bytes)
        .map_err(ResearchControlCodecError::InvalidInvestigationRecord)
}

fn encode_verification_value(
    record: &crate::VerificationRecord,
) -> Result<Value, ResearchControlCodecError> {
    let bytes = encode_verification_record(record)
        .map_err(ResearchControlCodecError::InvalidVerificationRecord)?;
    serde_json::from_slice(&bytes).map_err(|_| ResearchControlCodecError::Encoding)
}

fn decode_verification_value(
    value: Value,
) -> Result<crate::VerificationRecord, ResearchControlCodecError> {
    let bytes = serde_json::to_vec(&value).map_err(|_| ResearchControlCodecError::MalformedJson)?;
    decode_verification_record(&bytes).map_err(ResearchControlCodecError::InvalidVerificationRecord)
}
