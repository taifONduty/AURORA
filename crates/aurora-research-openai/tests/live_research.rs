use std::collections::BTreeSet;

use aurora_openai::{OpenAiBackend, OpenAiConfig};
use aurora_research::{
    ResearchControlLimits, ResearchControlState, ResearchControlStatus, ResearchRequest,
    RetrievedAt,
};
use aurora_research_openai::OpenAiTavilyResearcher;
use aurora_tavily::{TavilyConfig, TavilyInvestigator};
use ring::digest::{SHA256, digest};
use tokio_util::sync::CancellationToken;

fn credentials() -> (String, String, String) {
    let openai_api_key =
        std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY is required for ignored live tests");
    let openai_model = std::env::var("AURORA_OPENAI_MODEL")
        .expect("AURORA_OPENAI_MODEL is required for ignored live tests");
    let tavily_api_key =
        std::env::var("TAVILY_API_KEY").expect("TAVILY_API_KEY is required for ignored live tests");
    (openai_api_key, openai_model, tavily_api_key)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires OPENAI_API_KEY, AURORA_OPENAI_MODEL, TAVILY_API_KEY, network access, and provider credits"]
async fn default_adapters_preserve_inspectable_research_boundaries() {
    let (openai_api_key, openai_model, tavily_api_key) = credentials();
    let model = OpenAiBackend::new(
        OpenAiConfig::new(openai_api_key, openai_model)
            .expect("live OpenAI configuration is valid"),
    )
    .expect("OpenAI HTTP client builds");
    let retrieval = TavilyInvestigator::new(
        TavilyConfig::new(tavily_api_key).expect("live Tavily configuration is valid"),
    )
    .expect("Tavily HTTP client builds");
    let mut researcher = OpenAiTavilyResearcher::new(model, retrieval);

    let run = researcher
        .run(
            ResearchRequest::new(
                "What does the OpenAI API documentation say about the Responses API?".to_owned(),
            )
            .expect("research request is valid"),
            ResearchControlLimits::new(1),
            RetrievedAt::new("2026-08-29T00:00:00Z").expect("fixed retrieval time is valid"),
            CancellationToken::new(),
        )
        .await
        .expect("public model-driven run records a valid terminal outcome");

    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Completed
            | ResearchControlStatus::Failed(_)
            | ResearchControlStatus::Stopped(_)
    ));
    assert_eq!(
        ResearchControlState::reconstruct(run.records().to_vec())
            .expect("terminal records reconstruct"),
        *run.state()
    );

    let research = run.state().investigation().research();
    let evidence_ids = research
        .evidence_items()
        .map(|evidence| *evidence.id())
        .collect::<BTreeSet<_>>();
    for source in research.sources() {
        let full_snapshot = research
            .evidence_items()
            .find(|evidence| {
                evidence.source_id() == source.id()
                    && digest(&SHA256, evidence.excerpt().as_bytes()).as_ref()
                        == source.content_digest().as_sha256()
            })
            .expect("every source retains an immutable full-content snapshot");
        assert!(
            research
                .evidence_items()
                .filter(|evidence| evidence.source_id() == source.id())
                .all(|evidence| full_snapshot.excerpt().contains(evidence.excerpt()))
        );
    }
    assert!(research.claims().all(|claim| {
        claim
            .evidence_ids()
            .iter()
            .all(|evidence_id| evidence_ids.contains(evidence_id))
    }));

    if run.issue().is_none() {
        assert_eq!(run.state().status(), ResearchControlStatus::Completed);
    }
}
