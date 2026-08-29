use std::collections::BTreeMap;

use crate::{
    Claim, ClaimId, Evidence, EvidenceId, ResearchEvent, ResearchRecord, Source, SourceId,
};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransitionError {
    #[error("research sequence is exhausted")]
    SequenceExhausted,
    #[error("expected research record sequence {expected}, found {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("source identifier {0} is already present")]
    DuplicateSource(SourceId),
    #[error("evidence identifier {0} is already present")]
    DuplicateEvidence(EvidenceId),
    #[error("claim identifier {0} is already present")]
    DuplicateClaim(ClaimId),
    #[error("source identifier {0} is not present")]
    UnknownSource(SourceId),
    #[error("evidence identifier {0} is not present")]
    UnknownEvidence(EvidenceId),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResearchState {
    last_sequence: u64,
    sources: BTreeMap<SourceId, Source>,
    evidence: BTreeMap<EvidenceId, Evidence>,
    claims: BTreeMap<ClaimId, Claim>,
}

impl ResearchState {
    pub fn reconstruct<I>(records: I) -> Result<Self, TransitionError>
    where
        I: IntoIterator<Item = ResearchRecord>,
    {
        let mut state = Self::default();
        for record in records {
            state.apply(record)?;
        }
        Ok(state)
    }

    pub fn apply(&mut self, record: ResearchRecord) -> Result<(), TransitionError> {
        let expected = self
            .last_sequence
            .checked_add(1)
            .ok_or(TransitionError::SequenceExhausted)?;
        if record.sequence() != expected {
            return Err(TransitionError::Sequence {
                expected,
                actual: record.sequence(),
            });
        }

        match record.event() {
            ResearchEvent::SourceRecorded(source) => {
                if self.sources.contains_key(source.id()) {
                    return Err(TransitionError::DuplicateSource(*source.id()));
                }
            }
            ResearchEvent::EvidenceRecorded(evidence) => {
                if self.evidence.contains_key(evidence.id()) {
                    return Err(TransitionError::DuplicateEvidence(*evidence.id()));
                }
                if !self.sources.contains_key(evidence.source_id()) {
                    return Err(TransitionError::UnknownSource(*evidence.source_id()));
                }
            }
            ResearchEvent::ClaimProposed(claim) => {
                if self.claims.contains_key(claim.id()) {
                    return Err(TransitionError::DuplicateClaim(*claim.id()));
                }
                if let Some(missing) = claim
                    .evidence_ids()
                    .iter()
                    .find(|id| !self.evidence.contains_key(id))
                {
                    return Err(TransitionError::UnknownEvidence(*missing));
                }
            }
        }

        let sequence = record.sequence();
        match record.into_event() {
            ResearchEvent::SourceRecorded(source) => {
                self.sources.insert(*source.id(), source);
            }
            ResearchEvent::EvidenceRecorded(evidence) => {
                self.evidence.insert(*evidence.id(), evidence);
            }
            ResearchEvent::ClaimProposed(claim) => {
                self.claims.insert(*claim.id(), claim);
            }
        }
        self.last_sequence = sequence;
        Ok(())
    }

    pub const fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    pub fn source(&self, id: &SourceId) -> Option<&Source> {
        self.sources.get(id)
    }

    pub fn evidence(&self, id: &EvidenceId) -> Option<&Evidence> {
        self.evidence.get(id)
    }

    pub fn claim(&self, id: &ClaimId) -> Option<&Claim> {
        self.claims.get(id)
    }

    pub fn sources(&self) -> impl Iterator<Item = &Source> {
        self.sources.values()
    }

    pub fn evidence_items(&self) -> impl Iterator<Item = &Evidence> {
        self.evidence.values()
    }

    pub fn claims(&self) -> impl Iterator<Item = &Claim> {
        self.claims.values()
    }

    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    pub fn evidence_count(&self) -> usize {
        self.evidence.len()
    }

    pub fn claim_count(&self) -> usize {
        self.claims.len()
    }
}
