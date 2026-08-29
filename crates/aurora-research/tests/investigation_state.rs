use aurora_research::{
    BlockedReason, Claim, ClaimId, ContentDigest, Evidence, EvidenceId, InvestigationEvent,
    InvestigationFailure, InvestigationRecord, InvestigationResult, InvestigationState,
    InvestigationStatus, InvestigationTask, InvestigationTaskId, InvestigationTaskStatus,
    InvestigationTransitionError, MediaType, ResearchEvent, ResearchGap, ResearchPlan,
    ResearchRecord, ResearchRequest, ResearchStopReason, RetrievedAt, Source, SourceId,
    TransitionError,
};

#[test]
fn completed_work_without_follow_up_awaits_a_next_step() {
    let records = vec![
        request_record(1),
        plan_record(2, &[1, 2]),
        record(
            3,
            InvestigationEvent::TaskStarted {
                task_id: task_id(1),
            },
        ),
        record(
            4,
            InvestigationEvent::TaskCompleted {
                task_id: task_id(1),
                result: InvestigationResult::new(Vec::new()),
            },
        ),
        record(
            5,
            InvestigationEvent::TaskStarted {
                task_id: task_id(2),
            },
        ),
        record(
            6,
            InvestigationEvent::TaskFailed {
                task_id: task_id(2),
                failure: InvestigationFailure::new("No material found".to_owned())
                    .expect("failure is valid"),
            },
        ),
    ];

    let state = InvestigationState::reconstruct(records).expect("history is valid");

    assert_eq!(state.status(), InvestigationStatus::AwaitingNextStep);
    assert_eq!(
        state.task(&task_id(1)).expect("task exists").status(),
        &InvestigationTaskStatus::Completed
    );
    assert_eq!(
        state.task(&task_id(2)).expect("task exists").status(),
        &InvestigationTaskStatus::Failed(
            InvestigationFailure::new("No material found".to_owned()).expect("failure is valid")
        )
    );
    assert_eq!(state.next_pending_task(), None);
}

#[test]
fn task_order_is_stable_and_multiple_tasks_may_be_active() {
    let mut state =
        InvestigationState::reconstruct(vec![request_record(1), plan_record(2, &[7, 3, 9])])
            .expect("history is valid");

    assert_eq!(
        state.next_pending_task().map(|task| *task.id()),
        Some(task_id(7))
    );
    state
        .apply(record(
            3,
            InvestigationEvent::TaskStarted {
                task_id: task_id(7),
            },
        ))
        .expect("first task starts");
    state
        .apply(record(
            4,
            InvestigationEvent::TaskStarted {
                task_id: task_id(3),
            },
        ))
        .expect("second task starts");

    assert_eq!(
        state.next_pending_task().map(|task| *task.id()),
        Some(task_id(9))
    );
    assert_eq!(
        state
            .tasks()
            .map(|task| *task.task().id())
            .collect::<Vec<_>>(),
        vec![task_id(7), task_id(3), task_id(9)]
    );
}

#[test]
fn request_plan_and_task_lifecycle_order_is_enforced_without_mutation() {
    let mut state = InvestigationState::default();
    let before = state.clone();
    assert_eq!(
        state.apply(plan_record(1, &[1])),
        Err(InvestigationTransitionError::RequestRequired)
    );
    assert_eq!(state, before);

    state.apply(request_record(1)).expect("request applies");
    state
        .apply(plan_record(2, &[1]))
        .expect("plan applies after request");
    let before = state.clone();
    assert_eq!(
        state.apply(record(
            3,
            InvestigationEvent::TaskCompleted {
                task_id: task_id(1),
                result: InvestigationResult::new(Vec::new()),
            },
        )),
        Err(InvestigationTransitionError::TaskNotActive {
            task_id: task_id(1),
            actual: InvestigationTaskStatus::Pending,
        })
    );
    assert_eq!(state, before);
}

#[test]
fn request_and_plan_are_each_recorded_once() {
    let mut state = InvestigationState::default();
    state.apply(request_record(1)).expect("request applies");
    let before_duplicate_request = state.clone();
    assert_eq!(
        state.apply(request_record(2)),
        Err(InvestigationTransitionError::DuplicateRequest)
    );
    assert_eq!(state, before_duplicate_request);

    state
        .apply(plan_record(2, &[1]))
        .expect("first plan applies");
    let before_duplicate_plan = state.clone();
    assert_eq!(
        state.apply(plan_record(3, &[2])),
        Err(InvestigationTransitionError::DuplicatePlan)
    );
    assert_eq!(state, before_duplicate_plan);
}

#[test]
fn terminal_task_cannot_be_reopened() {
    let mut state = completed_state();
    let before = state.clone();

    assert_eq!(
        state.apply(record(
            5,
            InvestigationEvent::TaskStarted {
                task_id: task_id(1)
            }
        )),
        Err(InvestigationTransitionError::TaskNotPending {
            task_id: task_id(1),
            actual: InvestigationTaskStatus::Completed,
        })
    );
    assert_eq!(state, before);
}

#[test]
fn completion_atomically_admits_source_evidence_and_claim() {
    let mut state = active_state();

    state
        .apply(record(
            4,
            InvestigationEvent::TaskCompleted {
                task_id: task_id(1),
                result: InvestigationResult::new(research_history()),
            },
        ))
        .expect("completion applies");

    assert_eq!(state.research().source_count(), 1);
    assert_eq!(state.research().evidence_count(), 1);
    assert_eq!(state.research().claim_count(), 1);
    assert_eq!(state.research().last_sequence(), 3);
    assert_eq!(state.status(), InvestigationStatus::AwaitingNextStep);
}

#[test]
fn invalid_result_changes_neither_task_nor_research_state() {
    let mut state = active_state();
    let source = source();
    let unknown_source = source_id(99);
    let evidence = Evidence::new(evidence_id(2), unknown_source, "Excerpt".to_owned())
        .expect("evidence shape is valid");
    let result = InvestigationResult::new(vec![
        research_record(1, ResearchEvent::SourceRecorded(source)),
        research_record(2, ResearchEvent::EvidenceRecorded(evidence)),
    ]);
    let before = state.clone();

    let error = state
        .apply(record(
            4,
            InvestigationEvent::TaskCompleted {
                task_id: task_id(1),
                result,
            },
        ))
        .expect_err("invalid result is rejected");

    assert_eq!(
        error,
        InvestigationTransitionError::ResearchTransition(TransitionError::UnknownSource(
            unknown_source
        ))
    );
    assert_eq!(state, before);
}

#[test]
fn follow_up_requires_a_completed_parent_and_appends_in_order() {
    let mut state = InvestigationState::reconstruct(vec![request_record(1), plan_record(2, &[1])])
        .expect("history is valid");
    let follow_up = follow_up_task(2, 1);
    let before = state.clone();
    assert_eq!(
        state.apply(record(
            3,
            InvestigationEvent::FollowUpRecorded(follow_up.clone())
        )),
        Err(InvestigationTransitionError::ParentTaskNotCompleted(
            task_id(1)
        ))
    );
    assert_eq!(state, before);

    state
        .apply(record(
            3,
            InvestigationEvent::TaskStarted {
                task_id: task_id(1),
            },
        ))
        .expect("task starts");
    state
        .apply(record(
            4,
            InvestigationEvent::TaskCompleted {
                task_id: task_id(1),
                result: InvestigationResult::new(Vec::new()),
            },
        ))
        .expect("task completes");
    state
        .apply(record(5, InvestigationEvent::FollowUpRecorded(follow_up)))
        .expect("follow-up applies");

    assert_eq!(state.status(), InvestigationStatus::Investigating);
    assert_eq!(
        state
            .tasks()
            .map(|task| *task.task().id())
            .collect::<Vec<_>>(),
        vec![task_id(1), task_id(2)]
    );
    assert_eq!(
        state.next_pending_task().map(|task| *task.id()),
        Some(task_id(2))
    );
}

#[test]
fn follow_up_rejects_wrong_origin_unknown_parent_and_duplicate_identity() {
    let mut state = completed_state();
    let before = state.clone();
    let initial = InvestigationTask::initial(task_id(2), "Not a follow-up".to_owned())
        .expect("task is valid");
    assert_eq!(
        state.apply(record(5, InvestigationEvent::FollowUpRecorded(initial))),
        Err(InvestigationTransitionError::InvalidFollowUpOrigin(
            task_id(2)
        ))
    );
    assert_eq!(state, before);

    let unknown = follow_up_task(2, 99);
    assert_eq!(
        state.apply(record(5, InvestigationEvent::FollowUpRecorded(unknown))),
        Err(InvestigationTransitionError::UnknownTask(task_id(99)))
    );

    let duplicate = follow_up_task(1, 1);
    assert_eq!(
        state.apply(record(5, InvestigationEvent::FollowUpRecorded(duplicate))),
        Err(InvestigationTransitionError::DuplicateTask(task_id(1)))
    );
}

#[test]
fn explicit_stops_are_terminal_distinct_and_require_no_active_task() {
    let reasons = [
        ResearchStopReason::OperatorStopped,
        ResearchStopReason::BudgetExhausted,
        ResearchStopReason::Blocked(
            BlockedReason::new("Archive unavailable".to_owned()).expect("reason is valid"),
        ),
    ];
    for reason in reasons {
        let mut state = InvestigationState::default();
        state.apply(request_record(1)).expect("request applies");
        state
            .apply(record(
                2,
                InvestigationEvent::ResearchStopped(reason.clone()),
            ))
            .expect("stop applies without a plan");
        assert_eq!(state.status(), InvestigationStatus::Stopped(reason));

        let before = state.clone();
        assert_eq!(
            state.apply(plan_record(3, &[1])),
            Err(InvestigationTransitionError::ResearchAlreadyStopped)
        );
        assert_eq!(state, before);
    }

    let mut active = active_state();
    let before = active.clone();
    assert_eq!(
        active.apply(record(
            4,
            InvestigationEvent::ResearchStopped(ResearchStopReason::BudgetExhausted)
        )),
        Err(InvestigationTransitionError::ActiveTasksPreventStop)
    );
    assert_eq!(active, before);

    let mut empty = InvestigationState::default();
    assert_eq!(
        empty.apply(record(
            1,
            InvestigationEvent::ResearchStopped(ResearchStopReason::OperatorStopped)
        )),
        Err(InvestigationTransitionError::RequestRequired)
    );
    assert_eq!(empty, InvestigationState::default());
}

#[test]
fn sequence_mismatch_is_rejected_before_event_validation() {
    let mut state = InvestigationState::default();

    assert_eq!(
        state.apply(plan_record(2, &[1])),
        Err(InvestigationTransitionError::Sequence {
            expected: 1,
            actual: 2,
        })
    );
    assert_eq!(state, InvestigationState::default());
}

fn active_state() -> InvestigationState {
    InvestigationState::reconstruct(vec![
        request_record(1),
        plan_record(2, &[1]),
        record(
            3,
            InvestigationEvent::TaskStarted {
                task_id: task_id(1),
            },
        ),
    ])
    .expect("active state history is valid")
}

fn completed_state() -> InvestigationState {
    InvestigationState::reconstruct(vec![
        request_record(1),
        plan_record(2, &[1]),
        record(
            3,
            InvestigationEvent::TaskStarted {
                task_id: task_id(1),
            },
        ),
        record(
            4,
            InvestigationEvent::TaskCompleted {
                task_id: task_id(1),
                result: InvestigationResult::new(Vec::new()),
            },
        ),
    ])
    .expect("completed state history is valid")
}

fn request_record(sequence: u64) -> InvestigationRecord {
    record(
        sequence,
        InvestigationEvent::RequestRecorded(
            ResearchRequest::new("Research question".to_owned()).expect("request is valid"),
        ),
    )
}

fn plan_record(sequence: u64, ids: &[u128]) -> InvestigationRecord {
    let tasks = ids
        .iter()
        .map(|id| {
            InvestigationTask::initial(task_id(*id), format!("Objective {id}"))
                .expect("task is valid")
        })
        .collect();
    record(
        sequence,
        InvestigationEvent::PlanRecorded(ResearchPlan::new(tasks).expect("plan is valid")),
    )
}

fn follow_up_task(id: u128, parent: u128) -> InvestigationTask {
    InvestigationTask::follow_up(
        task_id(id),
        task_id(parent),
        format!("Follow-up objective {id}"),
        ResearchGap::new("Unresolved material".to_owned()).expect("gap is valid"),
    )
    .expect("follow-up is valid")
}

fn record(sequence: u64, event: InvestigationEvent) -> InvestigationRecord {
    InvestigationRecord::new(sequence, event).expect("record is valid")
}

fn research_history() -> Vec<ResearchRecord> {
    vec![
        research_record(1, ResearchEvent::SourceRecorded(source())),
        research_record(2, ResearchEvent::EvidenceRecorded(evidence())),
        research_record(3, ResearchEvent::ClaimProposed(claim())),
    ]
}

fn source() -> Source {
    Source::new(
        source_id(1),
        ContentDigest::sha256([7; 32]),
        "https://example.test/source".to_owned(),
        Some("Source title".to_owned()),
        RetrievedAt::new("2026-08-29T10:00:00Z").expect("time is valid"),
        MediaType::new("text/html").expect("media type is valid"),
    )
    .expect("source is valid")
}

fn evidence() -> Evidence {
    Evidence::new(evidence_id(2), source_id(1), "Recorded excerpt".to_owned())
        .expect("evidence is valid")
}

fn claim() -> Claim {
    Claim::new(
        claim_id(3),
        "Proposed claim".to_owned(),
        vec![evidence_id(2)],
    )
    .expect("claim is valid")
}

fn research_record(sequence: u64, event: ResearchEvent) -> ResearchRecord {
    ResearchRecord::new(sequence, event).expect("research record is valid")
}

fn task_id(value: u128) -> InvestigationTaskId {
    uuid(value).parse().expect("task identifier is valid")
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
    uuid::Uuid::from_bytes(bytes).hyphenated().to_string()
}
