mod support;

use aurora_eval::{
    EvaluationCase, EvaluationCaseId, EvaluationRun, ExpectedTerminalOutcome,
    SourceSnapshotFixture, evaluate_case,
};

#[test]
fn exact_source_fixtures_audit_grounding_without_mutating_history() {
    let fixture = support::supported_fixture();
    let case = EvaluationCase::new(
        EvaluationCaseId::new("grounded").expect("case id is valid"),
        "Does AURORA preserve evidence?".to_owned(),
        vec![
            SourceSnapshotFixture::new(fixture.source_id.to_string(), fixture.content.clone())
                .expect("snapshot is valid"),
        ],
        Vec::new(),
        Some(ExpectedTerminalOutcome::Completed),
        Some(0),
    )
    .expect("case is valid");
    let run = EvaluationRun::new(fixture.records, support::metadata());
    let before = run.records().to_vec();

    let result = evaluate_case(&case, &run);

    assert!(result.guarantees().history_reconstructed());
    assert!(result.guarantees().references_valid());
    assert!(result.guarantees().terminal_is_explicit());
    assert_eq!(result.grounding().exact_excerpts().matched(), 2);
    assert_eq!(result.grounding().exact_excerpts().total(), 2);
    assert_eq!(result.grounding().digest_matches().matched(), 1);
    assert_eq!(result.grounding().digest_matches().total(), 1);
    assert_eq!(result.grounding().missing_source_fixtures(), 0);
    assert_eq!(run.records(), before);
}

#[test]
fn missing_or_changed_source_fixture_never_receives_grounding_credit() {
    let fixture = support::supported_fixture();
    let missing_case = EvaluationCase::new(
        EvaluationCaseId::new("missing").expect("case id is valid"),
        "Question".to_owned(),
        Vec::new(),
        Vec::new(),
        None,
        None,
    )
    .expect("case is valid");
    let missing = evaluate_case(
        &missing_case,
        &EvaluationRun::new(fixture.records.clone(), support::metadata()),
    );
    assert_eq!(missing.grounding().exact_excerpts().matched(), 0);
    assert_eq!(missing.grounding().exact_excerpts().total(), 2);
    assert_eq!(missing.grounding().missing_source_fixtures(), 1);

    let changed_case = EvaluationCase::new(
        EvaluationCaseId::new("changed").expect("case id is valid"),
        "Question".to_owned(),
        vec![
            SourceSnapshotFixture::new(fixture.source_id.to_string(), "changed".to_owned())
                .expect("snapshot is valid"),
        ],
        Vec::new(),
        None,
        None,
    )
    .expect("case is valid");
    let changed = evaluate_case(
        &changed_case,
        &EvaluationRun::new(fixture.records, support::metadata()),
    );
    assert_eq!(changed.grounding().exact_excerpts().matched(), 0);
    assert_eq!(changed.grounding().digest_matches().matched(), 0);
}
