use aurora_core::{ModelBackend, ModelInput, ModelInvocation, ModelItem};
use aurora_zai::{
    ReasoningEffort, ZaiBackend, ZaiConfig, ZaiJsonObjectInvocation, ZaiJsonObjectRequest,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn live_backend() -> ZaiBackend {
    let key =
        std::env::var("ZAI_API_KEY").expect("ZAI_API_KEY is required for the ignored live test");
    let config = ZaiConfig::new(key)
        .expect("live API key forms a header")
        .with_reasoning_effort(ReasoningEffort::Low);
    ZaiBackend::new(config).expect("live backend builds")
}

#[tokio::test]
#[ignore = "requires ZAI_API_KEY, Coding Plan quota, and network access"]
async fn glm_5_3_returns_visible_text_through_the_core_boundary() {
    let mut backend = live_backend();
    let result = backend
        .invoke(
            ModelInput {
                context: vec![ModelItem::UserInput {
                    text: "Reply briefly with a neutral provider connectivity acknowledgement."
                        .into(),
                }],
                tools: Vec::new(),
            },
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(
        result,
        ModelInvocation::FinalResponse { ref text } if !text.trim().is_empty()
    ));
}

#[tokio::test]
#[ignore = "requires ZAI_API_KEY, Coding Plan quota, and network access"]
async fn glm_5_3_returns_a_json_object_for_local_validation() {
    let mut backend = live_backend();
    let result = backend
        .invoke_json_object(
            ZaiJsonObjectRequest::new(
                "Return one JSON object and no prose.",
                "Acknowledge provider connectivity.",
                json!({"status":"string"}),
            )
            .unwrap(),
            CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, ZaiJsonObjectInvocation::Output(ref value) if value.is_object()));
}
