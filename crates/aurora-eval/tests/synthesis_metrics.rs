mod support;

use aurora_eval::{
    AdjudicationOrigin, AssertionLocation, EvaluationCase, EvaluationCaseId, EvaluationRun,
    JudgeMetadata, ObservedAssertion, ObservedCitation, ObservedPresentation, ObservedSection,
    SemanticAdjudication, SemanticGrounding, SynthesisObservation, evaluate_case,
};
use aurora_research::{
    GroundedReport, ResearchControlState, SynthesisAssertionDraft, SynthesisBasis, SynthesisDraft,
    SynthesisSectionDraft,
};

#[test]
fn validated_report_resolves_claim_evidence_and_source_paths() {
    let fixture = support::supported_fixture();
    let state =
        ResearchControlState::reconstruct(fixture.records.clone()).expect("history reconstructs");
    let basis = SynthesisBasis::from_state(&state).expect("basis is valid");
    let report = GroundedReport::from_basis(
        &basis,
        SynthesisDraft::new(vec![
            SynthesisSectionDraft::new(vec![
                SynthesisAssertionDraft::new(
                    "AURORA preserves evidence.".to_owned(),
                    vec![support::claim_id(1)],
                )
                .expect("assertion is valid"),
            ])
            .expect("section is valid"),
        ])
        .expect("draft is valid"),
    )
    .expect("report is valid");
    let run = EvaluationRun::new(fixture.records, support::metadata())
        .with_synthesis(SynthesisObservation::from_report(&report));

    let result = evaluate_case(&case("validated"), &run);
    let synthesis = result.synthesis();

    assert_eq!(synthesis.assertions_with_valid_claims().matched(), 1);
    assert_eq!(synthesis.assertions_with_valid_claims().total(), 1);
    assert_eq!(synthesis.citation_resolution().matched(), 1);
    assert_eq!(synthesis.citation_resolution().total(), 1);
    assert_eq!(synthesis.reported_claims_with_citations().matched(), 1);
    assert_eq!(synthesis.qualification_mismatches(), 0);
    assert_eq!(synthesis.deterministic_rendering(), Some(true));
}

#[test]
fn imported_unknown_claim_and_invalid_citation_are_counted() {
    let fixture = support::supported_fixture();
    let observation =
        SynthesisObservation::new(vec![ObservedSection::new(vec![ObservedAssertion::new(
            "Invented assertion".to_owned(),
            vec!["00000000-0000-4000-8000-000000000099".to_owned()],
            ObservedPresentation::Established,
            vec![ObservedCitation::new(
                "00000000-0000-4000-8000-000000000099".to_owned(),
                fixture.evidence_id.to_string(),
                fixture.source_id.to_string(),
                "00".repeat(32),
            )],
        )])]);
    let run = EvaluationRun::new(fixture.records, support::metadata()).with_synthesis(observation);

    let result = evaluate_case(&case("invalid"), &run);

    assert_eq!(result.synthesis().invalid_claim_references(), 1);
    assert_eq!(result.synthesis().citation_resolution().matched(), 0);
    assert_eq!(result.synthesis().citation_resolution().total(), 1);
}

#[test]
fn substantive_assertions_need_one_reportable_claim_while_blank_text_gets_no_credit() {
    let fixture = support::supported_fixture();
    let observation = SynthesisObservation::new(vec![ObservedSection::new(vec![
        ObservedAssertion::new(
            "Mixed references".to_owned(),
            vec![
                support::claim_id(1).to_string(),
                support::claim_id(99).to_string(),
            ],
            ObservedPresentation::Established,
            Vec::new(),
        ),
        ObservedAssertion::new(
            "   ".to_owned(),
            vec![support::claim_id(1).to_string()],
            ObservedPresentation::Established,
            vec![ObservedCitation::new(
                support::claim_id(1).to_string(),
                fixture.evidence_id.to_string(),
                fixture.source_id.to_string(),
                support::content_digest(&fixture.content)
                    .as_sha256()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            )],
        ),
    ])]);
    let result = evaluate_case(
        &case("substantive"),
        &EvaluationRun::new(fixture.records, support::metadata()).with_synthesis(observation),
    );

    assert_eq!(
        result.synthesis().assertions_with_valid_claims().matched(),
        1
    );
    assert_eq!(result.synthesis().assertions_with_valid_claims().total(), 1);
    assert_eq!(result.synthesis().invalid_claim_references(), 1);
    assert_eq!(result.synthesis().blank_assertions(), 1);
    assert_eq!(result.synthesis().citation_resolution().total(), 0);
    assert_eq!(
        result
            .synthesis()
            .reported_claims_with_citations()
            .matched(),
        0
    );
}

#[test]
fn established_assertion_counts_each_resolved_qualification_risk() {
    let records = support::mixed_claim_presentations_history();
    ResearchControlState::reconstruct(records.clone()).expect("mixed history reconstructs");
    let observation =
        SynthesisObservation::new(vec![ObservedSection::new(vec![ObservedAssertion::new(
            "Both claims are settled".to_owned(),
            vec![
                support::claim_id(21).to_string(),
                support::claim_id(22).to_string(),
                support::claim_id(99).to_string(),
            ],
            ObservedPresentation::Established,
            Vec::new(),
        )])]);
    let result = evaluate_case(
        &case("mixed-risks"),
        &EvaluationRun::new(records, support::metadata()).with_synthesis(observation),
    );

    assert_eq!(
        result.synthesis().assertions_with_valid_claims().matched(),
        1
    );
    assert_eq!(result.synthesis().invalid_claim_references(), 1);
    assert_eq!(result.synthesis().insufficient_as_facts(), 1);
    assert_eq!(result.synthesis().contradictions_rendered_settled(), 1);
    assert_eq!(result.synthesis().qualification_mismatches(), 1);
}

#[test]
fn insufficient_and_contradictory_claims_cannot_score_as_settled() {
    let insufficient = support::stopped_gap_history(support::blocked_reason());
    let insufficient_observation = established_observation();
    let insufficient_result = evaluate_case(
        &case("insufficient"),
        &EvaluationRun::new(insufficient, support::metadata())
            .with_synthesis(insufficient_observation),
    );
    assert_eq!(insufficient_result.synthesis().insufficient_as_facts(), 1);

    let contradictory_result = evaluate_case(
        &case("contradictory"),
        &EvaluationRun::new(support::contradictory_history(), support::metadata())
            .with_synthesis(established_observation()),
    );
    assert_eq!(
        contradictory_result
            .synthesis()
            .contradictions_rendered_settled(),
        1
    );
}

#[test]
fn semantic_adjudication_stays_separate_from_structural_claim_references() {
    let fixture = support::supported_fixture();
    let observation = established_observation();
    let adjudication = SemanticAdjudication::new(
        AssertionLocation::new(0, 0),
        SemanticGrounding::Unsupported,
        AdjudicationOrigin::LabelledFixture,
    );
    let run = EvaluationRun::new(fixture.records, support::metadata())
        .with_synthesis(observation)
        .with_semantic_adjudication(adjudication)
        .expect("location is unique");

    let result = evaluate_case(&case("semantic"), &run);

    assert_eq!(
        result.synthesis().assertions_with_valid_claims().matched(),
        1
    );
    assert_eq!(result.synthesis().semantic().fixture().total(), 1);
    assert_eq!(result.synthesis().semantic().fixture().matched(), 0);
    assert_eq!(result.synthesis().semantic().fixture_unsupported(), 1);
    assert_eq!(result.synthesis().semantic().model_judged().total(), 0);
}

#[test]
fn fixture_and_model_judgments_share_a_location_without_hiding_invalid_locations() {
    let fixture = support::supported_fixture();
    let run = EvaluationRun::new(fixture.records, support::metadata())
        .with_synthesis(established_observation())
        .with_semantic_adjudication(SemanticAdjudication::new(
            AssertionLocation::new(0, 0),
            SemanticGrounding::Faithful,
            AdjudicationOrigin::LabelledFixture,
        ))
        .unwrap()
        .with_semantic_adjudication(SemanticAdjudication::new(
            AssertionLocation::new(0, 0),
            SemanticGrounding::Unsupported,
            AdjudicationOrigin::ModelJudge(
                JudgeMetadata::new(
                    "openai".to_owned(),
                    "judge".to_owned(),
                    "v1".to_owned(),
                    "strict".to_owned(),
                )
                .unwrap(),
            ),
        ))
        .unwrap()
        .with_semantic_adjudication(SemanticAdjudication::new(
            AssertionLocation::new(0, 9),
            SemanticGrounding::Faithful,
            AdjudicationOrigin::LabelledFixture,
        ))
        .unwrap();

    let result = evaluate_case(&case("mixed-judges"), &run);
    let semantic = result.synthesis().semantic();

    assert_eq!(semantic.fixture().matched(), 1);
    assert_eq!(semantic.fixture().total(), 1);
    assert_eq!(semantic.model_judged().matched(), 0);
    assert_eq!(semantic.model_judged().total(), 1);
    assert_eq!(semantic.unjudged_assertions(), 0);
    assert_eq!(semantic.invalid_adjudications(), 1);
    assert_eq!(semantic.judge_metadata().len(), 1);
}

#[test]
fn one_result_accepts_only_one_model_judgment_per_assertion() {
    let fixture = support::supported_fixture();
    let run = EvaluationRun::new(fixture.records, support::metadata())
        .with_synthesis(established_observation())
        .with_semantic_adjudication(SemanticAdjudication::new(
            AssertionLocation::new(0, 0),
            SemanticGrounding::Faithful,
            AdjudicationOrigin::ModelJudge(
                JudgeMetadata::new(
                    "openai".to_owned(),
                    "judge-a".to_owned(),
                    "v1".to_owned(),
                    "strict".to_owned(),
                )
                .unwrap(),
            ),
        ))
        .unwrap();

    let duplicate = run.with_semantic_adjudication(SemanticAdjudication::new(
        AssertionLocation::new(0, 0),
        SemanticGrounding::Unsupported,
        AdjudicationOrigin::ModelJudge(
            JudgeMetadata::new(
                "openai".to_owned(),
                "judge-b".to_owned(),
                "v1".to_owned(),
                "strict".to_owned(),
            )
            .unwrap(),
        ),
    ));

    assert!(duplicate.is_err());
}

#[test]
fn repeated_valid_evidence_citations_are_counted_after_the_first_use() {
    let fixture = support::supported_fixture();
    let digest = support::content_digest(&fixture.content)
        .as_sha256()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let citation = ObservedCitation::new(
        support::claim_id(1).to_string(),
        fixture.evidence_id.to_string(),
        fixture.source_id.to_string(),
        digest,
    );
    let observation =
        SynthesisObservation::new(vec![ObservedSection::new(vec![ObservedAssertion::new(
            "AURORA preserves evidence.".to_owned(),
            vec![support::claim_id(1).to_string()],
            ObservedPresentation::Established,
            vec![citation.clone(), citation],
        )])]);
    let result = evaluate_case(
        &case("repeated-citation"),
        &EvaluationRun::new(fixture.records, support::metadata()).with_synthesis(observation),
    );

    assert_eq!(result.synthesis().citation_resolution().matched(), 2);
    assert_eq!(result.synthesis().repeated_evidence_citations(), 1);
}

fn established_observation() -> SynthesisObservation {
    SynthesisObservation::new(vec![ObservedSection::new(vec![ObservedAssertion::new(
        "AURORA preserves evidence.".to_owned(),
        vec![support::claim_id(1).to_string()],
        ObservedPresentation::Established,
        Vec::new(),
    )])])
}

fn case(id: &str) -> EvaluationCase {
    EvaluationCase::new(
        EvaluationCaseId::new(id).expect("case id is valid"),
        "Question".to_owned(),
        Vec::new(),
        Vec::new(),
        None,
        None,
    )
    .expect("case is valid")
}
