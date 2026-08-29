use crate::{
    IdentifiedResearchGap, InvestigationRecord, ResearchControlLimits,
    ResearchControlValidationError, ResearchFailure, ResearchGapId, VerificationId,
    VerificationRecord,
};

pub const RESEARCH_CONTROL_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResearchControlEvent {
    LimitsRecorded(ResearchControlLimits),
    InvestigationAdvanced(InvestigationRecord),
    VerificationRecorded(VerificationRecord),
    GapIdentified(IdentifiedResearchGap),
    GapFollowUpRecorded {
        gap_id: ResearchGapId,
        investigation_record: InvestigationRecord,
    },
    GapResolved {
        gap_id: ResearchGapId,
        verification_id: VerificationId,
    },
    ResearchCompleted,
    ResearchFailed(ResearchFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchControlRecord {
    schema_version: u32,
    sequence: u64,
    event: ResearchControlEvent,
}

impl ResearchControlRecord {
    pub fn new(
        sequence: u64,
        event: ResearchControlEvent,
    ) -> Result<Self, ResearchControlValidationError> {
        if sequence == 0 {
            return Err(ResearchControlValidationError::ZeroResearchControlSequence);
        }
        Ok(Self {
            schema_version: RESEARCH_CONTROL_SCHEMA_VERSION,
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

    pub const fn event(&self) -> &ResearchControlEvent {
        &self.event
    }

    pub(crate) fn into_event(self) -> ResearchControlEvent {
        self.event
    }
}
