use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{
    Claim, ClaimId, ContentDigest, Evidence, EvidenceId, IdentityError, MediaType,
    RESEARCH_SCHEMA_VERSION, ResearchEvent, ResearchRecord, RetrievedAt, Source, SourceId,
    ValidationError,
};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    #[error("research record could not be encoded")]
    Encoding,
    #[error("research record JSON is malformed")]
    MalformedJson,
    #[error("unsupported research schema version {0}")]
    UnsupportedSchema(u32),
    #[error("research record identity is invalid: {0}")]
    InvalidIdentity(#[from] IdentityError),
    #[error("research record is invalid: {0}")]
    InvalidRecord(#[from] ValidationError),
}

#[derive(Serialize, Deserialize)]
struct WireRecord {
    schema_version: u32,
    sequence: u64,
    event: WireEvent,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireEvent {
    SourceRecorded { source: WireSource },
    EvidenceRecorded { evidence: WireEvidence },
    ClaimProposed { claim: WireClaim },
}

#[derive(Serialize, Deserialize)]
struct WireSource {
    id: String,
    content_digest: WireDigest,
    locator: String,
    title: Option<String>,
    retrieved_at: String,
    media_type: String,
}

#[derive(Serialize, Deserialize)]
struct WireDigest {
    algorithm: String,
    value: String,
}

#[derive(Serialize, Deserialize)]
struct WireEvidence {
    id: String,
    source_id: String,
    excerpt: String,
}

#[derive(Serialize, Deserialize)]
struct WireClaim {
    id: String,
    statement: String,
    evidence_ids: Vec<String>,
}

pub fn encode_record(record: &ResearchRecord) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(&WireRecord::from(record)).map_err(|_| CodecError::Encoding)
}

pub fn decode_record(bytes: &[u8]) -> Result<ResearchRecord, CodecError> {
    let wire: WireRecord = serde_json::from_slice(bytes).map_err(|_| CodecError::MalformedJson)?;
    if wire.schema_version != RESEARCH_SCHEMA_VERSION {
        return Err(CodecError::UnsupportedSchema(wire.schema_version));
    }
    let event = ResearchEvent::try_from(wire.event)?;
    ResearchRecord::new(wire.sequence, event).map_err(CodecError::InvalidRecord)
}

impl From<&ResearchRecord> for WireRecord {
    fn from(record: &ResearchRecord) -> Self {
        Self {
            schema_version: record.schema_version(),
            sequence: record.sequence(),
            event: WireEvent::from(record.event()),
        }
    }
}

impl From<&ResearchEvent> for WireEvent {
    fn from(event: &ResearchEvent) -> Self {
        match event {
            ResearchEvent::SourceRecorded(source) => Self::SourceRecorded {
                source: WireSource::from(source),
            },
            ResearchEvent::EvidenceRecorded(evidence) => Self::EvidenceRecorded {
                evidence: WireEvidence::from(evidence),
            },
            ResearchEvent::ClaimProposed(claim) => Self::ClaimProposed {
                claim: WireClaim::from(claim),
            },
        }
    }
}

impl From<&Source> for WireSource {
    fn from(source: &Source) -> Self {
        Self {
            id: source.id().to_string(),
            content_digest: WireDigest {
                algorithm: "sha256".to_owned(),
                value: encode_hex(source.content_digest().as_sha256()),
            },
            locator: source.locator().to_owned(),
            title: source.title().map(str::to_owned),
            retrieved_at: source.retrieved_at().as_str().to_owned(),
            media_type: source.media_type().as_str().to_owned(),
        }
    }
}

impl From<&Evidence> for WireEvidence {
    fn from(evidence: &Evidence) -> Self {
        Self {
            id: evidence.id().to_string(),
            source_id: evidence.source_id().to_string(),
            excerpt: evidence.excerpt().to_owned(),
        }
    }
}

impl From<&Claim> for WireClaim {
    fn from(claim: &Claim) -> Self {
        Self {
            id: claim.id().to_string(),
            statement: claim.statement().to_owned(),
            evidence_ids: claim
                .evidence_ids()
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }
}

impl TryFrom<WireEvent> for ResearchEvent {
    type Error = CodecError;

    fn try_from(event: WireEvent) -> Result<Self, Self::Error> {
        match event {
            WireEvent::SourceRecorded { source } => {
                Ok(Self::SourceRecorded(Source::try_from(source)?))
            }
            WireEvent::EvidenceRecorded { evidence } => {
                Ok(Self::EvidenceRecorded(Evidence::try_from(evidence)?))
            }
            WireEvent::ClaimProposed { claim } => Ok(Self::ClaimProposed(Claim::try_from(claim)?)),
        }
    }
}

impl TryFrom<WireSource> for Source {
    type Error = CodecError;

    fn try_from(source: WireSource) -> Result<Self, Self::Error> {
        let digest = decode_digest(source.content_digest)?;
        let retrieved_at = RetrievedAt::new(source.retrieved_at)?;
        let media_type = MediaType::new(source.media_type)?;
        Source::new(
            SourceId::from_str(&source.id)?,
            digest,
            source.locator,
            source.title,
            retrieved_at,
            media_type,
        )
        .map_err(CodecError::InvalidRecord)
    }
}

impl TryFrom<WireEvidence> for Evidence {
    type Error = CodecError;

    fn try_from(evidence: WireEvidence) -> Result<Self, Self::Error> {
        Evidence::new(
            EvidenceId::from_str(&evidence.id)?,
            SourceId::from_str(&evidence.source_id)?,
            evidence.excerpt,
        )
        .map_err(CodecError::InvalidRecord)
    }
}

impl TryFrom<WireClaim> for Claim {
    type Error = CodecError;

    fn try_from(claim: WireClaim) -> Result<Self, Self::Error> {
        let evidence_ids = claim
            .evidence_ids
            .iter()
            .map(|id| EvidenceId::from_str(id))
            .collect::<Result<Vec<_>, _>>()?;
        Claim::new(ClaimId::from_str(&claim.id)?, claim.statement, evidence_ids)
            .map_err(CodecError::InvalidRecord)
    }
}

fn encode_hex(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_digest(digest: WireDigest) -> Result<ContentDigest, CodecError> {
    if digest.algorithm != "sha256" || digest.value.len() != 64 {
        return Err(CodecError::InvalidRecord(
            ValidationError::InvalidContentDigest,
        ));
    }
    let mut bytes = [0_u8; 32];
    for (index, pair) in digest
        .value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .enumerate()
    {
        let high = decode_hex_nibble(pair[0])?;
        let low = decode_hex_nibble(pair[1])?;
        bytes[index] = (high << 4) | low;
    }
    Ok(ContentDigest::sha256(bytes))
}

fn decode_hex_nibble(byte: u8) -> Result<u8, CodecError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(CodecError::InvalidRecord(
            ValidationError::InvalidContentDigest,
        )),
    }
}
