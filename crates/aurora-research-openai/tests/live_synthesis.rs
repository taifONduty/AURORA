use std::str::FromStr;

use aurora_openai::{OpenAiBackend, OpenAiConfig};
use aurora_research::{
    Claim, ClaimId, ContentDigest, Evidence, EvidenceAssessment, EvidenceId, EvidenceRelation,
    EvidenceSufficiency, GroundedReport, InvestigationEvent, InvestigationRecord,
    InvestigationResult, InvestigationTask, InvestigationTaskId, MediaType, ResearchControlEvent,
    ResearchControlLimits, ResearchControlRecord, ResearchControlState, ResearchControlStatus,
    ResearchEvent, ResearchPlan, ResearchRecord, ResearchRequest, RetrievedAt, Source, SourceId,
    VerificationAssessment, VerificationId, VerificationRecord,
};
use aurora_research_openai::OpenAiResearchSynthesizer;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore = "requires OPENAI_API_KEY, AURORA_OPENAI_MODEL, network, and credits"]
async fn default_openai_synthesis_cites_only_local_completed_research() {
    let api_key =
        std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY is required for ignored live tests");
    let model = std::env::var("AURORA_OPENAI_MODEL")
        .expect("AURORA_OPENAI_MODEL is required for ignored live tests");
    let state = completed_state();
    let backend = OpenAiBackend::new(
        OpenAiConfig::new(api_key, model).expect("live OpenAI configuration is valid"),
    )
    .expect("OpenAI HTTP client builds");
    let mut synthesizer = OpenAiResearchSynthesizer::new(backend);

    let report = synthesizer
        .synthesize(&state, CancellationToken::new())
        .await
        .expect("public synthesis returns a grounded report");

    assert_eq!(state.status(), ResearchControlStatus::Completed);
    assert_report_citations_are_local(&report, &state);
}

fn assert_report_citations_are_local(report: &GroundedReport, state: &ResearchControlState) {
    let research = state.investigation().research();
    let mut citation_count = 0;

    for citation in report
        .sections()
        .flat_map(|section| section.assertions())
        .flat_map(|assertion| assertion.citations())
        .chain(report.citations())
    {
        citation_count += 1;
        assert!(!citation.is_fallback());
        let mut provenance_count = 0;
        for (verification_id, relation) in citation.provenance() {
            provenance_count += 1;
            assert_ne!(relation, EvidenceRelation::Irrelevant);
            let assessment = state
                .verification()
                .assessment(verification_id)
                .expect("citation verification exists in local research");
            assert!(
                assessment.claim_id() == citation.claim_id(),
                "citation verification targets the cited claim"
            );
            assert_eq!(
                assessment.relation(citation.evidence().id()),
                Some(relation),
                "citation relation matches the local verification"
            );
        }
        assert!(provenance_count > 0);
        assert_eq!(
            research.evidence(citation.evidence().id()),
            Some(citation.evidence()),
            "citation evidence matches local research"
        );
        assert_eq!(
            research.source(citation.evidence().source_id()),
            Some(citation.source()),
            "citation source matches local research"
        );
        assert_eq!(
            research
                .source(citation.source().id())
                .expect("citation source exists in local research")
                .content_digest(),
            citation.source().content_digest(),
            "citation source digest matches local research"
        );
        assert_eq!(citation.evidence().source_id(), citation.source().id());
    }

    assert!(
        citation_count > 0,
        "synthesis returns at least one citation"
    );
}

fn completed_state() -> ResearchControlState {
    let proposal_source = Source::new(
        source_id(),
        ContentDigest::sha256([1; 32]),
        "https://example.test/aurora-synthesis/proposal".to_owned(),
        Some("AURORA proposal fixture".to_owned()),
        RetrievedAt::new("2026-08-29T00:00:00Z").expect("timestamp is valid"),
        MediaType::new("text/plain").expect("media type is valid"),
    )
    .expect("source is valid");
    let proposal_evidence = Evidence::new(
        evidence_id(),
        source_id(),
        "The proposal basis names the local fixture claim.".to_owned(),
    )
    .expect("evidence is valid");
    let assessed_source = Source::new(
        assessed_source_id(),
        ContentDigest::sha256([2; 32]),
        "https://example.test/aurora-synthesis/assessment".to_owned(),
        Some("AURORA assessment fixture".to_owned()),
        RetrievedAt::new("2026-08-29T00:00:00Z").expect("timestamp is valid"),
        MediaType::new("text/plain").expect("media type is valid"),
    )
    .expect("source is valid");
    let assessed_evidence = Evidence::new(
        assessed_evidence_id(),
        assessed_source_id(),
        "The assessment evidence supports the local fixture claim.".to_owned(),
    )
    .expect("evidence is valid");
    let claim = Claim::new(
        claim_id(),
        "AURORA constructs reports from locally retained research evidence.".to_owned(),
        vec![evidence_id()],
    )
    .expect("claim is valid");
    let task = InvestigationTask::initial(task_id(), "Assess fixture evidence".to_owned())
        .expect("task is valid");
    let research_records = vec![
        research(1, ResearchEvent::SourceRecorded(proposal_source)),
        research(2, ResearchEvent::EvidenceRecorded(proposal_evidence)),
        research(3, ResearchEvent::SourceRecorded(assessed_source)),
        research(4, ResearchEvent::EvidenceRecorded(assessed_evidence)),
        research(5, ResearchEvent::ClaimProposed(claim)),
    ];

    ResearchControlState::reconstruct([
        control(
            1,
            ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(0)),
        ),
        control(
            2,
            ResearchControlEvent::InvestigationAdvanced(investigation(
                1,
                InvestigationEvent::RequestRecorded(
                    ResearchRequest::new(
                        "What does the local synthesis fixture establish?".to_owned(),
                    )
                    .expect("request is valid"),
                ),
            )),
        ),
        control(
            3,
            ResearchControlEvent::InvestigationAdvanced(investigation(
                2,
                InvestigationEvent::PlanRecorded(
                    ResearchPlan::new(vec![task.clone()]).expect("plan is valid"),
                ),
            )),
        ),
        control(
            4,
            ResearchControlEvent::InvestigationAdvanced(investigation(
                3,
                InvestigationEvent::TaskStarted { task_id: task_id() },
            )),
        ),
        control(
            5,
            ResearchControlEvent::InvestigationAdvanced(investigation(
                4,
                InvestigationEvent::TaskCompleted {
                    task_id: task_id(),
                    result: InvestigationResult::new(research_records),
                },
            )),
        ),
        control(
            6,
            ResearchControlEvent::VerificationRecorded(
                VerificationRecord::new(
                    1,
                    VerificationAssessment::new(
                        verification_id(),
                        claim_id(),
                        vec![EvidenceAssessment::new(
                            assessed_evidence_id(),
                            EvidenceRelation::Supports,
                        )],
                        EvidenceSufficiency::Sufficient,
                    )
                    .expect("assessment is valid"),
                )
                .expect("verification record is valid"),
            ),
        ),
        control(7, ResearchControlEvent::ResearchCompleted),
    ])
    .expect("completed fixture history reconstructs")
}

fn control(sequence: u64, event: ResearchControlEvent) -> ResearchControlRecord {
    ResearchControlRecord::new(sequence, event).expect("control record is valid")
}

fn investigation(sequence: u64, event: InvestigationEvent) -> InvestigationRecord {
    InvestigationRecord::new(sequence, event).expect("investigation record is valid")
}

fn research(sequence: u64, event: ResearchEvent) -> ResearchRecord {
    ResearchRecord::new(sequence, event).expect("research record is valid")
}

fn source_id() -> SourceId {
    SourceId::from_str("123e4567-e89b-42d3-a456-426614174101").expect("fixture ID is v4")
}

fn evidence_id() -> EvidenceId {
    EvidenceId::from_str("123e4567-e89b-42d3-a456-426614174102").expect("fixture ID is v4")
}

fn assessed_source_id() -> SourceId {
    SourceId::from_str("123e4567-e89b-42d3-a456-426614174106").expect("fixture ID is v4")
}

fn assessed_evidence_id() -> EvidenceId {
    EvidenceId::from_str("123e4567-e89b-42d3-a456-426614174107").expect("fixture ID is v4")
}

fn claim_id() -> ClaimId {
    ClaimId::from_str("123e4567-e89b-42d3-a456-426614174103").expect("fixture ID is v4")
}

fn task_id() -> InvestigationTaskId {
    InvestigationTaskId::from_str("123e4567-e89b-42d3-a456-426614174104").expect("fixture ID is v4")
}

fn verification_id() -> VerificationId {
    VerificationId::from_str("123e4567-e89b-42d3-a456-426614174105").expect("fixture ID is v4")
}
