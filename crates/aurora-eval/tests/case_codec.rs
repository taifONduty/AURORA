use aurora_eval::{
    EvaluationCase, EvaluationCaseError, EvaluationCaseId, EvaluationLabelId, EvidenceKey,
    ExpectedEvidenceRelation, ExpectedRelation, ExpectedSufficiency, ExpectedTerminalOutcome,
    SourceSnapshotFixture, VerificationExpectation, decode_case, encode_case,
};

const SOURCE_ID: &str = "00000000-0000-4000-8000-000000000001";

#[test]
fn case_round_trip_preserves_explicit_labels_and_fixtures() {
    let case = supported_case();

    let encoded = encode_case(&case).expect("case encodes");
    let decoded = decode_case(&encoded).expect("case decodes");

    assert_eq!(decoded, case);
    assert_eq!(
        encode_case(&decoded).expect("decoded case encodes"),
        encoded
    );
}

#[test]
fn case_rejects_duplicate_evidence_keys_without_partial_value() {
    let key = EvidenceKey::new("primary").expect("key is valid");
    let expectation = VerificationExpectation::new(
        EvaluationLabelId::new("verification").expect("label is valid"),
        ExpectedSufficiency::Sufficient,
        vec![
            ExpectedEvidenceRelation::new(key.clone(), ExpectedRelation::Supports),
            ExpectedEvidenceRelation::new(key, ExpectedRelation::Contradicts),
        ],
    );

    assert_eq!(
        expectation,
        Err(EvaluationCaseError::DuplicateEvidenceKey(
            "primary".to_owned()
        ))
    );
}

#[test]
fn case_rejects_unknown_fields_and_schema_versions() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&encode_case(&supported_case()).expect("case encodes"))
            .expect("encoded case is JSON");
    value["unexpected"] = serde_json::json!(true);
    assert!(decode_case(&serde_json::to_vec(&value).expect("JSON encodes")).is_err());

    let mut value: serde_json::Value =
        serde_json::from_slice(&encode_case(&supported_case()).expect("case encodes"))
            .expect("encoded case is JSON");
    value["schema_version"] = serde_json::json!(2);
    assert!(decode_case(&serde_json::to_vec(&value).expect("JSON encodes")).is_err());
}

fn supported_case() -> EvaluationCase {
    let snapshot = SourceSnapshotFixture::new(SOURCE_ID, "Pinned source content".to_owned())
        .expect("snapshot is valid");
    let relation = ExpectedEvidenceRelation::new(
        EvidenceKey::new("primary").expect("key is valid"),
        ExpectedRelation::Supports,
    );
    let verification = VerificationExpectation::new(
        EvaluationLabelId::new("verification").expect("label is valid"),
        ExpectedSufficiency::Sufficient,
        vec![relation],
    )
    .expect("expectation is valid");
    EvaluationCase::new(
        EvaluationCaseId::new("supported-claim").expect("case id is valid"),
        "What is supported?".to_owned(),
        vec![snapshot],
        vec![verification],
        Some(ExpectedTerminalOutcome::Completed),
        Some(0),
    )
    .expect("case is valid")
}
