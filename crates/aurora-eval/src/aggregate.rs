use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    CaseEvaluationResult, ClassMetric, EvidenceGroundingMetrics, ExecutionFailure, MetricCount,
    ObservedTerminalOutcome, RelationMetrics, SufficiencyMetrics, VerificationMetrics,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistributionSummary {
    count: u64,
    mean: Option<f64>,
    median: Option<f64>,
    population_standard_deviation: Option<f64>,
}

impl DistributionSummary {
    pub fn from_values(values: &[u64]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        let count = sorted.len() as u64;
        let sum = sorted.iter().map(|value| u128::from(*value)).sum::<u128>();
        let mean = sum as f64 / count as f64;
        let middle = sorted.len() / 2;
        let median = if sorted.len().is_multiple_of(2) {
            (u128::from(sorted[middle - 1]) + u128::from(sorted[middle])) as f64 / 2.0
        } else {
            sorted[middle] as f64
        };
        let origin = sorted[0];
        let centered_mean = sorted
            .iter()
            .map(|value| (*value - origin) as f64)
            .sum::<f64>()
            / count as f64;
        let variance = sorted
            .iter()
            .map(|value| {
                let delta = (*value - origin) as f64 - centered_mean;
                delta * delta
            })
            .sum::<f64>()
            / count as f64;
        Self {
            count,
            mean: Some(mean),
            median: Some(median),
            population_standard_deviation: Some(variance.sqrt()),
        }
    }

    pub const fn count(&self) -> u64 {
        self.count
    }

    pub const fn mean(&self) -> Option<f64> {
        self.mean
    }

    pub const fn median(&self) -> Option<f64> {
        self.median
    }

    pub const fn population_standard_deviation(&self) -> Option<f64> {
        self.population_standard_deviation
    }

    pub(crate) fn is_valid(&self) -> bool {
        let values = [self.mean, self.median, self.population_standard_deviation];
        if self.count == 0 {
            return values.iter().all(Option::is_none);
        }
        values
            .into_iter()
            .all(|value| value.is_some_and(|value| value.is_finite() && value >= 0.0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalCount {
    outcome: ObservedTerminalOutcome,
    count: u64,
}

impl TerminalCount {
    pub const fn outcome(&self) -> ObservedTerminalOutcome {
        self.outcome
    }

    pub const fn count(&self) -> u64 {
        self.count
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureCount {
    failure: ExecutionFailure,
    count: u64,
}

impl FailureCount {
    pub const fn failure(&self) -> ExecutionFailure {
        self.failure
    }

    pub const fn count(&self) -> u64 {
        self.count
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedTotal {
    observations: u64,
    total: u64,
}

impl ObservedTotal {
    fn record(&mut self, value: Option<u64>) -> Option<()> {
        if let Some(value) = value {
            self.observations = self.observations.checked_add(1)?;
            self.total = self.total.checked_add(value)?;
        }
        Some(())
    }

    pub const fn observations(&self) -> u64 {
        self.observations
    }

    pub const fn total(&self) -> u64 {
        self.total
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrencyTotal {
    currency: String,
    observations: u64,
    micros: u64,
    distribution: DistributionSummary,
}

impl CurrencyTotal {
    pub fn currency(&self) -> &str {
        &self.currency
    }

    pub const fn observations(&self) -> u64 {
        self.observations
    }

    pub const fn micros(&self) -> u64 {
        self.micros
    }

    pub const fn distribution(&self) -> &DistributionSummary {
        &self.distribution
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateUsage {
    cases_with_usage: u64,
    model_invocations: ObservedTotal,
    retrieval_calls: ObservedTotal,
    input_tokens: ObservedTotal,
    output_tokens: ObservedTotal,
    provider_costs: Vec<CurrencyTotal>,
}

impl AggregateUsage {
    pub const fn cases_with_usage(&self) -> u64 {
        self.cases_with_usage
    }

    pub const fn model_invocations(&self) -> &ObservedTotal {
        &self.model_invocations
    }

    pub const fn retrieval_calls(&self) -> &ObservedTotal {
        &self.retrieval_calls
    }

    pub const fn input_tokens(&self) -> &ObservedTotal {
        &self.input_tokens
    }

    pub const fn output_tokens(&self) -> &ObservedTotal {
        &self.output_tokens
    }

    pub fn provider_costs(&self) -> &[CurrencyTotal] {
        &self.provider_costs
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateSynthesisMetrics {
    assertions_with_valid_claims: MetricCount,
    citation_resolution: MetricCount,
    reported_claims_with_citations: MetricCount,
    invalid_claim_references: u64,
    insufficient_as_facts: u64,
    contradictions_rendered_settled: u64,
    qualification_mismatches: u64,
    repeated_evidence_citations: u64,
    fixture_semantic_grounding: MetricCount,
    model_judged_semantic_grounding: MetricCount,
    fixture_unsupported: u64,
    model_judged_unsupported: u64,
    unjudged_assertions: u64,
    invalid_adjudications: u64,
    blank_assertions: u64,
}

impl AggregateSynthesisMetrics {
    pub const fn assertions_with_valid_claims(&self) -> &MetricCount {
        &self.assertions_with_valid_claims
    }

    pub const fn citation_resolution(&self) -> &MetricCount {
        &self.citation_resolution
    }

    pub const fn reported_claims_with_citations(&self) -> &MetricCount {
        &self.reported_claims_with_citations
    }

    pub const fn invalid_claim_references(&self) -> u64 {
        self.invalid_claim_references
    }

    pub const fn insufficient_as_facts(&self) -> u64 {
        self.insufficient_as_facts
    }

    pub const fn contradictions_rendered_settled(&self) -> u64 {
        self.contradictions_rendered_settled
    }

    pub const fn qualification_mismatches(&self) -> u64 {
        self.qualification_mismatches
    }

    pub const fn repeated_evidence_citations(&self) -> u64 {
        self.repeated_evidence_citations
    }

    pub const fn fixture_semantic_grounding(&self) -> &MetricCount {
        &self.fixture_semantic_grounding
    }

    pub const fn model_judged_semantic_grounding(&self) -> &MetricCount {
        &self.model_judged_semantic_grounding
    }

    pub const fn fixture_unsupported(&self) -> u64 {
        self.fixture_unsupported
    }

    pub const fn model_judged_unsupported(&self) -> u64 {
        self.model_judged_unsupported
    }

    pub const fn unjudged_assertions(&self) -> u64 {
        self.unjudged_assertions
    }

    pub const fn invalid_adjudications(&self) -> u64 {
        self.invalid_adjudications
    }

    pub const fn blank_assertions(&self) -> u64 {
        self.blank_assertions
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateEvaluation {
    total_cases: u64,
    terminal_counts: Vec<TerminalCount>,
    failure_counts: Vec<FailureCount>,
    grounding: EvidenceGroundingMetrics,
    verification: VerificationMetrics,
    synthesis: AggregateSynthesisMetrics,
    false_completions: u64,
    invalid_research_histories: u64,
    usage: AggregateUsage,
    duration_millis: DistributionSummary,
    input_tokens: DistributionSummary,
    output_tokens: DistributionSummary,
    investigation_tasks: DistributionSummary,
    follow_up_tasks: DistributionSummary,
    sources: DistributionSummary,
    evidence_items: DistributionSummary,
    claims: DistributionSummary,
    verification_assessments: DistributionSummary,
    gap_resolution_steps: DistributionSummary,
}

impl AggregateEvaluation {
    fn from_cases(cases: &[CaseEvaluationResult]) -> Result<Self, EvaluationReportError> {
        let mut terminals = BTreeMap::<ObservedTerminalOutcome, u64>::new();
        let mut failures = BTreeMap::<ExecutionFailure, u64>::new();
        let mut grounding = EvidenceGroundingMetrics::default();
        let mut verification = VerificationMetrics::default();
        let mut synthesis = AggregateSynthesisMetrics::default();
        let mut false_completions = 0;
        let mut invalid_research_histories = 0;
        let mut usage = AggregateUsage::default();
        let mut costs = BTreeMap::<String, Vec<u64>>::new();
        let mut durations = Vec::new();
        let mut input_tokens = Vec::new();
        let mut output_tokens = Vec::new();
        let mut task_counts = Vec::new();
        let mut follow_up_counts = Vec::new();
        let mut source_counts = Vec::new();
        let mut evidence_counts = Vec::new();
        let mut claim_counts = Vec::new();
        let mut verification_counts = Vec::new();
        let mut gap_resolution_steps = Vec::new();
        for case in cases {
            let terminal_count = terminals.entry(case.terminal()).or_default();
            *terminal_count = terminal_count
                .checked_add(1)
                .ok_or(EvaluationReportError::MetricOverflow)?;
            for failure in case.failures() {
                let failure_count = failures.entry(*failure).or_default();
                *failure_count = failure_count
                    .checked_add(1)
                    .ok_or(EvaluationReportError::MetricOverflow)?;
            }
            grounding = add_grounding(grounding, *case.grounding())
                .ok_or(EvaluationReportError::MetricOverflow)?;
            verification = add_verification(verification, *case.verification())
                .ok_or(EvaluationReportError::MetricOverflow)?;
            add_synthesis(&mut synthesis, case.synthesis())
                .ok_or(EvaluationReportError::MetricOverflow)?;
            checked_add_assign(
                &mut false_completions,
                case.adaptive().false_completion_count(),
            )
            .ok_or(EvaluationReportError::MetricOverflow)?;
            checked_add_assign(
                &mut invalid_research_histories,
                u64::from(case.invalid_research_history()),
            )
            .ok_or(EvaluationReportError::MetricOverflow)?;
            if let Some(counts) = case.counts() {
                task_counts.push(counts.investigation_tasks());
                follow_up_counts.push(counts.follow_up_tasks());
                source_counts.push(counts.sources());
                evidence_counts.push(counts.evidence());
                claim_counts.push(counts.claims());
                verification_counts.push(counts.verification_assessments());
            }
            gap_resolution_steps.extend(case.adaptive().gap_resolution_steps());
            if let Some(observed) = case.usage() {
                usage.cases_with_usage = usage
                    .cases_with_usage
                    .checked_add(1)
                    .ok_or(EvaluationReportError::MetricOverflow)?;
                usage
                    .model_invocations
                    .record(observed.model_invocations())
                    .ok_or(EvaluationReportError::MetricOverflow)?;
                usage
                    .retrieval_calls
                    .record(observed.retrieval_calls())
                    .ok_or(EvaluationReportError::MetricOverflow)?;
                usage
                    .input_tokens
                    .record(observed.input_tokens())
                    .ok_or(EvaluationReportError::MetricOverflow)?;
                usage
                    .output_tokens
                    .record(observed.output_tokens())
                    .ok_or(EvaluationReportError::MetricOverflow)?;
                if let Some(value) = observed.input_tokens() {
                    input_tokens.push(value);
                }
                if let Some(value) = observed.output_tokens() {
                    output_tokens.push(value);
                }
                if let Some(duration) = observed.wall_clock_millis() {
                    durations.push(duration);
                }
                if let Some(cost) = observed.provider_cost() {
                    costs
                        .entry(cost.currency().to_owned())
                        .or_default()
                        .push(cost.micros());
                }
            }
        }
        usage.provider_costs = costs
            .into_iter()
            .map(|(currency, values)| {
                let micros = values
                    .iter()
                    .try_fold(0_u64, |total, value| total.checked_add(*value))?;
                Some(CurrencyTotal {
                    currency,
                    observations: u64::try_from(values.len()).ok()?,
                    micros,
                    distribution: DistributionSummary::from_values(&values),
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(EvaluationReportError::MetricOverflow)?;
        Ok(Self {
            total_cases: u64::try_from(cases.len())
                .map_err(|_| EvaluationReportError::MetricOverflow)?,
            terminal_counts: terminals
                .into_iter()
                .map(|(outcome, count)| TerminalCount { outcome, count })
                .collect(),
            failure_counts: failures
                .into_iter()
                .map(|(failure, count)| FailureCount { failure, count })
                .collect(),
            grounding,
            verification,
            synthesis,
            false_completions,
            invalid_research_histories,
            usage,
            duration_millis: DistributionSummary::from_values(&durations),
            input_tokens: DistributionSummary::from_values(&input_tokens),
            output_tokens: DistributionSummary::from_values(&output_tokens),
            investigation_tasks: DistributionSummary::from_values(&task_counts),
            follow_up_tasks: DistributionSummary::from_values(&follow_up_counts),
            sources: DistributionSummary::from_values(&source_counts),
            evidence_items: DistributionSummary::from_values(&evidence_counts),
            claims: DistributionSummary::from_values(&claim_counts),
            verification_assessments: DistributionSummary::from_values(&verification_counts),
            gap_resolution_steps: DistributionSummary::from_values(&gap_resolution_steps),
        })
    }

    pub const fn total_cases(&self) -> u64 {
        self.total_cases
    }

    pub fn terminal_counts(&self) -> &[TerminalCount] {
        &self.terminal_counts
    }

    pub fn failure_counts(&self) -> &[FailureCount] {
        &self.failure_counts
    }

    pub fn terminal_count(&self, outcome: ObservedTerminalOutcome) -> u64 {
        self.terminal_counts
            .iter()
            .find(|value| value.outcome == outcome)
            .map_or(0, |value| value.count)
    }

    pub fn failure_count(&self, failure: ExecutionFailure) -> u64 {
        self.failure_counts
            .iter()
            .find(|value| value.failure == failure)
            .map_or(0, |value| value.count)
    }

    pub fn completed_cases(&self) -> u64 {
        self.terminal_count(ObservedTerminalOutcome::Completed)
    }

    pub fn blocked_cases(&self) -> u64 {
        self.terminal_count(ObservedTerminalOutcome::Blocked)
    }

    pub const fn grounding(&self) -> &EvidenceGroundingMetrics {
        &self.grounding
    }

    pub const fn verification(&self) -> &VerificationMetrics {
        &self.verification
    }

    pub const fn synthesis(&self) -> &AggregateSynthesisMetrics {
        &self.synthesis
    }

    pub const fn exact_excerpts(&self) -> &MetricCount {
        self.grounding.exact_excerpts()
    }

    pub const fn relation_accuracy(&self) -> &MetricCount {
        self.verification.relations().accuracy()
    }

    pub const fn sufficiency_accuracy(&self) -> &MetricCount {
        self.verification.sufficiency().accuracy()
    }

    pub const fn assertions_with_valid_claims(&self) -> &MetricCount {
        self.synthesis.assertions_with_valid_claims()
    }

    pub const fn citation_resolution(&self) -> &MetricCount {
        self.synthesis.citation_resolution()
    }

    pub const fn false_completions(&self) -> u64 {
        self.false_completions
    }

    pub const fn unsupported_as_sufficient(&self) -> u64 {
        self.verification.unsupported_as_sufficient()
    }

    pub const fn insufficient_as_facts(&self) -> u64 {
        self.synthesis.insufficient_as_facts()
    }

    pub const fn contradictions_rendered_settled(&self) -> u64 {
        self.synthesis.contradictions_rendered_settled()
    }

    pub const fn invalid_research_histories(&self) -> u64 {
        self.invalid_research_histories
    }

    pub const fn usage(&self) -> &AggregateUsage {
        &self.usage
    }

    pub const fn duration_millis(&self) -> &DistributionSummary {
        &self.duration_millis
    }

    pub const fn input_tokens(&self) -> &DistributionSummary {
        &self.input_tokens
    }

    pub const fn output_tokens(&self) -> &DistributionSummary {
        &self.output_tokens
    }

    pub const fn investigation_tasks(&self) -> &DistributionSummary {
        &self.investigation_tasks
    }

    pub const fn follow_up_tasks(&self) -> &DistributionSummary {
        &self.follow_up_tasks
    }

    pub const fn sources(&self) -> &DistributionSummary {
        &self.sources
    }

    pub const fn evidence_items(&self) -> &DistributionSummary {
        &self.evidence_items
    }

    pub const fn claims(&self) -> &DistributionSummary {
        &self.claims
    }

    pub const fn verification_assessments(&self) -> &DistributionSummary {
        &self.verification_assessments
    }

    pub const fn gap_resolution_steps(&self) -> &DistributionSummary {
        &self.gap_resolution_steps
    }

    pub(crate) fn is_valid(&self) -> bool {
        [
            &self.duration_millis,
            &self.input_tokens,
            &self.output_tokens,
            &self.investigation_tasks,
            &self.follow_up_tasks,
            &self.sources,
            &self.evidence_items,
            &self.claims,
            &self.verification_assessments,
            &self.gap_resolution_steps,
        ]
        .into_iter()
        .all(DistributionSummary::is_valid)
            && unique_terminals(&self.terminal_counts)
            && unique_failures(&self.failure_counts)
            && self
                .usage
                .provider_costs
                .iter()
                .map(|cost| cost.currency.as_str())
                .collect::<BTreeSet<_>>()
                .len()
                == self.usage.provider_costs.len()
            && self.usage.provider_costs.iter().all(|cost| {
                cost.observations != 0
                    && cost.distribution.count() == cost.observations
                    && cost.distribution.is_valid()
            })
            && [
                self.usage.model_invocations.observations,
                self.usage.retrieval_calls.observations,
                self.usage.input_tokens.observations,
                self.usage.output_tokens.observations,
            ]
            .into_iter()
            .all(|observations| observations <= self.usage.cases_with_usage)
    }
}

fn checked_add_assign(total: &mut u64, value: u64) -> Option<()> {
    *total = total.checked_add(value)?;
    Some(())
}

fn add_counts(left: MetricCount, right: MetricCount) -> Option<MetricCount> {
    Some(MetricCount::new(
        left.matched().checked_add(right.matched())?,
        left.total().checked_add(right.total())?,
    ))
}

fn add_class(left: ClassMetric, right: ClassMetric) -> Option<ClassMetric> {
    Some(ClassMetric::new(
        left.true_positive().checked_add(right.true_positive())?,
        left.false_positive().checked_add(right.false_positive())?,
        left.false_negative().checked_add(right.false_negative())?,
    ))
}

fn add_grounding(
    left: EvidenceGroundingMetrics,
    right: EvidenceGroundingMetrics,
) -> Option<EvidenceGroundingMetrics> {
    Some(EvidenceGroundingMetrics::new(
        add_counts(*left.exact_excerpts(), *right.exact_excerpts())?,
        add_counts(*left.source_attributions(), *right.source_attributions())?,
        add_counts(*left.digest_matches(), *right.digest_matches())?,
        left.missing_source_fixtures()
            .checked_add(right.missing_source_fixtures())?,
    ))
}

fn add_verification(
    left: VerificationMetrics,
    right: VerificationMetrics,
) -> Option<VerificationMetrics> {
    let left_relations = left.relations();
    let right_relations = right.relations();
    let left_sufficiency = left.sufficiency();
    let right_sufficiency = right.sufficiency();
    Some(VerificationMetrics::new(
        RelationMetrics::new(
            add_counts(*left_relations.accuracy(), *right_relations.accuracy())?,
            left_relations
                .missing_predictions()
                .checked_add(right_relations.missing_predictions())?,
            add_class(*left_relations.supports(), *right_relations.supports())?,
            add_class(
                *left_relations.contradicts(),
                *right_relations.contradicts(),
            )?,
            add_class(*left_relations.unclear(), *right_relations.unclear())?,
            add_class(*left_relations.irrelevant(), *right_relations.irrelevant())?,
        ),
        SufficiencyMetrics::new(
            add_counts(*left_sufficiency.accuracy(), *right_sufficiency.accuracy())?,
            left_sufficiency
                .missing_predictions()
                .checked_add(right_sufficiency.missing_predictions())?,
            add_class(
                *left_sufficiency.sufficient(),
                *right_sufficiency.sufficient(),
            )?,
            add_class(
                *left_sufficiency.insufficient(),
                *right_sufficiency.insufficient(),
            )?,
            add_class(
                *left_sufficiency.indeterminate(),
                *right_sufficiency.indeterminate(),
            )?,
        ),
        left.unsupported_as_sufficient()
            .checked_add(right.unsupported_as_sufficient())?,
    ))
}

fn add_synthesis(
    total: &mut AggregateSynthesisMetrics,
    case: &crate::SynthesisMetrics,
) -> Option<()> {
    total.assertions_with_valid_claims = add_counts(
        total.assertions_with_valid_claims,
        *case.assertions_with_valid_claims(),
    )?;
    total.citation_resolution = add_counts(total.citation_resolution, *case.citation_resolution())?;
    total.reported_claims_with_citations = add_counts(
        total.reported_claims_with_citations,
        *case.reported_claims_with_citations(),
    )?;
    checked_add_assign(
        &mut total.invalid_claim_references,
        case.invalid_claim_references(),
    )?;
    checked_add_assign(
        &mut total.insufficient_as_facts,
        case.insufficient_as_facts(),
    )?;
    checked_add_assign(
        &mut total.contradictions_rendered_settled,
        case.contradictions_rendered_settled(),
    )?;
    checked_add_assign(
        &mut total.qualification_mismatches,
        case.qualification_mismatches(),
    )?;
    checked_add_assign(
        &mut total.repeated_evidence_citations,
        case.repeated_evidence_citations(),
    )?;
    total.fixture_semantic_grounding =
        add_counts(total.fixture_semantic_grounding, *case.semantic().fixture())?;
    total.model_judged_semantic_grounding = add_counts(
        total.model_judged_semantic_grounding,
        *case.semantic().model_judged(),
    )?;
    checked_add_assign(
        &mut total.fixture_unsupported,
        case.semantic().fixture_unsupported(),
    )?;
    checked_add_assign(
        &mut total.model_judged_unsupported,
        case.semantic().model_judged_unsupported(),
    )?;
    checked_add_assign(
        &mut total.unjudged_assertions,
        case.semantic().unjudged_assertions(),
    )?;
    checked_add_assign(
        &mut total.invalid_adjudications,
        case.semantic().invalid_adjudications(),
    )?;
    checked_add_assign(&mut total.blank_assertions, case.blank_assertions())?;
    Some(())
}

fn unique_terminals(values: &[TerminalCount]) -> bool {
    values
        .iter()
        .map(TerminalCount::outcome)
        .collect::<BTreeSet<_>>()
        .len()
        == values.len()
}

fn unique_failures(values: &[FailureCount]) -> bool {
    values
        .iter()
        .map(FailureCount::failure)
        .collect::<BTreeSet<_>>()
        .len()
        == values.len()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationReport {
    cases: Vec<CaseEvaluationResult>,
    aggregate: AggregateEvaluation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EvaluationReportError {
    #[error("evaluation report repeats a case identifier")]
    DuplicateCase,
    #[error("evaluation report metric overflowed")]
    MetricOverflow,
}

impl EvaluationReport {
    pub fn new(mut cases: Vec<CaseEvaluationResult>) -> Result<Self, EvaluationReportError> {
        cases.sort_by(|left, right| left.case_id().cmp(right.case_id()));
        if cases
            .windows(2)
            .any(|pair| pair[0].case_id() == pair[1].case_id())
        {
            return Err(EvaluationReportError::DuplicateCase);
        }
        let aggregate = AggregateEvaluation::from_cases(&cases)?;
        Ok(Self { cases, aggregate })
    }

    pub fn cases(&self) -> &[CaseEvaluationResult] {
        &self.cases
    }

    pub const fn aggregate(&self) -> &AggregateEvaluation {
        &self.aggregate
    }
}
