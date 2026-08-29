mod codec;
mod entity;
mod identity;
mod record;
mod state;

pub use codec::{CodecError, decode_record, encode_record};
pub use entity::{Claim, ContentDigest, Evidence, MediaType, RetrievedAt, Source, ValidationError};
pub use identity::{ClaimId, EvidenceId, IdentityError, SourceId};
pub use record::{RESEARCH_SCHEMA_VERSION, ResearchEvent, ResearchRecord};
pub use state::{ResearchState, TransitionError};
