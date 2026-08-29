use std::collections::BTreeMap;

use crate::{
    ClaimId, EvidenceId, ResearchState, VerificationAssessment, VerificationId, VerificationRecord,
};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum VerificationTransitionError {
    #[error("verification sequence is exhausted")]
    SequenceExhausted,
    #[error("expected verification record sequence {expected}, found {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("verification identifier {0} is already present")]
    DuplicateVerification(VerificationId),
    #[error("claim identifier {0} is not present")]
    UnknownClaim(ClaimId),
    #[error("evidence identifier {0} is not present")]
    UnknownEvidence(EvidenceId),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VerificationState {
    last_sequence: u64,
    assessments: BTreeMap<VerificationId, VerificationAssessment>,
}

impl VerificationState {
    pub fn reconstruct<I>(
        research: &ResearchState,
        records: I,
    ) -> Result<Self, VerificationTransitionError>
    where
        I: IntoIterator<Item = VerificationRecord>,
    {
        let mut state = Self::default();
        for record in records {
            state.apply(research, record)?;
        }
        Ok(state)
    }

    pub fn apply(
        &mut self,
        research: &ResearchState,
        record: VerificationRecord,
    ) -> Result<(), VerificationTransitionError> {
        let expected = self
            .last_sequence
            .checked_add(1)
            .ok_or(VerificationTransitionError::SequenceExhausted)?;
        if record.sequence() != expected {
            return Err(VerificationTransitionError::Sequence {
                expected,
                actual: record.sequence(),
            });
        }
        let assessment = record.assessment();
        if self.assessments.contains_key(assessment.id()) {
            return Err(VerificationTransitionError::DuplicateVerification(
                *assessment.id(),
            ));
        }
        if research.claim(assessment.claim_id()).is_none() {
            return Err(VerificationTransitionError::UnknownClaim(
                *assessment.claim_id(),
            ));
        }
        if let Some((missing, _)) = assessment
            .evidence_relations()
            .find(|(id, _)| research.evidence(id).is_none())
        {
            return Err(VerificationTransitionError::UnknownEvidence(*missing));
        }

        let sequence = record.sequence();
        let assessment = record.into_assessment();
        self.assessments.insert(*assessment.id(), assessment);
        self.last_sequence = sequence;
        Ok(())
    }

    pub const fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    pub fn assessment(&self, id: &VerificationId) -> Option<&VerificationAssessment> {
        self.assessments.get(id)
    }

    pub fn assessments(&self) -> impl Iterator<Item = &VerificationAssessment> {
        self.assessments.values()
    }
}
