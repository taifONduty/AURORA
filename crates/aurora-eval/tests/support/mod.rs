#![allow(dead_code)]

use aurora_research::{
    BlockedReason, Claim, ClaimId, ContentDigest, Evidence, EvidenceAssessment, EvidenceId,
    EvidenceRelation, EvidenceSufficiency, IdentifiedResearchGap, InvestigationEvent,
    InvestigationFailure, InvestigationRecord, InvestigationResult, InvestigationTask,
    InvestigationTaskId, MediaType, ResearchControlEvent, ResearchControlLimits,
    ResearchControlRecord, ResearchEvent, ResearchFailure, ResearchGap, ResearchGapCause,
    ResearchGapId, ResearchPlan, ResearchRecord, ResearchRequest, ResearchStopReason, RetrievedAt,
    Source, SourceId, VerificationAssessment, VerificationId, VerificationRecord,
};
use ring::digest;

use aurora_eval::{EvaluationCase, EvaluationCaseId, ExpectedTerminalOutcome};

pub struct SupportedFixture {
    pub records: Vec<ResearchControlRecord>,
    pub source_id: SourceId,
    pub evidence_id: EvidenceId,
    pub verification_id: VerificationId,
    pub content: String,
}

pub struct MultiSourceFixture {
    pub records: Vec<ResearchControlRecord>,
    pub first_source_id: SourceId,
    pub second_source_id: SourceId,
    pub first_content: String,
    pub second_content: String,
}

pub fn case(
    id: &str,
    expected_terminal: ExpectedTerminalOutcome,
    expected_follow_up_tasks: u32,
) -> EvaluationCase {
    EvaluationCase::new(
        EvaluationCaseId::new(id).expect("case identifier is valid"),
        "What does the fixture establish?".to_owned(),
        Vec::new(),
        Vec::new(),
        Some(expected_terminal),
        Some(expected_follow_up_tasks),
    )
    .expect("case is valid")
}

pub fn supported_fixture() -> SupportedFixture {
    let content = "The pinned source says Aurora preserves evidence.".to_owned();
    let source_id = source_id(1);
    let full_evidence_id = evidence_id(1);
    let evidence_id = evidence_id(2);
    let claim_id = claim_id(1);
    let verification_id = verification_id(1);
    let task_id = task_id(1);
    let source = Source::new(
        source_id,
        content_digest(&content),
        "https://example.test/aurora".to_owned(),
        Some("AURORA source".to_owned()),
        RetrievedAt::new("2026-08-30T00:00:00Z").expect("time is valid"),
        MediaType::new("text/plain").expect("media type is valid"),
    )
    .expect("source is valid");
    let full_evidence = Evidence::new(full_evidence_id, source_id, content.clone())
        .expect("full evidence is valid");
    let excerpt = Evidence::new(
        evidence_id,
        source_id,
        "Aurora preserves evidence".to_owned(),
    )
    .expect("excerpt is valid");
    let claim = Claim::new(
        claim_id,
        "Aurora preserves evidence.".to_owned(),
        vec![evidence_id],
    )
    .expect("claim is valid");
    let task = InvestigationTask::initial(task_id, "Find AURORA evidence".to_owned())
        .expect("task is valid");
    let plan = ResearchPlan::new(vec![task]).expect("plan is valid");
    let result = InvestigationResult::new(vec![
        research_record(1, ResearchEvent::SourceRecorded(source)),
        research_record(2, ResearchEvent::EvidenceRecorded(full_evidence)),
        research_record(3, ResearchEvent::EvidenceRecorded(excerpt)),
        research_record(4, ResearchEvent::ClaimProposed(claim)),
    ]);
    let assessment = VerificationAssessment::new(
        verification_id,
        claim_id,
        vec![EvidenceAssessment::new(
            evidence_id,
            EvidenceRelation::Supports,
        )],
        EvidenceSufficiency::Sufficient,
    )
    .expect("assessment is valid");
    let records = vec![
        control_record(
            1,
            ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(1)),
        ),
        control_record(
            2,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                1,
                InvestigationEvent::RequestRecorded(
                    ResearchRequest::new("Does AURORA preserve evidence?".to_owned())
                        .expect("request is valid"),
                ),
            )),
        ),
        control_record(
            3,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                2,
                InvestigationEvent::PlanRecorded(plan),
            )),
        ),
        control_record(
            4,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                3,
                InvestigationEvent::TaskStarted { task_id },
            )),
        ),
        control_record(
            5,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                4,
                InvestigationEvent::TaskCompleted { task_id, result },
            )),
        ),
        control_record(
            6,
            ResearchControlEvent::VerificationRecorded(
                VerificationRecord::new(1, assessment).expect("record is valid"),
            ),
        ),
        control_record(7, ResearchControlEvent::ResearchCompleted),
    ];
    SupportedFixture {
        records,
        source_id,
        evidence_id,
        verification_id,
        content,
    }
}

pub fn adaptive_history() -> Vec<ResearchControlRecord> {
    let mut records = insufficient_gap_prefix(1);
    let follow_up = InvestigationTask::follow_up(
        task_id(2),
        task_id(1),
        "Find corroborating evidence".to_owned(),
        ResearchGap::new("More evidence is needed".to_owned()).expect("gap is valid"),
    )
    .expect("follow-up is valid");
    records.extend([
        control_record(
            8,
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id: gap_id(1),
                investigation_record: investigation_record(
                    5,
                    InvestigationEvent::FollowUpRecorded(follow_up),
                ),
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
                    result: InvestigationResult::new(Vec::new()),
                },
            )),
        ),
        control_record(
            11,
            ResearchControlEvent::VerificationRecorded(
                VerificationRecord::new(
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
                )
                .expect("record is valid"),
            ),
        ),
        control_record(
            12,
            ResearchControlEvent::GapResolved {
                gap_id: gap_id(1),
                verification_id: verification_id(2),
            },
        ),
        control_record(13, ResearchControlEvent::ResearchCompleted),
    ]);
    records
}

pub fn repeated_follow_up_history() -> Vec<ResearchControlRecord> {
    let adaptive = adaptive_history();
    let mut records = adaptive[..10].to_vec();
    records[0] = control_record(
        1,
        ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(2)),
    );
    records.extend([
        control_record(
            11,
            ResearchControlEvent::VerificationRecorded(
                VerificationRecord::new(
                    2,
                    VerificationAssessment::new(
                        verification_id(2),
                        claim_id(1),
                        vec![EvidenceAssessment::new(
                            evidence_id(2),
                            EvidenceRelation::Supports,
                        )],
                        EvidenceSufficiency::Insufficient,
                    )
                    .expect("assessment is valid"),
                )
                .expect("record is valid"),
            ),
        ),
        control_record(
            12,
            ResearchControlEvent::GapIdentified(IdentifiedResearchGap::new(
                gap_id(2),
                ResearchGapCause::Verification(verification_id(2)),
                ResearchGap::new("Find corroborating evidence".to_owned()).expect("gap is valid"),
            )),
        ),
        control_record(
            13,
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id: gap_id(2),
                investigation_record: investigation_record(
                    8,
                    InvestigationEvent::FollowUpRecorded(
                        InvestigationTask::follow_up(
                            task_id(3),
                            task_id(2),
                            "Find corroborating evidence".to_owned(),
                            ResearchGap::new("Find corroborating evidence".to_owned())
                                .expect("gap is valid"),
                        )
                        .expect("follow-up is valid"),
                    ),
                ),
            },
        ),
        control_record(
            14,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                9,
                InvestigationEvent::TaskStarted {
                    task_id: task_id(3),
                },
            )),
        ),
        control_record(
            15,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                10,
                InvestigationEvent::TaskCompleted {
                    task_id: task_id(3),
                    result: InvestigationResult::new(Vec::new()),
                },
            )),
        ),
        control_record(
            16,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                11,
                InvestigationEvent::ResearchStopped(ResearchStopReason::BudgetExhausted),
            )),
        ),
    ]);
    records
}

pub fn contradictory_history() -> Vec<ResearchControlRecord> {
    let fixture = supported_fixture();
    let mut records = fixture.records[..5].to_vec();
    records.push(control_record(
        6,
        ResearchControlEvent::VerificationRecorded(
            VerificationRecord::new(
                1,
                VerificationAssessment::new(
                    verification_id(1),
                    claim_id(1),
                    vec![EvidenceAssessment::new(
                        evidence_id(2),
                        EvidenceRelation::Contradicts,
                    )],
                    EvidenceSufficiency::Sufficient,
                )
                .expect("assessment is valid"),
            )
            .expect("record is valid"),
        ),
    ));
    records.push(control_record(7, ResearchControlEvent::ResearchCompleted));
    records
}

pub fn mixed_verification_history() -> Vec<ResearchControlRecord> {
    let fixture = supported_fixture();
    let mut records = fixture.records[..5].to_vec();
    records[0] = control_record(
        1,
        ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(0)),
    );
    records.push(control_record(
        6,
        ResearchControlEvent::VerificationRecorded(
            VerificationRecord::new(
                1,
                VerificationAssessment::new(
                    verification_id(1),
                    claim_id(1),
                    vec![
                        EvidenceAssessment::new(evidence_id(1), EvidenceRelation::Supports),
                        EvidenceAssessment::new(evidence_id(2), EvidenceRelation::Contradicts),
                    ],
                    EvidenceSufficiency::Sufficient,
                )
                .expect("assessment is valid"),
            )
            .expect("record is valid"),
        ),
    ));
    records.push(control_record(
        7,
        ResearchControlEvent::GapIdentified(IdentifiedResearchGap::new(
            gap_id(1),
            ResearchGapCause::Verification(verification_id(1)),
            ResearchGap::new("The evidence conflicts".to_owned()).expect("gap is valid"),
        )),
    ));
    records.push(control_record(
        8,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            5,
            InvestigationEvent::ResearchStopped(blocked_reason()),
        )),
    ));
    records
}

pub fn retrieval_failure_history() -> Vec<ResearchControlRecord> {
    let task_id = task_id(1);
    let task = InvestigationTask::initial(task_id, "Find unavailable evidence".to_owned())
        .expect("task is valid");
    vec![
        control_record(
            1,
            ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(0)),
        ),
        control_record(
            2,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                1,
                InvestigationEvent::RequestRecorded(
                    ResearchRequest::new(
                        "Can the unavailable source establish the claim?".to_owned(),
                    )
                    .expect("request is valid"),
                ),
            )),
        ),
        control_record(
            3,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                2,
                InvestigationEvent::PlanRecorded(
                    ResearchPlan::new(vec![task]).expect("plan is valid"),
                ),
            )),
        ),
        control_record(
            4,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                3,
                InvestigationEvent::TaskStarted { task_id },
            )),
        ),
        control_record(
            5,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                4,
                InvestigationEvent::TaskFailed {
                    task_id,
                    failure: InvestigationFailure::new("retrieval failed".to_owned())
                        .expect("failure is valid"),
                },
            )),
        ),
        control_record(
            6,
            ResearchControlEvent::ResearchFailed(
                ResearchFailure::new("investigation could not continue".to_owned())
                    .expect("failure is valid"),
            ),
        ),
    ]
}

pub fn multi_source_fixture() -> MultiSourceFixture {
    let first_content = "Source one establishes the first finding.".to_owned();
    let second_content = "Source two establishes the second finding.".to_owned();
    let first_source_id = source_id(21);
    let second_source_id = source_id(22);
    let first_evidence_id = evidence_id(21);
    let second_evidence_id = evidence_id(22);
    let first_claim_id = claim_id(21);
    let second_claim_id = claim_id(22);
    let task_id = task_id(21);
    let source = |id, content: &String, locator: &str| {
        Source::new(
            id,
            content_digest(content),
            locator.to_owned(),
            None,
            RetrievedAt::new("2026-08-30T00:00:00Z").expect("time is valid"),
            MediaType::new("text/plain").expect("media type is valid"),
        )
        .expect("source is valid")
    };
    let result = InvestigationResult::new(vec![
        research_record(
            1,
            ResearchEvent::SourceRecorded(source(
                first_source_id,
                &first_content,
                "https://example.test/one",
            )),
        ),
        research_record(
            2,
            ResearchEvent::EvidenceRecorded(
                Evidence::new(first_evidence_id, first_source_id, first_content.clone())
                    .expect("evidence is valid"),
            ),
        ),
        research_record(
            3,
            ResearchEvent::ClaimProposed(
                Claim::new(
                    first_claim_id,
                    "The first finding is established.".to_owned(),
                    vec![first_evidence_id],
                )
                .expect("claim is valid"),
            ),
        ),
        research_record(
            4,
            ResearchEvent::SourceRecorded(source(
                second_source_id,
                &second_content,
                "https://example.test/two",
            )),
        ),
        research_record(
            5,
            ResearchEvent::EvidenceRecorded(
                Evidence::new(second_evidence_id, second_source_id, second_content.clone())
                    .expect("evidence is valid"),
            ),
        ),
        research_record(
            6,
            ResearchEvent::ClaimProposed(
                Claim::new(
                    second_claim_id,
                    "The second finding is established.".to_owned(),
                    vec![second_evidence_id],
                )
                .expect("claim is valid"),
            ),
        ),
    ]);
    let task = InvestigationTask::initial(task_id, "Find two independent findings".to_owned())
        .expect("task is valid");
    let assessment = |id, claim_id, evidence_id| {
        VerificationAssessment::new(
            id,
            claim_id,
            vec![EvidenceAssessment::new(
                evidence_id,
                EvidenceRelation::Supports,
            )],
            EvidenceSufficiency::Sufficient,
        )
        .expect("assessment is valid")
    };
    let records = vec![
        control_record(
            1,
            ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(0)),
        ),
        control_record(
            2,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                1,
                InvestigationEvent::RequestRecorded(
                    ResearchRequest::new("What do both sources establish?".to_owned())
                        .expect("request is valid"),
                ),
            )),
        ),
        control_record(
            3,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                2,
                InvestigationEvent::PlanRecorded(
                    ResearchPlan::new(vec![task]).expect("plan is valid"),
                ),
            )),
        ),
        control_record(
            4,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                3,
                InvestigationEvent::TaskStarted { task_id },
            )),
        ),
        control_record(
            5,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                4,
                InvestigationEvent::TaskCompleted { task_id, result },
            )),
        ),
        control_record(
            6,
            ResearchControlEvent::VerificationRecorded(
                VerificationRecord::new(
                    1,
                    assessment(verification_id(21), first_claim_id, first_evidence_id),
                )
                .expect("record is valid"),
            ),
        ),
        control_record(
            7,
            ResearchControlEvent::VerificationRecorded(
                VerificationRecord::new(
                    2,
                    assessment(verification_id(22), second_claim_id, second_evidence_id),
                )
                .expect("record is valid"),
            ),
        ),
        control_record(8, ResearchControlEvent::ResearchCompleted),
    ];
    MultiSourceFixture {
        records,
        first_source_id,
        second_source_id,
        first_content,
        second_content,
    }
}

pub fn mixed_claim_presentations_history() -> Vec<ResearchControlRecord> {
    let fixture = multi_source_fixture();
    let mut records = fixture.records[..5].to_vec();
    records[0] = control_record(
        1,
        ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(0)),
    );
    records.extend([
        control_record(
            6,
            ResearchControlEvent::VerificationRecorded(
                VerificationRecord::new(
                    1,
                    VerificationAssessment::new(
                        verification_id(21),
                        claim_id(21),
                        vec![EvidenceAssessment::new(
                            evidence_id(21),
                            EvidenceRelation::Supports,
                        )],
                        EvidenceSufficiency::Insufficient,
                    )
                    .expect("assessment is valid"),
                )
                .expect("record is valid"),
            ),
        ),
        control_record(
            7,
            ResearchControlEvent::GapIdentified(IdentifiedResearchGap::new(
                gap_id(21),
                ResearchGapCause::Verification(verification_id(21)),
                ResearchGap::new("The first claim needs more evidence".to_owned())
                    .expect("gap is valid"),
            )),
        ),
        control_record(
            8,
            ResearchControlEvent::VerificationRecorded(
                VerificationRecord::new(
                    2,
                    VerificationAssessment::new(
                        verification_id(22),
                        claim_id(22),
                        vec![EvidenceAssessment::new(
                            evidence_id(22),
                            EvidenceRelation::Contradicts,
                        )],
                        EvidenceSufficiency::Sufficient,
                    )
                    .expect("assessment is valid"),
                )
                .expect("record is valid"),
            ),
        ),
        control_record(
            9,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                5,
                InvestigationEvent::ResearchStopped(blocked_reason()),
            )),
        ),
    ]);
    records
}

pub fn stopped_gap_history(reason: ResearchStopReason) -> Vec<ResearchControlRecord> {
    let mut records = insufficient_gap_prefix(0);
    records.push(control_record(
        8,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            5,
            InvestigationEvent::ResearchStopped(reason),
        )),
    ));
    records
}

pub fn blocked_reason() -> ResearchStopReason {
    ResearchStopReason::Blocked(
        BlockedReason::new("No further evidence is available".to_owned()).expect("reason is valid"),
    )
}

fn insufficient_gap_prefix(limit: u32) -> Vec<ResearchControlRecord> {
    let fixture = supported_fixture();
    let mut records = fixture.records[..5].to_vec();
    records[0] = control_record(
        1,
        ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(limit)),
    );
    records.push(control_record(
        6,
        ResearchControlEvent::VerificationRecorded(
            VerificationRecord::new(
                1,
                VerificationAssessment::new(
                    verification_id(1),
                    claim_id(1),
                    vec![EvidenceAssessment::new(
                        evidence_id(2),
                        EvidenceRelation::Supports,
                    )],
                    EvidenceSufficiency::Insufficient,
                )
                .expect("assessment is valid"),
            )
            .expect("record is valid"),
        ),
    ));
    records.push(control_record(
        7,
        ResearchControlEvent::GapIdentified(IdentifiedResearchGap::new(
            gap_id(1),
            ResearchGapCause::Verification(verification_id(1)),
            ResearchGap::new("More evidence is needed".to_owned()).expect("gap is valid"),
        )),
    ));
    records
}

pub fn metadata() -> aurora_eval::EvaluationMetadata {
    aurora_eval::EvaluationMetadata::new(
        "2c64314".to_owned(),
        "phase-2h-fixtures".to_owned(),
        "deterministic".to_owned(),
        "2026-08-30T00:00:00Z".to_owned(),
    )
    .expect("metadata is valid")
}

pub fn control_record(sequence: u64, event: ResearchControlEvent) -> ResearchControlRecord {
    ResearchControlRecord::new(sequence, event).expect("control record is valid")
}

pub fn investigation_record(sequence: u64, event: InvestigationEvent) -> InvestigationRecord {
    InvestigationRecord::new(sequence, event).expect("investigation record is valid")
}

pub fn research_record(sequence: u64, event: ResearchEvent) -> ResearchRecord {
    ResearchRecord::new(sequence, event).expect("research record is valid")
}

pub fn source_id(value: u64) -> SourceId {
    uuid(value).parse().expect("source identifier is valid")
}

pub fn evidence_id(value: u64) -> EvidenceId {
    uuid(value).parse().expect("evidence identifier is valid")
}

pub fn claim_id(value: u64) -> ClaimId {
    uuid(value).parse().expect("claim identifier is valid")
}

pub fn verification_id(value: u64) -> VerificationId {
    uuid(value)
        .parse()
        .expect("verification identifier is valid")
}

pub fn task_id(value: u64) -> InvestigationTaskId {
    uuid(value).parse().expect("task identifier is valid")
}

pub fn gap_id(value: u64) -> ResearchGapId {
    uuid(value).parse().expect("gap identifier is valid")
}

pub fn content_digest(content: &str) -> ContentDigest {
    let hashed = digest::digest(&digest::SHA256, content.as_bytes());
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(hashed.as_ref());
    ContentDigest::sha256(bytes)
}

fn uuid(value: u64) -> String {
    format!("00000000-0000-4000-8000-{value:012x}")
}
