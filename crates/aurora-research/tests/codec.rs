use aurora_research::{
    Claim, ClaimId, CodecError, ContentDigest, Evidence, EvidenceId, IdentityError, MediaType,
    RESEARCH_SCHEMA_VERSION, ResearchEvent, ResearchRecord, RetrievedAt, Source, SourceId,
    ValidationError, decode_record, encode_record,
};
use serde_json::{Value, json};

#[test]
fn every_research_event_round_trips_canonically() {
    let source = source();
    let evidence = evidence();
    let claim = claim();
    let records = [
        record(1, ResearchEvent::SourceRecorded(source)),
        record(2, ResearchEvent::EvidenceRecorded(evidence)),
        record(3, ResearchEvent::ClaimProposed(claim)),
    ];

    for record in records {
        let encoded = encode_record(&record).expect("record encodes");
        let decoded = decode_record(&encoded).expect("record decodes");

        assert_eq!(decoded, record);
        assert_eq!(
            encode_record(&decoded).expect("decoded record encodes"),
            encoded
        );
    }
}

#[test]
fn source_wire_shape_is_explicit_and_versioned() {
    let encoded = encode_record(&record(1, ResearchEvent::SourceRecorded(source())))
        .expect("source record encodes");
    let value: Value = serde_json::from_slice(&encoded).expect("encoded record is JSON");

    assert_eq!(value["schema_version"], RESEARCH_SCHEMA_VERSION);
    assert_eq!(value["sequence"], 1);
    assert_eq!(value["event"]["type"], "source_recorded");
    assert_eq!(
        value["event"]["source"]["id"],
        "00000000-0000-4000-8000-000000000001"
    );
    assert_eq!(
        value["event"]["source"]["content_digest"]["algorithm"],
        "sha256"
    );
    assert_eq!(
        value["event"]["source"]["content_digest"]["value"],
        "07".repeat(32)
    );
    assert_eq!(
        value["event"]["source"]["locator"],
        "https://example.test/paper"
    );
    assert_eq!(value["event"]["source"]["title"], "Research title");
    assert_eq!(
        value["event"]["source"]["retrieved_at"],
        "2026-08-29T10:00:00Z"
    );
    assert_eq!(value["event"]["source"]["media_type"], "text/html");
}

#[test]
fn claim_wire_evidence_is_canonically_ordered() {
    let claim = Claim::new(
        claim_id(3),
        "A grounded claim".to_owned(),
        vec![evidence_id(9), evidence_id(2)],
    )
    .expect("claim fixture is valid");
    let encoded = encode_record(&record(1, ResearchEvent::ClaimProposed(claim)))
        .expect("claim record encodes");
    let value: Value = serde_json::from_slice(&encoded).expect("encoded record is JSON");

    assert_eq!(
        value["event"]["claim"]["evidence_ids"],
        json!([
            "00000000-0000-4000-8000-000000000002",
            "00000000-0000-4000-8000-000000000009"
        ])
    );
}

#[test]
fn malformed_json_has_a_structural_error_without_echoing_input() {
    let secret = b"{not-json-private-payload";
    let error = decode_record(secret).expect_err("malformed JSON is rejected");

    assert_eq!(error, CodecError::MalformedJson);
    assert!(!error.to_string().contains("private-payload"));
}

#[test]
fn unsupported_schema_precedes_entity_validation() {
    let mut value = source_value();
    value["schema_version"] = json!(99);
    value["event"]["source"]["locator"] = json!("");

    assert_eq!(decode_value(value), Err(CodecError::UnsupportedSchema(99)));
}

#[test]
fn zero_sequence_is_an_invalid_record() {
    let mut value = source_value();
    value["sequence"] = json!(0);

    assert_eq!(
        decode_value(value),
        Err(CodecError::InvalidRecord(ValidationError::ZeroSequence))
    );
}

#[test]
fn invalid_wire_identifiers_cannot_create_domain_values() {
    let mut malformed = source_value();
    malformed["event"]["source"]["id"] = json!("not-a-uuid");
    assert_eq!(
        decode_value(malformed),
        Err(CodecError::InvalidIdentity(IdentityError::InvalidUuid))
    );

    let mut non_v4 = evidence_value();
    non_v4["event"]["evidence"]["source_id"] = json!("00000000-0000-0000-8000-000000000001");
    assert_eq!(
        decode_value(non_v4),
        Err(CodecError::InvalidIdentity(IdentityError::NotVersion4))
    );
}

#[test]
fn invalid_digest_shapes_are_rejected() {
    for (algorithm, digest) in [
        ("sha512", "07".repeat(32)),
        ("sha256", "07".repeat(31)),
        ("sha256", "07".repeat(33)),
        ("sha256", "GG".repeat(32)),
        ("sha256", "AB".repeat(32)),
    ] {
        let mut value = source_value();
        value["event"]["source"]["content_digest"]["algorithm"] = json!(algorithm);
        value["event"]["source"]["content_digest"]["value"] = json!(digest);

        assert_eq!(
            decode_value(value),
            Err(CodecError::InvalidRecord(
                ValidationError::InvalidContentDigest
            ))
        );
    }
}

#[test]
fn invalid_source_metadata_is_rejected() {
    let cases = [
        ("locator", " ", ValidationError::EmptySourceLocator),
        ("title", "\t", ValidationError::EmptySourceTitle),
        (
            "retrieved_at",
            "private-time",
            ValidationError::InvalidRetrievedAt,
        ),
        (
            "media_type",
            "private-media",
            ValidationError::InvalidMediaType,
        ),
    ];

    for (field, invalid_value, expected) in cases {
        let mut value = source_value();
        value["event"]["source"][field] = json!(invalid_value);

        let error = decode_value(value).expect_err("invalid source metadata is rejected");
        assert_eq!(error, CodecError::InvalidRecord(expected));
        if invalid_value.starts_with("private-") {
            assert!(!error.to_string().contains(invalid_value));
        }
    }
}

#[test]
fn invalid_evidence_and_claim_text_is_rejected_without_echoing_values() {
    let mut evidence = evidence_value();
    evidence["event"]["evidence"]["excerpt"] = json!(" \n");
    let evidence_error = decode_value(evidence).expect_err("blank excerpt is rejected");
    assert_eq!(
        evidence_error,
        CodecError::InvalidRecord(ValidationError::EmptyEvidenceExcerpt)
    );
    assert!(!evidence_error.to_string().contains("\n"));

    let mut claim = claim_value();
    claim["event"]["claim"]["statement"] = json!(" \t");
    let claim_error = decode_value(claim).expect_err("blank statement is rejected");
    assert_eq!(
        claim_error,
        CodecError::InvalidRecord(ValidationError::EmptyClaimStatement)
    );
    assert!(!claim_error.to_string().contains("\t"));
}

#[test]
fn claim_wire_requires_distinct_nonempty_evidence() {
    let mut empty = claim_value();
    empty["event"]["claim"]["evidence_ids"] = json!([]);
    assert_eq!(
        decode_value(empty),
        Err(CodecError::InvalidRecord(
            ValidationError::ClaimHasNoEvidence
        ))
    );

    let repeated_id = "00000000-0000-4000-8000-000000000002";
    let mut duplicate = claim_value();
    duplicate["event"]["claim"]["evidence_ids"] = json!([repeated_id, repeated_id]);
    assert_eq!(
        decode_value(duplicate),
        Err(CodecError::InvalidRecord(
            ValidationError::DuplicateClaimEvidence(evidence_id(2))
        ))
    );
}

#[test]
fn invalid_media_type_wire_forms_are_rejected() {
    for media_type in ["text", "*/plain", "text/*", "text/plain; charset=utf-8"] {
        let mut value = source_value();
        value["event"]["source"]["media_type"] = json!(media_type);
        assert_eq!(
            decode_value(value),
            Err(CodecError::InvalidRecord(ValidationError::InvalidMediaType))
        );
    }
}

fn source_value() -> Value {
    serde_json::from_slice(
        &encode_record(&record(1, ResearchEvent::SourceRecorded(source())))
            .expect("source record encodes"),
    )
    .expect("source record is JSON")
}

fn evidence_value() -> Value {
    serde_json::from_slice(
        &encode_record(&record(1, ResearchEvent::EvidenceRecorded(evidence())))
            .expect("evidence record encodes"),
    )
    .expect("evidence record is JSON")
}

fn claim_value() -> Value {
    serde_json::from_slice(
        &encode_record(&record(1, ResearchEvent::ClaimProposed(claim())))
            .expect("claim record encodes"),
    )
    .expect("claim record is JSON")
}

fn decode_value(value: Value) -> Result<ResearchRecord, CodecError> {
    decode_record(&serde_json::to_vec(&value).expect("JSON value encodes"))
}

fn record(sequence: u64, event: ResearchEvent) -> ResearchRecord {
    ResearchRecord::new(sequence, event).expect("fixture record is valid")
}

fn source() -> Source {
    Source::new(
        source_id(1),
        ContentDigest::sha256([7; 32]),
        "https://example.test/paper".to_owned(),
        Some("Research title".to_owned()),
        RetrievedAt::new("2026-08-29T10:00:00Z").expect("fixture time is valid"),
        MediaType::new("text/html").expect("fixture media type is valid"),
    )
    .expect("source fixture is valid")
}

fn evidence() -> Evidence {
    Evidence::new(
        evidence_id(2),
        source_id(1),
        "Exact quoted passage.".to_owned(),
    )
    .expect("evidence fixture is valid")
}

fn claim() -> Claim {
    Claim::new(
        claim_id(3),
        "A grounded claim".to_owned(),
        vec![evidence_id(2)],
    )
    .expect("claim fixture is valid")
}

fn source_id(suffix: u8) -> SourceId {
    format!("00000000-0000-4000-8000-{suffix:012}")
        .parse()
        .expect("fixture source identifier is valid")
}

fn evidence_id(suffix: u8) -> EvidenceId {
    format!("00000000-0000-4000-8000-{suffix:012}")
        .parse()
        .expect("fixture evidence identifier is valid")
}

fn claim_id(suffix: u8) -> ClaimId {
    format!("00000000-0000-4000-8000-{suffix:012}")
        .parse()
        .expect("fixture claim identifier is valid")
}
