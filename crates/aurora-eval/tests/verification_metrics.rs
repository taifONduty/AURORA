mod support;

use aurora_eval::{
    EvaluationCase, EvaluationCaseId, EvaluationLabelId, EvaluationRun, EvidenceBinding,
    EvidenceKey, ExecutionFailure, ExpectedEvidenceRelation, ExpectedRelation, ExpectedSufficiency,
    VerificationBinding, VerificationExpectation, evaluate_case,
};

#[test]
fn exact_verification_binding_scores_relation_and_sufficiency() {
    let fixture = support::supported_fixture();
    let expectation_id = EvaluationLabelId::new("supported").expect("label is valid");
    let evidence_key = EvidenceKey::new("primary").expect("key is valid");
    let case = verification_case(
        expectation_id.clone(),
        evidence_key.clone(),
        ExpectedRelation::Supports,
        ExpectedSufficiency::Sufficient,
    );
    let binding = VerificationBinding::new(
        expectation_id,
        fixture.verification_id.to_string(),
        vec![
            EvidenceBinding::new(evidence_key, fixture.evidence_id.to_string())
                .expect("binding is valid"),
        ],
    )
    .expect("binding is valid");
    let run = EvaluationRun::new(fixture.records, support::metadata())
        .with_verification_binding(binding)
        .expect("binding is unique");

    let result = evaluate_case(&case, &run);
    let relations = result.verification().relations();
    let supports = relations.supports();

    assert_eq!(relations.accuracy().matched(), 1);
    assert_eq!(relations.accuracy().total(), 1);
    assert_eq!(supports.true_positive(), 1);
    assert_eq!(supports.false_positive(), 0);
    assert_eq!(supports.false_negative(), 0);
    assert_eq!(supports.precision(), Some(1.0));
    assert_eq!(supports.recall(), Some(1.0));
    assert_eq!(supports.f1(), Some(1.0));
    assert_eq!(result.verification().sufficiency().accuracy().matched(), 1);
    assert_eq!(result.verification().contradiction_recall(), None);
}

#[test]
fn missing_binding_remains_in_accuracy_and_false_negative_denominators() {
    let fixture = support::supported_fixture();
    let case = verification_case(
        EvaluationLabelId::new("missing").expect("label is valid"),
        EvidenceKey::new("primary").expect("key is valid"),
        ExpectedRelation::Supports,
        ExpectedSufficiency::Sufficient,
    );

    let result = evaluate_case(
        &case,
        &EvaluationRun::new(fixture.records, support::metadata()),
    );

    assert_eq!(result.verification().relations().accuracy().total(), 1);
    assert_eq!(result.verification().relations().accuracy().matched(), 0);
    assert_eq!(result.verification().relations().missing_predictions(), 1);
    assert_eq!(
        result
            .verification()
            .relations()
            .supports()
            .false_negative(),
        1
    );
    assert_eq!(result.verification().sufficiency().missing_predictions(), 1);
}

#[test]
fn sufficient_prediction_for_labelled_insufficiency_is_counted() {
    let fixture = support::supported_fixture();
    let expectation_id = EvaluationLabelId::new("insufficient").expect("label is valid");
    let evidence_key = EvidenceKey::new("primary").expect("key is valid");
    let case = verification_case(
        expectation_id.clone(),
        evidence_key.clone(),
        ExpectedRelation::Supports,
        ExpectedSufficiency::Insufficient,
    );
    let binding = VerificationBinding::new(
        expectation_id,
        fixture.verification_id.to_string(),
        vec![
            EvidenceBinding::new(evidence_key, fixture.evidence_id.to_string())
                .expect("binding is valid"),
        ],
    )
    .expect("binding is valid");
    let run = EvaluationRun::new(fixture.records, support::metadata())
        .with_verification_binding(binding)
        .expect("binding is unique");

    let result = evaluate_case(&case, &run);

    assert_eq!(result.verification().unsupported_as_sufficient(), 1);
    assert_eq!(result.verification().sufficiency().accuracy().matched(), 0);
}

#[test]
fn unknown_verification_binding_is_scored_missing_and_recorded_as_mapping_failure() {
    let fixture = support::supported_fixture();
    let expectation_id = EvaluationLabelId::new("unknown").unwrap();
    let evidence_key = EvidenceKey::new("primary").unwrap();
    let case = verification_case(
        expectation_id.clone(),
        evidence_key.clone(),
        ExpectedRelation::Supports,
        ExpectedSufficiency::Sufficient,
    );
    let binding = VerificationBinding::new(
        expectation_id,
        support::verification_id(99).to_string(),
        vec![EvidenceBinding::new(evidence_key, fixture.evidence_id.to_string()).unwrap()],
    )
    .unwrap();
    let run = EvaluationRun::new(fixture.records, support::metadata())
        .with_verification_binding(binding)
        .unwrap();

    let result = evaluate_case(&case, &run);

    assert_eq!(result.verification().relations().missing_predictions(), 1);
    assert!(
        result
            .failures()
            .contains(&ExecutionFailure::BenchmarkMapping)
    );
}

#[test]
fn extra_evidence_mapping_cannot_receive_verification_credit() {
    let fixture = support::supported_fixture();
    let expectation_id = EvaluationLabelId::new("extra").unwrap();
    let evidence_key = EvidenceKey::new("primary").unwrap();
    let case = verification_case(
        expectation_id.clone(),
        evidence_key.clone(),
        ExpectedRelation::Supports,
        ExpectedSufficiency::Sufficient,
    );
    let binding = VerificationBinding::new(
        expectation_id,
        fixture.verification_id.to_string(),
        vec![
            EvidenceBinding::new(evidence_key, fixture.evidence_id.to_string()).unwrap(),
            EvidenceBinding::new(
                EvidenceKey::new("unexpected").unwrap(),
                fixture.evidence_id.to_string(),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let run = EvaluationRun::new(fixture.records, support::metadata())
        .with_verification_binding(binding)
        .unwrap();

    let result = evaluate_case(&case, &run);

    assert_eq!(result.verification().relations().accuracy().matched(), 0);
    assert_eq!(result.verification().relations().missing_predictions(), 1);
    assert_eq!(result.verification().sufficiency().accuracy().matched(), 0);
    assert_eq!(result.verification().sufficiency().missing_predictions(), 1);
    assert!(
        result
            .failures()
            .contains(&ExecutionFailure::BenchmarkMapping)
    );
}

fn verification_case(
    id: EvaluationLabelId,
    evidence_key: EvidenceKey,
    relation: ExpectedRelation,
    sufficiency: ExpectedSufficiency,
) -> EvaluationCase {
    EvaluationCase::new(
        EvaluationCaseId::new(format!("case-{}", id.as_str())).expect("case id is valid"),
        "Question".to_owned(),
        Vec::new(),
        vec![
            VerificationExpectation::new(
                id,
                sufficiency,
                vec![ExpectedEvidenceRelation::new(evidence_key, relation)],
            )
            .expect("expectation is valid"),
        ],
        None,
        None,
    )
    .expect("case is valid")
}
