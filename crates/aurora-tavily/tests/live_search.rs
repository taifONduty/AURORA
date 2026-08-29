use aurora_research::{
    InvestigationTask, InvestigationTaskId, ResearchEvent, ResearchState, RetrievedAt,
};
use aurora_tavily::{TavilyConfig, TavilyInvestigator};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires TAVILY_API_KEY, network access, and Tavily credits"]
async fn live_search_acquires_at_least_one_snapshot() {
    let api_key =
        std::env::var("TAVILY_API_KEY").expect("TAVILY_API_KEY is required for ignored live tests");
    let investigator = TavilyInvestigator::new(
        TavilyConfig::new(api_key).expect("live Tavily configuration is valid"),
    )
    .expect("live Tavily HTTP client builds");
    let task = InvestigationTask::initial(
        InvestigationTaskId::generate(),
        "What is Rust ownership?".to_owned(),
    )
    .expect("live investigation task is valid");
    let retrieved_at =
        RetrievedAt::new("2026-08-29T12:34:56Z").expect("live retrieval time is valid");

    let result = investigator
        .investigate(&task, 1, retrieved_at)
        .await
        .expect("live Tavily search returns an investigation result");
    let records = result.research_records();
    let [source_record, evidence_record, ..] = records else {
        panic!("live Tavily search records at least one source and evidence pair");
    };
    let ResearchEvent::SourceRecorded(source) = source_record.event() else {
        panic!("live Tavily search records a source before its evidence");
    };
    let ResearchEvent::EvidenceRecorded(evidence) = evidence_record.event() else {
        panic!("live Tavily search records evidence after its source");
    };

    assert_eq!(evidence.source_id(), source.id());
    assert!(
        source
            .content_digest()
            .as_sha256()
            .iter()
            .any(|byte| *byte != 0)
    );

    let reconstructed = ResearchState::reconstruct(records.to_vec())
        .expect("live Tavily research records reconstruct");
    assert_eq!(reconstructed.source(source.id()), Some(source));
    assert_eq!(reconstructed.evidence(evidence.id()), Some(evidence));
}
