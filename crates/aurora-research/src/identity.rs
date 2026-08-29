use std::{fmt, str::FromStr};

use uuid::{Uuid, Variant, Version};

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error("identifier is not a UUID")]
    InvalidUuid,
    #[error("identifier is not UUID version 4")]
    NotVersion4,
    #[error("identifier does not use the RFC 4122 variant")]
    NotRfc4122,
}

fn parse_uuid_v4(value: &str) -> Result<Uuid, IdentityError> {
    let uuid = Uuid::parse_str(value).map_err(|_| IdentityError::InvalidUuid)?;
    if uuid.get_version() != Some(Version::Random) {
        return Err(IdentityError::NotVersion4);
    }
    if uuid.get_variant() != Variant::RFC4122 {
        return Err(IdentityError::NotRfc4122);
    }
    Ok(uuid)
}

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Uuid);

        impl $name {
            pub fn generate() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl FromStr for $name {
            type Err = IdentityError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                parse_uuid_v4(value).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.hyphenated().fmt(formatter)
            }
        }
    };
}

define_id!(SourceId);
define_id!(EvidenceId);
define_id!(ClaimId);
define_id!(InvestigationTaskId);
define_id!(VerificationId);
