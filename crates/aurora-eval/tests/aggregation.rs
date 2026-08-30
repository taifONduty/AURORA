mod support;

use aurora_eval::{
    DistributionSummary, EvaluationReport, EvaluationReportError, EvaluationRun, ExecutionFailure,
    ExpectedTerminalOutcome, ObservedUsage, ProviderCost, decode_report, encode_report,
    evaluate_case,
};

#[test]
fn distribution_uses_population_standard_deviation() {
    let odd = DistributionSummary::from_values(&[1, 2, 3]);
    assert_eq!(odd.count(), 3);
    assert_eq!(odd.mean(), Some(2.0));
    assert_eq!(odd.median(), Some(2.0));
    assert_eq!(
        odd.population_standard_deviation(),
        Some((2.0_f64 / 3.0).sqrt())
    );

    let even = DistributionSummary::from_values(&[1, 2, 3, 4]);
    assert_eq!(even.mean(), Some(2.5));
    assert_eq!(even.median(), Some(2.5));
    assert_eq!(even.population_standard_deviation(), Some(1.25_f64.sqrt()));
}

#[test]
fn report_retains_sorted_cases_and_aggregates_outcomes_failures_and_work() {
    let supported = support::supported_fixture();
    let completed = evaluate_case(
        &support::case("z-completed", ExpectedTerminalOutcome::Completed, 0),
        &EvaluationRun::new(supported.records, support::metadata()).with_usage(ObservedUsage::new(
            Some(1),
            Some(2),
            Some(10),
            Some(5),
            Some(100),
            Some(ProviderCost::new("USD".to_owned(), 100).unwrap()),
        )),
    );
    let blocked = evaluate_case(
        &support::case("a-blocked", ExpectedTerminalOutcome::Blocked, 0),
        &EvaluationRun::new(
            support::stopped_gap_history(support::blocked_reason()),
            support::metadata(),
        )
        .with_usage(ObservedUsage::new(
            Some(2),
            Some(1),
            Some(20),
            None,
            Some(300),
            Some(ProviderCost::new("USD".to_owned(), 300).unwrap()),
        )),
    );

    let report = EvaluationReport::new(vec![completed, blocked]).unwrap();

    assert_eq!(report.cases()[0].case_id().as_str(), "a-blocked");
    assert_eq!(report.aggregate().total_cases(), 2);
    assert_eq!(report.aggregate().completed_cases(), 1);
    assert_eq!(report.aggregate().blocked_cases(), 1);
    assert_eq!(
        report
            .aggregate()
            .failure_count(ExecutionFailure::Retrieval),
        0
    );
    assert_eq!(report.aggregate().usage().model_invocations().total(), 3);
    assert_eq!(report.aggregate().usage().retrieval_calls().total(), 3);
    assert_eq!(report.aggregate().usage().input_tokens().total(), 30);
    assert_eq!(report.aggregate().usage().output_tokens().observations(), 1);
    assert_eq!(report.aggregate().usage().provider_costs()[0].micros(), 400);
    assert_eq!(
        report.aggregate().usage().provider_costs()[0].observations(),
        2
    );
    assert_eq!(
        report.aggregate().usage().provider_costs()[0]
            .distribution()
            .mean(),
        Some(200.0)
    );
    assert_eq!(
        report.aggregate().usage().provider_costs()[0]
            .distribution()
            .median(),
        Some(200.0)
    );
    assert_eq!(
        report.aggregate().usage().provider_costs()[0]
            .distribution()
            .population_standard_deviation(),
        Some(100.0)
    );
    assert_eq!(report.aggregate().duration_millis().mean(), Some(200.0));
    assert_eq!(report.aggregate().investigation_tasks().count(), 2);
}

#[test]
fn invalid_history_is_counted_but_excluded_from_derived_distributions() {
    let valid_fixture = support::supported_fixture();
    let valid = evaluate_case(
        &support::case("valid-counts", ExpectedTerminalOutcome::Completed, 0),
        &EvaluationRun::new(valid_fixture.records, support::metadata()),
    );
    let mut invalid_fixture = support::supported_fixture();
    invalid_fixture.records.remove(0);
    let invalid = evaluate_case(
        &support::case("invalid-counts", ExpectedTerminalOutcome::Failed, 0),
        &EvaluationRun::new(invalid_fixture.records, support::metadata()),
    );

    let report = EvaluationReport::new(vec![valid, invalid]).unwrap();

    assert_eq!(report.aggregate().invalid_research_histories(), 1);
    assert_eq!(report.aggregate().sources().count(), 1);
    assert_eq!(report.aggregate().evidence_items().count(), 1);
    assert_eq!(report.aggregate().claims().count(), 1);
    assert_eq!(report.aggregate().investigation_tasks().count(), 1);
}

#[test]
fn aggregate_overflow_is_an_explicit_error() {
    let first_fixture = support::supported_fixture();
    let second_fixture = support::supported_fixture();
    let usage = ObservedUsage::new(Some(u64::MAX), None, None, None, None, None);
    let first = evaluate_case(
        &support::case("overflow-a", ExpectedTerminalOutcome::Completed, 0),
        &EvaluationRun::new(first_fixture.records, support::metadata()).with_usage(usage.clone()),
    );
    let second = evaluate_case(
        &support::case("overflow-b", ExpectedTerminalOutcome::Completed, 0),
        &EvaluationRun::new(second_fixture.records, support::metadata()).with_usage(usage),
    );

    assert_eq!(
        EvaluationReport::new(vec![first, second]),
        Err(EvaluationReportError::MetricOverflow)
    );
}

#[test]
fn large_provider_costs_encode_with_their_recomputed_distribution() {
    let first_fixture = support::supported_fixture();
    let second_fixture = support::supported_fixture();
    let first = evaluate_case(
        &support::case("cost-boundary-a", ExpectedTerminalOutcome::Completed, 0),
        &EvaluationRun::new(first_fixture.records, support::metadata()).with_usage(
            ObservedUsage::new(
                None,
                None,
                None,
                None,
                None,
                Some(ProviderCost::new("USD".to_owned(), 1).unwrap()),
            ),
        ),
    );
    let second = evaluate_case(
        &support::case("cost-boundary-b", ExpectedTerminalOutcome::Completed, 0),
        &EvaluationRun::new(second_fixture.records, support::metadata()).with_usage(
            ObservedUsage::new(
                None,
                None,
                None,
                None,
                None,
                Some(ProviderCost::new("USD".to_owned(), 9_007_199_254_740_993).unwrap()),
            ),
        ),
    );

    let report = EvaluationReport::new(vec![first, second]).unwrap();
    let distribution = report.aggregate().usage().provider_costs()[0].distribution();
    assert_eq!(distribution.mean(), Some(4_503_599_627_370_497.0));
    assert_eq!(distribution.median(), Some(4_503_599_627_370_497.0));
    assert_eq!(
        distribution.population_standard_deviation(),
        Some(4_503_599_627_370_496.0)
    );
    let encoded = encode_report(&report).unwrap();

    assert_eq!(decode_report(&encoded).unwrap(), report);
}

#[test]
fn provider_cost_overflow_is_an_explicit_error() {
    let first_fixture = support::supported_fixture();
    let second_fixture = support::supported_fixture();
    let first = evaluate_case(
        &support::case("cost-overflow-a", ExpectedTerminalOutcome::Completed, 0),
        &EvaluationRun::new(first_fixture.records, support::metadata()).with_usage(
            ObservedUsage::new(
                None,
                None,
                None,
                None,
                None,
                Some(ProviderCost::new("USD".to_owned(), u64::MAX).unwrap()),
            ),
        ),
    );
    let second = evaluate_case(
        &support::case("cost-overflow-b", ExpectedTerminalOutcome::Completed, 0),
        &EvaluationRun::new(second_fixture.records, support::metadata()).with_usage(
            ObservedUsage::new(
                None,
                None,
                None,
                None,
                None,
                Some(ProviderCost::new("USD".to_owned(), 1).unwrap()),
            ),
        ),
    );

    assert_eq!(
        EvaluationReport::new(vec![first, second]),
        Err(EvaluationReportError::MetricOverflow)
    );
}

#[test]
fn report_rejects_duplicate_cases_and_tampered_aggregates() {
    let fixture = support::supported_fixture();
    let case = support::case("duplicate", ExpectedTerminalOutcome::Completed, 0);
    let result = evaluate_case(
        &case,
        &EvaluationRun::new(fixture.records, support::metadata()),
    );
    assert!(EvaluationReport::new(vec![result.clone(), result.clone()]).is_err());

    let report = EvaluationReport::new(vec![result]).unwrap();
    let encoded = encode_report(&report).unwrap();
    assert_eq!(decode_report(&encoded).unwrap(), report);
    assert_eq!(encode_report(&report).unwrap(), encoded);

    let mut tampered: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    tampered["aggregate"]["total_cases"] = serde_json::json!(99);
    assert!(decode_report(&serde_json::to_vec(&tampered).unwrap()).is_err());
}
