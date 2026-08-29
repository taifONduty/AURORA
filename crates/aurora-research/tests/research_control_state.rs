use aurora_research::{
    BlockedReason, Claim, ClaimId, ContentDigest, Evidence, EvidenceAssessment, EvidenceId,
    EvidenceRelation, EvidenceSufficiency, IdentifiedResearchGap, InvestigationEvent,
    InvestigationFailure, InvestigationRecord, InvestigationResult, InvestigationTask,
    InvestigationTaskId, MediaType, ResearchControlEvent, ResearchControlLimits,
    ResearchControlRecord, ResearchControlState, ResearchControlStatus,
    ResearchControlTransitionError, ResearchEvent, ResearchFailure, ResearchGap, ResearchGapCause,
    ResearchGapId, ResearchGapStatus, ResearchPlan, ResearchRecord, ResearchRequest,
    ResearchStopReason, RetrievedAt, Source, SourceId, VerificationAssessment, VerificationId,
    VerificationRecord, VerificationTransitionError,
};
use uuid::Uuid;

#[test]
fn limits_are_first_unique_and_sequence_ordered_without_mutation() {
    let mut state = ResearchControlState::default();
    let before = state.clone();

    assert_eq!(
        state.apply(control_record(1, request_event(1))),
        Err(ResearchControlTransitionError::LimitsRequired)
    );
    assert_eq!(state, before);

    state
        .apply(control_record(1, limits_event(2)))
        .expect("limits apply");
    assert_eq!(state.status(), ResearchControlStatus::AwaitingNextStep);
    assert_eq!(state.limits(), Some(&ResearchControlLimits::new(2)));
    let configured = state.clone();

    assert_eq!(
        state.apply(control_record(3, request_event(1))),
        Err(ResearchControlTransitionError::Sequence {
            expected: 2,
            actual: 3,
        })
    );
    assert_eq!(state, configured);
    assert_eq!(
        state.apply(control_record(2, limits_event(3))),
        Err(ResearchControlTransitionError::DuplicateLimits)
    );
    assert_eq!(state, configured);
}

#[test]
fn nested_investigation_and_verification_use_current_research_state() {
    let mut state = completed_initial_task(2, true);
    let verification = assessment_record(
        1,
        1,
        31,
        &[(21, EvidenceRelation::Supports)],
        EvidenceSufficiency::Insufficient,
    );

    state
        .apply(control_record(
            6,
            ResearchControlEvent::VerificationRecorded(verification.clone()),
        ))
        .expect("current claim and evidence may be assessed");

    assert_eq!(
        state.verification().assessment(&verification_id(1)),
        Some(verification.assessment())
    );
    assert_eq!(state.investigation().research().claims().count(), 1);

    let before = state.clone();
    let future = assessment_record(
        2,
        2,
        31,
        &[(99, EvidenceRelation::Supports)],
        EvidenceSufficiency::Sufficient,
    );
    assert_eq!(
        state.apply(control_record(
            7,
            ResearchControlEvent::VerificationRecorded(future),
        )),
        Err(ResearchControlTransitionError::Verification(
            VerificationTransitionError::UnknownEvidence(evidence_id(99))
        ))
    );
    assert_eq!(state, before);
}

#[test]
fn generic_investigation_events_cannot_bypass_gap_linkage() {
    let mut state = completed_initial_task(1, false);
    let before = state.clone();

    assert_eq!(
        state.apply(control_record(
            6,
            ResearchControlEvent::InvestigationAdvanced(follow_up_record(5, 2, 1, "Gap")),
        )),
        Err(ResearchControlTransitionError::UnlinkedFollowUp)
    );
    assert_eq!(state, before);
}

#[test]
fn only_verification_assessments_needing_more_work_can_cause_gaps() {
    let cases = [
        (
            vec![(21, EvidenceRelation::Supports)],
            EvidenceSufficiency::Insufficient,
            true,
        ),
        (
            vec![(21, EvidenceRelation::Supports)],
            EvidenceSufficiency::Indeterminate,
            true,
        ),
        (
            vec![
                (21, EvidenceRelation::Supports),
                (22, EvidenceRelation::Contradicts),
            ],
            EvidenceSufficiency::Sufficient,
            true,
        ),
        (
            vec![(21, EvidenceRelation::Unclear)],
            EvidenceSufficiency::Sufficient,
            true,
        ),
        (
            vec![(21, EvidenceRelation::Supports)],
            EvidenceSufficiency::Sufficient,
            false,
        ),
    ];

    for (index, (relations, sufficiency, needs_gap)) in cases.into_iter().enumerate() {
        let mut state = completed_initial_task(1, true);
        let verification = index as u128 + 1;
        state
            .apply(control_record(
                6,
                ResearchControlEvent::VerificationRecorded(assessment_record(
                    1,
                    verification,
                    31,
                    &relations,
                    sufficiency,
                )),
            ))
            .expect("assessment applies");
        let gap = identified_gap(
            verification,
            ResearchGapCause::Verification(verification_id(verification)),
            "More evidence is needed",
        );
        let before = state.clone();
        let result = state.apply(control_record(7, ResearchControlEvent::GapIdentified(gap)));

        if needs_gap {
            result.expect("assessment admits a gap");
            assert!(state.gap(&gap_id(verification)).is_some());
        } else {
            assert_eq!(
                result,
                Err(
                    ResearchControlTransitionError::VerificationDoesNotRequireFollowUp(
                        verification_id(verification)
                    )
                )
            );
            assert_eq!(state, before);
        }
    }
}

#[test]
fn an_exact_failed_task_can_cause_one_gap() {
    let mut state = failed_initial_task();
    state
        .apply(control_record(
            6,
            ResearchControlEvent::GapIdentified(identified_gap(
                1,
                ResearchGapCause::InvestigationFailure(task_id(1)),
                "Initial investigation failed",
            )),
        ))
        .expect("failed task admits a gap");

    assert_eq!(state.gaps().count(), 1);

    let before = state.clone();
    assert_eq!(
        state.apply(control_record(
            7,
            ResearchControlEvent::GapIdentified(identified_gap(
                2,
                ResearchGapCause::InvestigationFailure(task_id(1)),
                "Duplicate cause",
            )),
        )),
        Err(ResearchControlTransitionError::DuplicateGapCause)
    );
    assert_eq!(state, before);
}

#[test]
fn failed_task_gap_cannot_invent_completed_parent_lineage() {
    let mut state = failed_initial_task();
    state
        .apply(control_record(
            6,
            ResearchControlEvent::GapIdentified(identified_gap(
                1,
                ResearchGapCause::InvestigationFailure(task_id(1)),
                "Initial investigation failed",
            )),
        ))
        .expect("failed task admits a gap");
    let before = state.clone();

    assert_eq!(
        state.apply(control_record(
            7,
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id: gap_id(1),
                investigation_record: follow_up_record(5, 2, 1, "Initial investigation failed",),
            },
        )),
        Err(ResearchControlTransitionError::InvestigationFailureCannotReplan(task_id(1)))
    );
    assert_eq!(state, before);
}

#[test]
fn gap_linked_follow_up_requires_matching_text_and_consumes_the_limit_atomically() {
    let mut state = verification_gap_state(1);
    let before_mismatch = state.clone();
    assert_eq!(
        state.apply(control_record(
            8,
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id: gap_id(1),
                investigation_record: follow_up_record(5, 2, 1, "Different gap"),
            },
        )),
        Err(ResearchControlTransitionError::FollowUpGapMismatch(gap_id(
            1
        )))
    );
    assert_eq!(state, before_mismatch);

    state
        .apply(control_record(
            8,
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id: gap_id(1),
                investigation_record: follow_up_record(5, 2, 1, "More evidence is needed"),
            },
        ))
        .expect("matching follow-up applies");
    assert_eq!(state.follow_up_count(), 1);
    assert_eq!(
        state
            .gap(&gap_id(1))
            .expect("gap exists")
            .follow_up_task_id(),
        Some(&task_id(2))
    );

    state
        .apply(control_record(
            9,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                6,
                InvestigationEvent::TaskStarted {
                    task_id: task_id(2),
                },
            )),
        ))
        .expect("follow-up starts");
    state
        .apply(control_record(
            10,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                7,
                InvestigationEvent::TaskCompleted {
                    task_id: task_id(2),
                    result: InvestigationResult::new(Vec::new()),
                },
            )),
        ))
        .expect("follow-up completes");
    state
        .apply(control_record(
            11,
            ResearchControlEvent::GapIdentified(identified_gap(
                2,
                ResearchGapCause::Verification(verification_id(1)),
                "Another gap",
            )),
        ))
        .expect_err("the same cause cannot open another gap");
}

#[test]
fn zero_follow_up_limit_rejection_changes_no_projection_or_counter() {
    let mut state = verification_gap_state(0);
    let before = state.clone();

    assert_eq!(
        state.apply(control_record(
            8,
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id: gap_id(1),
                investigation_record: follow_up_record(5, 2, 1, "More evidence is needed"),
            },
        )),
        Err(ResearchControlTransitionError::FollowUpLimitReached { limit: 0 })
    );
    assert_eq!(state, before);
}

#[test]
fn follow_up_limit_is_global_across_successive_gaps() {
    let mut state = linked_verification_gap();
    complete_follow_up(&mut state, Vec::new());
    state
        .apply(control_record(
            11,
            ResearchControlEvent::VerificationRecorded(assessment_record(
                2,
                2,
                31,
                &[(21, EvidenceRelation::Supports)],
                EvidenceSufficiency::Insufficient,
            )),
        ))
        .expect("second assessment applies");
    state
        .apply(control_record(
            12,
            ResearchControlEvent::GapIdentified(identified_gap(
                2,
                ResearchGapCause::Verification(verification_id(2)),
                "Independent corroboration is needed",
            )),
        ))
        .expect("second gap applies");
    let before = state.clone();

    assert_eq!(
        state.apply(control_record(
            13,
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id: gap_id(2),
                investigation_record: follow_up_record(
                    8,
                    3,
                    2,
                    "Independent corroboration is needed",
                ),
            },
        )),
        Err(ResearchControlTransitionError::FollowUpLimitReached { limit: 1 })
    );
    assert_eq!(state, before);
    assert_eq!(state.follow_up_count(), 1);
}

#[test]
fn unresolved_gap_may_exhaust_follow_up_work_without_becoming_completed() {
    let mut state = verification_gap_state(0);

    assert_eq!(
        state.apply(control_record(
            8,
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id: gap_id(1),
                investigation_record: follow_up_record(5, 2, 1, "More evidence is needed"),
            },
        )),
        Err(ResearchControlTransitionError::FollowUpLimitReached { limit: 0 })
    );
    state
        .apply(control_record(
            8,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                5,
                InvestigationEvent::ResearchStopped(ResearchStopReason::BudgetExhausted),
            )),
        ))
        .expect("exhausted research stops with the gap open");

    assert_eq!(
        state.status(),
        ResearchControlStatus::Stopped(ResearchStopReason::BudgetExhausted)
    );
    assert_eq!(
        state.gap(&gap_id(1)).expect("gap exists").status(),
        &ResearchGapStatus::Open
    );
    assert_eq!(
        state.apply(control_record(9, ResearchControlEvent::ResearchCompleted)),
        Err(ResearchControlTransitionError::ResearchAlreadyTerminal)
    );
}

#[test]
fn gap_resolution_requires_completed_follow_up_and_later_control_ready_verification() {
    let mut state = linked_verification_gap();
    let before = state.clone();
    assert_eq!(
        state.apply(control_record(
            9,
            ResearchControlEvent::GapResolved {
                gap_id: gap_id(1),
                verification_id: verification_id(1),
            },
        )),
        Err(ResearchControlTransitionError::FollowUpNotCompleted(
            task_id(2)
        ))
    );
    assert_eq!(state, before);

    complete_follow_up(&mut state, Vec::new());
    let before_old_verification = state.clone();
    assert_eq!(
        state.apply(control_record(
            11,
            ResearchControlEvent::GapResolved {
                gap_id: gap_id(1),
                verification_id: verification_id(1),
            },
        )),
        Err(ResearchControlTransitionError::ResolutionPrecedesFollowUp(
            verification_id(1)
        ))
    );
    assert_eq!(state, before_old_verification);

    state
        .apply(control_record(
            11,
            ResearchControlEvent::VerificationRecorded(assessment_record(
                2,
                2,
                31,
                &[(21, EvidenceRelation::Supports)],
                EvidenceSufficiency::Sufficient,
            )),
        ))
        .expect("later verification applies");
    state
        .apply(control_record(
            12,
            ResearchControlEvent::GapResolved {
                gap_id: gap_id(1),
                verification_id: verification_id(2),
            },
        ))
        .expect("later directional verification resolves the gap");

    assert_eq!(
        state.gap(&gap_id(1)).expect("gap exists").status(),
        &ResearchGapStatus::Resolved(verification_id(2))
    );
}

#[test]
fn wrong_claim_conflict_and_directionless_verification_do_not_resolve_a_gap() {
    let candidates = [
        (
            32,
            vec![(21, EvidenceRelation::Supports)],
            EvidenceSufficiency::Sufficient,
            ResearchControlTransitionError::ResolutionClaimMismatch {
                expected: claim_id(31),
                actual: claim_id(32),
            },
        ),
        (
            31,
            vec![
                (21, EvidenceRelation::Supports),
                (22, EvidenceRelation::Contradicts),
            ],
            EvidenceSufficiency::Sufficient,
            ResearchControlTransitionError::VerificationDoesNotResolveGap(verification_id(2)),
        ),
        (
            31,
            vec![(21, EvidenceRelation::Irrelevant)],
            EvidenceSufficiency::Sufficient,
            ResearchControlTransitionError::VerificationDoesNotResolveGap(verification_id(2)),
        ),
    ];

    for (claim, relations, sufficiency, expected) in candidates {
        let mut state = linked_verification_gap();
        complete_follow_up(&mut state, additional_claim_if_needed(claim));
        state
            .apply(control_record(
                11,
                ResearchControlEvent::VerificationRecorded(assessment_record(
                    2,
                    2,
                    claim,
                    &relations,
                    sufficiency,
                )),
            ))
            .expect("candidate verification applies");
        let before = state.clone();

        assert_eq!(
            state.apply(control_record(
                12,
                ResearchControlEvent::GapResolved {
                    gap_id: gap_id(1),
                    verification_id: verification_id(2),
                },
            )),
            Err(expected)
        );
        assert_eq!(state, before);
    }
}

#[test]
fn finite_adaptive_history_resolves_the_gap_then_completes_explicitly() {
    let mut state = linked_verification_gap();
    complete_follow_up(
        &mut state,
        vec![
            research_record(5, ResearchEvent::SourceRecorded(source(12))),
            research_record(6, ResearchEvent::EvidenceRecorded(evidence(23, 12))),
        ],
    );
    state
        .apply(control_record(
            11,
            ResearchControlEvent::VerificationRecorded(assessment_record(
                2,
                2,
                31,
                &[
                    (21, EvidenceRelation::Supports),
                    (23, EvidenceRelation::Supports),
                ],
                EvidenceSufficiency::Sufficient,
            )),
        ))
        .expect("sufficient verification applies");
    state
        .apply(control_record(
            12,
            ResearchControlEvent::GapResolved {
                gap_id: gap_id(1),
                verification_id: verification_id(2),
            },
        ))
        .expect("gap resolves");

    assert_eq!(state.status(), ResearchControlStatus::AwaitingNextStep);
    state
        .apply(control_record(13, ResearchControlEvent::ResearchCompleted))
        .expect("completion applies explicitly");
    assert_eq!(state.status(), ResearchControlStatus::Completed);

    let replayed = ResearchControlState::reconstruct(complete_history())
        .expect("complete history reconstructs");
    assert_eq!(replayed, state);
}

#[test]
fn completion_rejects_empty_unassessed_unaddressed_and_failed_research() {
    let mut empty = completed_initial_task(1, false);
    assert_eq!(
        empty.apply(control_record(6, ResearchControlEvent::ResearchCompleted)),
        Err(ResearchControlTransitionError::NoClaims)
    );

    let mut unassessed = completed_initial_task(1, true);
    assert_eq!(
        unassessed.apply(control_record(6, ResearchControlEvent::ResearchCompleted)),
        Err(ResearchControlTransitionError::ClaimNeedsAssessment(
            claim_id(31)
        ))
    );

    let mut adverse = completed_initial_task(1, true);
    adverse
        .apply(control_record(
            6,
            ResearchControlEvent::VerificationRecorded(assessment_record(
                1,
                1,
                31,
                &[(21, EvidenceRelation::Supports)],
                EvidenceSufficiency::Insufficient,
            )),
        ))
        .expect("assessment applies");
    adverse
        .apply(control_record(
            7,
            ResearchControlEvent::VerificationRecorded(assessment_record(
                2,
                2,
                31,
                &[(21, EvidenceRelation::Supports)],
                EvidenceSufficiency::Sufficient,
            )),
        ))
        .expect("control-ready assessment applies");
    assert_eq!(
        adverse.apply(control_record(8, ResearchControlEvent::ResearchCompleted)),
        Err(ResearchControlTransitionError::VerificationNeedsGap(
            verification_id(1)
        ))
    );

    let mut failed = failed_task_with_research();
    failed
        .apply(control_record(
            8,
            ResearchControlEvent::VerificationRecorded(assessment_record(
                1,
                1,
                31,
                &[(21, EvidenceRelation::Supports)],
                EvidenceSufficiency::Sufficient,
            )),
        ))
        .expect("control-ready assessment applies");
    assert_eq!(
        failed.apply(control_record(9, ResearchControlEvent::ResearchCompleted)),
        Err(ResearchControlTransitionError::InvestigationFailurePreventsCompletion(task_id(2)))
    );
}

#[test]
fn blocked_exhausted_operator_stopped_failed_and_completed_remain_distinct() {
    let stops = [
        ResearchStopReason::OperatorStopped,
        ResearchStopReason::BudgetExhausted,
        ResearchStopReason::Blocked(
            BlockedReason::new("Archive unavailable".to_owned()).expect("reason is valid"),
        ),
    ];
    for reason in stops {
        let mut state = completed_initial_task(0, false);
        state
            .apply(control_record(
                6,
                ResearchControlEvent::InvestigationAdvanced(investigation_record(
                    5,
                    InvestigationEvent::ResearchStopped(reason.clone()),
                )),
            ))
            .expect("stop applies");
        assert_eq!(
            state.status(),
            ResearchControlStatus::Stopped(reason.clone())
        );
        assert_eq!(
            state.apply(control_record(7, ResearchControlEvent::ResearchCompleted)),
            Err(ResearchControlTransitionError::ResearchAlreadyTerminal)
        );
    }

    let mut failed = failed_initial_task();
    let failure =
        ResearchFailure::new("Control process failed".to_owned()).expect("failure is valid");
    failed
        .apply(control_record(
            6,
            ResearchControlEvent::ResearchFailed(failure.clone()),
        ))
        .expect("overall failure applies");
    assert_eq!(failed.status(), ResearchControlStatus::Failed(failure));
}

fn linked_verification_gap() -> ResearchControlState {
    let mut state = verification_gap_state(1);
    state
        .apply(control_record(
            8,
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id: gap_id(1),
                investigation_record: follow_up_record(5, 2, 1, "More evidence is needed"),
            },
        ))
        .expect("follow-up links to the gap");
    state
}

fn complete_follow_up(state: &mut ResearchControlState, records: Vec<ResearchRecord>) {
    state
        .apply(control_record(
            9,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                6,
                InvestigationEvent::TaskStarted {
                    task_id: task_id(2),
                },
            )),
        ))
        .expect("follow-up starts");
    state
        .apply(control_record(
            10,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                7,
                InvestigationEvent::TaskCompleted {
                    task_id: task_id(2),
                    result: InvestigationResult::new(records),
                },
            )),
        ))
        .expect("follow-up completes");
}

fn additional_claim_if_needed(claim: u128) -> Vec<ResearchRecord> {
    if claim == 31 {
        Vec::new()
    } else {
        vec![research_record(
            5,
            ResearchEvent::ClaimProposed(
                Claim::new(
                    claim_id(claim),
                    format!("Proposed claim {claim}"),
                    vec![evidence_id(21)],
                )
                .expect("claim is valid"),
            ),
        )]
    }
}

fn complete_history() -> Vec<ResearchControlRecord> {
    vec![
        control_record(1, limits_event(1)),
        control_record(2, request_event(1)),
        control_record(3, plan_event(2, &[1])),
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
                    result: InvestigationResult::new(initial_research_records()),
                },
            )),
        ),
        control_record(
            6,
            ResearchControlEvent::VerificationRecorded(assessment_record(
                1,
                1,
                31,
                &[(21, EvidenceRelation::Supports)],
                EvidenceSufficiency::Insufficient,
            )),
        ),
        control_record(
            7,
            ResearchControlEvent::GapIdentified(identified_gap(
                1,
                ResearchGapCause::Verification(verification_id(1)),
                "More evidence is needed",
            )),
        ),
        control_record(
            8,
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id: gap_id(1),
                investigation_record: follow_up_record(5, 2, 1, "More evidence is needed"),
            },
        ),
        control_record(
            9,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                6,
                InvestigationEvent::TaskStarted {
                    task_id: task_id(2),
                },
            )),
        ),
        control_record(
            10,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                7,
                InvestigationEvent::TaskCompleted {
                    task_id: task_id(2),
                    result: InvestigationResult::new(vec![
                        research_record(5, ResearchEvent::SourceRecorded(source(12))),
                        research_record(6, ResearchEvent::EvidenceRecorded(evidence(23, 12))),
                    ]),
                },
            )),
        ),
        control_record(
            11,
            ResearchControlEvent::VerificationRecorded(assessment_record(
                2,
                2,
                31,
                &[
                    (21, EvidenceRelation::Supports),
                    (23, EvidenceRelation::Supports),
                ],
                EvidenceSufficiency::Sufficient,
            )),
        ),
        control_record(
            12,
            ResearchControlEvent::GapResolved {
                gap_id: gap_id(1),
                verification_id: verification_id(2),
            },
        ),
        control_record(13, ResearchControlEvent::ResearchCompleted),
    ]
}

fn verification_gap_state(limit: u32) -> ResearchControlState {
    let mut state = completed_initial_task(limit, true);
    state
        .apply(control_record(
            6,
            ResearchControlEvent::VerificationRecorded(assessment_record(
                1,
                1,
                31,
                &[(21, EvidenceRelation::Supports)],
                EvidenceSufficiency::Insufficient,
            )),
        ))
        .expect("assessment applies");
    state
        .apply(control_record(
            7,
            ResearchControlEvent::GapIdentified(identified_gap(
                1,
                ResearchGapCause::Verification(verification_id(1)),
                "More evidence is needed",
            )),
        ))
        .expect("gap applies");
    state
}

fn completed_initial_task(limit: u32, with_research: bool) -> ResearchControlState {
    let result = if with_research {
        InvestigationResult::new(initial_research_records())
    } else {
        InvestigationResult::new(Vec::new())
    };
    ResearchControlState::reconstruct(vec![
        control_record(1, limits_event(limit)),
        control_record(2, request_event(1)),
        control_record(3, plan_event(2, &[1])),
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
                    result,
                },
            )),
        ),
    ])
    .expect("initial task history is valid")
}

fn failed_initial_task() -> ResearchControlState {
    ResearchControlState::reconstruct(vec![
        control_record(1, limits_event(1)),
        control_record(2, request_event(1)),
        control_record(3, plan_event(2, &[1])),
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
                    failure: InvestigationFailure::new("Search failed".to_owned())
                        .expect("failure is valid"),
                },
            )),
        ),
    ])
    .expect("failed task history is valid")
}

fn failed_task_with_research() -> ResearchControlState {
    ResearchControlState::reconstruct(vec![
        control_record(1, limits_event(1)),
        control_record(2, request_event(1)),
        control_record(3, plan_event(2, &[1, 2])),
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
                    result: InvestigationResult::new(initial_research_records()),
                },
            )),
        ),
        control_record(
            6,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                5,
                InvestigationEvent::TaskStarted {
                    task_id: task_id(2),
                },
            )),
        ),
        control_record(
            7,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                6,
                InvestigationEvent::TaskFailed {
                    task_id: task_id(2),
                    failure: InvestigationFailure::new("Search failed".to_owned())
                        .expect("failure is valid"),
                },
            )),
        ),
    ])
    .expect("mixed task history is valid")
}

fn limits_event(limit: u32) -> ResearchControlEvent {
    ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(limit))
}

fn request_event(sequence: u64) -> ResearchControlEvent {
    ResearchControlEvent::InvestigationAdvanced(investigation_record(
        sequence,
        InvestigationEvent::RequestRecorded(
            ResearchRequest::new("Research question".to_owned()).expect("request is valid"),
        ),
    ))
}

fn plan_event(sequence: u64, ids: &[u128]) -> ResearchControlEvent {
    let tasks = ids
        .iter()
        .map(|id| {
            InvestigationTask::initial(task_id(*id), format!("Objective {id}"))
                .expect("task is valid")
        })
        .collect();
    ResearchControlEvent::InvestigationAdvanced(investigation_record(
        sequence,
        InvestigationEvent::PlanRecorded(ResearchPlan::new(tasks).expect("plan is valid")),
    ))
}

fn follow_up_record(sequence: u64, id: u128, parent: u128, gap: &str) -> InvestigationRecord {
    investigation_record(
        sequence,
        InvestigationEvent::FollowUpRecorded(
            InvestigationTask::follow_up(
                task_id(id),
                task_id(parent),
                format!("Follow-up objective {id}"),
                ResearchGap::new(gap.to_owned()).expect("gap is valid"),
            )
            .expect("follow-up is valid"),
        ),
    )
}

fn identified_gap(id: u128, cause: ResearchGapCause, description: &str) -> IdentifiedResearchGap {
    IdentifiedResearchGap::new(
        gap_id(id),
        cause,
        ResearchGap::new(description.to_owned()).expect("gap is valid"),
    )
}

fn assessment_record(
    sequence: u64,
    id: u128,
    claim: u128,
    relations: &[(u128, EvidenceRelation)],
    sufficiency: EvidenceSufficiency,
) -> VerificationRecord {
    VerificationRecord::new(
        sequence,
        VerificationAssessment::new(
            verification_id(id),
            claim_id(claim),
            relations
                .iter()
                .map(|(evidence, relation)| {
                    EvidenceAssessment::new(evidence_id(*evidence), *relation)
                })
                .collect(),
            sufficiency,
        )
        .expect("assessment is valid"),
    )
    .expect("verification record is valid")
}

fn initial_research_records() -> Vec<ResearchRecord> {
    vec![
        research_record(1, ResearchEvent::SourceRecorded(source(11))),
        research_record(2, ResearchEvent::EvidenceRecorded(evidence(21, 11))),
        research_record(3, ResearchEvent::EvidenceRecorded(evidence(22, 11))),
        research_record(4, ResearchEvent::ClaimProposed(claim(31, 21))),
    ]
}

fn source(id: u128) -> Source {
    Source::new(
        source_id(id),
        ContentDigest::sha256([id as u8; 32]),
        format!("https://example.test/source/{id}"),
        None,
        RetrievedAt::new("2026-08-29T10:00:00Z").expect("time is valid"),
        MediaType::new("text/plain").expect("media type is valid"),
    )
    .expect("source is valid")
}

fn evidence(id: u128, source: u128) -> Evidence {
    Evidence::new(
        evidence_id(id),
        source_id(source),
        format!("Evidence excerpt {id}"),
    )
    .expect("evidence is valid")
}

fn claim(id: u128, evidence: u128) -> Claim {
    Claim::new(
        claim_id(id),
        format!("Proposed claim {id}"),
        vec![evidence_id(evidence)],
    )
    .expect("claim is valid")
}

fn control_record(sequence: u64, event: ResearchControlEvent) -> ResearchControlRecord {
    ResearchControlRecord::new(sequence, event).expect("control record is valid")
}

fn investigation_record(sequence: u64, event: InvestigationEvent) -> InvestigationRecord {
    InvestigationRecord::new(sequence, event).expect("investigation record is valid")
}

fn research_record(sequence: u64, event: ResearchEvent) -> ResearchRecord {
    ResearchRecord::new(sequence, event).expect("research record is valid")
}

fn gap_id(value: u128) -> ResearchGapId {
    uuid(value).parse().expect("gap identifier is valid")
}

fn task_id(value: u128) -> InvestigationTaskId {
    uuid(value).parse().expect("task identifier is valid")
}

fn verification_id(value: u128) -> VerificationId {
    uuid(value)
        .parse()
        .expect("verification identifier is valid")
}

fn source_id(value: u128) -> SourceId {
    uuid(value).parse().expect("source identifier is valid")
}

fn evidence_id(value: u128) -> EvidenceId {
    uuid(value).parse().expect("evidence identifier is valid")
}

fn claim_id(value: u128) -> ClaimId {
    uuid(value).parse().expect("claim identifier is valid")
}

fn uuid(value: u128) -> String {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).hyphenated().to_string()
}
