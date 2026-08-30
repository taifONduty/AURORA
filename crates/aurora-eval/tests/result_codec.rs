mod support;

use aurora_eval::{
    EvaluationCase, EvaluationCaseId, EvaluationMetadata, EvaluationRun, ExecutionFailure,
    ExpectedTerminalOutcome, ModelConfiguration, ObservedUsage, ProviderCost,
    RetrievalConfiguration, decode_case_result, encode_case_result, evaluate_case,
};

#[test]
fn result_preserves_usage_failures_and_derived_counts() {
    let fixture = support::supported_fixture();
    let case = EvaluationCase::new(
        EvaluationCaseId::new("result-metadata").unwrap(),
        "What does the fixture establish?".to_owned(),
        vec![],
        vec![],
        Some(ExpectedTerminalOutcome::Completed),
        Some(0),
    )
    .unwrap();
    let usage = ObservedUsage::new(
        Some(3),
        Some(2),
        Some(120),
        Some(40),
        Some(1_250),
        Some(ProviderCost::new("USD".to_owned(), 2_345).unwrap()),
    );
    let run = EvaluationRun::new(fixture.records, support::metadata())
        .with_usage(usage.clone())
        .with_failure(ExecutionFailure::Provider)
        .unwrap()
        .with_failure(ExecutionFailure::Synthesis)
        .unwrap();

    let result = evaluate_case(&case, &run);

    assert_eq!(result.usage(), Some(&usage));
    assert_eq!(
        result.failures(),
        &[ExecutionFailure::Provider, ExecutionFailure::Synthesis]
    );
    let counts = result.counts().unwrap();
    assert_eq!(counts.records(), 7);
    assert_eq!(counts.sources(), 1);
    assert_eq!(counts.evidence(), 2);
    assert_eq!(counts.claims(), 1);
    assert_eq!(counts.verification_assessments(), 1);
    assert_eq!(counts.investigation_tasks(), 1);
    assert_eq!(counts.follow_up_tasks(), 0);
}

#[test]
fn absent_usage_remains_absent() {
    let fixture = support::supported_fixture();
    let case = support::case("no-usage", ExpectedTerminalOutcome::Completed, 0);
    let result = evaluate_case(
        &case,
        &EvaluationRun::new(fixture.records, support::metadata()),
    );

    assert_eq!(result.usage(), None);
    assert!(result.failures().is_empty());
}

#[test]
fn case_result_codec_is_strict_canonical_and_secret_free() {
    let fixture = support::supported_fixture();
    let case = support::case("codec-result", ExpectedTerminalOutcome::Completed, 0);
    let run = EvaluationRun::new(fixture.records, support::metadata())
        .with_failure(ExecutionFailure::Retrieval)
        .unwrap();
    let result = evaluate_case(&case, &run);

    let encoded = encode_case_result(&result).unwrap();
    let decoded = decode_case_result(&encoded).unwrap();

    assert_eq!(decoded, result);
    assert_eq!(encode_case_result(&decoded).unwrap(), encoded);
    let text = String::from_utf8(encoded.clone()).unwrap();
    assert!(!text.contains("api_key"));
    assert!(!text.contains("authorization"));
    assert!(!text.contains("reasoning"));

    let mut unknown: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    unknown["unexpected"] = serde_json::json!(true);
    assert!(decode_case_result(&serde_json::to_vec(&unknown).unwrap()).is_err());

    let mut version: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    version["schema_version"] = serde_json::json!(2);
    assert!(decode_case_result(&serde_json::to_vec(&version).unwrap()).is_err());

    let mut duplicate: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    duplicate["result"]["failures"] = serde_json::json!(["retrieval", "retrieval"]);
    assert!(decode_case_result(&serde_json::to_vec(&duplicate).unwrap()).is_err());

    for field in [
        "history_reconstructed",
        "references_valid",
        "terminal_is_explicit",
        "record_count_unchanged",
    ] {
        let mut tampered: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        tampered["result"]["guarantees"][field] = serde_json::json!(false);
        assert!(decode_case_result(&serde_json::to_vec(&tampered).unwrap()).is_err());
    }
}

#[test]
fn metadata_requires_bounded_identifiers_and_utc() {
    assert!(
        EvaluationMetadata::new(
            "rev".to_owned(),
            "suite".to_owned(),
            "config".to_owned(),
            "2026-08-30T00:00:00+06:00".to_owned(),
        )
        .is_err()
    );
    assert!(ProviderCost::new(" ".to_owned(), 1).is_err());

    let metadata = EvaluationMetadata::new(
        "rev".to_owned(),
        "suite".to_owned(),
        "config".to_owned(),
        "2026-08-30T00:00:00Z".to_owned(),
    )
    .unwrap()
    .with_model(
        ModelConfiguration::new(
            "openai".to_owned(),
            "gpt-test".to_owned(),
            "planner-v1".to_owned(),
        )
        .unwrap(),
    )
    .with_retrieval(RetrievalConfiguration::new("tavily".to_owned(), "basic".to_owned()).unwrap())
    .with_follow_up_limit(2)
    .with_repeated_run_seed(7);
    assert_eq!(metadata.model().unwrap().model_id(), "gpt-test");
    assert_eq!(metadata.retrieval().unwrap().provider_id(), "tavily");
    assert_eq!(metadata.follow_up_limit(), Some(2));
    assert_eq!(metadata.repeated_run_seed(), Some(7));
}

#[test]
fn invalid_history_is_retained_as_an_explicit_failure() {
    let mut fixture = support::supported_fixture();
    fixture.records.remove(0);
    let case = support::case("invalid-history", ExpectedTerminalOutcome::Failed, 0);

    let result = evaluate_case(
        &case,
        &EvaluationRun::new(fixture.records, support::metadata()),
    );

    assert!(result.invalid_research_history());
    assert_eq!(result.counts(), None);
    assert!(
        result
            .failures()
            .contains(&ExecutionFailure::InvalidResearchHistory)
    );
    assert!(!result.guarantees().history_reconstructed());
    assert!(encode_case_result(&result).is_ok());

    let encoded = encode_case_result(&result).unwrap();
    let mut terminal: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    terminal["result"]["terminal"] = serde_json::json!("completed");
    assert!(decode_case_result(&serde_json::to_vec(&terminal).unwrap()).is_err());

    let mut guarantee: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    guarantee["result"]["guarantees"]["history_reconstructed"] = serde_json::json!(true);
    assert!(decode_case_result(&serde_json::to_vec(&guarantee).unwrap()).is_err());
}

#[test]
fn evaluator_binds_case_and_authoritative_limits_to_metadata() {
    let fixture = support::supported_fixture();
    let case = support::case("bound-metadata", ExpectedTerminalOutcome::Completed, 0);
    let supplied = support::metadata()
        .with_case_id(EvaluationCaseId::new("wrong-case").unwrap())
        .with_follow_up_limit(99);

    let result = evaluate_case(&case, &EvaluationRun::new(fixture.records, supplied));

    assert_eq!(result.metadata().case_id(), Some(case.id()));
    assert_eq!(result.metadata().follow_up_limit(), Some(1));
    assert!(
        result
            .failures()
            .contains(&ExecutionFailure::BenchmarkMapping)
    );
}

#[test]
fn invalid_history_failure_is_derived_instead_of_accepted_from_the_caller() {
    let fixture = support::supported_fixture();
    let case = support::case(
        "derived-history-failure",
        ExpectedTerminalOutcome::Completed,
        0,
    );
    let run = EvaluationRun::new(fixture.records, support::metadata())
        .with_failure(ExecutionFailure::InvalidResearchHistory)
        .unwrap();

    let result = evaluate_case(&case, &run);

    assert!(!result.invalid_research_history());
    assert!(
        !result
            .failures()
            .contains(&ExecutionFailure::InvalidResearchHistory)
    );
    assert!(encode_case_result(&result).is_ok());
}

#[test]
fn result_codec_rejects_impossible_metric_relationships() {
    let fixture = support::supported_fixture();
    let case = support::case(
        "metric-relationships",
        ExpectedTerminalOutcome::Completed,
        0,
    );
    let result = evaluate_case(
        &case,
        &EvaluationRun::new(fixture.records, support::metadata()),
    );
    let encoded = encode_case_result(&result).unwrap();

    let mut excerpt_without_attribution: serde_json::Value =
        serde_json::from_slice(&encoded).unwrap();
    excerpt_without_attribution["result"]["grounding"]["exact_excerpts"]["matched"] =
        serde_json::json!(1);
    assert!(
        decode_case_result(&serde_json::to_vec(&excerpt_without_attribution).unwrap()).is_err()
    );

    let mut matched_and_missing_source: serde_json::Value =
        serde_json::from_slice(&encoded).unwrap();
    matched_and_missing_source["result"]["grounding"]["digest_matches"]["matched"] =
        serde_json::json!(1);
    assert!(decode_case_result(&serde_json::to_vec(&matched_and_missing_source).unwrap()).is_err());

    let mut impossible_qualification: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    impossible_qualification["result"]["synthesis"]["qualification_mismatches"] =
        serde_json::json!(1);
    assert!(decode_case_result(&serde_json::to_vec(&impossible_qualification).unwrap()).is_err());

    let mut impossible_repetition: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    impossible_repetition["result"]["adaptive"]["follow_up_tasks"] = serde_json::json!(1);
    impossible_repetition["result"]["adaptive"]["repeated_follow_up_objectives"] =
        serde_json::json!(1);
    impossible_repetition["result"]["counts"]["investigation_tasks"] = serde_json::json!(2);
    impossible_repetition["result"]["counts"]["follow_up_tasks"] = serde_json::json!(1);
    assert!(decode_case_result(&serde_json::to_vec(&impossible_repetition).unwrap()).is_err());

    let mut missing_semantic_judgment: serde_json::Value =
        serde_json::from_slice(&encoded).unwrap();
    missing_semantic_judgment["result"]["synthesis"]["assertions_with_valid_claims"] =
        serde_json::json!({"matched": 1, "total": 1});
    assert!(decode_case_result(&serde_json::to_vec(&missing_semantic_judgment).unwrap()).is_err());

    let mut model_without_provenance: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    model_without_provenance["result"]["synthesis"]["semantic"]["model_judged"] =
        serde_json::json!({"matched": 1, "total": 1});
    assert!(decode_case_result(&serde_json::to_vec(&model_without_provenance).unwrap()).is_err());

    let mut too_many_reported_claims: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    too_many_reported_claims["result"]["synthesis"]["reported_claims_with_citations"]["total"] =
        serde_json::json!(2);
    assert!(decode_case_result(&serde_json::to_vec(&too_many_reported_claims).unwrap()).is_err());

    let mut missing_false_completion: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    missing_false_completion["result"]["adaptive"]["expected_terminal_match"] =
        serde_json::json!(false);
    assert!(decode_case_result(&serde_json::to_vec(&missing_false_completion).unwrap()).is_err());

    let mut impossible_citation_repetition: serde_json::Value =
        serde_json::from_slice(&encoded).unwrap();
    impossible_citation_repetition["result"]["synthesis"]["assertions_with_valid_claims"] =
        serde_json::json!({"matched": 1, "total": 1});
    impossible_citation_repetition["result"]["synthesis"]["citation_resolution"] =
        serde_json::json!({"matched": 1, "total": 1});
    impossible_citation_repetition["result"]["synthesis"]["reported_claims_with_citations"] =
        serde_json::json!({"matched": 1, "total": 1});
    impossible_citation_repetition["result"]["synthesis"]["semantic"]["unjudged_assertions"] =
        serde_json::json!(1);
    impossible_citation_repetition["result"]["synthesis"]["repeated_evidence_citations"] =
        serde_json::json!(1);
    assert!(
        decode_case_result(&serde_json::to_vec(&impossible_citation_repetition).unwrap()).is_err()
    );
}

#[test]
fn result_codec_rejects_gap_resolution_steps_outside_the_recorded_sequence() {
    let case = support::case("gap-step-bounds", ExpectedTerminalOutcome::Completed, 1);
    let result = evaluate_case(
        &case,
        &EvaluationRun::new(support::adaptive_history(), support::metadata()),
    );
    let encoded = encode_case_result(&result).unwrap();

    let mut zero: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    zero["result"]["adaptive"]["gap_resolution_steps"][0] = serde_json::json!(0);
    assert!(decode_case_result(&serde_json::to_vec(&zero).unwrap()).is_err());

    let mut oversized: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    oversized["result"]["adaptive"]["gap_resolution_steps"][0] =
        serde_json::json!(result.counts().unwrap().records());
    assert!(decode_case_result(&serde_json::to_vec(&oversized).unwrap()).is_err());
}
