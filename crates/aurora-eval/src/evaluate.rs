use std::collections::BTreeMap;

use aurora_research::{
    ClaimId, ClaimPresentation, EvidenceId, EvidenceRelation, EvidenceSufficiency,
    InvestigationTaskId, InvestigationTaskStatus, ResearchControlEvent, ResearchControlState,
    ResearchControlStatus, ResearchGapId, ResearchGapStatus, ResearchStopReason, SourceId,
    SynthesisBasis, TaskOrigin,
};
use ring::digest;

use crate::classification::score_classes;
use crate::result::CaseEvaluationParts;
use crate::{
    AdaptiveLoopMetrics, CaseEvaluationResult, DerivedRunCounts, EvaluationCase, EvaluationRun,
    EvidenceGroundingMetrics, GuaranteeAudit, MetricCount, ObservedTerminalOutcome,
    RelationMetrics, SemanticGroundingMetrics, SufficiencyMetrics, SynthesisMetrics,
    VerificationMetrics,
};

pub fn evaluate_case(case: &EvaluationCase, run: &EvaluationRun) -> CaseEvaluationResult {
    let Ok(state) = ResearchControlState::reconstruct(run.records().to_vec()) else {
        let mut failures = run.failures().to_vec();
        if !failures.contains(&crate::ExecutionFailure::InvalidResearchHistory) {
            failures.push(crate::ExecutionFailure::InvalidResearchHistory);
        }
        failures.sort_unstable();
        let metadata_conflict = run.metadata().conflicts_with(case.id(), None);
        if metadata_conflict && !failures.contains(&crate::ExecutionFailure::BenchmarkMapping) {
            failures.push(crate::ExecutionFailure::BenchmarkMapping);
            failures.sort_unstable();
        }
        return CaseEvaluationResult::new(CaseEvaluationParts {
            case_id: case.id().clone(),
            metadata: run.metadata().clone().bind(case.id().clone(), None),
            terminal: ObservedTerminalOutcome::NonTerminal,
            guarantees: GuaranteeAudit::invalid_history(),
            grounding: EvidenceGroundingMetrics::default(),
            verification: VerificationMetrics::default(),
            adaptive: AdaptiveLoopMetrics::default(),
            synthesis: SynthesisMetrics::default(),
            invalid_research_history: true,
            usage: run.usage().cloned(),
            failures,
            counts: None,
        });
    };
    let terminal = terminal_outcome(&state);
    let mut failures = run
        .failures()
        .iter()
        .copied()
        .filter(|failure| *failure != crate::ExecutionFailure::InvalidResearchHistory)
        .collect::<Vec<_>>();
    let follow_up_limit = state.limits().map(|limits| limits.max_follow_up_tasks());
    if terminal == ObservedTerminalOutcome::Failed
        && !failures.contains(&crate::ExecutionFailure::ResearchExecution)
    {
        failures.push(crate::ExecutionFailure::ResearchExecution);
    }
    if benchmark_mapping_is_invalid(case, run, &state)
        && !failures.contains(&crate::ExecutionFailure::BenchmarkMapping)
    {
        failures.push(crate::ExecutionFailure::BenchmarkMapping);
    }
    if run.metadata().conflicts_with(case.id(), follow_up_limit)
        && !failures.contains(&crate::ExecutionFailure::BenchmarkMapping)
    {
        failures.push(crate::ExecutionFailure::BenchmarkMapping);
    }
    failures.sort_unstable();
    let guarantees = GuaranteeAudit::valid(terminal != ObservedTerminalOutcome::NonTerminal);
    let grounding = grounding_metrics(case, &state);
    let verification = verification_metrics(case, run, &state);
    let adaptive = adaptive_metrics(case, run, &state, terminal);
    let synthesis = synthesis_metrics(run, &state);
    let counts = derived_counts(run, &state);
    CaseEvaluationResult::new(CaseEvaluationParts {
        case_id: case.id().clone(),
        metadata: run
            .metadata()
            .clone()
            .bind(case.id().clone(), follow_up_limit),
        terminal,
        guarantees,
        grounding,
        verification,
        adaptive,
        synthesis,
        invalid_research_history: false,
        usage: run.usage().cloned(),
        failures,
        counts: Some(counts),
    })
}

fn benchmark_mapping_is_invalid(
    case: &EvaluationCase,
    run: &EvaluationRun,
    state: &ResearchControlState,
) -> bool {
    run.verification_bindings().iter().any(|binding| {
        let Some(expectation) = case
            .verification_expectations()
            .iter()
            .find(|expectation| expectation.id() == binding.expectation_id())
        else {
            return true;
        };
        !verification_binding_is_valid(expectation, binding, state)
    })
}

fn verification_binding_is_valid(
    expectation: &crate::VerificationExpectation,
    binding: &crate::VerificationBinding,
    state: &ResearchControlState,
) -> bool {
    let Some(assessment) = state.verification().assessment(binding.verification_id()) else {
        return false;
    };
    let expected_keys = expectation
        .relations()
        .iter()
        .map(|relation| relation.evidence_key())
        .collect::<std::collections::BTreeSet<_>>();
    let bound_keys = binding
        .evidence()
        .iter()
        .map(|evidence| evidence.evidence_key())
        .collect::<std::collections::BTreeSet<_>>();
    expected_keys == bound_keys
        && binding.evidence().iter().all(|evidence| {
            state
                .investigation()
                .research()
                .evidence(evidence.evidence_id())
                .is_some()
                && assessment.relation(evidence.evidence_id()).is_some()
        })
}

fn derived_counts(run: &EvaluationRun, state: &ResearchControlState) -> DerivedRunCounts {
    let research = state.investigation().research();
    DerivedRunCounts::new(
        run.records().len() as u64,
        research.sources().count() as u64,
        research.evidence_items().count() as u64,
        research.claims().count() as u64,
        state.verification().assessments().count() as u64,
        state.investigation().tasks().count() as u64,
        state
            .investigation()
            .tasks()
            .filter(|task| matches!(task.task().origin(), TaskOrigin::FollowUp { .. }))
            .count() as u64,
        state.gaps().count() as u64,
    )
}

fn synthesis_metrics(run: &EvaluationRun, state: &ResearchControlState) -> SynthesisMetrics {
    let Some(observation) = run.synthesis() else {
        return SynthesisMetrics::default();
    };
    let basis = SynthesisBasis::from_state(state).ok();
    let research = state.investigation().research();
    let mut assertion_total = 0_u64;
    let mut valid_assertions = 0_u64;
    let mut invalid_claim_references = 0_u64;
    let mut citation_total = 0_u64;
    let mut valid_citations = 0_u64;
    let mut reported_claims = std::collections::BTreeSet::<ClaimId>::new();
    let mut cited_claims = std::collections::BTreeSet::<ClaimId>::new();
    let mut insufficient_as_facts = 0_u64;
    let mut contradictions_rendered_settled = 0_u64;
    let mut qualification_mismatches = 0_u64;
    let mut cited_evidence = std::collections::BTreeSet::<EvidenceId>::new();
    let mut repeated_evidence_citations = 0_u64;
    let mut blank_assertions = 0_u64;

    for section in observation.sections() {
        for assertion in section.assertions() {
            let substantive = !assertion.text().trim().is_empty();
            if substantive {
                assertion_total += 1;
            } else {
                blank_assertions += 1;
            }
            let mut resolved_claims = Vec::new();
            for raw in assertion.claim_ids() {
                let resolved = raw
                    .parse::<ClaimId>()
                    .ok()
                    .and_then(|id| basis.as_ref()?.claim(&id).map(|claim| (id, claim)));
                match resolved {
                    Some((id, claim)) => {
                        if substantive {
                            reported_claims.insert(id);
                        }
                        resolved_claims.push((id, claim.presentation()));
                    }
                    None => {
                        invalid_claim_references += 1;
                    }
                }
            }
            if substantive && !resolved_claims.is_empty() {
                valid_assertions += 1;
                let expected = aggregate_presentation(
                    resolved_claims
                        .iter()
                        .map(|(_, presentation)| *presentation),
                );
                if Some(assertion.presentation()) != expected {
                    qualification_mismatches += 1;
                }
                if assertion.presentation() == crate::ObservedPresentation::Established
                    && resolved_claims
                        .iter()
                        .any(|(_, presentation)| *presentation == ClaimPresentation::Unresolved)
                {
                    insufficient_as_facts += 1;
                }
                if assertion.presentation() == crate::ObservedPresentation::Established
                    && resolved_claims
                        .iter()
                        .any(|(_, presentation)| *presentation == ClaimPresentation::Contested)
                {
                    contradictions_rendered_settled += 1;
                }
            }
            let assertion_claims = resolved_claims
                .iter()
                .map(|(id, _)| *id)
                .collect::<std::collections::BTreeSet<_>>();
            for citation in assertion.citations() {
                if !substantive {
                    continue;
                }
                citation_total += 1;
                let claim_id = citation.claim_id().parse::<ClaimId>().ok();
                let evidence_id = citation.evidence_id().parse::<EvidenceId>().ok();
                let source_id = citation.source_id().parse::<SourceId>().ok();
                let valid = match (claim_id, evidence_id, source_id, basis.as_ref()) {
                    (Some(claim_id), Some(evidence_id), Some(source_id), Some(basis))
                        if assertion_claims.contains(&claim_id) =>
                    {
                        basis.claim(&claim_id).is_some_and(|claim| {
                            claim.citations().any(|path| {
                                path.evidence().id() == &evidence_id
                                    && path.source().id() == &source_id
                                    && digest_hex(path.source().content_digest())
                                        == citation.source_digest()
                                    && research
                                        .evidence(&evidence_id)
                                        .is_some_and(|evidence| evidence.source_id() == &source_id)
                            })
                        })
                    }
                    _ => false,
                };
                if valid {
                    valid_citations += 1;
                    let claim_id = claim_id.expect("valid citation parsed its claim");
                    let evidence_id = evidence_id.expect("valid citation parsed its evidence");
                    cited_claims.insert(claim_id);
                    if !cited_evidence.insert(evidence_id) {
                        repeated_evidence_citations += 1;
                    }
                }
            }
        }
    }
    let semantic = semantic_metrics(run, observation, assertion_total);
    SynthesisMetrics::new(
        MetricCount::new(valid_assertions, assertion_total),
        MetricCount::new(valid_citations, citation_total),
        MetricCount::new(
            reported_claims.intersection(&cited_claims).count() as u64,
            reported_claims.len() as u64,
        ),
        invalid_claim_references,
        insufficient_as_facts,
        contradictions_rendered_settled,
        qualification_mismatches,
        observation.deterministic_rendering(),
        repeated_evidence_citations,
        semantic,
        blank_assertions,
    )
}

fn aggregate_presentation(
    presentations: impl IntoIterator<Item = ClaimPresentation>,
) -> Option<crate::ObservedPresentation> {
    presentations
        .into_iter()
        .map(crate::ObservedPresentation::from)
        .max_by_key(|presentation| match presentation {
            crate::ObservedPresentation::Established => 0,
            crate::ObservedPresentation::Unresolved => 1,
            crate::ObservedPresentation::Contested => 2,
        })
}

fn semantic_metrics(
    run: &EvaluationRun,
    observation: &crate::SynthesisObservation,
    assertion_total: u64,
) -> SemanticGroundingMetrics {
    let mut fixture_total = 0_u64;
    let mut fixture_faithful = 0_u64;
    let mut model_total = 0_u64;
    let mut model_faithful = 0_u64;
    let mut fixture_unsupported = 0_u64;
    let mut model_unsupported = 0_u64;
    let mut invalid_adjudications = 0_u64;
    let mut judge_metadata = Vec::new();
    let mut judged_locations = std::collections::BTreeSet::new();
    for adjudication in run.semantic_adjudications() {
        if !adjudication_location_is_substantive(observation, adjudication.location()) {
            invalid_adjudications += 1;
            continue;
        }
        judged_locations.insert(*adjudication.location());
        match adjudication.origin() {
            crate::AdjudicationOrigin::LabelledFixture => {
                fixture_total += 1;
                match adjudication.grounding() {
                    crate::SemanticGrounding::Faithful => fixture_faithful += 1,
                    crate::SemanticGrounding::Unsupported => fixture_unsupported += 1,
                }
            }
            crate::AdjudicationOrigin::ModelJudge(metadata) => {
                model_total += 1;
                match adjudication.grounding() {
                    crate::SemanticGrounding::Faithful => model_faithful += 1,
                    crate::SemanticGrounding::Unsupported => model_unsupported += 1,
                }
                if !judge_metadata.contains(metadata) {
                    judge_metadata.push(metadata.clone());
                }
            }
        }
    }
    SemanticGroundingMetrics::new(
        MetricCount::new(fixture_faithful, fixture_total),
        MetricCount::new(model_faithful, model_total),
        fixture_unsupported,
        model_unsupported,
        assertion_total.saturating_sub(judged_locations.len() as u64),
        invalid_adjudications,
        judge_metadata,
    )
}

fn adjudication_location_is_substantive(
    observation: &crate::SynthesisObservation,
    location: &crate::AssertionLocation,
) -> bool {
    usize::try_from(location.section_index())
        .ok()
        .and_then(|section| observation.sections().get(section))
        .and_then(|section| {
            usize::try_from(location.assertion_index())
                .ok()
                .and_then(|assertion| section.assertions().get(assertion))
        })
        .is_some_and(|assertion| !assertion.text().trim().is_empty())
}

fn digest_hex(digest: &aurora_research::ContentDigest) -> String {
    digest
        .as_sha256()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn adaptive_metrics(
    case: &EvaluationCase,
    run: &EvaluationRun,
    state: &ResearchControlState,
    terminal: ObservedTerminalOutcome,
) -> AdaptiveLoopMetrics {
    let expected_terminal_match = case
        .expected_terminal()
        .map(|expected| terminal == expected_terminal(expected));
    let false_completion = u64::from(
        terminal == ObservedTerminalOutcome::Completed
            && case
                .expected_terminal()
                .is_some_and(|expected| expected != crate::ExpectedTerminalOutcome::Completed),
    );
    let mut initial_tasks = 0_u64;
    let mut follow_up_tasks = 0_u64;
    let mut failed_tasks = 0_u64;
    let mut objectives = BTreeMap::<&str, u64>::new();
    let mut parents = BTreeMap::<InvestigationTaskId, InvestigationTaskId>::new();
    for task in state.investigation().tasks() {
        if matches!(task.status(), InvestigationTaskStatus::Failed(_)) {
            failed_tasks += 1;
        }
        match task.task().origin() {
            TaskOrigin::Initial => initial_tasks += 1,
            TaskOrigin::FollowUp { parent_task_id, .. } => {
                follow_up_tasks += 1;
                *objectives.entry(task.task().objective()).or_default() += 1;
                parents.insert(*task.task().id(), *parent_task_id);
            }
        }
    }
    let repeated_follow_up_objectives = objectives
        .values()
        .map(|count| count.saturating_sub(1))
        .sum();
    let cyclic_follow_up_lineages = parents
        .keys()
        .filter(|task_id| lineage_has_cycle(**task_id, &parents))
        .count() as u64;
    let mut open_gaps = 0_u64;
    let mut resolved_gaps = 0_u64;
    let mut open_gaps_without_follow_up = 0_u64;
    for gap in state.gaps() {
        match gap.status() {
            ResearchGapStatus::Open => {
                open_gaps += 1;
                if gap.follow_up_task_id().is_none() {
                    open_gaps_without_follow_up += 1;
                }
            }
            ResearchGapStatus::Resolved(_) => resolved_gaps += 1,
        }
    }
    let gap_resolution_steps = gap_resolution_steps(run);
    let excess_follow_up_tasks = case
        .expected_follow_up_tasks()
        .map(|expected| follow_up_tasks.saturating_sub(u64::from(expected)));
    AdaptiveLoopMetrics::new(
        expected_terminal_match,
        false_completion,
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
    )
}

fn expected_terminal(expected: crate::ExpectedTerminalOutcome) -> ObservedTerminalOutcome {
    match expected {
        crate::ExpectedTerminalOutcome::Completed => ObservedTerminalOutcome::Completed,
        crate::ExpectedTerminalOutcome::Failed => ObservedTerminalOutcome::Failed,
        crate::ExpectedTerminalOutcome::OperatorStopped => ObservedTerminalOutcome::OperatorStopped,
        crate::ExpectedTerminalOutcome::BudgetExhausted => ObservedTerminalOutcome::BudgetExhausted,
        crate::ExpectedTerminalOutcome::Blocked => ObservedTerminalOutcome::Blocked,
    }
}

fn lineage_has_cycle(
    start: InvestigationTaskId,
    parents: &BTreeMap<InvestigationTaskId, InvestigationTaskId>,
) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    let mut current = start;
    while let Some(parent) = parents.get(&current) {
        if !seen.insert(current) {
            return true;
        }
        current = *parent;
    }
    false
}

fn gap_resolution_steps(run: &EvaluationRun) -> Vec<u64> {
    let mut identified = BTreeMap::<ResearchGapId, u64>::new();
    let mut steps = Vec::new();
    for record in run.records() {
        match record.event() {
            ResearchControlEvent::GapIdentified(gap) => {
                identified.insert(*gap.id(), record.sequence());
            }
            ResearchControlEvent::GapResolved { gap_id, .. } => {
                if let Some(start) = identified.get(gap_id) {
                    steps.push(record.sequence().saturating_sub(*start));
                }
            }
            ResearchControlEvent::LimitsRecorded(_)
            | ResearchControlEvent::InvestigationAdvanced(_)
            | ResearchControlEvent::VerificationRecorded(_)
            | ResearchControlEvent::GapFollowUpRecorded { .. }
            | ResearchControlEvent::ResearchCompleted
            | ResearchControlEvent::ResearchFailed(_) => {}
        }
    }
    steps
}

fn verification_metrics(
    case: &EvaluationCase,
    run: &EvaluationRun,
    state: &ResearchControlState,
) -> VerificationMetrics {
    let bindings = run
        .verification_bindings()
        .iter()
        .map(|binding| (binding.expectation_id(), binding))
        .collect::<BTreeMap<_, _>>();
    let mut relation_samples = Vec::new();
    let mut sufficiency_samples = Vec::new();
    let mut unsupported_as_sufficient = 0_u64;
    for expectation in case.verification_expectations() {
        let assessment = bindings
            .get(expectation.id())
            .filter(|binding| verification_binding_is_valid(expectation, binding, state))
            .and_then(|binding| state.verification().assessment(binding.verification_id()));
        let actual_sufficiency = assessment.map(|assessment| match assessment.sufficiency() {
            EvidenceSufficiency::Sufficient => crate::ExpectedSufficiency::Sufficient,
            EvidenceSufficiency::Insufficient => crate::ExpectedSufficiency::Insufficient,
            EvidenceSufficiency::Indeterminate => crate::ExpectedSufficiency::Indeterminate,
        });
        if expectation.sufficiency() != crate::ExpectedSufficiency::Sufficient
            && actual_sufficiency == Some(crate::ExpectedSufficiency::Sufficient)
        {
            unsupported_as_sufficient += 1;
        }
        sufficiency_samples.push((expectation.sufficiency(), actual_sufficiency));
        for expected in expectation.relations() {
            let actual = bindings
                .get(expectation.id())
                .and_then(|binding| {
                    binding
                        .evidence()
                        .iter()
                        .find(|candidate| candidate.evidence_key() == expected.evidence_key())
                })
                .and_then(|binding| {
                    assessment.and_then(|value| value.relation(binding.evidence_id()))
                })
                .map(|relation| match relation {
                    EvidenceRelation::Supports => crate::ExpectedRelation::Supports,
                    EvidenceRelation::Contradicts => crate::ExpectedRelation::Contradicts,
                    EvidenceRelation::Unclear => crate::ExpectedRelation::Unclear,
                    EvidenceRelation::Irrelevant => crate::ExpectedRelation::Irrelevant,
                });
            relation_samples.push((expected.relation(), actual));
        }
    }
    let relations = score_classes(
        &[
            crate::ExpectedRelation::Supports,
            crate::ExpectedRelation::Contradicts,
            crate::ExpectedRelation::Unclear,
            crate::ExpectedRelation::Irrelevant,
        ],
        relation_samples,
    );
    let sufficiency = score_classes(
        &[
            crate::ExpectedSufficiency::Sufficient,
            crate::ExpectedSufficiency::Insufficient,
            crate::ExpectedSufficiency::Indeterminate,
        ],
        sufficiency_samples,
    );
    VerificationMetrics::new(
        RelationMetrics::new(
            relations.accuracy,
            relations.missing_predictions,
            *relations
                .classes
                .get(&crate::ExpectedRelation::Supports)
                .expect("supports class exists"),
            *relations
                .classes
                .get(&crate::ExpectedRelation::Contradicts)
                .expect("contradicts class exists"),
            *relations
                .classes
                .get(&crate::ExpectedRelation::Unclear)
                .expect("unclear class exists"),
            *relations
                .classes
                .get(&crate::ExpectedRelation::Irrelevant)
                .expect("irrelevant class exists"),
        ),
        SufficiencyMetrics::new(
            sufficiency.accuracy,
            sufficiency.missing_predictions,
            *sufficiency
                .classes
                .get(&crate::ExpectedSufficiency::Sufficient)
                .expect("sufficient class exists"),
            *sufficiency
                .classes
                .get(&crate::ExpectedSufficiency::Insufficient)
                .expect("insufficient class exists"),
            *sufficiency
                .classes
                .get(&crate::ExpectedSufficiency::Indeterminate)
                .expect("indeterminate class exists"),
        ),
        unsupported_as_sufficient,
    )
}

fn grounding_metrics(
    case: &EvaluationCase,
    state: &ResearchControlState,
) -> EvidenceGroundingMetrics {
    let fixtures = case
        .source_snapshots()
        .iter()
        .map(|fixture| (*fixture.source_id(), fixture.content()))
        .collect::<BTreeMap<SourceId, &str>>();
    let research = state.investigation().research();
    let mut digest_matches = 0_u64;
    let mut missing_fixtures = 0_u64;
    for source in research.sources() {
        let Some(content) = fixtures.get(source.id()) else {
            missing_fixtures += 1;
            continue;
        };
        let hashed = digest::digest(&digest::SHA256, content.as_bytes());
        if hashed.as_ref() == source.content_digest().as_sha256() {
            digest_matches += 1;
        }
    }

    let mut exact_excerpts = 0_u64;
    let mut source_attributions = 0_u64;
    for evidence in research.evidence_items() {
        let Some(content) = fixtures.get(evidence.source_id()) else {
            continue;
        };
        source_attributions += 1;
        if content.contains(evidence.excerpt()) {
            exact_excerpts += 1;
        }
    }
    EvidenceGroundingMetrics::new(
        MetricCount::new(exact_excerpts, research.evidence_count() as u64),
        MetricCount::new(source_attributions, research.evidence_count() as u64),
        MetricCount::new(digest_matches, research.source_count() as u64),
        missing_fixtures,
    )
}

fn terminal_outcome(state: &ResearchControlState) -> ObservedTerminalOutcome {
    match state.status() {
        ResearchControlStatus::Completed => ObservedTerminalOutcome::Completed,
        ResearchControlStatus::Failed(_) => ObservedTerminalOutcome::Failed,
        ResearchControlStatus::Stopped(ResearchStopReason::OperatorStopped) => {
            ObservedTerminalOutcome::OperatorStopped
        }
        ResearchControlStatus::Stopped(ResearchStopReason::BudgetExhausted) => {
            ObservedTerminalOutcome::BudgetExhausted
        }
        ResearchControlStatus::Stopped(ResearchStopReason::Blocked(_)) => {
            ObservedTerminalOutcome::Blocked
        }
        ResearchControlStatus::AwaitingLimits
        | ResearchControlStatus::Researching
        | ResearchControlStatus::AwaitingNextStep => ObservedTerminalOutcome::NonTerminal,
    }
}
