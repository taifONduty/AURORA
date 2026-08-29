use crate::{VerificationAssessment, VerificationValidationError};

pub const VERIFICATION_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationRecord {
    schema_version: u32,
    sequence: u64,
    assessment: VerificationAssessment,
}

impl VerificationRecord {
    pub fn new(
        sequence: u64,
        assessment: VerificationAssessment,
    ) -> Result<Self, VerificationValidationError> {
        if sequence == 0 {
            return Err(VerificationValidationError::ZeroVerificationSequence);
        }
        Ok(Self {
            schema_version: VERIFICATION_SCHEMA_VERSION,
            sequence,
            assessment,
        })
    }

    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    pub const fn assessment(&self) -> &VerificationAssessment {
        &self.assessment
    }

    pub(crate) fn into_assessment(self) -> VerificationAssessment {
        self.assessment
    }
}
