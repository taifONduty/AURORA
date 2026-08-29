use aurora_research::{
    Claim, ClaimId, ContentDigest, Evidence, EvidenceAssessment, EvidenceId, EvidenceRelation,
    EvidenceSufficiency, IdentifiedResearchGap, InvestigationEvent, InvestigationRecord,
    InvestigationResult, InvestigationTask, InvestigationTaskId, MediaType, ResearchControlEvent,
    ResearchControlLimits, ResearchControlRecord, ResearchControlState, ResearchControlStatus,
    ResearchControlTransitionError, ResearchEvent, ResearchGap, ResearchGapCause, ResearchGapId,
    ResearchPlan, ResearchRecord, ResearchRequest, RetrievedAt, Source, SourceId,
    VerificationAssessment, VerificationId, VerificationRecord, decode_research_control_record,
    encode_research_control_record,
};
use proptest::prelude::*;
use uuid::Uuid;

prop_compose! {
    fn complete_histories()
        (
            seed in any::<u128>(),
            first_digest in any::<[u8; 32]>(),
            second_digest in any::<[u8; 32]>(),
            issue in 0_u8..4,
            final_relation in prop_oneof![
                Just(EvidenceRelation::Supports),
                Just(EvidenceRelation::Contradicts),
            ],
        ) -> Vec<ResearchControlRecord>
    {
        complete_history(seed, first_digest, second_digest, issue, final_relation)
    }
}

proptest! {
    #[test]
    fn every_generated_control_record_survives_codec_round_trip(
        records in complete_histories()
    ) {
        for record in records {
            let encoded = encode_research_control_record(&record).expect("record encodes");
            let decoded = decode_research_control_record(&encoded).expect("record decodes");
            prop_assert_eq!(decoded, record);
        }
    }

    #[test]
    fn reconstruction_matches_incremental_control_application(
        records in complete_histories()
    ) {
        let replayed = ResearchControlState::reconstruct(records.clone())
            .expect("history reconstructs");
        let mut incremental = ResearchControlState::default();
        for record in records {
            incremental.apply(record).expect("record applies");
        }
        prop_assert_eq!(replayed, incremental);
    }

    #[test]
    fn codec_replay_preserves_the_complete_control_projection(
        records in complete_histories()
    ) {
        let expected = ResearchControlState::reconstruct(records.clone())
            .expect("history reconstructs");
        let decoded = records
            .iter()
            .map(|record| {
                decode_research_control_record(
                    &encode_research_control_record(record).expect("record encodes")
                )
                .expect("record decodes")
            })
            .collect::<Vec<_>>();
        let actual = ResearchControlState::reconstruct(decoded)
            .expect("decoded history reconstructs");
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn over_limit_follow_up_rejection_changes_no_component_state(
        seed in any::<u128>(),
        digest in any::<[u8; 32]>(),
    ) {
        let limits = control_record(
            1,
            ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(0)),
        );
        let mut zero_limit_prefix = complete_history(
            seed,
            digest,
            digest,
            0,
            EvidenceRelation::Supports,
        );
        zero_limit_prefix[0] = limits;
        zero_limit_prefix.truncate(7);
        let mut state = ResearchControlState::reconstruct(zero_limit_prefix)
            .expect("zero-limit gap prefix reconstructs");
        let before = state.clone();
        let candidate = control_record(
            8,
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id: gap_id(seed ^ 0x70),
                investigation_record: follow_up_record(
                    5,
                    task_id(seed ^ 0x20),
                    task_id(seed ^ 0x10),
                    "Generated unresolved evidence",
                ),
            },
        );

        prop_assert_eq!(
            state.apply(candidate),
            Err(ResearchControlTransitionError::FollowUpLimitReached { limit: 0 })
        );
        prop_assert_eq!(state, before);
    }

    #[test]
    fn removing_explicit_completion_never_projects_completed(
        mut records in complete_histories()
    ) {
        let final_record = records.pop().expect("complete history has a terminal record");
        prop_assert!(matches!(final_record.event(), ResearchControlEvent::ResearchCompleted));
        let state = ResearchControlState::reconstruct(records)
            .expect("non-terminal history reconstructs");
        prop_assert_eq!(state.status(), ResearchControlStatus::AwaitingNextStep);
    }
}

fn complete_history(
    seed: u128,
    first_digest: [u8; 32],
    second_digest: [u8; 32],
    issue: u8,
    final_relation: EvidenceRelation,
) -> Vec<ResearchControlRecord> {
    let initial_task = task_id(seed ^ 0x10);
    let follow_up_task = task_id(seed ^ 0x20);
    let source_one = source_id(seed ^ 0x30);
    let source_two = source_id(seed ^ 0x31);
    let evidence_one = evidence_id(seed ^ 0x40);
    let evidence_two = evidence_id(seed ^ 0x41);
    let evidence_three = evidence_id(seed ^ 0x42);
    let claim = claim_id(seed ^ 0x50);
    let first_verification = verification_id(seed ^ 0x60);
    let second_verification = verification_id(seed ^ 0x61);
    let gap = gap_id(seed ^ 0x70);
    let (relations, sufficiency) = issue_assessment(issue, evidence_one, evidence_two);

    vec![
        control_record(
            1,
            ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(1)),
        ),
        control_record(
            2,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                1,
                InvestigationEvent::RequestRecorded(
                    ResearchRequest::new("Generated research question".to_owned())
                        .expect("request is valid"),
                ),
            )),
        ),
        control_record(
            3,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                2,
                InvestigationEvent::PlanRecorded(
                    ResearchPlan::new(vec![
                        InvestigationTask::initial(
                            initial_task,
                            "Generated initial objective".to_owned(),
                        )
                        .expect("task is valid"),
                    ])
                    .expect("plan is valid"),
                ),
            )),
        ),
        control_record(
            4,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                3,
                InvestigationEvent::TaskStarted {
                    task_id: initial_task,
                },
            )),
        ),
        control_record(
            5,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                4,
                InvestigationEvent::TaskCompleted {
                    task_id: initial_task,
                    result: InvestigationResult::new(vec![
                        research_record(
                            1,
                            ResearchEvent::SourceRecorded(
                                Source::new(
                                    source_one,
                                    ContentDigest::sha256(first_digest),
                                    "https://example.test/generated/one".to_owned(),
                                    None,
                                    retrieved_at(),
                                    media_type(),
                                )
                                .expect("source is valid"),
                            ),
                        ),
                        research_record(
                            2,
                            ResearchEvent::EvidenceRecorded(
                                Evidence::new(
                                    evidence_one,
                                    source_one,
                                    "Generated first excerpt".to_owned(),
                                )
                                .expect("evidence is valid"),
                            ),
                        ),
                        research_record(
                            3,
                            ResearchEvent::EvidenceRecorded(
                                Evidence::new(
                                    evidence_two,
                                    source_one,
                                    "Generated second excerpt".to_owned(),
                                )
                                .expect("evidence is valid"),
                            ),
                        ),
                        research_record(
                            4,
                            ResearchEvent::ClaimProposed(
                                Claim::new(
                                    claim,
                                    "Generated proposed claim".to_owned(),
                                    vec![evidence_one],
                                )
                                .expect("claim is valid"),
                            ),
                        ),
                    ]),
                },
            )),
        ),
        control_record(
            6,
            ResearchControlEvent::VerificationRecorded(verification_record(
                1,
                first_verification,
                claim,
                relations,
                sufficiency,
            )),
        ),
        control_record(
            7,
            ResearchControlEvent::GapIdentified(IdentifiedResearchGap::new(
                gap,
                ResearchGapCause::Verification(first_verification),
                ResearchGap::new("Generated unresolved evidence".to_owned()).expect("gap is valid"),
            )),
        ),
        control_record(
            8,
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id: gap,
                investigation_record: follow_up_record(
                    5,
                    follow_up_task,
                    initial_task,
                    "Generated unresolved evidence",
                ),
            },
        ),
        control_record(
            9,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                6,
                InvestigationEvent::TaskStarted {
                    task_id: follow_up_task,
                },
            )),
        ),
        control_record(
            10,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                7,
                InvestigationEvent::TaskCompleted {
                    task_id: follow_up_task,
                    result: InvestigationResult::new(vec![
                        research_record(
                            5,
                            ResearchEvent::SourceRecorded(
                                Source::new(
                                    source_two,
                                    ContentDigest::sha256(second_digest),
                                    "https://example.test/generated/two".to_owned(),
                                    None,
                                    retrieved_at(),
                                    media_type(),
                                )
                                .expect("source is valid"),
                            ),
                        ),
                        research_record(
                            6,
                            ResearchEvent::EvidenceRecorded(
                                Evidence::new(
                                    evidence_three,
                                    source_two,
                                    "Generated follow-up excerpt".to_owned(),
                                )
                                .expect("evidence is valid"),
                            ),
                        ),
                    ]),
                },
            )),
        ),
        control_record(
            11,
            ResearchControlEvent::VerificationRecorded(verification_record(
                2,
                second_verification,
                claim,
                vec![
                    EvidenceAssessment::new(evidence_one, final_relation),
                    EvidenceAssessment::new(evidence_three, final_relation),
                ],
                EvidenceSufficiency::Sufficient,
            )),
        ),
        control_record(
            12,
            ResearchControlEvent::GapResolved {
                gap_id: gap,
                verification_id: second_verification,
            },
        ),
        control_record(13, ResearchControlEvent::ResearchCompleted),
    ]
}

fn issue_assessment(
    issue: u8,
    first: EvidenceId,
    second: EvidenceId,
) -> (Vec<EvidenceAssessment>, EvidenceSufficiency) {
    match issue {
        0 => (
            vec![EvidenceAssessment::new(first, EvidenceRelation::Supports)],
            EvidenceSufficiency::Insufficient,
        ),
        1 => (
            vec![EvidenceAssessment::new(first, EvidenceRelation::Supports)],
            EvidenceSufficiency::Indeterminate,
        ),
        2 => (
            vec![
                EvidenceAssessment::new(first, EvidenceRelation::Supports),
                EvidenceAssessment::new(second, EvidenceRelation::Contradicts),
            ],
            EvidenceSufficiency::Sufficient,
        ),
        _ => (
            vec![EvidenceAssessment::new(first, EvidenceRelation::Unclear)],
            EvidenceSufficiency::Sufficient,
        ),
    }
}

fn follow_up_record(
    sequence: u64,
    id: InvestigationTaskId,
    parent: InvestigationTaskId,
    gap: &str,
) -> InvestigationRecord {
    investigation_record(
        sequence,
        InvestigationEvent::FollowUpRecorded(
            InvestigationTask::follow_up(
                id,
                parent,
                "Generated follow-up objective".to_owned(),
                ResearchGap::new(gap.to_owned()).expect("gap is valid"),
            )
            .expect("follow-up is valid"),
        ),
    )
}

fn verification_record(
    sequence: u64,
    id: VerificationId,
    claim: ClaimId,
    evidence: Vec<EvidenceAssessment>,
    sufficiency: EvidenceSufficiency,
) -> VerificationRecord {
    VerificationRecord::new(
        sequence,
        VerificationAssessment::new(id, claim, evidence, sufficiency).expect("assessment is valid"),
    )
    .expect("verification record is valid")
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

fn retrieved_at() -> RetrievedAt {
    RetrievedAt::new("2026-08-29T10:00:00Z").expect("time is valid")
}

fn media_type() -> MediaType {
    MediaType::new("text/plain").expect("media type is valid")
}

fn gap_id(value: u128) -> ResearchGapId {
    uuid_v4(value).parse().expect("gap identifier is valid")
}

fn task_id(value: u128) -> InvestigationTaskId {
    uuid_v4(value).parse().expect("task identifier is valid")
}

fn verification_id(value: u128) -> VerificationId {
    uuid_v4(value)
        .parse()
        .expect("verification identifier is valid")
}

fn source_id(value: u128) -> SourceId {
    uuid_v4(value).parse().expect("source identifier is valid")
}

fn evidence_id(value: u128) -> EvidenceId {
    uuid_v4(value)
        .parse()
        .expect("evidence identifier is valid")
}

fn claim_id(value: u128) -> ClaimId {
    uuid_v4(value).parse().expect("claim identifier is valid")
}

fn uuid_v4(value: u128) -> String {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).hyphenated().to_string()
}
