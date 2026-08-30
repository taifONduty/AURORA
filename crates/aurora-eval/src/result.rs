use crate::{EvaluationCaseId, EvaluationMetadata, ExecutionFailure, ObservedUsage};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricCount {
    matched: u64,
    total: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassMetric {
    true_positive: u64,
    false_positive: u64,
    false_negative: u64,
}

impl ClassMetric {
    pub(crate) const fn new(true_positive: u64, false_positive: u64, false_negative: u64) -> Self {
        Self {
            true_positive,
            false_positive,
            false_negative,
        }
    }

    pub const fn true_positive(&self) -> u64 {
        self.true_positive
    }

    pub const fn false_positive(&self) -> u64 {
        self.false_positive
    }

    pub const fn false_negative(&self) -> u64 {
        self.false_negative
    }

    pub const fn support(&self) -> u64 {
        self.true_positive + self.false_negative
    }

    pub fn precision(&self) -> Option<f64> {
        ratio(self.true_positive, self.true_positive + self.false_positive)
    }

    pub fn recall(&self) -> Option<f64> {
        ratio(self.true_positive, self.true_positive + self.false_negative)
    }

    pub fn f1(&self) -> Option<f64> {
        let precision = self.precision()?;
        let recall = self.recall()?;
        if precision + recall == 0.0 {
            Some(0.0)
        } else {
            Some(2.0 * precision * recall / (precision + recall))
        }
    }
}

fn ratio(numerator: u64, denominator: u64) -> Option<f64> {
    (denominator != 0).then(|| numerator as f64 / denominator as f64)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationMetrics {
    accuracy: MetricCount,
    missing_predictions: u64,
    supports: ClassMetric,
    contradicts: ClassMetric,
    unclear: ClassMetric,
    irrelevant: ClassMetric,
}

impl RelationMetrics {
    pub(crate) const fn new(
        accuracy: MetricCount,
        missing_predictions: u64,
        supports: ClassMetric,
        contradicts: ClassMetric,
        unclear: ClassMetric,
        irrelevant: ClassMetric,
    ) -> Self {
        Self {
            accuracy,
            missing_predictions,
            supports,
            contradicts,
            unclear,
            irrelevant,
        }
    }

    pub const fn accuracy(&self) -> &MetricCount {
        &self.accuracy
    }

    pub const fn missing_predictions(&self) -> u64 {
        self.missing_predictions
    }

    pub const fn supports(&self) -> &ClassMetric {
        &self.supports
    }

    pub const fn contradicts(&self) -> &ClassMetric {
        &self.contradicts
    }

    pub const fn unclear(&self) -> &ClassMetric {
        &self.unclear
    }

    pub const fn irrelevant(&self) -> &ClassMetric {
        &self.irrelevant
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SufficiencyMetrics {
    accuracy: MetricCount,
    missing_predictions: u64,
    sufficient: ClassMetric,
    insufficient: ClassMetric,
    indeterminate: ClassMetric,
}

impl SufficiencyMetrics {
    pub(crate) const fn new(
        accuracy: MetricCount,
        missing_predictions: u64,
        sufficient: ClassMetric,
        insufficient: ClassMetric,
        indeterminate: ClassMetric,
    ) -> Self {
        Self {
            accuracy,
            missing_predictions,
            sufficient,
            insufficient,
            indeterminate,
        }
    }

    pub const fn accuracy(&self) -> &MetricCount {
        &self.accuracy
    }

    pub const fn missing_predictions(&self) -> u64 {
        self.missing_predictions
    }

    pub const fn sufficient(&self) -> &ClassMetric {
        &self.sufficient
    }

    pub const fn insufficient(&self) -> &ClassMetric {
        &self.insufficient
    }

    pub const fn indeterminate(&self) -> &ClassMetric {
        &self.indeterminate
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationMetrics {
    relations: RelationMetrics,
    sufficiency: SufficiencyMetrics,
    unsupported_as_sufficient: u64,
}

impl VerificationMetrics {
    pub(crate) const fn new(
        relations: RelationMetrics,
        sufficiency: SufficiencyMetrics,
        unsupported_as_sufficient: u64,
    ) -> Self {
        Self {
            relations,
            sufficiency,
            unsupported_as_sufficient,
        }
    }

    pub const fn relations(&self) -> &RelationMetrics {
        &self.relations
    }

    pub const fn sufficiency(&self) -> &SufficiencyMetrics {
        &self.sufficiency
    }

    pub fn contradiction_recall(&self) -> Option<f64> {
        self.relations.contradicts().recall()
    }

    pub const fn unsupported_as_sufficient(&self) -> u64 {
        self.unsupported_as_sufficient
    }
}

impl MetricCount {
    pub const fn new(matched: u64, total: u64) -> Self {
        Self { matched, total }
    }

    pub const fn matched(&self) -> u64 {
        self.matched
    }

    pub const fn total(&self) -> u64 {
        self.total
    }

    pub fn rate(&self) -> Option<f64> {
        (self.total != 0).then(|| self.matched as f64 / self.total as f64)
    }

    const fn is_valid(&self) -> bool {
        self.matched <= self.total
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuaranteeAudit {
    history_reconstructed: bool,
    references_valid: bool,
    terminal_is_explicit: bool,
    record_count_unchanged: bool,
}

impl GuaranteeAudit {
    pub(crate) const fn valid(terminal_is_explicit: bool) -> Self {
        Self {
            history_reconstructed: true,
            references_valid: true,
            terminal_is_explicit,
            record_count_unchanged: true,
        }
    }

    pub(crate) const fn invalid_history() -> Self {
        Self {
            history_reconstructed: false,
            references_valid: false,
            terminal_is_explicit: false,
            record_count_unchanged: true,
        }
    }

    pub const fn history_reconstructed(&self) -> bool {
        self.history_reconstructed
    }

    pub const fn references_valid(&self) -> bool {
        self.references_valid
    }

    pub const fn terminal_is_explicit(&self) -> bool {
        self.terminal_is_explicit
    }

    pub const fn record_count_unchanged(&self) -> bool {
        self.record_count_unchanged
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceGroundingMetrics {
    exact_excerpts: MetricCount,
    source_attributions: MetricCount,
    digest_matches: MetricCount,
    missing_source_fixtures: u64,
}

impl EvidenceGroundingMetrics {
    pub(crate) const fn new(
        exact_excerpts: MetricCount,
        source_attributions: MetricCount,
        digest_matches: MetricCount,
        missing_source_fixtures: u64,
    ) -> Self {
        Self {
            exact_excerpts,
            source_attributions,
            digest_matches,
            missing_source_fixtures,
        }
    }

    pub const fn exact_excerpts(&self) -> &MetricCount {
        &self.exact_excerpts
    }

    pub const fn source_attributions(&self) -> &MetricCount {
        &self.source_attributions
    }

    pub const fn digest_matches(&self) -> &MetricCount {
        &self.digest_matches
    }

    pub const fn missing_source_fixtures(&self) -> u64 {
        self.missing_source_fixtures
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedTerminalOutcome {
    NonTerminal,
    Completed,
    Failed,
    OperatorStopped,
    BudgetExhausted,
    Blocked,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdaptiveLoopMetrics {
    expected_terminal_match: Option<bool>,
    false_completion_count: u64,
    initial_tasks: u64,
    follow_up_tasks: u64,
    repeated_follow_up_objectives: u64,
    cyclic_follow_up_lineages: u64,
    open_gaps: u64,
    resolved_gaps: u64,
    open_gaps_without_follow_up: u64,
    failed_tasks: u64,
    excess_follow_up_tasks: Option<u64>,
    gap_resolution_steps: Vec<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticGroundingMetrics {
    fixture: MetricCount,
    model_judged: MetricCount,
    fixture_unsupported: u64,
    model_judged_unsupported: u64,
    unjudged_assertions: u64,
    invalid_adjudications: u64,
    judge_metadata: Vec<crate::JudgeMetadata>,
}

impl SemanticGroundingMetrics {
    pub(crate) const fn new(
        fixture: MetricCount,
        model_judged: MetricCount,
        fixture_unsupported: u64,
        model_judged_unsupported: u64,
        unjudged_assertions: u64,
        invalid_adjudications: u64,
        judge_metadata: Vec<crate::JudgeMetadata>,
    ) -> Self {
        Self {
            fixture,
            model_judged,
            fixture_unsupported,
            model_judged_unsupported,
            unjudged_assertions,
            invalid_adjudications,
            judge_metadata,
        }
    }

    pub const fn fixture(&self) -> &MetricCount {
        &self.fixture
    }

    pub const fn model_judged(&self) -> &MetricCount {
        &self.model_judged
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

    pub fn judge_metadata(&self) -> &[crate::JudgeMetadata] {
        &self.judge_metadata
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SynthesisMetrics {
    assertions_with_valid_claims: MetricCount,
    citation_resolution: MetricCount,
    reported_claims_with_citations: MetricCount,
    invalid_claim_references: u64,
    insufficient_as_facts: u64,
    contradictions_rendered_settled: u64,
    qualification_mismatches: u64,
    deterministic_rendering: Option<bool>,
    repeated_evidence_citations: u64,
    semantic: SemanticGroundingMetrics,
    blank_assertions: u64,
}

impl SynthesisMetrics {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        assertions_with_valid_claims: MetricCount,
        citation_resolution: MetricCount,
        reported_claims_with_citations: MetricCount,
        invalid_claim_references: u64,
        insufficient_as_facts: u64,
        contradictions_rendered_settled: u64,
        qualification_mismatches: u64,
        deterministic_rendering: Option<bool>,
        repeated_evidence_citations: u64,
        semantic: SemanticGroundingMetrics,
        blank_assertions: u64,
    ) -> Self {
        Self {
            assertions_with_valid_claims,
            citation_resolution,
            reported_claims_with_citations,
            invalid_claim_references,
            insufficient_as_facts,
            contradictions_rendered_settled,
            qualification_mismatches,
            deterministic_rendering,
            repeated_evidence_citations,
            semantic,
            blank_assertions,
        }
    }

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

    pub const fn deterministic_rendering(&self) -> Option<bool> {
        self.deterministic_rendering
    }

    pub const fn repeated_evidence_citations(&self) -> u64 {
        self.repeated_evidence_citations
    }

    pub const fn semantic(&self) -> &SemanticGroundingMetrics {
        &self.semantic
    }

    pub const fn blank_assertions(&self) -> u64 {
        self.blank_assertions
    }
}

impl AdaptiveLoopMetrics {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        expected_terminal_match: Option<bool>,
        false_completion_count: u64,
        initial_tasks: u64,
        follow_up_tasks: u64,
        repeated_follow_up_objectives: u64,
        cyclic_follow_up_lineages: u64,
        open_gaps: u64,
        resolved_gaps: u64,
        open_gaps_without_follow_up: u64,
        failed_tasks: u64,
        excess_follow_up_tasks: Option<u64>,
        gap_resolution_steps: Vec<u64>,
    ) -> Self {
        Self {
            expected_terminal_match,
            false_completion_count,
            initial_tasks,
            follow_up_tasks,
            repeated_follow_up_objectives,
            cyclic_follow_up_lineages,
            open_gaps,
            resolved_gaps,
            open_gaps_without_follow_up,
            failed_tasks,
            excess_follow_up_tasks,
            gap_resolution_steps,
        }
    }

    pub const fn expected_terminal_match(&self) -> Option<bool> {
        self.expected_terminal_match
    }

    pub const fn false_completion_count(&self) -> u64 {
        self.false_completion_count
    }

    pub const fn initial_tasks(&self) -> u64 {
        self.initial_tasks
    }

    pub const fn follow_up_tasks(&self) -> u64 {
        self.follow_up_tasks
    }

    pub const fn repeated_follow_up_objectives(&self) -> u64 {
        self.repeated_follow_up_objectives
    }

    pub const fn cyclic_follow_up_lineages(&self) -> u64 {
        self.cyclic_follow_up_lineages
    }

    pub const fn open_gaps(&self) -> u64 {
        self.open_gaps
    }

    pub const fn resolved_gaps(&self) -> u64 {
        self.resolved_gaps
    }

    pub const fn open_gaps_without_follow_up(&self) -> u64 {
        self.open_gaps_without_follow_up
    }

    pub const fn failed_tasks(&self) -> u64 {
        self.failed_tasks
    }

    pub const fn excess_follow_up_tasks(&self) -> Option<u64> {
        self.excess_follow_up_tasks
    }

    pub fn gap_resolution_steps(&self) -> &[u64] {
        &self.gap_resolution_steps
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseEvaluationResult {
    case_id: EvaluationCaseId,
    metadata: EvaluationMetadata,
    terminal: ObservedTerminalOutcome,
    guarantees: GuaranteeAudit,
    grounding: EvidenceGroundingMetrics,
    verification: VerificationMetrics,
    adaptive: AdaptiveLoopMetrics,
    synthesis: SynthesisMetrics,
    invalid_research_history: bool,
    usage: Option<ObservedUsage>,
    failures: Vec<ExecutionFailure>,
    counts: Option<DerivedRunCounts>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivedRunCounts {
    records: u64,
    sources: u64,
    evidence: u64,
    claims: u64,
    verification_assessments: u64,
    investigation_tasks: u64,
    follow_up_tasks: u64,
    gaps: u64,
}

impl DerivedRunCounts {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        records: u64,
        sources: u64,
        evidence: u64,
        claims: u64,
        verification_assessments: u64,
        investigation_tasks: u64,
        follow_up_tasks: u64,
        gaps: u64,
    ) -> Self {
        Self {
            records,
            sources,
            evidence,
            claims,
            verification_assessments,
            investigation_tasks,
            follow_up_tasks,
            gaps,
        }
    }

    pub const fn records(&self) -> u64 {
        self.records
    }
    pub const fn sources(&self) -> u64 {
        self.sources
    }
    pub const fn evidence(&self) -> u64 {
        self.evidence
    }
    pub const fn claims(&self) -> u64 {
        self.claims
    }
    pub const fn verification_assessments(&self) -> u64 {
        self.verification_assessments
    }
    pub const fn investigation_tasks(&self) -> u64 {
        self.investigation_tasks
    }
    pub const fn follow_up_tasks(&self) -> u64 {
        self.follow_up_tasks
    }
    pub const fn gaps(&self) -> u64 {
        self.gaps
    }
}

impl CaseEvaluationResult {
    pub(crate) fn new(parts: CaseEvaluationParts) -> Self {
        Self {
            case_id: parts.case_id,
            metadata: parts.metadata,
            terminal: parts.terminal,
            guarantees: parts.guarantees,
            grounding: parts.grounding,
            verification: parts.verification,
            adaptive: parts.adaptive,
            synthesis: parts.synthesis,
            invalid_research_history: parts.invalid_research_history,
            usage: parts.usage,
            failures: parts.failures,
            counts: parts.counts,
        }
    }

    pub const fn case_id(&self) -> &EvaluationCaseId {
        &self.case_id
    }

    pub const fn metadata(&self) -> &EvaluationMetadata {
        &self.metadata
    }

    pub const fn terminal(&self) -> ObservedTerminalOutcome {
        self.terminal
    }

    pub const fn guarantees(&self) -> &GuaranteeAudit {
        &self.guarantees
    }

    pub const fn grounding(&self) -> &EvidenceGroundingMetrics {
        &self.grounding
    }

    pub const fn verification(&self) -> &VerificationMetrics {
        &self.verification
    }

    pub const fn adaptive(&self) -> &AdaptiveLoopMetrics {
        &self.adaptive
    }

    pub const fn synthesis(&self) -> &SynthesisMetrics {
        &self.synthesis
    }

    pub const fn invalid_research_history(&self) -> bool {
        self.invalid_research_history
    }

    pub const fn usage(&self) -> Option<&ObservedUsage> {
        self.usage.as_ref()
    }
    pub fn failures(&self) -> &[ExecutionFailure] {
        &self.failures
    }
    pub const fn counts(&self) -> Option<&DerivedRunCounts> {
        self.counts.as_ref()
    }

    pub(crate) fn is_valid(&self) -> bool {
        let unique_failures = self
            .failures
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == self.failures.len();
        let ordered_failures = self.failures.windows(2).all(|pair| pair[0] < pair[1]);
        let counts_are_valid = self
            .counts
            .as_ref()
            .is_none_or(|counts| counts.follow_up_tasks <= counts.investigation_tasks);
        let metric_counts_are_valid = [
            self.grounding.exact_excerpts,
            self.grounding.source_attributions,
            self.grounding.digest_matches,
            self.verification.relations.accuracy,
            self.verification.sufficiency.accuracy,
            self.synthesis.assertions_with_valid_claims,
            self.synthesis.citation_resolution,
            self.synthesis.reported_claims_with_citations,
            self.synthesis.semantic.fixture,
            self.synthesis.semantic.model_judged,
        ]
        .iter()
        .all(MetricCount::is_valid);
        let identifiers_are_valid = EvaluationCaseId::new(self.case_id.as_str()).is_ok()
            && self.metadata.is_valid()
            && self.metadata.case_id() == Some(&self.case_id)
            && self.usage.as_ref().is_none_or(ObservedUsage::is_valid)
            && self
                .synthesis
                .semantic
                .judge_metadata
                .iter()
                .all(crate::JudgeMetadata::is_valid);
        let grounding_is_consistent = self.counts.as_ref().is_none_or(|counts| {
            self.grounding.exact_excerpts.total == counts.evidence
                && self.grounding.source_attributions.total == counts.evidence
                && self.grounding.digest_matches.total == counts.sources
                && self.grounding.missing_source_fixtures <= counts.sources
                && self.grounding.exact_excerpts.matched
                    <= self.grounding.source_attributions.matched
                && self
                    .grounding
                    .digest_matches
                    .matched
                    .checked_add(self.grounding.missing_source_fixtures)
                    .is_some_and(|observed| observed <= counts.sources)
        });
        let classification_is_consistent = classification_is_valid(
            self.verification.relations.accuracy,
            self.verification.relations.missing_predictions,
            &[
                self.verification.relations.supports,
                self.verification.relations.contradicts,
                self.verification.relations.unclear,
                self.verification.relations.irrelevant,
            ],
        ) && classification_is_valid(
            self.verification.sufficiency.accuracy,
            self.verification.sufficiency.missing_predictions,
            &[
                self.verification.sufficiency.sufficient,
                self.verification.sufficiency.insufficient,
                self.verification.sufficiency.indeterminate,
            ],
        ) && self.verification.unsupported_as_sufficient
            <= self.verification.sufficiency.accuracy.total;
        let semantic_is_consistent = semantic_is_valid(
            &self.synthesis.semantic,
            self.synthesis.assertions_with_valid_claims.total,
        );
        let synthesis_is_consistent = [
            self.synthesis.insufficient_as_facts,
            self.synthesis.contradictions_rendered_settled,
            self.synthesis.qualification_mismatches,
        ]
        .into_iter()
        .all(|count| count <= self.synthesis.assertions_with_valid_claims.matched)
            && self.synthesis.repeated_evidence_citations
                <= self.synthesis.citation_resolution.matched.saturating_sub(1)
            && self.synthesis.reported_claims_with_citations.matched
                <= self.synthesis.citation_resolution.matched
            && self.counts.as_ref().is_none_or(|counts| {
                self.synthesis.reported_claims_with_citations.total <= counts.claims
            });
        let failure_state_is_consistent = self.invalid_research_history
            == self
                .failures
                .contains(&ExecutionFailure::InvalidResearchHistory)
            && (self.terminal != ObservedTerminalOutcome::Failed
                || self.failures.contains(&ExecutionFailure::ResearchExecution));
        let history_shape_is_consistent = if self.invalid_research_history {
            self.terminal == ObservedTerminalOutcome::NonTerminal
                && self.guarantees == GuaranteeAudit::invalid_history()
                && self.grounding == EvidenceGroundingMetrics::default()
                && self.verification == VerificationMetrics::default()
                && self.adaptive == AdaptiveLoopMetrics::default()
                && self.synthesis == SynthesisMetrics::default()
                && self.counts.is_none()
                && self.metadata.follow_up_limit().is_none()
        } else {
            self.guarantees
                == GuaranteeAudit::valid(self.terminal != ObservedTerminalOutcome::NonTerminal)
                && self.counts.is_some()
                && (self.metadata.follow_up_limit().is_some()
                    || (self.terminal == ObservedTerminalOutcome::NonTerminal
                        && self.counts.is_some_and(|counts| counts.records == 0)))
        };
        let adaptive_is_consistent = self.counts.as_ref().is_none_or(|counts| {
            self.adaptive
                .initial_tasks
                .checked_add(self.adaptive.follow_up_tasks)
                == Some(counts.investigation_tasks)
                && self.adaptive.follow_up_tasks == counts.follow_up_tasks
                && self
                    .adaptive
                    .open_gaps
                    .checked_add(self.adaptive.resolved_gaps)
                    == Some(counts.gaps)
                && self.adaptive.failed_tasks <= counts.investigation_tasks
        }) && self.adaptive.repeated_follow_up_objectives
            <= self.adaptive.follow_up_tasks.saturating_sub(1)
            && self.adaptive.cyclic_follow_up_lineages <= self.adaptive.follow_up_tasks
            && self.adaptive.open_gaps_without_follow_up <= self.adaptive.open_gaps
            && self.adaptive.false_completion_count
                == u64::from(
                    self.terminal == ObservedTerminalOutcome::Completed
                        && self.adaptive.expected_terminal_match == Some(false),
                )
            && self
                .adaptive
                .excess_follow_up_tasks
                .is_none_or(|excess| excess <= self.adaptive.follow_up_tasks)
            && u64::try_from(self.adaptive.gap_resolution_steps.len()).ok()
                == Some(self.adaptive.resolved_gaps)
            && self.counts.as_ref().is_none_or(|counts| {
                self.adaptive
                    .gap_resolution_steps
                    .iter()
                    .all(|step| *step > 0 && *step < counts.records)
            });
        unique_failures
            && ordered_failures
            && counts_are_valid
            && metric_counts_are_valid
            && identifiers_are_valid
            && grounding_is_consistent
            && classification_is_consistent
            && semantic_is_consistent
            && synthesis_is_consistent
            && failure_state_is_consistent
            && history_shape_is_consistent
            && adaptive_is_consistent
    }
}

fn classification_is_valid(
    accuracy: MetricCount,
    missing_predictions: u64,
    classes: &[ClassMetric],
) -> bool {
    let Some(true_positives) = checked_sum(classes.iter().map(ClassMetric::true_positive)) else {
        return false;
    };
    let Some(false_positives) = checked_sum(classes.iter().map(ClassMetric::false_positive)) else {
        return false;
    };
    let Some(false_negatives) = checked_sum(classes.iter().map(ClassMetric::false_negative)) else {
        return false;
    };
    true_positives == accuracy.matched
        && false_negatives == accuracy.total.saturating_sub(accuracy.matched)
        && false_positives == false_negatives.saturating_sub(missing_predictions)
        && missing_predictions <= false_negatives
}

fn semantic_is_valid(metrics: &SemanticGroundingMetrics, assertion_total: u64) -> bool {
    let Some(judged_assertions) = assertion_total.checked_sub(metrics.unjudged_assertions) else {
        return false;
    };
    let Some(adjudications) = metrics
        .fixture
        .total
        .checked_add(metrics.model_judged.total)
    else {
        return false;
    };
    let Ok(judge_metadata_count) = u64::try_from(metrics.judge_metadata.len()) else {
        return false;
    };
    metrics
        .fixture
        .matched
        .checked_add(metrics.fixture_unsupported)
        == Some(metrics.fixture.total)
        && metrics
            .model_judged
            .matched
            .checked_add(metrics.model_judged_unsupported)
            == Some(metrics.model_judged.total)
        && metrics.fixture.total <= judged_assertions
        && metrics.model_judged.total <= judged_assertions
        && judged_assertions <= adjudications
        && (metrics.model_judged.total == 0) == metrics.judge_metadata.is_empty()
        && judge_metadata_count <= metrics.model_judged.total
        && metrics
            .judge_metadata
            .iter()
            .enumerate()
            .all(|(index, metadata)| !metrics.judge_metadata[..index].contains(metadata))
}

fn checked_sum(values: impl IntoIterator<Item = u64>) -> Option<u64> {
    values
        .into_iter()
        .try_fold(0_u64, |total, value| total.checked_add(value))
}

pub(crate) struct CaseEvaluationParts {
    pub case_id: EvaluationCaseId,
    pub metadata: EvaluationMetadata,
    pub terminal: ObservedTerminalOutcome,
    pub guarantees: GuaranteeAudit,
    pub grounding: EvidenceGroundingMetrics,
    pub verification: VerificationMetrics,
    pub adaptive: AdaptiveLoopMetrics,
    pub synthesis: SynthesisMetrics,
    pub invalid_research_history: bool,
    pub usage: Option<ObservedUsage>,
    pub failures: Vec<ExecutionFailure>,
    pub counts: Option<DerivedRunCounts>,
}
