use std::str::FromStr;

use aurora_research::{
    Claim, ClaimId, ContentDigest, Evidence, EvidenceId, IdentityError, MediaType, RetrievedAt,
    Source, SourceId, ValidationError,
};

#[test]
fn identifiers_are_typed_canonical_uuid_v4_values() {
    let source = SourceId::from_str("00000000-0000-4000-8000-000000000001")
        .expect("fixed source identifier is valid");
    let evidence = EvidenceId::from_str("00000000-0000-4000-8000-000000000002")
        .expect("fixed evidence identifier is valid");
    let claim = ClaimId::from_str("00000000-0000-4000-8000-000000000003")
        .expect("fixed claim identifier is valid");

    assert_eq!(source.to_string(), "00000000-0000-4000-8000-000000000001");
    assert_eq!(evidence.to_string(), "00000000-0000-4000-8000-000000000002");
    assert_eq!(claim.to_string(), "00000000-0000-4000-8000-000000000003");
    assert_ne!(SourceId::generate(), SourceId::generate());
}

#[test]
fn identifiers_reject_malformed_and_non_v4_values() {
    assert_eq!(
        "not-a-uuid".parse::<SourceId>(),
        Err(IdentityError::InvalidUuid)
    );
    assert_eq!(
        "00000000-0000-0000-8000-000000000001".parse::<EvidenceId>(),
        Err(IdentityError::NotVersion4)
    );
    assert_eq!(
        "00000000-0000-4000-0000-000000000001".parse::<ClaimId>(),
        Err(IdentityError::NotRfc4122)
    );
}

#[test]
fn source_preserves_valid_snapshot_metadata() {
    let source = source("Research title", "text/html");

    assert_eq!(source.id(), &source_id());
    assert_eq!(source.content_digest(), &ContentDigest::sha256([7; 32]));
    assert_eq!(source.locator(), "https://example.test/paper");
    assert_eq!(source.title(), Some("Research title"));
    assert_eq!(source.retrieved_at().as_str(), "2026-08-29T10:00:00Z");
    assert_eq!(source.media_type().as_str(), "text/html");
}

#[test]
fn source_rejects_blank_locator_and_title() {
    let retrieved_at = retrieved_at();
    let media_type = media_type("text/plain");

    assert_eq!(
        Source::new(
            source_id(),
            ContentDigest::sha256([1; 32]),
            " \n".to_owned(),
            None,
            retrieved_at.clone(),
            media_type.clone(),
        ),
        Err(ValidationError::EmptySourceLocator)
    );
    assert_eq!(
        Source::new(
            source_id(),
            ContentDigest::sha256([1; 32]),
            "local:source".to_owned(),
            Some("\t".to_owned()),
            retrieved_at,
            media_type,
        ),
        Err(ValidationError::EmptySourceTitle)
    );
}

#[test]
fn retrieval_time_requires_utc_rfc3339() {
    for value in [
        "2026-08-29T10:00:00+06:00",
        "2026-08-29 10:00:00Z",
        "not-a-time",
    ] {
        assert_eq!(
            RetrievedAt::new(value),
            Err(ValidationError::InvalidRetrievedAt),
            "{value}"
        );
    }
    assert_eq!(
        RetrievedAt::new("2026-08-29T10:00:00+00:00")
            .expect("zero UTC offset is accepted")
            .as_str(),
        "2026-08-29T10:00:00+00:00"
    );
}

#[test]
fn media_type_requires_a_concrete_mime_essence() {
    for value in [
        "",
        "text",
        "/plain",
        "text/",
        "*/plain",
        "text/*",
        "text/plain; charset=utf-8",
        "text/pla in",
        "téxt/plain",
    ] {
        assert_eq!(
            MediaType::new(value),
            Err(ValidationError::InvalidMediaType),
            "{value}"
        );
    }
    assert_eq!(
        MediaType::new("application/vnd.aurora+json")
            .expect("token characters are accepted")
            .as_str(),
        "application/vnd.aurora+json"
    );
}

#[test]
fn evidence_requires_a_non_blank_excerpt() {
    assert_eq!(
        Evidence::new(evidence_id(2), source_id(), "  \n".to_owned()),
        Err(ValidationError::EmptyEvidenceExcerpt)
    );

    let evidence = Evidence::new(
        evidence_id(2),
        source_id(),
        "Exact quoted passage.".to_owned(),
    )
    .expect("non-blank evidence is accepted");
    assert_eq!(evidence.id(), &evidence_id(2));
    assert_eq!(evidence.source_id(), &source_id());
    assert_eq!(evidence.excerpt(), "Exact quoted passage.");
}

#[test]
fn claim_requires_a_statement_and_distinct_evidence() {
    assert_eq!(
        Claim::new(claim_id(), " \t".to_owned(), vec![evidence_id(2)]),
        Err(ValidationError::EmptyClaimStatement)
    );
    assert_eq!(
        Claim::new(claim_id(), "A claim".to_owned(), Vec::new()),
        Err(ValidationError::ClaimHasNoEvidence)
    );
    assert_eq!(
        Claim::new(
            claim_id(),
            "A claim".to_owned(),
            vec![evidence_id(2), evidence_id(2)],
        ),
        Err(ValidationError::DuplicateClaimEvidence(evidence_id(2)))
    );
}

#[test]
fn claim_evidence_has_deterministic_order() {
    let claim = Claim::new(
        claim_id(),
        "A grounded claim".to_owned(),
        vec![evidence_id(9), evidence_id(2)],
    )
    .expect("distinct evidence is accepted");

    assert_eq!(claim.id(), &claim_id());
    assert_eq!(claim.statement(), "A grounded claim");
    assert_eq!(
        claim.evidence_ids().iter().copied().collect::<Vec<_>>(),
        vec![evidence_id(2), evidence_id(9)]
    );
}

fn source(title: &str, media_type_value: &str) -> Source {
    Source::new(
        source_id(),
        ContentDigest::sha256([7; 32]),
        "https://example.test/paper".to_owned(),
        Some(title.to_owned()),
        retrieved_at(),
        media_type(media_type_value),
    )
    .expect("source fixture is valid")
}

fn retrieved_at() -> RetrievedAt {
    RetrievedAt::new("2026-08-29T10:00:00Z").expect("fixture time is valid")
}

fn media_type(value: &str) -> MediaType {
    MediaType::new(value).expect("fixture media type is valid")
}

fn source_id() -> SourceId {
    "00000000-0000-4000-8000-000000000001"
        .parse()
        .expect("fixture source identifier is valid")
}

fn evidence_id(suffix: u8) -> EvidenceId {
    format!("00000000-0000-4000-8000-{suffix:012}")
        .parse()
        .expect("fixture evidence identifier is valid")
}

fn claim_id() -> ClaimId {
    "00000000-0000-4000-8000-000000000003"
        .parse()
        .expect("fixture claim identifier is valid")
}
