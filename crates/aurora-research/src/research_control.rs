use crate::{InvestigationTaskId, ResearchGap, ResearchGapId, VerificationId};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResearchControlValidationError {
    #[error("research failure is empty")]
    EmptyResearchFailure,
    #[error("research control record sequence is zero")]
    ZeroResearchControlSequence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResearchControlLimits {
    max_follow_up_tasks: u32,
}

impl ResearchControlLimits {
    pub const fn new(max_follow_up_tasks: u32) -> Self {
        Self {
            max_follow_up_tasks,
        }
    }

    pub const fn max_follow_up_tasks(&self) -> u32 {
        self.max_follow_up_tasks
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchFailure(String);

impl ResearchFailure {
    pub fn new(value: String) -> Result<Self, ResearchControlValidationError> {
        if value.trim().is_empty() {
            return Err(ResearchControlValidationError::EmptyResearchFailure);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResearchGapCause {
    Verification(VerificationId),
    InvestigationFailure(InvestigationTaskId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentifiedResearchGap {
    id: ResearchGapId,
    cause: ResearchGapCause,
    description: ResearchGap,
}

impl IdentifiedResearchGap {
    pub const fn new(id: ResearchGapId, cause: ResearchGapCause, description: ResearchGap) -> Self {
        Self {
            id,
            cause,
            description,
        }
    }

    pub const fn id(&self) -> &ResearchGapId {
        &self.id
    }

    pub const fn cause(&self) -> &ResearchGapCause {
        &self.cause
    }

    pub const fn description(&self) -> &ResearchGap {
        &self.description
    }
}
