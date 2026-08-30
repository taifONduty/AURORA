mod support;

use aurora_eval::{
    AdjudicationOrigin, AssertionLocation, CaseEvaluationResult, ClassMetric, DistributionSummary,
    EvaluationCase, EvaluationCaseId, EvaluationLabelId, EvaluationReport, EvaluationRun,
    EvidenceBinding, EvidenceKey, ExecutionFailure, ExpectedEvidenceRelation, ExpectedRelation,
    ExpectedSufficiency, ExpectedTerminalOutcome, JudgeMetadata, MetricCount, ObservedAssertion,
    ObservedCitation, ObservedPresentation, ObservedSection, ObservedTerminalOutcome,
    ObservedUsage, ProviderCost, SemanticAdjudication, SemanticGrounding, SourceSnapshotFixture,
    SynthesisObservation, VerificationBinding, VerificationExpectation, decode_case,
    decode_case_result, decode_report, encode_case, encode_case_result, encode_report,
    evaluate_case,
};
use proptest::prelude::*;

type GeneratedInput = (bool, bool, bool, bool, bool, u16, u16, u16, u16);

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn distributions_are_finite_and_permutation_invariant(mut values in prop::collection::vec(0_u64..1_000_000, 0..32)) {
        let first = DistributionSummary::from_values(&values);
        values.reverse();
        let second = DistributionSummary::from_values(&values);
        prop_assert_eq!(&first, &second);
        for value in [first.mean(), first.median(), first.population_standard_deviation()].into_iter().flatten() {
            prop_assert!(value.is_finite());
            prop_assert!(value >= 0.0);
        }
    }


    #[test]
    fn valid_cases_round_trip_canonically(
        id in "[a-z][a-z0-9-]{0,30}",
        question in "[A-Za-z0-9][A-Za-z0-9 ?]{0,79}",
        content in "[A-Za-z0-9][A-Za-z0-9 .]{0,99}",
    ) {
        let case = EvaluationCase::new(
            EvaluationCaseId::new(id).unwrap(),
            question,
            vec![SourceSnapshotFixture::new(support::source_id(99).to_string(), content).unwrap()],
            Vec::new(),
            Some(ExpectedTerminalOutcome::Completed),
            Some(0),
        ).unwrap();
        let encoded = encode_case(&case).unwrap();
        let decoded = decode_case(&encoded).unwrap();
        prop_assert_eq!(&decoded, &case);
        prop_assert_eq!(encode_case(&decoded).unwrap(), encoded);
    }

    #[test]
    fn generated_usage_aggregates_exactly_and_rates_remain_bounded(
        first_invocations in any::<u32>(),
        second_invocations in any::<u32>(),
    ) {
        let first_fixture = support::supported_fixture();
        let second_fixture = support::supported_fixture();
        let first = evaluate_case(
            &support::case("generated-a", ExpectedTerminalOutcome::Completed, 0),
            &EvaluationRun::new(first_fixture.records, support::metadata()).with_usage(
                ObservedUsage::new(Some(u64::from(first_invocations)), None, None, None, None, None),
            ),
        );
        let second = evaluate_case(
            &support::case("generated-b", ExpectedTerminalOutcome::Completed, 0),
            &EvaluationRun::new(second_fixture.records, support::metadata()).with_usage(
                ObservedUsage::new(Some(u64::from(second_invocations)), None, None, None, None, None),
            ),
        );

        let report = EvaluationReport::new(vec![first, second]).unwrap();
        let total = u64::from(first_invocations) + u64::from(second_invocations);
        prop_assert_eq!(report.aggregate().usage().model_invocations().total(), total);
        prop_assert_eq!(report.aggregate().usage().model_invocations().observations(), 2);
        for rate in [
            report.aggregate().grounding().exact_excerpts().rate(),
            report.aggregate().grounding().source_attributions().rate(),
            report.aggregate().grounding().digest_matches().rate(),
        ]
        .into_iter()
        .flatten()
        {
            prop_assert!(rate.is_finite());
            prop_assert!((0.0..=1.0).contains(&rate));
        }
    }

    #[test]
    fn generated_results_and_reports_preserve_counts_codecs_order_and_rates(
        inputs in prop::collection::vec(
            (
                any::<bool>(),
                any::<bool>(),
                any::<bool>(),
                any::<bool>(),
                any::<bool>(),
                any::<u16>(),
                any::<u16>(),
                any::<u16>(),
                any::<u16>(),
            ),
            1..8,
        ),
    ) {
        let results = inputs
            .iter()
            .enumerate()
            .map(|(index, input)| generated_result(index, input))
            .collect::<Vec<_>>();
        for result in &results {
            let encoded = encode_case_result(result).unwrap();
            let decoded = decode_case_result(&encoded).unwrap();
            prop_assert_eq!(&decoded, result);
            prop_assert_eq!(encode_case_result(&decoded).unwrap(), encoded);
            assert_result_rates(result);
        }

        let report = EvaluationReport::new(results.clone()).unwrap();
        let encoded = encode_report(&report).unwrap();
        prop_assert_eq!(decode_report(&encoded).unwrap(), report.clone());
        prop_assert_eq!(encode_report(&report).unwrap(), encoded);

        let mut reversed = results.clone();
        reversed.reverse();
        prop_assert_eq!(EvaluationReport::new(reversed).unwrap(), report.clone());
        assert_aggregate_conservation(&report, &results);
        assert_aggregate_rates(&report);
    }
}

#[test]
fn result_and_report_round_trip_and_repeated_evaluation_is_identical() {
    let fixture = support::supported_fixture();
    let case = support::case("deterministic", ExpectedTerminalOutcome::Completed, 0);
    let run = EvaluationRun::new(fixture.records, support::metadata());

    let first = evaluate_case(&case, &run);
    let second = evaluate_case(&case, &run);
    assert_eq!(first, second);
    let result_bytes = encode_case_result(&first).unwrap();
    assert_eq!(decode_case_result(&result_bytes).unwrap(), first);

    let report = EvaluationReport::new(vec![second]).unwrap();
    let report_bytes = encode_report(&report).unwrap();
    assert_eq!(decode_report(&report_bytes).unwrap(), report);
}

#[test]
fn report_aggregation_is_invariant_to_case_order() {
    let first_fixture = support::supported_fixture();
    let second_fixture = support::supported_fixture();
    let first = evaluate_case(
        &support::case("a", ExpectedTerminalOutcome::Completed, 0),
        &EvaluationRun::new(first_fixture.records, support::metadata()),
    );
    let second = evaluate_case(
        &support::case("b", ExpectedTerminalOutcome::Completed, 0),
        &EvaluationRun::new(second_fixture.records, support::metadata()),
    );

    let forward = EvaluationReport::new(vec![first.clone(), second.clone()]).unwrap();
    let reversed = EvaluationReport::new(vec![second, first]).unwrap();

    assert_eq!(forward, reversed);
    assert_eq!(
        encode_report(&forward).unwrap(),
        encode_report(&reversed).unwrap()
    );
}

fn generated_result(index: usize, input: &GeneratedInput) -> CaseEvaluationResult {
    let (
        exact_snapshot,
        correct_relation,
        correct_sufficiency,
        expected_completion,
        fixture_judgment,
        model_invocations,
        retrieval_calls,
        input_tokens,
        output_tokens,
    ) = *input;
    let fixture = support::supported_fixture();
    let case_id = EvaluationCaseId::new(format!("generated-{index}")).unwrap();
    let expectation_id = EvaluationLabelId::new("assessment").unwrap();
    let evidence_key = EvidenceKey::new("primary").unwrap();
    let snapshot_content = if exact_snapshot {
        fixture.content.clone()
    } else {
        format!("changed snapshot {index}")
    };
    let case = EvaluationCase::new(
        case_id,
        "Does AURORA preserve evidence?".to_owned(),
        vec![SourceSnapshotFixture::new(fixture.source_id.to_string(), snapshot_content).unwrap()],
        vec![
            VerificationExpectation::new(
                expectation_id.clone(),
                if correct_sufficiency {
                    ExpectedSufficiency::Sufficient
                } else {
                    ExpectedSufficiency::Insufficient
                },
                vec![ExpectedEvidenceRelation::new(
                    evidence_key.clone(),
                    if correct_relation {
                        ExpectedRelation::Supports
                    } else {
                        ExpectedRelation::Contradicts
                    },
                )],
            )
            .unwrap(),
        ],
        Some(if expected_completion {
            ExpectedTerminalOutcome::Completed
        } else {
            ExpectedTerminalOutcome::Blocked
        }),
        Some(0),
    )
    .unwrap();
    let digest = support::content_digest(&fixture.content)
        .as_sha256()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let synthesis =
        SynthesisObservation::new(vec![ObservedSection::new(vec![ObservedAssertion::new(
            "AURORA preserves evidence.".to_owned(),
            vec![support::claim_id(1).to_string()],
            ObservedPresentation::Established,
            vec![ObservedCitation::new(
                support::claim_id(1).to_string(),
                fixture.evidence_id.to_string(),
                fixture.source_id.to_string(),
                digest,
            )],
        )])]);
    let binding = VerificationBinding::new(
        expectation_id,
        fixture.verification_id.to_string(),
        vec![EvidenceBinding::new(evidence_key, fixture.evidence_id.to_string()).unwrap()],
    )
    .unwrap();
    let origin = if fixture_judgment {
        AdjudicationOrigin::LabelledFixture
    } else {
        AdjudicationOrigin::ModelJudge(
            JudgeMetadata::new(
                "fixture-provider".to_owned(),
                "fixture-model".to_owned(),
                "v1".to_owned(),
                "deterministic".to_owned(),
            )
            .unwrap(),
        )
    };
    let run = EvaluationRun::new(fixture.records, support::metadata())
        .with_verification_binding(binding)
        .unwrap()
        .with_synthesis(synthesis)
        .with_semantic_adjudication(SemanticAdjudication::new(
            AssertionLocation::new(0, 0),
            if correct_relation {
                SemanticGrounding::Faithful
            } else {
                SemanticGrounding::Unsupported
            },
            origin,
        ))
        .unwrap()
        .with_usage(ObservedUsage::new(
            Some(u64::from(model_invocations)),
            Some(u64::from(retrieval_calls)),
            Some(u64::from(input_tokens)),
            Some(u64::from(output_tokens)),
            Some(u64::from(input_tokens) + u64::from(output_tokens)),
            Some(ProviderCost::new("USD".to_owned(), u64::from(output_tokens)).unwrap()),
        ));
    evaluate_case(&case, &run)
}

fn assert_result_rates(result: &CaseEvaluationResult) {
    for metric in [
        result.grounding().exact_excerpts(),
        result.grounding().source_attributions(),
        result.grounding().digest_matches(),
        result.verification().relations().accuracy(),
        result.verification().sufficiency().accuracy(),
        result.synthesis().assertions_with_valid_claims(),
        result.synthesis().citation_resolution(),
        result.synthesis().reported_claims_with_citations(),
        result.synthesis().semantic().fixture(),
        result.synthesis().semantic().model_judged(),
    ] {
        assert_metric_rate(metric);
    }
    for metric in relation_classes(result.verification().relations())
        .into_iter()
        .chain(sufficiency_classes(result.verification().sufficiency()))
    {
        assert_class_rates(metric);
    }
    assert_optional_rate(result.verification().contradiction_recall());
}

fn assert_aggregate_rates(report: &EvaluationReport) {
    let aggregate = report.aggregate();
    for metric in [
        aggregate.grounding().exact_excerpts(),
        aggregate.grounding().source_attributions(),
        aggregate.grounding().digest_matches(),
        aggregate.verification().relations().accuracy(),
        aggregate.verification().sufficiency().accuracy(),
        aggregate.synthesis().assertions_with_valid_claims(),
        aggregate.synthesis().citation_resolution(),
        aggregate.synthesis().reported_claims_with_citations(),
        aggregate.synthesis().fixture_semantic_grounding(),
        aggregate.synthesis().model_judged_semantic_grounding(),
    ] {
        assert_metric_rate(metric);
    }
    for metric in relation_classes(aggregate.verification().relations())
        .into_iter()
        .chain(sufficiency_classes(aggregate.verification().sufficiency()))
    {
        assert_class_rates(metric);
    }
    assert_optional_rate(aggregate.verification().contradiction_recall());
}

fn relation_classes(metrics: &aurora_eval::RelationMetrics) -> [&ClassMetric; 4] {
    [
        metrics.supports(),
        metrics.contradicts(),
        metrics.unclear(),
        metrics.irrelevant(),
    ]
}

fn sufficiency_classes(metrics: &aurora_eval::SufficiencyMetrics) -> [&ClassMetric; 3] {
    [
        metrics.sufficient(),
        metrics.insufficient(),
        metrics.indeterminate(),
    ]
}

fn assert_metric_rate(metric: &MetricCount) {
    assert_optional_rate(metric.rate());
}

fn assert_class_rates(metric: &ClassMetric) {
    for rate in [metric.precision(), metric.recall(), metric.f1()] {
        assert_optional_rate(rate);
    }
}

fn assert_optional_rate(rate: Option<f64>) {
    if let Some(rate) = rate {
        assert!(rate.is_finite());
        assert!((0.0..=1.0).contains(&rate));
    }
}

fn assert_aggregate_conservation(report: &EvaluationReport, cases: &[CaseEvaluationResult]) {
    let aggregate = report.aggregate();
    assert_metric_sum(aggregate.grounding().exact_excerpts(), cases, |case| {
        case.grounding().exact_excerpts()
    });
    assert_metric_sum(aggregate.grounding().source_attributions(), cases, |case| {
        case.grounding().source_attributions()
    });
    assert_metric_sum(aggregate.grounding().digest_matches(), cases, |case| {
        case.grounding().digest_matches()
    });
    assert_metric_sum(
        aggregate.verification().relations().accuracy(),
        cases,
        |case| case.verification().relations().accuracy(),
    );
    assert_metric_sum(
        aggregate.verification().sufficiency().accuracy(),
        cases,
        |case| case.verification().sufficiency().accuracy(),
    );
    assert_metric_sum(
        aggregate.synthesis().assertions_with_valid_claims(),
        cases,
        |case| case.synthesis().assertions_with_valid_claims(),
    );
    assert_metric_sum(aggregate.synthesis().citation_resolution(), cases, |case| {
        case.synthesis().citation_resolution()
    });
    assert_metric_sum(
        aggregate.synthesis().reported_claims_with_citations(),
        cases,
        |case| case.synthesis().reported_claims_with_citations(),
    );
    assert_metric_sum(
        aggregate.synthesis().fixture_semantic_grounding(),
        cases,
        |case| case.synthesis().semantic().fixture(),
    );
    assert_metric_sum(
        aggregate.synthesis().model_judged_semantic_grounding(),
        cases,
        |case| case.synthesis().semantic().model_judged(),
    );
    for (aggregate_class, index) in relation_classes(aggregate.verification().relations())
        .into_iter()
        .zip(0..)
    {
        assert_class_sum(aggregate_class, cases, |case| {
            relation_classes(case.verification().relations())[index]
        });
    }
    for (aggregate_class, index) in sufficiency_classes(aggregate.verification().sufficiency())
        .into_iter()
        .zip(0..)
    {
        assert_class_sum(aggregate_class, cases, |case| {
            sufficiency_classes(case.verification().sufficiency())[index]
        });
    }
    assert_eq!(
        aggregate.false_completions(),
        cases
            .iter()
            .map(|case| case.adaptive().false_completion_count())
            .sum::<u64>()
    );
    assert_eq!(
        aggregate.unsupported_as_sufficient(),
        cases
            .iter()
            .map(|case| case.verification().unsupported_as_sufficient())
            .sum::<u64>()
    );
    assert_u64_sum(
        aggregate.grounding().missing_source_fixtures(),
        cases,
        |case| case.grounding().missing_source_fixtures(),
    );
    assert_u64_sum(
        aggregate.verification().relations().missing_predictions(),
        cases,
        |case| case.verification().relations().missing_predictions(),
    );
    assert_u64_sum(
        aggregate.verification().sufficiency().missing_predictions(),
        cases,
        |case| case.verification().sufficiency().missing_predictions(),
    );
    assert_u64_sum(
        aggregate.synthesis().invalid_claim_references(),
        cases,
        |case| case.synthesis().invalid_claim_references(),
    );
    assert_u64_sum(
        aggregate.synthesis().insufficient_as_facts(),
        cases,
        |case| case.synthesis().insufficient_as_facts(),
    );
    assert_u64_sum(
        aggregate.synthesis().contradictions_rendered_settled(),
        cases,
        |case| case.synthesis().contradictions_rendered_settled(),
    );
    assert_u64_sum(
        aggregate.synthesis().qualification_mismatches(),
        cases,
        |case| case.synthesis().qualification_mismatches(),
    );
    assert_u64_sum(
        aggregate.synthesis().repeated_evidence_citations(),
        cases,
        |case| case.synthesis().repeated_evidence_citations(),
    );
    assert_u64_sum(aggregate.synthesis().fixture_unsupported(), cases, |case| {
        case.synthesis().semantic().fixture_unsupported()
    });
    assert_u64_sum(
        aggregate.synthesis().model_judged_unsupported(),
        cases,
        |case| case.synthesis().semantic().model_judged_unsupported(),
    );
    assert_u64_sum(aggregate.synthesis().unjudged_assertions(), cases, |case| {
        case.synthesis().semantic().unjudged_assertions()
    });
    assert_u64_sum(
        aggregate.synthesis().invalid_adjudications(),
        cases,
        |case| case.synthesis().semantic().invalid_adjudications(),
    );
    assert_u64_sum(aggregate.synthesis().blank_assertions(), cases, |case| {
        case.synthesis().blank_assertions()
    });
    assert_eq!(
        aggregate.usage().model_invocations().total(),
        cases
            .iter()
            .filter_map(|case| case.usage()?.model_invocations())
            .sum::<u64>()
    );
    assert_eq!(
        aggregate.usage().retrieval_calls().total(),
        cases
            .iter()
            .filter_map(|case| case.usage()?.retrieval_calls())
            .sum::<u64>()
    );
    assert_eq!(
        aggregate.usage().input_tokens().total(),
        cases
            .iter()
            .filter_map(|case| case.usage()?.input_tokens())
            .sum::<u64>()
    );
    assert_eq!(
        aggregate.usage().output_tokens().total(),
        cases
            .iter()
            .filter_map(|case| case.usage()?.output_tokens())
            .sum::<u64>()
    );
    assert_eq!(aggregate.usage().provider_costs().len(), 1);
    assert_eq!(
        aggregate.usage().provider_costs()[0].micros(),
        cases
            .iter()
            .filter_map(|case| case.usage()?.provider_cost().map(ProviderCost::micros))
            .sum::<u64>()
    );
    assert_eq!(
        aggregate.usage().provider_costs()[0].observations(),
        cases.len() as u64
    );
    for outcome in [
        ObservedTerminalOutcome::NonTerminal,
        ObservedTerminalOutcome::Completed,
        ObservedTerminalOutcome::Failed,
        ObservedTerminalOutcome::OperatorStopped,
        ObservedTerminalOutcome::BudgetExhausted,
        ObservedTerminalOutcome::Blocked,
    ] {
        assert_eq!(
            aggregate.terminal_count(outcome),
            cases
                .iter()
                .filter(|case| case.terminal() == outcome)
                .count() as u64
        );
    }
    for failure in [
        ExecutionFailure::Provider,
        ExecutionFailure::Retrieval,
        ExecutionFailure::MalformedModelProposal,
        ExecutionFailure::DomainInvalidProposal,
        ExecutionFailure::ResearchExecution,
        ExecutionFailure::Synthesis,
        ExecutionFailure::BenchmarkMapping,
        ExecutionFailure::Scoring,
        ExecutionFailure::InvalidResearchHistory,
    ] {
        assert_eq!(
            aggregate.failure_count(failure),
            cases
                .iter()
                .filter(|case| case.failures().contains(&failure))
                .count() as u64
        );
    }
}

fn assert_metric_sum<F>(aggregate: &MetricCount, cases: &[CaseEvaluationResult], select: F)
where
    F: Fn(&CaseEvaluationResult) -> &MetricCount,
{
    assert_eq!(
        aggregate.matched(),
        cases.iter().map(|case| select(case).matched()).sum::<u64>()
    );
    assert_eq!(
        aggregate.total(),
        cases.iter().map(|case| select(case).total()).sum::<u64>()
    );
}

fn assert_class_sum<F>(aggregate: &ClassMetric, cases: &[CaseEvaluationResult], select: F)
where
    F: Fn(&CaseEvaluationResult) -> &ClassMetric,
{
    assert_eq!(
        aggregate.true_positive(),
        cases
            .iter()
            .map(|case| select(case).true_positive())
            .sum::<u64>()
    );
    assert_eq!(
        aggregate.false_positive(),
        cases
            .iter()
            .map(|case| select(case).false_positive())
            .sum::<u64>()
    );
    assert_eq!(
        aggregate.false_negative(),
        cases
            .iter()
            .map(|case| select(case).false_negative())
            .sum::<u64>()
    );
}

fn assert_u64_sum<F>(aggregate: u64, cases: &[CaseEvaluationResult], select: F)
where
    F: Fn(&CaseEvaluationResult) -> u64,
{
    assert_eq!(aggregate, cases.iter().map(select).sum::<u64>());
}
