use aurora_research::{
    Claim, ClaimId, ContentDigest, Evidence, EvidenceId, InvestigationEvent, InvestigationRecord,
    InvestigationResult, InvestigationState, InvestigationTask, InvestigationTaskId,
    InvestigationTransitionError, MediaType, ResearchEvent, ResearchGap, ResearchPlan,
    ResearchRecord, ResearchRequest, RetrievedAt, Source, SourceId, TransitionError,
    decode_investigation_record, encode_investigation_record,
};
use proptest::prelude::*;
use uuid::Uuid;

prop_compose! {
    fn valid_investigation_history()
        (
            seed in any::<u128>(),
            digest in any::<[u8; 32]>(),
            question in nonblank_text(47),
            objective in nonblank_text(47),
            gap in nonblank_text(47),
            locator in nonblank_text(47),
            excerpt in nonblank_text(63),
            statement in nonblank_text(63),
            with_research in any::<bool>(),
            with_follow_up in any::<bool>(),
        ) -> Vec<InvestigationRecord>
    {
        let initial_id = task_id(seed);
        let initial = InvestigationTask::initial(initial_id, objective)
            .expect("generated task is valid");
        let result = if with_research {
            InvestigationResult::new(research_history(seed, digest, locator, excerpt, statement))
        } else {
            InvestigationResult::new(Vec::new())
        };
        let mut records = vec![
            investigation_record(
                1,
                InvestigationEvent::RequestRecorded(
                    ResearchRequest::new(question).expect("generated request is valid"),
                ),
            ),
            investigation_record(
                2,
                InvestigationEvent::PlanRecorded(
                    ResearchPlan::new(vec![initial]).expect("generated plan is valid"),
                ),
            ),
            investigation_record(3, InvestigationEvent::TaskStarted { task_id: initial_id }),
            investigation_record(
                4,
                InvestigationEvent::TaskCompleted { task_id: initial_id, result },
            ),
        ];
        if with_follow_up {
            let follow_up = InvestigationTask::follow_up(
                task_id(seed ^ 1),
                initial_id,
                "Investigate the generated gap".to_owned(),
                ResearchGap::new(gap).expect("generated gap is valid"),
            )
            .expect("generated follow-up is valid");
            records.push(investigation_record(
                5,
                InvestigationEvent::FollowUpRecorded(follow_up),
            ));
        }
        records
    }
}

proptest! {
    #[test]
    fn every_valid_investigation_record_survives_codec_round_trip(
        records in valid_investigation_history()
    ) {
        for record in records {
            let encoded = encode_investigation_record(&record).expect("record encodes");
            let decoded = decode_investigation_record(&encoded).expect("record decodes");
            prop_assert_eq!(decoded, record);
        }
    }

    #[test]
    fn replay_matches_incremental_investigation_application(
        records in valid_investigation_history()
    ) {
        let replayed = InvestigationState::reconstruct(records.clone())
            .expect("generated history reconstructs");
        let mut incremental = InvestigationState::default();
        for record in records {
            incremental.apply(record).expect("generated record applies");
        }
        prop_assert_eq!(replayed, incremental);
    }

    #[test]
    fn codec_replay_preserves_complete_investigation_state(
        records in valid_investigation_history()
    ) {
        let expected = InvestigationState::reconstruct(records.clone())
            .expect("generated history reconstructs");
        let decoded = records
            .iter()
            .map(|record| {
                let encoded = encode_investigation_record(record).expect("record encodes");
                decode_investigation_record(&encoded).expect("record decodes")
            })
            .collect::<Vec<_>>();
        let actual = InvestigationState::reconstruct(decoded)
            .expect("decoded history reconstructs");
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn codec_and_replay_preserve_task_admission_order(
        records in valid_investigation_history()
    ) {
        let expected = InvestigationState::reconstruct(records.clone())
            .expect("generated history reconstructs")
            .tasks()
            .map(|task| *task.task().id())
            .collect::<Vec<_>>();
        let decoded = records
            .iter()
            .map(|record| {
                decode_investigation_record(
                    &encode_investigation_record(record).expect("record encodes")
                ).expect("record decodes")
            })
            .collect::<Vec<_>>();
        let actual = InvestigationState::reconstruct(decoded)
            .expect("decoded history reconstructs")
            .tasks()
            .map(|task| *task.task().id())
            .collect::<Vec<_>>();
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn rejected_result_batch_never_partially_changes_state(
        seed in any::<u128>(),
        digest in any::<[u8; 32]>(),
        locator in nonblank_text(47),
        excerpt in nonblank_text(63),
    ) {
        let active_task = task_id(seed);
        let request = investigation_record(
            1,
            InvestigationEvent::RequestRecorded(
                ResearchRequest::new("Generated question".to_owned())
                    .expect("request is valid"),
            ),
        );
        let task = InvestigationTask::initial(active_task, "Generated objective".to_owned())
            .expect("task is valid");
        let plan = investigation_record(
            2,
            InvestigationEvent::PlanRecorded(
                ResearchPlan::new(vec![task]).expect("plan is valid"),
            ),
        );
        let started = investigation_record(
            3,
            InvestigationEvent::TaskStarted { task_id: active_task },
        );
        let mut state = InvestigationState::reconstruct(vec![request, plan, started])
            .expect("active history reconstructs");
        let before = state.clone();
        let recorded_source = source_id(seed.rotate_left(7));
        let missing_source = source_id(seed.rotate_left(7) ^ 1);
        let source = Source::new(
            recorded_source,
            ContentDigest::sha256(digest),
            locator,
            None,
            RetrievedAt::new("2026-08-29T10:00:00Z").expect("fixed time is valid"),
            MediaType::new("text/plain").expect("fixed media type is valid"),
        )
        .expect("generated source is valid");
        let evidence = Evidence::new(evidence_id(seed), missing_source, excerpt)
            .expect("generated evidence is valid");
        let result = InvestigationResult::new(vec![
            research_record(1, ResearchEvent::SourceRecorded(source)),
            research_record(2, ResearchEvent::EvidenceRecorded(evidence)),
        ]);

        let actual = state.apply(investigation_record(
            4,
            InvestigationEvent::TaskCompleted { task_id: active_task, result },
        ));

        prop_assert_eq!(
            actual,
            Err(InvestigationTransitionError::ResearchTransition(
                TransitionError::UnknownSource(missing_source)
            ))
        );
        prop_assert_eq!(state, before);
    }
}

fn research_history(
    seed: u128,
    digest: [u8; 32],
    locator: String,
    excerpt: String,
    statement: String,
) -> Vec<ResearchRecord> {
    let source_id = source_id(seed.rotate_left(7));
    let evidence_id = evidence_id(seed.rotate_left(19));
    let source = Source::new(
        source_id,
        ContentDigest::sha256(digest),
        locator,
        None,
        RetrievedAt::new("2026-08-29T10:00:00Z").expect("fixed time is valid"),
        MediaType::new("text/plain").expect("fixed media type is valid"),
    )
    .expect("generated source is valid");
    let evidence =
        Evidence::new(evidence_id, source_id, excerpt).expect("generated evidence is valid");
    let claim = Claim::new(claim_id(seed.rotate_left(31)), statement, vec![evidence_id])
        .expect("generated claim is valid");
    vec![
        research_record(1, ResearchEvent::SourceRecorded(source)),
        research_record(2, ResearchEvent::EvidenceRecorded(evidence)),
        research_record(3, ResearchEvent::ClaimProposed(claim)),
    ]
}

fn nonblank_text(max_tail_length: usize) -> impl Strategy<Value = String> {
    (
        any::<char>().prop_filter("first character is not whitespace", |value| {
            !value.is_whitespace()
        }),
        prop::collection::vec(any::<char>(), 0..=max_tail_length),
    )
        .prop_map(|(first, tail)| std::iter::once(first).chain(tail).collect())
}

fn investigation_record(sequence: u64, event: InvestigationEvent) -> InvestigationRecord {
    InvestigationRecord::new(sequence, event).expect("generated record is valid")
}

fn research_record(sequence: u64, event: ResearchEvent) -> ResearchRecord {
    ResearchRecord::new(sequence, event).expect("generated research record is valid")
}

fn task_id(seed: u128) -> InvestigationTaskId {
    uuid_v4(seed)
        .parse()
        .expect("generated task identifier is valid")
}

fn source_id(seed: u128) -> SourceId {
    uuid_v4(seed)
        .parse()
        .expect("generated source identifier is valid")
}

fn evidence_id(seed: u128) -> EvidenceId {
    uuid_v4(seed)
        .parse()
        .expect("generated evidence identifier is valid")
}

fn claim_id(seed: u128) -> ClaimId {
    uuid_v4(seed)
        .parse()
        .expect("generated claim identifier is valid")
}

fn uuid_v4(seed: u128) -> String {
    let mut bytes = seed.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).hyphenated().to_string()
}
