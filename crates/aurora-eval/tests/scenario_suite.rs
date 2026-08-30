mod support;

use aurora_eval::{
    AdjudicationOrigin, AssertionLocation, EvaluationCase, EvaluationCaseId, EvaluationLabelId,
    EvaluationReport, EvaluationRun, EvidenceBinding, EvidenceKey, ExecutionFailure,
    ExpectedEvidenceRelation, ExpectedRelation, ExpectedSufficiency, ExpectedTerminalOutcome,
    SemanticAdjudication, SemanticGrounding, SourceSnapshotFixture, SynthesisObservation,
    VerificationBinding, VerificationExpectation, encode_report, evaluate_case,
};
use aurora_research::{
    ClaimId, GroundedReport, ResearchControlRecord, ResearchControlState, SynthesisAssertionDraft,
    SynthesisBasis, SynthesisDraft, SynthesisSectionDraft,
};

#[test]
fn seven_case_suite_exercises_the_frozen_research_path() {
    let supported = support::supported_fixture();
    let supported_observation = report_observation(&supported.records, &[support::claim_id(1)]);
    let supported_case = labelled_case(
        "supported",
        vec![snapshot(supported.source_id, &supported.content)],
        ExpectedTerminalOutcome::Completed,
        0,
        ExpectedSufficiency::Sufficient,
        vec![("primary", ExpectedRelation::Supports)],
    );
    let supported_run = bind(
        EvaluationRun::new(supported.records, support::metadata())
            .with_synthesis(supported_observation),
        support::verification_id(1),
        vec![("primary", support::evidence_id(2))],
    );

    let conflict_fixture = support::supported_fixture();
    let conflict_records = support::mixed_verification_history();
    let conflict_observation = report_observation(&conflict_records, &[support::claim_id(1)]);
    let conflict_case = labelled_case(
        "conflicting",
        vec![snapshot(
            conflict_fixture.source_id,
            &conflict_fixture.content,
        )],
        ExpectedTerminalOutcome::Blocked,
        0,
        ExpectedSufficiency::Sufficient,
        vec![
            ("support", ExpectedRelation::Supports),
            ("contradiction", ExpectedRelation::Contradicts),
        ],
    );
    let conflict_run = bind(
        EvaluationRun::new(conflict_records, support::metadata())
            .with_synthesis(conflict_observation),
        support::verification_id(1),
        vec![
            ("support", support::evidence_id(1)),
            ("contradiction", support::evidence_id(2)),
        ],
    );

    let adaptive_fixture = support::supported_fixture();
    let adaptive_records = support::adaptive_history();
    let adaptive_observation = report_observation(&adaptive_records, &[support::claim_id(1)]);
    let adaptive_case = case(
        "follow-up-resolved",
        vec![snapshot(
            adaptive_fixture.source_id,
            &adaptive_fixture.content,
        )],
        ExpectedTerminalOutcome::Completed,
        1,
    );
    let adaptive_run = EvaluationRun::new(adaptive_records, support::metadata())
        .with_synthesis(adaptive_observation);

    let blocked_fixture = support::supported_fixture();
    let blocked_records = support::stopped_gap_history(support::blocked_reason());
    let blocked_observation = report_observation(&blocked_records, &[support::claim_id(1)]);
    let blocked_case = case(
        "blocked",
        vec![snapshot(
            blocked_fixture.source_id,
            &blocked_fixture.content,
        )],
        ExpectedTerminalOutcome::Blocked,
        0,
    );
    let blocked_run = EvaluationRun::new(blocked_records, support::metadata())
        .with_synthesis(blocked_observation);

    let multi = support::multi_source_fixture();
    let multi_observation = report_observation(
        &multi.records,
        &[support::claim_id(21), support::claim_id(22)],
    );
    let multi_case = case(
        "multiple-sources",
        vec![
            snapshot(multi.first_source_id, &multi.first_content),
            snapshot(multi.second_source_id, &multi.second_content),
        ],
        ExpectedTerminalOutcome::Completed,
        0,
    );
    let multi_run =
        EvaluationRun::new(multi.records, support::metadata()).with_synthesis(multi_observation);

    let unsupported = support::supported_fixture();
    let unsupported_observation = report_observation(&unsupported.records, &[support::claim_id(1)]);
    let unsupported_case = case(
        "tempting-unsupported",
        vec![snapshot(unsupported.source_id, &unsupported.content)],
        ExpectedTerminalOutcome::Completed,
        0,
    );
    let unsupported_run = EvaluationRun::new(unsupported.records, support::metadata())
        .with_synthesis(unsupported_observation)
        .with_semantic_adjudication(SemanticAdjudication::new(
            AssertionLocation::new(0, 0),
            SemanticGrounding::Unsupported,
            AdjudicationOrigin::LabelledFixture,
        ))
        .unwrap();

    let retrieval_case = case(
        "retrieval-failure",
        Vec::new(),
        ExpectedTerminalOutcome::Failed,
        0,
    );
    let retrieval_run =
        EvaluationRun::new(support::retrieval_failure_history(), support::metadata())
            .with_failure(ExecutionFailure::Retrieval)
            .unwrap();

    let inputs = [
        (&supported_case, &supported_run),
        (&conflict_case, &conflict_run),
        (&adaptive_case, &adaptive_run),
        (&blocked_case, &blocked_run),
        (&multi_case, &multi_run),
        (&unsupported_case, &unsupported_run),
        (&retrieval_case, &retrieval_run),
    ];
    let results = inputs
        .into_iter()
        .map(|(case, run)| evaluate_case(case, run))
        .collect::<Vec<_>>();
    let report = EvaluationReport::new(results).unwrap();

    assert_eq!(report.aggregate().total_cases(), 7);
    assert_eq!(report.aggregate().completed_cases(), 4);
    assert_eq!(report.aggregate().blocked_cases(), 2);
    assert_eq!(
        report
            .aggregate()
            .terminal_count(aurora_eval::ObservedTerminalOutcome::Failed),
        1
    );
    assert_eq!(
        report
            .aggregate()
            .failure_count(ExecutionFailure::Retrieval),
        1
    );
    assert_eq!(report.aggregate().false_completions(), 0);
    assert_eq!(report.aggregate().relation_accuracy().matched(), 3);
    assert_eq!(report.aggregate().relation_accuracy().total(), 3);
    assert_eq!(
        report
            .aggregate()
            .verification()
            .relations()
            .supports()
            .true_positive(),
        2
    );
    assert_eq!(
        report
            .aggregate()
            .verification()
            .relations()
            .contradicts()
            .true_positive(),
        1
    );
    assert_eq!(
        report
            .cases()
            .iter()
            .find(|result| result.case_id().as_str() == "tempting-unsupported")
            .unwrap()
            .synthesis()
            .semantic()
            .fixture_unsupported(),
        1
    );

    let first = encode_report(&report).unwrap();
    let repeated = EvaluationReport::new(
        inputs
            .into_iter()
            .map(|(case, run)| evaluate_case(case, run))
            .collect(),
    )
    .unwrap();
    assert_eq!(encode_report(&repeated).unwrap(), first);
}

fn snapshot(id: aurora_research::SourceId, content: &str) -> SourceSnapshotFixture {
    SourceSnapshotFixture::new(id.to_string(), content.to_owned()).unwrap()
}

fn case(
    id: &str,
    snapshots: Vec<SourceSnapshotFixture>,
    terminal: ExpectedTerminalOutcome,
    follow_ups: u32,
) -> EvaluationCase {
    EvaluationCase::new(
        EvaluationCaseId::new(id).unwrap(),
        "What does the evidence establish?".to_owned(),
        snapshots,
        Vec::new(),
        Some(terminal),
        Some(follow_ups),
    )
    .unwrap()
}

fn labelled_case(
    id: &str,
    snapshots: Vec<SourceSnapshotFixture>,
    terminal: ExpectedTerminalOutcome,
    follow_ups: u32,
    sufficiency: ExpectedSufficiency,
    relations: Vec<(&str, ExpectedRelation)>,
) -> EvaluationCase {
    let relations = relations
        .into_iter()
        .map(|(key, relation)| {
            ExpectedEvidenceRelation::new(EvidenceKey::new(key).unwrap(), relation)
        })
        .collect();
    EvaluationCase::new(
        EvaluationCaseId::new(id).unwrap(),
        "What does the evidence establish?".to_owned(),
        snapshots,
        vec![
            VerificationExpectation::new(
                EvaluationLabelId::new("assessment").unwrap(),
                sufficiency,
                relations,
            )
            .unwrap(),
        ],
        Some(terminal),
        Some(follow_ups),
    )
    .unwrap()
}

fn bind(
    run: EvaluationRun,
    verification_id: aurora_research::VerificationId,
    evidence: Vec<(&str, aurora_research::EvidenceId)>,
) -> EvaluationRun {
    let evidence = evidence
        .into_iter()
        .map(|(key, id)| {
            EvidenceBinding::new(EvidenceKey::new(key).unwrap(), id.to_string()).unwrap()
        })
        .collect();
    run.with_verification_binding(
        VerificationBinding::new(
            EvaluationLabelId::new("assessment").unwrap(),
            verification_id.to_string(),
            evidence,
        )
        .unwrap(),
    )
    .unwrap()
}

fn report_observation(
    records: &[ResearchControlRecord],
    claims: &[ClaimId],
) -> SynthesisObservation {
    let state = ResearchControlState::reconstruct(records.to_vec())
        .unwrap_or_else(|error| panic!("{} records did not reconstruct: {error}", records.len()));
    let basis = SynthesisBasis::from_state(&state).unwrap();
    let assertions = claims
        .iter()
        .map(|claim_id| {
            SynthesisAssertionDraft::new(
                format!("Finding grounded by claim {claim_id}."),
                vec![*claim_id],
            )
            .unwrap()
        })
        .collect();
    let report = GroundedReport::from_basis(
        &basis,
        SynthesisDraft::new(vec![SynthesisSectionDraft::new(assertions).unwrap()]).unwrap(),
    )
    .unwrap();
    SynthesisObservation::from_report(&report)
}
