mod codec;
mod entity;
mod identity;
mod investigation_codec;
mod investigation_record;
mod investigation_state;
mod planning;
mod record;
mod research_control;
mod research_control_codec;
mod research_control_record;
mod research_control_state;
mod state;
mod synthesis;
mod verification;
mod verification_codec;
mod verification_record;
mod verification_state;

pub use codec::{CodecError, decode_record, encode_record};
pub use entity::{Claim, ContentDigest, Evidence, MediaType, RetrievedAt, Source, ValidationError};
pub use identity::{
    ClaimId, EvidenceId, IdentityError, InvestigationTaskId, ResearchGapId, SourceId,
    VerificationId,
};
pub use investigation_codec::{
    InvestigationCodecError, decode_investigation_record, encode_investigation_record,
};
pub use investigation_record::{
    INVESTIGATION_SCHEMA_VERSION, InvestigationEvent, InvestigationRecord,
};
pub use investigation_state::{
    InvestigationState, InvestigationStatus, InvestigationTaskState, InvestigationTaskStatus,
    InvestigationTransitionError,
};
pub use planning::{
    BlockedReason, InvestigationFailure, InvestigationResult, InvestigationTask,
    PlanningValidationError, ResearchGap, ResearchPlan, ResearchRequest, ResearchStopReason,
    TaskOrigin,
};
pub use record::{RESEARCH_SCHEMA_VERSION, ResearchEvent, ResearchRecord};
pub use research_control::{
    IdentifiedResearchGap, ResearchControlLimits, ResearchControlValidationError, ResearchFailure,
    ResearchGapCause,
};
pub use research_control_codec::{
    ResearchControlCodecError, decode_research_control_record, encode_research_control_record,
};
pub use research_control_record::{
    RESEARCH_CONTROL_SCHEMA_VERSION, ResearchControlEvent, ResearchControlRecord,
};
pub use research_control_state::{
    ResearchControlState, ResearchControlStatus, ResearchControlTransitionError, ResearchGapState,
    ResearchGapStatus,
};
pub use state::{ResearchState, TransitionError};
pub use synthesis::{
    ClaimPresentation, GroundedAssertion, GroundedReport, GroundedReportSection, GroundingCitation,
    SynthesisAssertionDraft, SynthesisBasis, SynthesisClaimBasis, SynthesisDraft,
    SynthesisReportScope, SynthesisSectionDraft, SynthesisValidationError,
};
pub use verification::{
    EvidenceAssessment, EvidenceRelation, EvidenceSufficiency, VerificationAssessment,
    VerificationValidationError,
};
pub use verification_codec::{
    VerificationCodecError, decode_verification_record, encode_verification_record,
};
pub use verification_record::{VERIFICATION_SCHEMA_VERSION, VerificationRecord};
pub use verification_state::{VerificationState, VerificationTransitionError};
