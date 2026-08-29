use std::str::FromStr;

use aurora_research::{
    BlockedReason, Claim, ClaimId, ContentDigest, Evidence, EvidenceAssessment, EvidenceId,
    EvidenceRelation, EvidenceSufficiency, InvestigationEvent, InvestigationFailure,
    InvestigationRecord, InvestigationResult, InvestigationTask, InvestigationTaskId, MediaType,
    ResearchControlEvent, ResearchControlLimits, ResearchControlRecord, ResearchControlState,
    ResearchEvent, ResearchFailure, ResearchGap, ResearchGapCause, ResearchGapId, ResearchPlan,
    ResearchRecord, ResearchRequest, ResearchStopReason, RetrievedAt, Source, SourceId,
    SynthesisBasis, SynthesisReportScope, SynthesisValidationError, VerificationAssessment,
    VerificationId, VerificationRecord,
};
use uuid::Uuid;

type AssessmentFixture = (u128, Vec<(u128, EvidenceRelation)>, EvidenceSufficiency);

#[test]
fn terminal_completed_and_stopped_research_produce_their_report_scopes() {
    let completed = completed_state();
    assert_eq!(
        SynthesisBasis::from_state(&completed)
            .expect("completed research has a synthesis basis")
            .scope(),
        &SynthesisReportScope::Complete
    );

    for reason in [
        ResearchStopReason::OperatorStopped,
        ResearchStopReason::BudgetExhausted,
        ResearchStopReason::Blocked(
            BlockedReason::new("source access is unavailable".to_owned()).expect("reason is valid"),
        ),
    ] {
        let basis = SynthesisBasis::from_state(&stopped_state(reason.clone(), true))
            .expect("stopped research with an assessment has a synthesis basis");
        assert_eq!(basis.scope(), &SynthesisReportScope::Partial(reason));
    }
}

#[test]
fn rejects_failed_nonterminal_and_unassessed_stopped_research() {
    assert!(matches!(
        SynthesisBasis::from_state(&failed_state()),
        Err(SynthesisValidationError::FailedResearch)
    ));
    assert!(matches!(
        SynthesisBasis::from_state(&ResearchControlState::default()),
        Err(SynthesisValidationError::ResearchNotTerminal)
    ));
    assert!(matches!(
        SynthesisBasis::from_state(&stopped_state(ResearchStopReason::OperatorStopped, false)),
        Err(SynthesisValidationError::NoReportableClaims)
    ));
}

#[test]
fn classifies_assessment_history_conservatively() {
    let cases = [
        (
            vec![(
                1,
                vec![(1, EvidenceRelation::Supports)],
                EvidenceSufficiency::Sufficient,
            )],
            aurora_research::ClaimPresentation::Established,
        ),
        (
            vec![(
                1,
                vec![(1, EvidenceRelation::Supports)],
                EvidenceSufficiency::Insufficient,
            )],
            aurora_research::ClaimPresentation::Unresolved,
        ),
        (
            vec![(
                1,
                vec![(1, EvidenceRelation::Unclear)],
                EvidenceSufficiency::Indeterminate,
            )],
            aurora_research::ClaimPresentation::Unresolved,
        ),
        (
            vec![(
                1,
                vec![(1, EvidenceRelation::Contradicts)],
                EvidenceSufficiency::Sufficient,
            )],
            aurora_research::ClaimPresentation::Contested,
        ),
        (
            vec![
                (
                    1,
                    vec![(1, EvidenceRelation::Supports)],
                    EvidenceSufficiency::Sufficient,
                ),
                (
                    2,
                    vec![(2, EvidenceRelation::Contradicts)],
                    EvidenceSufficiency::Sufficient,
                ),
            ],
            aurora_research::ClaimPresentation::Contested,
        ),
    ];

    for (assessments, expected) in cases {
        let basis = SynthesisBasis::from_state(&stopped_with_assessments(assessments))
            .expect("assessed stopped research has a basis");
        assert_eq!(
            basis
                .claim(&claim_id(1))
                .expect("assessed claim is reportable")
                .presentation(),
            expected
        );
    }
}

#[test]
fn preserves_relevant_gap_history_and_exact_nonirrelevant_citation_candidates() {
    let open = SynthesisBasis::from_state(&open_gap_state())
        .expect("stopped research with an open gap has a basis");
    let open_claim = open.claim(&claim_id(1)).expect("claim is reportable");
    assert_eq!(
        open_claim.presentation(),
        aurora_research::ClaimPresentation::Unresolved
    );
    assert_eq!(open_claim.gaps().count(), 1);

    let resolved = SynthesisBasis::from_state(&resolved_gap_state())
        .expect("stopped research with a resolved gap has a basis");
    let resolved_claim = resolved.claim(&claim_id(1)).expect("claim is reportable");
    assert_eq!(
        resolved_claim.presentation(),
        aurora_research::ClaimPresentation::Established
    );
    assert_eq!(resolved_claim.gaps().count(), 1);

    let historically_contested = SynthesisBasis::from_state(&resolved_contradiction_gap_state())
        .expect("resolved contradiction history has a basis");
    assert_eq!(
        historically_contested
            .claim(&claim_id(1))
            .expect("claim is reportable")
            .presentation(),
        aurora_research::ClaimPresentation::Contested
    );

    let basis = SynthesisBasis::from_state(&stopped_with_assessments(vec![(
        1,
        vec![
            (1, EvidenceRelation::Irrelevant),
            (2, EvidenceRelation::Supports),
        ],
        EvidenceSufficiency::Sufficient,
    )]))
    .expect("assessed stopped research has a basis");
    let claim = basis.claim(&claim_id(1)).expect("claim is reportable");
    let citations = claim.citations().collect::<Vec<_>>();
    assert_eq!(citations.len(), 1);
    assert_eq!(citations[0].evidence().id(), &evidence_id(2));
    assert_eq!(citations[0].source().id(), &source_id(2));
    assert!(claim.evidence(&evidence_id(2)).is_some());
    assert!(claim.source(&source_id(2)).is_some());
    assert!(basis.is_known_claim(&claim_id(1)));
}

#[test]
fn citation_candidates_follow_presentation_and_retain_their_claim_identifier() {
    let established = SynthesisBasis::from_state(&stopped_with_assessments(vec![(
        1,
        vec![
            (1, EvidenceRelation::Supports),
            (2, EvidenceRelation::Unclear),
        ],
        EvidenceSufficiency::Sufficient,
    )]))
    .expect("established claim has a basis");
    let established_citations = established
        .claim(&claim_id(1))
        .expect("claim is reportable")
        .citations()
        .collect::<Vec<_>>();
    assert_eq!(established_citations.len(), 1);
    assert_eq!(established_citations[0].evidence().id(), &evidence_id(1));
    assert_eq!(established_citations[0].claim_id(), &claim_id(1));
    assert_eq!(
        established_citations[0]
            .provenance()
            .map(|(verification_id, relation)| (*verification_id, relation))
            .collect::<Vec<_>>(),
        vec![(verification_id(1), EvidenceRelation::Supports)]
    );
    assert!(!established_citations[0].is_fallback());

    let contested = SynthesisBasis::from_state(&stopped_with_assessments(vec![
        (
            1,
            vec![
                (1, EvidenceRelation::Supports),
                (2, EvidenceRelation::Contradicts),
            ],
            EvidenceSufficiency::Sufficient,
        ),
        (
            2,
            vec![(1, EvidenceRelation::Unclear)],
            EvidenceSufficiency::Indeterminate,
        ),
    ]))
    .expect("contested claim has a basis");
    let contested_citations = contested
        .claim(&claim_id(1))
        .expect("claim is reportable")
        .citations()
        .collect::<Vec<_>>();
    assert_eq!(contested_citations.len(), 2);
    assert_eq!(contested_citations[0].evidence().id(), &evidence_id(1));
    assert_eq!(contested_citations[1].evidence().id(), &evidence_id(2));

    let unresolved = SynthesisBasis::from_state(&stopped_with_assessments(vec![(
        1,
        vec![(2, EvidenceRelation::Irrelevant)],
        EvidenceSufficiency::Indeterminate,
    )]))
    .expect("unresolved claim has a basis");
    let unresolved_claim = unresolved.claim(&claim_id(1)).expect("claim is reportable");
    let unresolved_citations = unresolved_claim.citations().collect::<Vec<_>>();
    assert_eq!(unresolved_citations.len(), 1);
    assert_eq!(unresolved_citations[0].evidence().id(), &evidence_id(1));
    assert!(unresolved_citations[0].provenance().next().is_none());
    assert!(unresolved_citations[0].is_fallback());
    assert!(unresolved_claim.evidence(&evidence_id(1)).is_some());
    assert!(unresolved_claim.source(&source_id(1)).is_some());
}

fn completed_state() -> ResearchControlState {
    let mut records = assessed_research_records();
    records.push(control_record(7, ResearchControlEvent::ResearchCompleted));
    ResearchControlState::reconstruct(records).expect("completed history reconstructs")
}

fn stopped_state(reason: ResearchStopReason, assessed: bool) -> ResearchControlState {
    let mut records = if assessed {
        assessed_research_records()
    } else {
        initial_research_records()
    };
    let sequence = records.len() as u64 + 1;
    records.push(control_record(
        sequence,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            5,
            InvestigationEvent::ResearchStopped(reason),
        )),
    ));
    ResearchControlState::reconstruct(records).expect("stopped history reconstructs")
}

fn stopped_with_assessments(assessments: Vec<AssessmentFixture>) -> ResearchControlState {
    let mut records = initial_research_records();
    let mut outer_sequence = 6;
    for (sequence, (id, relations, sufficiency)) in assessments.into_iter().enumerate() {
        records.push(control_record(
            outer_sequence,
            ResearchControlEvent::VerificationRecorded(verification_record(
                (sequence + 1) as u64,
                VerificationAssessment::new(
                    verification_id(id),
                    claim_id(1),
                    relations
                        .into_iter()
                        .map(|(evidence, relation)| {
                            EvidenceAssessment::new(evidence_id(evidence), relation)
                        })
                        .collect(),
                    sufficiency,
                )
                .expect("assessment is valid"),
            )),
        ));
        outer_sequence += 1;
    }
    records.push(control_record(
        outer_sequence,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            5,
            InvestigationEvent::ResearchStopped(ResearchStopReason::OperatorStopped),
        )),
    ));
    ResearchControlState::reconstruct(records).expect("stopped history reconstructs")
}

fn open_gap_state() -> ResearchControlState {
    let mut records = insufficient_assessed_research_records(EvidenceRelation::Supports);
    records.push(control_record(
        7,
        ResearchControlEvent::GapIdentified(identified_gap(1, 1, "More evidence is needed")),
    ));
    records.push(control_record(
        8,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            5,
            InvestigationEvent::ResearchStopped(ResearchStopReason::OperatorStopped),
        )),
    ));
    ResearchControlState::reconstruct(records).expect("open-gap history reconstructs")
}

fn resolved_gap_state() -> ResearchControlState {
    resolved_gap_state_with_initial_relation(EvidenceRelation::Supports)
}

fn resolved_contradiction_gap_state() -> ResearchControlState {
    resolved_gap_state_with_initial_relation(EvidenceRelation::Contradicts)
}

fn resolved_gap_state_with_initial_relation(
    initial_relation: EvidenceRelation,
) -> ResearchControlState {
    let mut records = insufficient_assessed_research_records(initial_relation);
    let gap = identified_gap(1, 1, "More evidence is needed");
    records.push(control_record(7, ResearchControlEvent::GapIdentified(gap)));
    records.push(control_record(
        8,
        ResearchControlEvent::GapFollowUpRecorded {
            gap_id: gap_id(1),
            investigation_record: investigation_record(
                5,
                InvestigationEvent::FollowUpRecorded(
                    InvestigationTask::follow_up(
                        task_id(2),
                        task_id(1),
                        "Find more evidence".to_owned(),
                        ResearchGap::new("More evidence is needed".to_owned())
                            .expect("gap is valid"),
                    )
                    .expect("follow-up task is valid"),
                ),
            ),
        },
    ));
    records.push(control_record(
        9,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            6,
            InvestigationEvent::TaskStarted {
                task_id: task_id(2),
            },
        )),
    ));
    records.push(control_record(
        10,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            7,
            InvestigationEvent::TaskCompleted {
                task_id: task_id(2),
                result: InvestigationResult::new(Vec::new()),
            },
        )),
    ));
    records.push(control_record(
        11,
        ResearchControlEvent::VerificationRecorded(verification_record(
            2,
            VerificationAssessment::new(
                verification_id(2),
                claim_id(1),
                vec![EvidenceAssessment::new(
                    evidence_id(2),
                    EvidenceRelation::Supports,
                )],
                EvidenceSufficiency::Sufficient,
            )
            .expect("assessment is valid"),
        )),
    ));
    records.push(control_record(
        12,
        ResearchControlEvent::GapResolved {
            gap_id: gap_id(1),
            verification_id: verification_id(2),
        },
    ));
    records.push(control_record(
        13,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            8,
            InvestigationEvent::ResearchStopped(ResearchStopReason::OperatorStopped),
        )),
    ));
    ResearchControlState::reconstruct(records).expect("resolved-gap history reconstructs")
}

fn failed_state() -> ResearchControlState {
    ResearchControlState::reconstruct([
        control_record(
            1,
            ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(1)),
        ),
        control_record(
            2,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                1,
                InvestigationEvent::RequestRecorded(request()),
            )),
        ),
        control_record(
            3,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                2,
                InvestigationEvent::PlanRecorded(plan()),
            )),
        ),
        control_record(
            4,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                3,
                InvestigationEvent::TaskStarted {
                    task_id: task_id(1),
                },
            )),
        ),
        control_record(
            5,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                4,
                InvestigationEvent::TaskFailed {
                    task_id: task_id(1),
                    failure: InvestigationFailure::new("retrieval failed".to_owned())
                        .expect("failure is valid"),
                },
            )),
        ),
        control_record(
            6,
            ResearchControlEvent::ResearchFailed(
                ResearchFailure::new("research failed".to_owned()).expect("failure is valid"),
            ),
        ),
    ])
    .expect("failed history reconstructs")
}

fn assessed_research_records() -> Vec<ResearchControlRecord> {
    let mut records = initial_research_records();
    records.push(control_record(
        6,
        ResearchControlEvent::VerificationRecorded(verification_record(
            1,
            VerificationAssessment::new(
                verification_id(1),
                claim_id(1),
                vec![EvidenceAssessment::new(
                    evidence_id(1),
                    EvidenceRelation::Supports,
                )],
                EvidenceSufficiency::Sufficient,
            )
            .expect("assessment is valid"),
        )),
    ));
    records
}

fn insufficient_assessed_research_records(
    relation: EvidenceRelation,
) -> Vec<ResearchControlRecord> {
    let mut records = initial_research_records();
    records.push(control_record(
        6,
        ResearchControlEvent::VerificationRecorded(verification_record(
            1,
            VerificationAssessment::new(
                verification_id(1),
                claim_id(1),
                vec![EvidenceAssessment::new(evidence_id(1), relation)],
                EvidenceSufficiency::Insufficient,
            )
            .expect("assessment is valid"),
        )),
    ));
    records
}

fn initial_research_records() -> Vec<ResearchControlRecord> {
    vec![
        control_record(
            1,
            ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(1)),
        ),
        control_record(
            2,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                1,
                InvestigationEvent::RequestRecorded(request()),
            )),
        ),
        control_record(
            3,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                2,
                InvestigationEvent::PlanRecorded(plan()),
            )),
        ),
        control_record(
            4,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                3,
                InvestigationEvent::TaskStarted {
                    task_id: task_id(1),
                },
            )),
        ),
        control_record(
            5,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                4,
                InvestigationEvent::TaskCompleted {
                    task_id: task_id(1),
                    result: InvestigationResult::new(research_records()),
                },
            )),
        ),
    ]
}

fn request() -> ResearchRequest {
    ResearchRequest::new("What does the evidence establish?".to_owned()).expect("request is valid")
}

fn plan() -> ResearchPlan {
    ResearchPlan::new(vec![
        InvestigationTask::initial(task_id(1), "Assess the available source".to_owned())
            .expect("task is valid"),
    ])
    .expect("plan is valid")
}

fn research_records() -> Vec<ResearchRecord> {
    vec![
        ResearchRecord::new(
            1,
            ResearchEvent::SourceRecorded(
                Source::new(
                    source_id(1),
                    ContentDigest::sha256([1; 32]),
                    "https://example.test/source".to_owned(),
                    Some("Source".to_owned()),
                    RetrievedAt::new("2026-08-29T00:00:00Z".to_owned())
                        .expect("timestamp is valid"),
                    MediaType::new("text/plain".to_owned()).expect("media type is valid"),
                )
                .expect("source is valid"),
            ),
        )
        .expect("record is valid"),
        ResearchRecord::new(
            2,
            ResearchEvent::EvidenceRecorded(
                Evidence::new(
                    evidence_id(1),
                    source_id(1),
                    "The source supports the claim.".to_owned(),
                )
                .expect("evidence is valid"),
            ),
        )
        .expect("record is valid"),
        ResearchRecord::new(
            3,
            ResearchEvent::SourceRecorded(
                Source::new(
                    source_id(2),
                    ContentDigest::sha256([2; 32]),
                    "https://example.test/second-source".to_owned(),
                    Some("Second source".to_owned()),
                    RetrievedAt::new("2026-08-29T00:00:00Z".to_owned())
                        .expect("timestamp is valid"),
                    MediaType::new("text/plain".to_owned()).expect("media type is valid"),
                )
                .expect("source is valid"),
            ),
        )
        .expect("record is valid"),
        ResearchRecord::new(
            4,
            ResearchEvent::EvidenceRecorded(
                Evidence::new(
                    evidence_id(2),
                    source_id(2),
                    "The second source supports the claim.".to_owned(),
                )
                .expect("evidence is valid"),
            ),
        )
        .expect("record is valid"),
        ResearchRecord::new(
            5,
            ResearchEvent::ClaimProposed(
                Claim::new(
                    claim_id(1),
                    "The claim is supported.".to_owned(),
                    vec![evidence_id(1)],
                )
                .expect("claim is valid"),
            ),
        )
        .expect("record is valid"),
    ]
}

fn control_record(sequence: u64, event: ResearchControlEvent) -> ResearchControlRecord {
    ResearchControlRecord::new(sequence, event).expect("control record is valid")
}

fn investigation_record(sequence: u64, event: InvestigationEvent) -> InvestigationRecord {
    InvestigationRecord::new(sequence, event).expect("investigation record is valid")
}

fn verification_record(sequence: u64, assessment: VerificationAssessment) -> VerificationRecord {
    VerificationRecord::new(sequence, assessment).expect("verification record is valid")
}

fn source_id(value: u128) -> SourceId {
    SourceId::from_str(&uuid(value)).expect("source identifier is valid")
}

fn evidence_id(value: u128) -> EvidenceId {
    EvidenceId::from_str(&uuid(value)).expect("evidence identifier is valid")
}

fn claim_id(value: u128) -> ClaimId {
    ClaimId::from_str(&uuid(value)).expect("claim identifier is valid")
}

fn task_id(value: u128) -> InvestigationTaskId {
    InvestigationTaskId::from_str(&uuid(value)).expect("task identifier is valid")
}

fn verification_id(value: u128) -> VerificationId {
    VerificationId::from_str(&uuid(value)).expect("verification identifier is valid")
}

fn gap_id(value: u128) -> ResearchGapId {
    ResearchGapId::from_str(&uuid(value)).expect("gap identifier is valid")
}

fn identified_gap(
    id: u128,
    verification: u128,
    description: &str,
) -> aurora_research::IdentifiedResearchGap {
    aurora_research::IdentifiedResearchGap::new(
        gap_id(id),
        ResearchGapCause::Verification(verification_id(verification)),
        ResearchGap::new(description.to_owned()).expect("gap is valid"),
    )
}

fn uuid(value: u128) -> String {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).to_string()
}
