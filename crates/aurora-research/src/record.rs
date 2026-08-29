use crate::{Claim, Evidence, Source, ValidationError};

pub const RESEARCH_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResearchEvent {
    SourceRecorded(Source),
    EvidenceRecorded(Evidence),
    ClaimProposed(Claim),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchRecord {
    schema_version: u32,
    sequence: u64,
    event: ResearchEvent,
}

impl ResearchRecord {
    pub fn new(sequence: u64, event: ResearchEvent) -> Result<Self, ValidationError> {
        if sequence == 0 {
            return Err(ValidationError::ZeroSequence);
        }
        Ok(Self {
            schema_version: RESEARCH_SCHEMA_VERSION,
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

    pub const fn event(&self) -> &ResearchEvent {
        &self.event
    }

    pub(crate) fn into_event(self) -> ResearchEvent {
        self.event
    }
}
