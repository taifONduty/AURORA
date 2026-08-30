mod support;

use aurora_eval::{
    EvaluationCase, EvaluationCaseId, EvaluationRun, ExpectedTerminalOutcome,
    ObservedTerminalOutcome, evaluate_case,
};
use aurora_research::ResearchStopReason;

#[test]
fn resolved_follow_up_path_measures_explicit_adaptation_and_completion() {
    let case = labelled_case("adaptive", ExpectedTerminalOutcome::Completed, Some(1));
    let result = evaluate_case(
        &case,
        &EvaluationRun::new(support::adaptive_history(), support::metadata()),
    );
    let adaptive = result.adaptive();

    assert_eq!(result.terminal(), ObservedTerminalOutcome::Completed);
    assert_eq!(adaptive.expected_terminal_match(), Some(true));
    assert_eq!(adaptive.false_completion_count(), 0);
    assert_eq!(adaptive.initial_tasks(), 1);
    assert_eq!(adaptive.follow_up_tasks(), 1);
    assert_eq!(adaptive.resolved_gaps(), 1);
    assert_eq!(adaptive.open_gaps(), 0);
    assert_eq!(adaptive.open_gaps_without_follow_up(), 0);
    assert_eq!(adaptive.repeated_follow_up_objectives(), 0);
    assert_eq!(adaptive.cyclic_follow_up_lineages(), 0);
    assert_eq!(adaptive.excess_follow_up_tasks(), Some(0));
    assert_eq!(adaptive.gap_resolution_steps(), &[5]);
}

#[test]
fn blocked_and_exhausted_outcomes_remain_distinct_and_unresolved() {
    let blocked = evaluate_case(
        &labelled_case("blocked", ExpectedTerminalOutcome::Blocked, None),
        &EvaluationRun::new(
            support::stopped_gap_history(support::blocked_reason()),
            support::metadata(),
        ),
    );
    let exhausted = evaluate_case(
        &labelled_case("exhausted", ExpectedTerminalOutcome::BudgetExhausted, None),
        &EvaluationRun::new(
            support::stopped_gap_history(ResearchStopReason::BudgetExhausted),
            support::metadata(),
        ),
    );

    assert_eq!(blocked.terminal(), ObservedTerminalOutcome::Blocked);
    assert_eq!(
        exhausted.terminal(),
        ObservedTerminalOutcome::BudgetExhausted
    );
    assert_eq!(blocked.adaptive().open_gaps(), 1);
    assert_eq!(exhausted.adaptive().open_gaps(), 1);
    assert_eq!(blocked.adaptive().expected_terminal_match(), Some(true));
    assert_eq!(exhausted.adaptive().expected_terminal_match(), Some(true));
}

#[test]
fn completion_against_noncompletion_label_is_a_false_completion() {
    let fixture = support::supported_fixture();
    let result = evaluate_case(
        &labelled_case(
            "false-completion",
            ExpectedTerminalOutcome::Blocked,
            Some(0),
        ),
        &EvaluationRun::new(fixture.records, support::metadata()),
    );

    assert_eq!(result.adaptive().expected_terminal_match(), Some(false));
    assert_eq!(result.adaptive().false_completion_count(), 1);
}

#[test]
fn exact_repeated_follow_up_objectives_are_measured() {
    let result = evaluate_case(
        &labelled_case(
            "repeated-follow-up",
            ExpectedTerminalOutcome::BudgetExhausted,
            Some(2),
        ),
        &EvaluationRun::new(support::repeated_follow_up_history(), support::metadata()),
    );

    assert_eq!(result.adaptive().follow_up_tasks(), 2);
    assert_eq!(result.adaptive().repeated_follow_up_objectives(), 1);
    assert_eq!(result.adaptive().cyclic_follow_up_lineages(), 0);
    assert_eq!(result.terminal(), ObservedTerminalOutcome::BudgetExhausted);
}

fn labelled_case(
    id: &str,
    expected: ExpectedTerminalOutcome,
    expected_follow_ups: Option<u32>,
) -> EvaluationCase {
    EvaluationCase::new(
        EvaluationCaseId::new(id).expect("case id is valid"),
        "Question".to_owned(),
        Vec::new(),
        Vec::new(),
        Some(expected),
        expected_follow_ups,
    )
    .expect("case is valid")
}
