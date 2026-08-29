use aurora_research::{
    ClaimId, EvidenceAssessment, EvidenceId, EvidenceRelation, EvidenceSufficiency, IdentityError,
    VERIFICATION_SCHEMA_VERSION, VerificationAssessment, VerificationCodecError, VerificationId,
    VerificationRecord, VerificationValidationError, decode_verification_record,
    encode_verification_record,
};
use serde_json::{Value, json};

#[test]
fn every_relation_and_sufficiency_round_trips_canonically() {
    for record in records() {
        let encoded = encode_verification_record(&record).expect("record encodes");
        let decoded = decode_verification_record(&encoded).expect("record decodes");

        assert_eq!(decoded, record);
        assert_eq!(
            encode_verification_record(&decoded).expect("decoded record encodes"),
            encoded
        );
    }
}

#[test]
fn wire_shape_is_explicit_versioned_and_ordered() {
    let value = encoded_value(&records()[0]);

    assert_eq!(value["schema_version"], VERIFICATION_SCHEMA_VERSION);
    assert_eq!(value["sequence"], 1);
    assert_eq!(
        value["assessment"]["id"],
        "00000000-0000-4000-8000-000000000001"
    );
    assert_eq!(
        value["assessment"]["claim_id"],
        "00000000-0000-4000-8000-000000000015"
    );
    assert_eq!(
        value["assessment"]["evidence_relations"],
        json!([
            {
                "evidence_id": "00000000-0000-4000-8000-00000000000b",
                "relation": "supports"
            },
            {
                "evidence_id": "00000000-0000-4000-8000-00000000000c",
                "relation": "contradicts"
            },
            {
                "evidence_id": "00000000-0000-4000-8000-00000000000d",
                "relation": "unclear"
            },
            {
                "evidence_id": "00000000-0000-4000-8000-00000000000e",
                "relation": "irrelevant"
            }
        ])
    );
    assert_eq!(value["assessment"]["sufficiency"], "sufficient");
}

#[test]
fn malformed_json_is_structural_and_does_not_echo_input() {
    let bytes = b"{private-verification-payload";
    let error = decode_verification_record(bytes).expect_err("malformed JSON is rejected");

    assert_eq!(error, VerificationCodecError::MalformedJson);
    assert!(!error.to_string().contains("private-verification-payload"));
}

#[test]
fn unsupported_schema_precedes_local_validation() {
    let mut value = encoded_value(&records()[0]);
    value["schema_version"] = json!(99);
    value["assessment"]["evidence_relations"] = json!([]);

    assert_eq!(
        decode_value(value),
        Err(VerificationCodecError::UnsupportedSchema(99))
    );
}

#[test]
fn zero_sequence_and_invalid_identities_are_rejected() {
    let mut zero = encoded_value(&records()[0]);
    zero["sequence"] = json!(0);
    assert_eq!(
        decode_value(zero),
        Err(VerificationCodecError::InvalidRecord(
            VerificationValidationError::ZeroVerificationSequence
        ))
    );

    let mut malformed = encoded_value(&records()[0]);
    malformed["assessment"]["id"] = json!("not-a-uuid");
    assert_eq!(
        decode_value(malformed),
        Err(VerificationCodecError::InvalidIdentity(
            IdentityError::InvalidUuid
        ))
    );

    let mut non_v4 = encoded_value(&records()[0]);
    non_v4["assessment"]["claim_id"] = json!("00000000-0000-0000-8000-000000000001");
    assert_eq!(
        decode_value(non_v4),
        Err(VerificationCodecError::InvalidIdentity(
            IdentityError::NotVersion4
        ))
    );
}

#[test]
fn empty_and_duplicate_evidence_relations_are_rejected() {
    let mut empty = encoded_value(&records()[0]);
    empty["assessment"]["evidence_relations"] = json!([]);
    assert_eq!(
        decode_value(empty),
        Err(VerificationCodecError::InvalidRecord(
            VerificationValidationError::NoAssessedEvidence
        ))
    );

    let mut duplicate = encoded_value(&records()[0]);
    let first = duplicate["assessment"]["evidence_relations"][0].clone();
    duplicate["assessment"]["evidence_relations"] = json!([first.clone(), first]);
    assert_eq!(
        decode_value(duplicate),
        Err(VerificationCodecError::InvalidRecord(
            VerificationValidationError::DuplicateAssessedEvidence(evidence_id(11))
        ))
    );
}

#[test]
fn isolated_decode_defers_research_reference_validation() {
    let record = record(
        1,
        assessment(
            99,
            98,
            &[(97, EvidenceRelation::Supports)],
            EvidenceSufficiency::Indeterminate,
        ),
    );
    let encoded = encode_verification_record(&record).expect("record encodes");

    assert_eq!(
        decode_verification_record(&encoded).expect("shape is valid"),
        record
    );
}

fn records() -> Vec<VerificationRecord> {
    vec![
        record(
            1,
            assessment(
                1,
                21,
                &[
                    (14, EvidenceRelation::Irrelevant),
                    (12, EvidenceRelation::Contradicts),
                    (11, EvidenceRelation::Supports),
                    (13, EvidenceRelation::Unclear),
                ],
                EvidenceSufficiency::Sufficient,
            ),
        ),
        record(
            2,
            assessment(
                2,
                22,
                &[(11, EvidenceRelation::Supports)],
                EvidenceSufficiency::Insufficient,
            ),
        ),
        record(
            3,
            assessment(
                3,
                23,
                &[(12, EvidenceRelation::Unclear)],
                EvidenceSufficiency::Indeterminate,
            ),
        ),
    ]
}

fn assessment(
    verification: u128,
    claim: u128,
    evidence: &[(u128, EvidenceRelation)],
    sufficiency: EvidenceSufficiency,
) -> VerificationAssessment {
    VerificationAssessment::new(
        verification_id(verification),
        claim_id(claim),
        evidence
            .iter()
            .map(|(id, relation)| EvidenceAssessment::new(evidence_id(*id), *relation))
            .collect(),
        sufficiency,
    )
    .expect("assessment is valid")
}

fn record(sequence: u64, assessment: VerificationAssessment) -> VerificationRecord {
    VerificationRecord::new(sequence, assessment).expect("record is valid")
}

fn encoded_value(record: &VerificationRecord) -> Value {
    serde_json::from_slice(
        &encode_verification_record(record).expect("verification record encodes"),
    )
    .expect("encoded record is JSON")
}

fn decode_value(value: Value) -> Result<VerificationRecord, VerificationCodecError> {
    decode_verification_record(&serde_json::to_vec(&value).expect("value encodes"))
}

fn verification_id(value: u128) -> VerificationId {
    uuid(value)
        .parse()
        .expect("verification identifier is valid")
}

fn claim_id(value: u128) -> ClaimId {
    uuid(value).parse().expect("claim identifier is valid")
}

fn evidence_id(value: u128) -> EvidenceId {
    uuid(value).parse().expect("evidence identifier is valid")
}

fn uuid(value: u128) -> String {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).hyphenated().to_string()
}
