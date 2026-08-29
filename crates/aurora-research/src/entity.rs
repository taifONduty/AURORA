use std::collections::BTreeSet;

use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

use crate::{ClaimId, EvidenceId, SourceId};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("source locator is empty")]
    EmptySourceLocator,
    #[error("source title is empty")]
    EmptySourceTitle,
    #[error("source retrieval time is not an RFC 3339 UTC timestamp")]
    InvalidRetrievedAt,
    #[error("source media type is invalid")]
    InvalidMediaType,
    #[error("evidence excerpt is empty")]
    EmptyEvidenceExcerpt,
    #[error("claim statement is empty")]
    EmptyClaimStatement,
    #[error("claim has no evidence")]
    ClaimHasNoEvidence,
    #[error("claim repeats evidence identifier {0}")]
    DuplicateClaimEvidence(EvidenceId),
    #[error("content digest is invalid")]
    InvalidContentDigest,
    #[error("research record sequence is zero")]
    ZeroSequence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentDigest([u8; 32]);

impl ContentDigest {
    pub const fn sha256(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_sha256(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetrievedAt(String);

impl RetrievedAt {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.as_bytes().get(10) != Some(&b'T') {
            return Err(ValidationError::InvalidRetrievedAt);
        }
        let parsed = OffsetDateTime::parse(&value, &Rfc3339)
            .map_err(|_| ValidationError::InvalidRetrievedAt)?;
        if parsed.offset() != UtcOffset::UTC {
            return Err(ValidationError::InvalidRetrievedAt);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaType(String);

impl MediaType {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let Some((top_level, subtype)) = value.split_once('/') else {
            return Err(ValidationError::InvalidMediaType);
        };
        if top_level == "*"
            || subtype == "*"
            || top_level.is_empty()
            || subtype.is_empty()
            || subtype.contains('/')
            || !top_level.bytes().all(is_media_type_token)
            || !subtype.bytes().all(is_media_type_token)
        {
            return Err(ValidationError::InvalidMediaType);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_media_type_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    id: SourceId,
    content_digest: ContentDigest,
    locator: String,
    title: Option<String>,
    retrieved_at: RetrievedAt,
    media_type: MediaType,
}

impl Source {
    pub fn new(
        id: SourceId,
        content_digest: ContentDigest,
        locator: String,
        title: Option<String>,
        retrieved_at: RetrievedAt,
        media_type: MediaType,
    ) -> Result<Self, ValidationError> {
        if locator.trim().is_empty() {
            return Err(ValidationError::EmptySourceLocator);
        }
        if title.as_ref().is_some_and(|value| value.trim().is_empty()) {
            return Err(ValidationError::EmptySourceTitle);
        }
        Ok(Self {
            id,
            content_digest,
            locator,
            title,
            retrieved_at,
            media_type,
        })
    }

    pub const fn id(&self) -> &SourceId {
        &self.id
    }

    pub const fn content_digest(&self) -> &ContentDigest {
        &self.content_digest
    }

    pub fn locator(&self) -> &str {
        &self.locator
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub const fn retrieved_at(&self) -> &RetrievedAt {
        &self.retrieved_at
    }

    pub const fn media_type(&self) -> &MediaType {
        &self.media_type
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    id: EvidenceId,
    source_id: SourceId,
    excerpt: String,
}

impl Evidence {
    pub fn new(
        id: EvidenceId,
        source_id: SourceId,
        excerpt: String,
    ) -> Result<Self, ValidationError> {
        if excerpt.trim().is_empty() {
            return Err(ValidationError::EmptyEvidenceExcerpt);
        }
        Ok(Self {
            id,
            source_id,
            excerpt,
        })
    }

    pub const fn id(&self) -> &EvidenceId {
        &self.id
    }

    pub const fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    pub fn excerpt(&self) -> &str {
        &self.excerpt
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    id: ClaimId,
    statement: String,
    evidence_ids: BTreeSet<EvidenceId>,
}

impl Claim {
    pub fn new(
        id: ClaimId,
        statement: String,
        evidence_ids: Vec<EvidenceId>,
    ) -> Result<Self, ValidationError> {
        if statement.trim().is_empty() {
            return Err(ValidationError::EmptyClaimStatement);
        }
        if evidence_ids.is_empty() {
            return Err(ValidationError::ClaimHasNoEvidence);
        }
        let mut distinct = BTreeSet::new();
        for evidence_id in evidence_ids {
            if !distinct.insert(evidence_id) {
                return Err(ValidationError::DuplicateClaimEvidence(evidence_id));
            }
        }
        Ok(Self {
            id,
            statement,
            evidence_ids: distinct,
        })
    }

    pub const fn id(&self) -> &ClaimId {
        &self.id
    }

    pub fn statement(&self) -> &str {
        &self.statement
    }

    pub const fn evidence_ids(&self) -> &BTreeSet<EvidenceId> {
        &self.evidence_ids
    }
}
