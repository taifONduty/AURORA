use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{
    ClaimId, EvidenceAssessment, EvidenceId, EvidenceRelation, EvidenceSufficiency, IdentityError,
    VERIFICATION_SCHEMA_VERSION, VerificationAssessment, VerificationId, VerificationRecord,
    VerificationValidationError,
};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum VerificationCodecError {
    #[error("verification record could not be encoded")]
    Encoding,
    #[error("verification record JSON is malformed")]
    MalformedJson,
    #[error("unsupported verification schema version {0}")]
    UnsupportedSchema(u32),
    #[error("verification record identity is invalid: {0}")]
    InvalidIdentity(#[from] IdentityError),
    #[error("verification record is invalid: {0}")]
    InvalidRecord(#[from] VerificationValidationError),
}

#[derive(Serialize, Deserialize)]
struct WireRecord {
    schema_version: u32,
    sequence: u64,
    assessment: WireAssessment,
}

#[derive(Serialize, Deserialize)]
struct WireAssessment {
    id: String,
    claim_id: String,
    evidence_relations: Vec<WireEvidenceAssessment>,
    sufficiency: WireSufficiency,
}

#[derive(Serialize, Deserialize)]
struct WireEvidenceAssessment {
    evidence_id: String,
    relation: WireRelation,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireRelation {
    Supports,
    Contradicts,
    Unclear,
    Irrelevant,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireSufficiency {
    Sufficient,
    Insufficient,
    Indeterminate,
}

pub fn encode_verification_record(
    record: &VerificationRecord,
) -> Result<Vec<u8>, VerificationCodecError> {
    serde_json::to_vec(&WireRecord::from(record)).map_err(|_| VerificationCodecError::Encoding)
}

pub fn decode_verification_record(
    bytes: &[u8],
) -> Result<VerificationRecord, VerificationCodecError> {
    let wire: WireRecord =
        serde_json::from_slice(bytes).map_err(|_| VerificationCodecError::MalformedJson)?;
    if wire.schema_version != VERIFICATION_SCHEMA_VERSION {
        return Err(VerificationCodecError::UnsupportedSchema(
            wire.schema_version,
        ));
    }
    VerificationRecord::new(
        wire.sequence,
        VerificationAssessment::try_from(wire.assessment)?,
    )
    .map_err(VerificationCodecError::InvalidRecord)
}

impl From<&VerificationRecord> for WireRecord {
    fn from(record: &VerificationRecord) -> Self {
        Self {
            schema_version: record.schema_version(),
            sequence: record.sequence(),
            assessment: WireAssessment::from(record.assessment()),
        }
    }
}

impl From<&VerificationAssessment> for WireAssessment {
    fn from(assessment: &VerificationAssessment) -> Self {
        Self {
            id: assessment.id().to_string(),
            claim_id: assessment.claim_id().to_string(),
            evidence_relations: assessment
                .evidence_relations()
                .map(|(evidence_id, relation)| WireEvidenceAssessment {
                    evidence_id: evidence_id.to_string(),
                    relation: WireRelation::from(relation),
                })
                .collect(),
            sufficiency: WireSufficiency::from(assessment.sufficiency()),
        }
    }
}

impl From<EvidenceRelation> for WireRelation {
    fn from(relation: EvidenceRelation) -> Self {
        match relation {
            EvidenceRelation::Supports => Self::Supports,
            EvidenceRelation::Contradicts => Self::Contradicts,
            EvidenceRelation::Unclear => Self::Unclear,
            EvidenceRelation::Irrelevant => Self::Irrelevant,
        }
    }
}

impl From<EvidenceSufficiency> for WireSufficiency {
    fn from(sufficiency: EvidenceSufficiency) -> Self {
        match sufficiency {
            EvidenceSufficiency::Sufficient => Self::Sufficient,
            EvidenceSufficiency::Insufficient => Self::Insufficient,
            EvidenceSufficiency::Indeterminate => Self::Indeterminate,
        }
    }
}

impl TryFrom<WireAssessment> for VerificationAssessment {
    type Error = VerificationCodecError;

    fn try_from(assessment: WireAssessment) -> Result<Self, Self::Error> {
        let evidence = assessment
            .evidence_relations
            .into_iter()
            .map(|item| {
                Ok(EvidenceAssessment::new(
                    EvidenceId::from_str(&item.evidence_id)?,
                    EvidenceRelation::from(item.relation),
                ))
            })
            .collect::<Result<_, VerificationCodecError>>()?;
        Self::new(
            VerificationId::from_str(&assessment.id)?,
            ClaimId::from_str(&assessment.claim_id)?,
            evidence,
            EvidenceSufficiency::from(assessment.sufficiency),
        )
        .map_err(VerificationCodecError::InvalidRecord)
    }
}

impl From<WireRelation> for EvidenceRelation {
    fn from(relation: WireRelation) -> Self {
        match relation {
            WireRelation::Supports => Self::Supports,
            WireRelation::Contradicts => Self::Contradicts,
            WireRelation::Unclear => Self::Unclear,
            WireRelation::Irrelevant => Self::Irrelevant,
        }
    }
}

impl From<WireSufficiency> for EvidenceSufficiency {
    fn from(sufficiency: WireSufficiency) -> Self {
        match sufficiency {
            WireSufficiency::Sufficient => Self::Sufficient,
            WireSufficiency::Insufficient => Self::Insufficient,
            WireSufficiency::Indeterminate => Self::Indeterminate,
        }
    }
}
