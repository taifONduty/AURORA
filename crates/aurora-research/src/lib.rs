mod codec;
mod entity;
mod identity;
mod investigation_codec;
mod investigation_record;
mod investigation_state;
mod planning;
mod record;
mod state;

pub use codec::{CodecError, decode_record, encode_record};
pub use entity::{Claim, ContentDigest, Evidence, MediaType, RetrievedAt, Source, ValidationError};
pub use identity::{ClaimId, EvidenceId, IdentityError, InvestigationTaskId, SourceId};
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
pub use state::{ResearchState, TransitionError};
