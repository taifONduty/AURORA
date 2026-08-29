use std::str::FromStr;

use aurora_research::{
    Claim, ClaimId, ContentDigest, Evidence, EvidenceAssessment, EvidenceId, EvidenceRelation,
    EvidenceSufficiency, InvestigationEvent, InvestigationRecord, InvestigationResult,
    InvestigationTask, InvestigationTaskId, MediaType, ResearchControlEvent, ResearchControlLimits,
    ResearchControlRecord, ResearchControlState, ResearchEvent, ResearchGap, ResearchGapCause,
    ResearchGapId, ResearchPlan, ResearchRecord, ResearchRequest, RetrievedAt, Source, SourceId,
    SynthesisBasis, VerificationAssessment, VerificationId, VerificationRecord,
};
use serde_json::Value;

use crate::synthesis_context::{
    MAX_SYNTHESIS_CONTEXT_BYTES, SynthesisContextError, synthesis_context,
};

#[test]
fn synthesis_context_serializes_only_authoritative_basis_data() {
    let context = synthesis_context(&fixture_basis("The source supports the claim."))
        .expect("fixture basis fits context limit");
    let value: Value = serde_json::from_str(&context).expect("context is JSON");

    assert_eq!(
        value["research_question"],
        "What does the evidence establish?"
    );
    assert_eq!(value["scope"]["status"], "partial");
    assert_eq!(value["scope"]["reason"]["kind"], "operator_stopped");
    assert_eq!(value["claims"].as_array().map(Vec::len), Some(1));
    let claim = &value["claims"][0];
    assert_eq!(claim["claim_id"], claim_id().to_string());
    assert_eq!(claim["statement"], "The claim is supported.");
    assert_eq!(claim["presentation"], "established");
    assert_eq!(
        claim["assessments"][0]["verification_id"],
        verification_id().to_string()
    );
    assert_eq!(claim["assessments"][0]["sufficiency"], "sufficient");
    assert_eq!(
        claim["assessments"][0]["relations"][0]["relation"],
        "supports"
    );
    assert_eq!(
        claim["evidence"][0]["evidence_id"],
        evidence_id().to_string()
    );
    assert_eq!(
        claim["evidence"][0]["excerpt"],
        "The source supports the claim."
    );
    assert_eq!(
        claim["evidence"][0]["source"]["source_id"],
        source_id().to_string()
    );
    assert_eq!(
        claim["evidence"][0]["source"]["content_digest"],
        "0101010101010101010101010101010101010101010101010101010101010101"
    );
    assert_eq!(claim["gaps"].as_array().map(Vec::len), Some(0));
    assert!(value.get("response_id").is_none());
    assert!(value.get("continuation").is_none());
    assert!(!context.contains("tavily"));
    assert!(!context.contains("The unassessed claim."));
}

#[test]
fn synthesis_context_rejects_oversize_serialization_without_truncation() {
    let basis = fixture_basis(&"x".repeat(MAX_SYNTHESIS_CONTEXT_BYTES));
    assert_eq!(
        synthesis_context(&basis),
        Err(SynthesisContextError::TooLarge)
    );
}

#[test]
fn synthesis_context_includes_open_verification_gap_identity_cause_and_status() {
    let context = synthesis_context(&fixture_basis_with_open_gap("Evidence needs follow-up."))
        .expect("fixture basis fits context limit");
    let value: Value = serde_json::from_str(&context).expect("context is JSON");
    let gap = &value["claims"][0]["gaps"][0];

    assert_eq!(gap["gap_id"], gap_id().to_string());
    assert_eq!(gap["description"], "More evidence is needed.");
    assert_eq!(gap["cause"]["kind"], "verification");
    assert_eq!(
        gap["cause"]["verification_id"],
        verification_id().to_string()
    );
    assert_eq!(gap["status"]["kind"], "open");
}

fn fixture_basis(excerpt: &str) -> SynthesisBasis {
    fixture_basis_with(excerpt, false)
}

fn fixture_basis_with_open_gap(excerpt: &str) -> SynthesisBasis {
    fixture_basis_with(excerpt, true)
}

fn fixture_basis_with(excerpt: &str, open_gap: bool) -> SynthesisBasis {
    let source = Source::new(
        source_id(),
        ContentDigest::sha256([1; 32]),
        "https://example.test/source".to_owned(),
        Some("Source title".to_owned()),
        RetrievedAt::new("2026-08-29T00:00:00Z".to_owned()).expect("timestamp is valid"),
        MediaType::new("text/plain".to_owned()).expect("media type is valid"),
    )
    .expect("source is valid");
    let evidence =
        Evidence::new(evidence_id(), source_id(), excerpt.to_owned()).expect("evidence is valid");
    let claim = Claim::new(
        claim_id(),
        "The claim is supported.".to_owned(),
        vec![evidence_id()],
    )
    .expect("claim is valid");
    let unassessed = Claim::new(
        unassessed_claim_id(),
        "The unassessed claim.".to_owned(),
        vec![evidence_id()],
    )
    .expect("claim is valid");
    let mut records = vec![
        control_record(
            1,
            ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(0)),
        ),
        control_record(
            2,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                1,
                InvestigationEvent::RequestRecorded(
                    ResearchRequest::new("What does the evidence establish?".to_owned())
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
                        InvestigationTask::initial(task_id(), "Assess source".to_owned())
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
                InvestigationEvent::TaskStarted { task_id: task_id() },
            )),
        ),
        control_record(
            5,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                4,
                InvestigationEvent::TaskCompleted {
                    task_id: task_id(),
                    result: InvestigationResult::new(vec![
                        research_record(1, ResearchEvent::SourceRecorded(source)),
                        research_record(2, ResearchEvent::EvidenceRecorded(evidence)),
                        research_record(3, ResearchEvent::ClaimProposed(claim)),
                        research_record(4, ResearchEvent::ClaimProposed(unassessed)),
                    ]),
                },
            )),
        ),
        control_record(
            6,
            ResearchControlEvent::VerificationRecorded(
                VerificationRecord::new(
                    1,
                    VerificationAssessment::new(
                        verification_id(),
                        claim_id(),
                        vec![EvidenceAssessment::new(
                            evidence_id(),
                            EvidenceRelation::Supports,
                        )],
                        if open_gap {
                            EvidenceSufficiency::Insufficient
                        } else {
                            EvidenceSufficiency::Sufficient
                        },
                    )
                    .expect("assessment is valid"),
                )
                .expect("verification record is valid"),
            ),
        ),
    ];
    let stop_sequence = if open_gap {
        records.push(control_record(
            7,
            ResearchControlEvent::GapIdentified(aurora_research::IdentifiedResearchGap::new(
                gap_id(),
                ResearchGapCause::Verification(verification_id()),
                ResearchGap::new("More evidence is needed.".to_owned()).expect("gap is valid"),
            )),
        ));
        8
    } else {
        7
    };
    records.push(control_record(
        stop_sequence,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            5,
            InvestigationEvent::ResearchStopped(
                aurora_research::ResearchStopReason::OperatorStopped,
            ),
        )),
    ));
    let state = ResearchControlState::reconstruct(records).expect("control state is valid");
    SynthesisBasis::from_state(&state).expect("stopped state produces a basis")
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

fn source_id() -> SourceId {
    SourceId::from_str("123e4567-e89b-42d3-a456-426614174001").expect("fixture ID is v4")
}

fn evidence_id() -> EvidenceId {
    EvidenceId::from_str("123e4567-e89b-42d3-a456-426614174002").expect("fixture ID is v4")
}

fn claim_id() -> ClaimId {
    ClaimId::from_str("123e4567-e89b-42d3-a456-426614174003").expect("fixture ID is v4")
}

fn unassessed_claim_id() -> ClaimId {
    ClaimId::from_str("123e4567-e89b-42d3-a456-426614174004").expect("fixture ID is v4")
}

fn task_id() -> InvestigationTaskId {
    InvestigationTaskId::from_str("123e4567-e89b-42d3-a456-426614174005").expect("fixture ID is v4")
}

fn verification_id() -> VerificationId {
    VerificationId::from_str("123e4567-e89b-42d3-a456-426614174006").expect("fixture ID is v4")
}

fn gap_id() -> ResearchGapId {
    ResearchGapId::from_str("123e4567-e89b-42d3-a456-426614174007").expect("fixture ID is v4")
}
