use std::collections::BTreeMap;

use crate::{ClaimId, EvidenceId, VerificationId};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum VerificationValidationError {
    #[error("verification assessment has no evidence")]
    NoAssessedEvidence,
    #[error("verification assessment repeats evidence identifier {0}")]
    DuplicateAssessedEvidence(EvidenceId),
    #[error("verification record sequence is zero")]
    ZeroVerificationSequence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceRelation {
    Supports,
    Contradicts,
    Unclear,
    Irrelevant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceSufficiency {
    Sufficient,
    Insufficient,
    Indeterminate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvidenceAssessment {
    evidence_id: EvidenceId,
    relation: EvidenceRelation,
}

impl EvidenceAssessment {
    pub const fn new(evidence_id: EvidenceId, relation: EvidenceRelation) -> Self {
        Self {
            evidence_id,
            relation,
        }
    }

    pub const fn evidence_id(&self) -> &EvidenceId {
        &self.evidence_id
    }

    pub const fn relation(&self) -> EvidenceRelation {
        self.relation
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationAssessment {
    id: VerificationId,
    claim_id: ClaimId,
    evidence_relations: BTreeMap<EvidenceId, EvidenceRelation>,
    sufficiency: EvidenceSufficiency,
}

impl VerificationAssessment {
    pub fn new(
        id: VerificationId,
        claim_id: ClaimId,
        evidence: Vec<EvidenceAssessment>,
        sufficiency: EvidenceSufficiency,
    ) -> Result<Self, VerificationValidationError> {
        if evidence.is_empty() {
            return Err(VerificationValidationError::NoAssessedEvidence);
        }
        let mut evidence_relations = BTreeMap::new();
        for assessment in evidence {
            if evidence_relations
                .insert(*assessment.evidence_id(), assessment.relation())
                .is_some()
            {
                return Err(VerificationValidationError::DuplicateAssessedEvidence(
                    *assessment.evidence_id(),
                ));
            }
        }
        Ok(Self {
            id,
            claim_id,
            evidence_relations,
            sufficiency,
        })
    }

    pub const fn id(&self) -> &VerificationId {
        &self.id
    }

    pub const fn claim_id(&self) -> &ClaimId {
        &self.claim_id
    }

    pub fn relation(&self, evidence_id: &EvidenceId) -> Option<EvidenceRelation> {
        self.evidence_relations.get(evidence_id).copied()
    }

    pub fn evidence_relations(&self) -> impl Iterator<Item = (&EvidenceId, EvidenceRelation)> {
        self.evidence_relations
            .iter()
            .map(|(id, relation)| (id, *relation))
    }

    pub const fn sufficiency(&self) -> EvidenceSufficiency {
        self.sufficiency
    }
}
