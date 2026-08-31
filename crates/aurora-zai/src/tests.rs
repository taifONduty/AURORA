use std::{
    future::Future,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc::{self, Receiver, Sender},
    task::Poll,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use aurora_core::{
    ModelBackend, ModelFuture, ModelInput, ModelInvocation, ModelItem, ModelRequestFailure,
    ToolDefinition, ToolEffect,
};
use reqwest::Url;
use tokio_util::sync::CancellationToken;

use super::*;

const FIXTURE_TIMEOUT: Duration = Duration::from_secs(5);

#[test]
fn configuration_rejects_empty_or_header_invalid_keys() {
    assert!(matches!(ZaiConfig::new(""), Err(ConfigError::EmptyApiKey)));
    assert!(matches!(
        ZaiConfig::new("bad\nkey"),
        Err(ConfigError::InvalidApiKey)
    ));
}

#[test]
fn configuration_defaults_to_high_and_redacts_the_key() {
    let key = "visible-only-to-this-test";
    let config = ZaiConfig::new(key).expect("test key forms a header");
    let debug = format!("{config:?}");
    assert_eq!(
        config.endpoint.as_str(),
        "https://api.z.ai/api/coding/paas/v4/chat/completions"
    );
    assert!(!debug.contains(key));
    assert!(debug.contains("[REDACTED]"));
    assert!(debug.contains("glm-5.3"));
    assert!(debug.contains("api.z.ai"));
    assert!(debug.contains("/api/coding/paas/v4/chat/completions"));
    assert!(debug.contains("High"));
}

#[test]
fn each_reasoning_effort_is_explicitly_selectable() {
    for effort in [
        ReasoningEffort::Low,
        ReasoningEffort::High,
        ReasoningEffort::Max,
    ] {
        let config = ZaiConfig::new("key")
            .expect("test key forms a header")
            .with_reasoning_effort(effort);
        assert!(format!("{config:?}").contains(&format!("{effort:?}")));
    }
}

#[test]
fn config_and_backend_debug_omit_endpoint_query_secrets() {
    let endpoint =
        Url::parse("https://fixture.invalid/chat/completions?api_key=endpoint-query-secret")
            .unwrap();
    let config = ZaiConfig::for_endpoint("header-secret", endpoint).unwrap();
    let config_debug = format!("{config:?}");
    let backend_debug = format!("{:?}", ZaiBackend::new(config).unwrap());

    for debug in [config_debug, backend_debug] {
        assert!(!debug.contains("endpoint-query-secret"));
        assert!(!debug.contains("header-secret"));
        assert!(!debug.contains("?api_key="));
    }
}

#[test]
fn plain_body_failures_map_to_exact_core_outcomes() {
    assert_eq!(
        plain_body_failure_outcome(BoundedBodyFailure::Cancelled),
        ModelInvocation::Cancelled
    );
    assert_eq!(
        plain_body_failure_outcome(BoundedBodyFailure::TooLarge),
        ModelInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse)
    );
    assert_eq!(
        plain_body_failure_outcome(BoundedBodyFailure::Transport),
        ModelInvocation::RequestFailure(ModelRequestFailure::Transport)
    );
}

#[test]
fn json_object_body_failures_map_to_exact_public_outcomes() {
    assert_eq!(
        json_object_body_failure_outcome(BoundedBodyFailure::Cancelled),
        ZaiJsonObjectInvocation::Cancelled
    );
    assert_eq!(
        json_object_body_failure_outcome(BoundedBodyFailure::TooLarge),
        ZaiJsonObjectInvocation::ResponseTooLarge
    );
    assert_eq!(
        json_object_body_failure_outcome(BoundedBodyFailure::Transport),
        ZaiJsonObjectInvocation::RequestFailure(ModelRequestFailure::Transport)
    );
}

#[test]
fn fixture_cleanup_joins_worker_when_completion_confirmation_is_missing() {
    let deadline = Instant::now() + Duration::from_millis(20);
    let endpoint = Url::parse("http://127.0.0.1:1/chat/completions").unwrap();
    let (_observed_tx, observed) = mpsc::channel();
    let (_phase_tx, phases) = mpsc::channel();
    let (_attempts_tx, attempts) = mpsc::channel();
    let (_completed_tx, completed) = mpsc::channel();
    let (control, _control_rx) = mpsc::channel();
    let (worker_done_tx, worker_done) = mpsc::channel();
    let server = thread::spawn(move || {
        while Instant::now() < deadline {
            thread::yield_now();
        }
        worker_done_tx.send(()).ok();
    });
    let fixture = ChatFixture {
        endpoint,
        deadline,
        observed,
        phases,
        attempts,
        completed,
        control,
        server: Some(server),
    };

    let result = fixture.shutdown_without_connection();
    assert!(result.is_err());
    assert_eq!(worker_done.try_recv(), Ok(()));
}

#[tokio::test]
async fn plain_invocation_sends_the_fixed_stateless_glm_request() {
    let fixture = ChatFixture::responding_with(
        200,
        serde_json::json!({
            "choices": [{
                "message": {"role":"assistant", "content":"visible"},
                "finish_reason":"stop"
            }]
        }),
    );
    let config = ZaiConfig::for_endpoint("test-key", fixture.endpoint().clone())
        .expect("fixture configuration is valid")
        .with_reasoning_effort(ReasoningEffort::Low);
    let mut backend = ZaiBackend::new(config).expect("client builds");
    let invocation = bounded_invocation(ModelBackend::invoke(
        &mut backend,
        ModelInput {
            context: vec![ModelItem::UserInput {
                text: "hello".into(),
            }],
            tools: Vec::new(),
        },
        CancellationToken::new(),
    ))
    .await;
    let (request, content_type) = fixture
        .finish_with_content_type()
        .expect("fixture completes");
    assert_eq!(
        invocation,
        Ok(ModelInvocation::FinalResponse {
            text: "visible".into()
        })
    );
    assert_eq!(content_type, "application/json");
    assert!(request.authorization == "Bearer test-key");
    assert_eq!(request.body["model"], "glm-5.3");
    assert_eq!(
        request.body["thinking"],
        serde_json::json!({
            "type":"enabled",
            "clear_thinking":true
        })
    );
    assert_eq!(request.body["reasoning_effort"], "low");
    assert_eq!(request.body["stream"], false);
    assert!(request.body.get("response_format").is_none());
}

#[tokio::test]
async fn exact_limit_plain_request_is_sent() {
    let input = plain_input_for_serialized_size(MAX_REQUEST_BYTES);
    let fixture = ChatFixture::responding_with_raw(200, success_body().to_vec());
    let mut backend = backend_for(&fixture);

    let result = bounded_invocation(backend.invoke(input, CancellationToken::new())).await;
    let request = fixture.finish().expect("fixture completes");
    assert_eq!(request.body_len, MAX_REQUEST_BYTES);
    assert_eq!(
        result,
        Ok(ModelInvocation::FinalResponse {
            text: "visible".into()
        })
    );
}

#[tokio::test]
async fn json_object_invocation_enables_json_mode_and_prompts_the_shape() {
    let fixture = ChatFixture::responding_with(
        200,
        serde_json::json!({
            "choices": [{
                "message": {
                    "role":"assistant",
                    "content":"{\"status\":\"ok\"}",
                    "reasoning_content":"private"
                },
                "finish_reason":"stop"
            }]
        }),
    );
    let config = ZaiConfig::for_endpoint("test-key", fixture.endpoint().clone())
        .unwrap()
        .with_reasoning_effort(ReasoningEffort::Low);
    let mut backend = ZaiBackend::new(config).unwrap();
    let result = bounded_future(
        backend.invoke_json_object(
            ZaiJsonObjectRequest::new(
                "Return the required object.",
                "fixture input",
                serde_json::json!({"status":"string"}),
            )
            .unwrap(),
            CancellationToken::new(),
        ),
    )
    .await;
    let request = fixture.finish().expect("fixture completes");
    assert_eq!(
        result,
        Ok(ZaiJsonObjectInvocation::Output(
            serde_json::json!({"status":"ok"})
        ))
    );
    assert_eq!(
        request.body["response_format"],
        serde_json::json!({"type":"json_object"})
    );
    assert_eq!(request.body["thinking"]["clear_thinking"], true);
    assert_eq!(
        request.body["messages"][0],
        serde_json::json!({
            "role":"system",
            "content":"Return the required object.\n\nExpected top-level JSON object shape:\n{\"status\":\"string\"}"
        })
    );
    assert_eq!(request.body["messages"][1]["content"], "fixture input");
}

#[tokio::test]
async fn json_object_output_rejects_non_objects_and_invalid_json() {
    for content in ["not json", "[]", "true", "42", "\"text\"", ""] {
        let fixture = ChatFixture::responding_with(
            200,
            serde_json::json!({
                "choices": [{
                    "message": {"role":"assistant", "content":content},
                    "finish_reason":"stop"
                }]
            }),
        );
        let config = ZaiConfig::for_endpoint("test-key", fixture.endpoint().clone()).unwrap();
        let mut backend = ZaiBackend::new(config).unwrap();
        let result =
            bounded_future(backend.invoke_json_object(json_request(), CancellationToken::new()))
                .await;
        let fixture_result = fixture.finish();
        assert!(fixture_result.is_ok());
        assert_eq!(result, Ok(ZaiJsonObjectInvocation::MalformedOutput));
    }
}

#[tokio::test]
async fn one_byte_over_json_object_request_limit_is_rejected() {
    let request = json_request_for_serialized_size(MAX_REQUEST_BYTES + 1);
    let fixture = ChatFixture::holding_before_headers();
    let mut backend = backend_for(&fixture);

    let result =
        bounded_future(backend.invoke_json_object(request, CancellationToken::new())).await;
    let fixture_result = fixture.shutdown_without_connection();
    assert_eq!(fixture_result, Ok(()));
    assert_eq!(result, Ok(ZaiJsonObjectInvocation::RequestTooLarge));
}

#[tokio::test]
async fn exact_limit_json_object_request_is_sent() {
    let request = json_request_for_serialized_size(MAX_REQUEST_BYTES);
    let fixture = ChatFixture::responding_with(
        200,
        serde_json::json!({
            "choices": [{
                "message": {"role":"assistant", "content":"{}"},
                "finish_reason":"stop"
            }]
        }),
    );
    let mut backend = backend_for(&fixture);

    let result =
        bounded_future(backend.invoke_json_object(request, CancellationToken::new())).await;
    let observed = fixture.finish().expect("fixture completes");
    assert_eq!(observed.body_len, MAX_REQUEST_BYTES);
    assert_eq!(
        result,
        Ok(ZaiJsonObjectInvocation::Output(serde_json::json!({})))
    );
}

#[tokio::test]
async fn one_byte_over_json_object_response_limit_is_rejected() {
    let fixture = ChatFixture::holding_after_chunks(
        200,
        vec![vec![b' '; MAX_RESPONSE_BYTES], vec![b'x'; 1]],
        vec![b"provider-tail".to_vec()],
    );
    let mut backend = backend_for(&fixture);
    let mut invocation = backend.invoke_json_object(json_request(), CancellationToken::new());
    let held = await_json_phase_while_pending(&fixture, FixturePhase::Held, &mut invocation).await;
    let result = if held.is_ok() {
        bounded_future(invocation).await
    } else {
        drop(invocation);
        Err("invocation did not reach held overflow tail")
    };
    let peer_closed = fixture.wait_for_phase(FixturePhase::PeerClosed).await;
    let fixture_result = fixture.finish();
    assert!(fixture_result.is_ok());
    assert_eq!(held, Ok(()));
    assert_eq!(peer_closed, Ok(()));
    assert_eq!(result, Ok(ZaiJsonObjectInvocation::ResponseTooLarge));
}

#[tokio::test]
async fn exact_limit_json_object_response_is_accepted() {
    let (body, value_len) = json_response_for_size(MAX_RESPONSE_BYTES);
    let fixture = ChatFixture::responding_with_raw(200, body);
    let mut backend = backend_for(&fixture);

    let result =
        bounded_future(backend.invoke_json_object(json_request(), CancellationToken::new())).await;
    let fixture_result = fixture.finish();
    assert!(fixture_result.is_ok());
    match result {
        Ok(ZaiJsonObjectInvocation::Output(value)) => {
            assert_eq!(value["value"].as_str().map(str::len), Some(value_len));
        }
        other => panic!("exact-limit JSON response was not accepted: {other:?}"),
    }
}

#[tokio::test]
async fn json_object_cancellation_returns_cancelled() {
    let fixture = ChatFixture::holding_before_headers();
    let mut backend = backend_for(&fixture);
    let cancellation = CancellationToken::new();
    let mut invocation = backend.invoke_json_object(json_request(), cancellation.clone());
    let held = await_json_phase_while_pending(&fixture, FixturePhase::Held, &mut invocation).await;
    let result = if held.is_ok() {
        cancellation.cancel();
        bounded_future(invocation).await
    } else {
        drop(invocation);
        Err("invocation did not reach held headers")
    };
    let fixture_result = if held.is_ok() {
        fixture.finish().map(|_| ())
    } else {
        fixture.shutdown_without_connection()
    };
    assert_eq!(fixture_result, Ok(()));
    assert_eq!(held, Ok(()));
    assert_eq!(result, Ok(ZaiJsonObjectInvocation::Cancelled));
}

#[tokio::test]
async fn json_object_cancellation_after_a_response_chunk_returns_cancelled() {
    let fixture = ChatFixture::holding_after_first_chunk(200, b"{".to_vec());
    let mut backend = backend_for(&fixture);
    let cancellation = CancellationToken::new();
    let mut invocation = backend.invoke_json_object(json_request(), cancellation.clone());
    let held = await_json_phase_while_pending(&fixture, FixturePhase::Held, &mut invocation).await;
    let result = if held.is_ok() {
        cancellation.cancel();
        bounded_future(invocation).await
    } else {
        drop(invocation);
        Err("invocation did not reach held response body")
    };
    let fixture_result = fixture.finish();
    assert!(fixture_result.is_ok());
    assert_eq!(held, Ok(()));
    assert_eq!(result, Ok(ZaiJsonObjectInvocation::Cancelled));
}

#[tokio::test]
async fn json_object_provider_failures_preserve_categories() {
    for (status, expected) in [
        (401, ModelRequestFailure::Authentication),
        (400, ModelRequestFailure::RequestRejected),
        (403, ModelRequestFailure::RequestRejected),
        (429, ModelRequestFailure::RateLimited),
        (500, ModelRequestFailure::ServiceUnavailable),
        (503, ModelRequestFailure::ServiceUnavailable),
        (302, ModelRequestFailure::Transport),
    ] {
        let fixture = ChatFixture::responding_with(
            status,
            serde_json::json!({"error":{"message":"private-provider-detail"}}),
        );
        let mut backend = backend_for(&fixture);
        let result =
            bounded_future(backend.invoke_json_object(json_request(), CancellationToken::new()))
                .await;
        let fixture_result = fixture.finish();
        assert!(fixture_result.is_ok());
        assert_eq!(
            result,
            Ok(ZaiJsonObjectInvocation::RequestFailure(expected))
        );
        assert!(!format!("{result:?}").contains("private-provider-detail"));
    }
}

#[tokio::test]
async fn json_object_non_success_status_does_not_wait_for_provider_body() {
    let fixture = ChatFixture::holding_after_headers(503);
    let mut backend = backend_for(&fixture);
    let mut invocation = backend.invoke_json_object(json_request(), CancellationToken::new());
    let held = await_json_phase_while_pending(&fixture, FixturePhase::Held, &mut invocation).await;
    let result = if held.is_ok() {
        bounded_future(invocation).await
    } else {
        drop(invocation);
        Err("invocation did not reach held response body")
    };
    let fixture_result = fixture.finish();
    assert!(fixture_result.is_ok());
    assert_eq!(held, Ok(()));
    assert_eq!(
        result,
        Ok(ZaiJsonObjectInvocation::RequestFailure(
            ModelRequestFailure::ServiceUnavailable
        ))
    );
}

#[tokio::test]
async fn json_mode_does_not_claim_expected_shape_conformance() {
    let fixture = ChatFixture::responding_with(
        200,
        serde_json::json!({
            "choices": [{
                "message": {"role":"assistant", "content":"{\"different\":1}"},
                "finish_reason":"stop"
            }]
        }),
    );
    let mut backend = backend_for(&fixture);
    let request = ZaiJsonObjectRequest::new(
        "Return the object.",
        "input",
        serde_json::json!({"status":"string"}),
    )
    .unwrap();

    let result =
        bounded_future(backend.invoke_json_object(request, CancellationToken::new())).await;
    let fixture_result = fixture.finish();
    assert!(fixture_result.is_ok());
    assert_eq!(
        result,
        Ok(ZaiJsonObjectInvocation::Output(
            serde_json::json!({"different":1})
        ))
    );
}

#[tokio::test]
async fn malformed_raw_success_response_is_malformed_output() {
    let fixture = ChatFixture::responding_with_raw(200, b"not json".to_vec());
    let mut backend = backend_for(&fixture);

    let result =
        bounded_invocation(backend.invoke(plain_input("hello"), CancellationToken::new())).await;
    let fixture_result = fixture.finish();
    assert!(fixture_result.is_ok());
    assert_eq!(result, Ok(ModelInvocation::MalformedOutput));
}

#[tokio::test]
async fn http_failures_keep_their_public_category_and_discard_private_bodies() {
    for (status, expected) in [
        (401, ModelRequestFailure::Authentication),
        (400, ModelRequestFailure::RequestRejected),
        (403, ModelRequestFailure::RequestRejected),
        (429, ModelRequestFailure::RateLimited),
        (500, ModelRequestFailure::ServiceUnavailable),
        (503, ModelRequestFailure::ServiceUnavailable),
        (302, ModelRequestFailure::Transport),
    ] {
        let fixture = ChatFixture::responding_with(
            status,
            serde_json::json!({"error":{"message":"private-provider-detail"}}),
        );
        let config = ZaiConfig::for_endpoint("test-key", fixture.endpoint().clone()).unwrap();
        let mut backend = ZaiBackend::new(config).unwrap();
        let result =
            bounded_invocation(backend.invoke(plain_input("hello"), CancellationToken::new()))
                .await;
        let fixture_result = fixture.finish();
        assert!(fixture_result.is_ok());
        assert_eq!(result, Ok(ModelInvocation::RequestFailure(expected)));
        assert!(!format!("{result:?}").contains("private-provider-detail"));
    }
}

#[tokio::test]
async fn closed_port_is_a_transport_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserved transport port binds");
    let address = listener.local_addr().expect("reserved port has an address");
    let endpoint = Url::parse(&format!("http://{address}/chat/completions")).unwrap();
    drop(listener);
    let config = ZaiConfig::for_endpoint("test-key", endpoint).unwrap();
    let mut backend = ZaiBackend::new(config).unwrap();

    assert_eq!(
        bounded_invocation(backend.invoke(plain_input("hello"), CancellationToken::new(),)).await,
        Ok(ModelInvocation::RequestFailure(
            ModelRequestFailure::Transport
        ))
    );
}

#[tokio::test]
async fn non_success_status_does_not_wait_for_provider_body() {
    let fixture = ChatFixture::holding_after_headers(503);
    let mut backend = backend_for(&fixture);
    let mut invocation = backend.invoke(plain_input("hello"), CancellationToken::new());
    let held = await_phase_while_pending(&fixture, FixturePhase::Held, &mut invocation).await;
    let result = if held.is_ok() {
        bounded_invocation(invocation).await
    } else {
        drop(invocation);
        Err("invocation did not reach held response body")
    };

    let fixture_result = fixture.finish();
    assert!(fixture_result.is_ok());
    assert_eq!(held, Ok(()));
    assert_eq!(
        result,
        Ok(ModelInvocation::RequestFailure(
            ModelRequestFailure::ServiceUnavailable
        ))
    );
    assert!(!format!("{result:?}").contains("private-provider-detail"));
}

#[tokio::test]
async fn cancelled_before_polling_performs_no_http() {
    let fixture = ChatFixture::holding_before_headers();
    let mut backend = backend_for(&fixture);
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let result = bounded_invocation(backend.invoke(plain_input("hello"), cancellation)).await;
    let fixture_result = fixture.shutdown_without_connection();
    assert_eq!(fixture_result, Ok(()));
    assert_eq!(result, Ok(ModelInvocation::Cancelled));
}

#[tokio::test]
async fn tool_bearing_model_input_is_rejected_before_http() {
    let fixture = ChatFixture::holding_before_headers();
    let mut backend = backend_for(&fixture);
    let input = ModelInput {
        context: vec![ModelItem::UserInput {
            text: "hello".into(),
        }],
        tools: vec![ToolDefinition {
            name: "search".into(),
            description: "Search fixture content.".into(),
            input_schema: serde_json::json!({"type":"object"}),
            effect: ToolEffect::ReadOnly,
        }],
    };

    let result = bounded_invocation(backend.invoke(input, CancellationToken::new())).await;
    let fixture_result = fixture.shutdown_without_connection();
    assert_eq!(fixture_result, Ok(()));
    assert_eq!(
        result,
        Ok(ModelInvocation::RequestFailure(
            ModelRequestFailure::RequestRejected
        ))
    );
}

#[tokio::test]
async fn invocation_construction_does_not_connect() {
    let fixture = ChatFixture::holding_before_headers();
    let mut backend = backend_for(&fixture);

    let invocation = backend.invoke(plain_input("hello"), CancellationToken::new());
    let connected = !matches!(fixture.phases.try_recv(), Err(mpsc::TryRecvError::Empty));
    drop(invocation);
    let fixture_result = fixture.shutdown_without_connection();
    assert_eq!(fixture_result, Ok(()));
    assert!(!connected);
}

#[tokio::test]
async fn cancellation_while_waiting_for_headers_returns_cancelled() {
    let fixture = ChatFixture::holding_before_headers();
    let mut backend = backend_for(&fixture);
    let cancellation = CancellationToken::new();
    let mut invocation = backend.invoke(plain_input("hello"), cancellation.clone());
    let held = await_phase_while_pending(&fixture, FixturePhase::Held, &mut invocation).await;
    let result = if held.is_ok() {
        cancellation.cancel();
        bounded_invocation(invocation).await
    } else {
        drop(invocation);
        Err("invocation did not reach held headers")
    };
    let fixture_result = fixture.finish();
    assert!(fixture_result.is_ok());
    assert_eq!(held, Ok(()));
    assert_eq!(result, Ok(ModelInvocation::Cancelled));
}

#[tokio::test]
async fn cancellation_while_reading_body_returns_cancelled() {
    let fixture = ChatFixture::holding_after_first_chunk(200, b"{".to_vec());
    let client = reqwest::Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .retry(reqwest::retry::never())
        .build()
        .expect("test client builds");
    let response = bounded_future(
        client
            .post(fixture.endpoint().clone())
            .header(AUTHORIZATION, "Bearer test-key")
            .header(CONTENT_TYPE, "application/json")
            .body(b"{}".to_vec())
            .send(),
    )
    .await;
    let response = match response {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(_)) => Err("request failed before response headers"),
        Err(failure) => Err(failure),
    };
    let held = fixture.wait_for_phase(FixturePhase::Held).await;
    let cancellation = CancellationToken::new();
    let (body_reader_was_pending, result) = match response {
        Ok(response) if held.is_ok() => {
            let mut body_reader = Box::pin(read_bounded_body(response, cancellation.clone()));
            match poll_once(body_reader.as_mut()).await {
                Poll::Pending => {
                    cancellation.cancel();
                    (true, bounded_future(body_reader.as_mut()).await)
                }
                Poll::Ready(result) => (false, Ok(result)),
            }
        }
        Ok(response) => {
            drop(response);
            (false, Err("fixture did not hold the response body"))
        }
        Err(failure) => (false, Err(failure)),
    };
    let fixture_result = fixture.finish();
    assert!(fixture_result.is_ok());
    assert_eq!(held, Ok(()));
    assert!(body_reader_was_pending);
    assert_eq!(result, Ok(Err(BoundedBodyFailure::Cancelled)));
}

#[tokio::test]
async fn plain_invocation_cancellation_after_a_response_chunk_returns_cancelled() {
    let fixture = ChatFixture::holding_after_first_chunk(200, b"{".to_vec());
    let mut backend = backend_for(&fixture);
    let cancellation = CancellationToken::new();
    let mut invocation = backend.invoke(plain_input("hello"), cancellation.clone());
    let held = await_phase_while_pending(&fixture, FixturePhase::Held, &mut invocation).await;
    let result = if held.is_ok() {
        cancellation.cancel();
        bounded_invocation(invocation).await
    } else {
        drop(invocation);
        Err("invocation did not reach held response body")
    };
    let fixture_result = fixture.finish();
    assert!(fixture_result.is_ok());
    assert_eq!(held, Ok(()));
    assert_eq!(result, Ok(ModelInvocation::Cancelled));
}

#[tokio::test]
async fn dropped_future_leaves_no_adapter_task() {
    let fixture = ChatFixture::holding_before_headers();
    let mut backend = backend_for(&fixture);
    let held = {
        let mut invocation = backend.invoke(plain_input("hello"), CancellationToken::new());
        let held = await_phase_while_pending(&fixture, FixturePhase::Held, &mut invocation).await;
        drop(invocation);
        held
    };
    let peer_closed = fixture.wait_for_phase(FixturePhase::PeerClosed).await;
    let fixture_result = fixture.finish();
    assert!(fixture_result.is_ok());
    assert_eq!(held, Ok(()));
    assert_eq!(peer_closed, Ok(()));
}

#[tokio::test]
async fn one_byte_over_plain_request_limit_fails_before_http() {
    let oversized = plain_input_for_serialized_size(MAX_REQUEST_BYTES + 1);
    let fixture = ChatFixture::holding_before_headers();
    let mut backend = backend_for(&fixture);

    let result = bounded_invocation(backend.invoke(oversized, CancellationToken::new())).await;
    let fixture_result = fixture.shutdown_without_connection();
    assert_eq!(fixture_result, Ok(()));
    assert_eq!(
        result,
        Ok(ModelInvocation::RequestFailure(
            ModelRequestFailure::RequestRejected
        ))
    );
}

#[tokio::test]
async fn one_byte_over_plain_response_limit_stops_without_partial_output() {
    let fixture = ChatFixture::holding_after_chunks(
        200,
        vec![vec![b' '; MAX_RESPONSE_BYTES], vec![b'x'; 1]],
        vec![b"provider-tail".to_vec()],
    );
    let mut backend = backend_for(&fixture);
    let mut invocation = backend.invoke(plain_input("hello"), CancellationToken::new());
    let held = await_phase_while_pending(&fixture, FixturePhase::Held, &mut invocation).await;
    let result = if held.is_ok() {
        bounded_invocation(invocation).await
    } else {
        drop(invocation);
        Err("invocation did not reach held overflow tail")
    };
    let peer_closed = fixture.wait_for_phase(FixturePhase::PeerClosed).await;
    let fixture_result = fixture.finish();
    assert!(fixture_result.is_ok());
    assert_eq!(held, Ok(()));
    assert_eq!(peer_closed, Ok(()));
    assert_eq!(
        result,
        Ok(ModelInvocation::RequestFailure(
            ModelRequestFailure::UnsupportedResponse
        ))
    );
}

#[tokio::test]
async fn exact_limit_plain_response_is_accepted() {
    let (body, content_len) = plain_response_for_size(MAX_RESPONSE_BYTES);
    let fixture = ChatFixture::responding_with_raw(200, body);
    let mut backend = backend_for(&fixture);

    let result =
        bounded_invocation(backend.invoke(plain_input("hello"), CancellationToken::new())).await;
    let fixture_result = fixture.finish();
    assert!(fixture_result.is_ok());
    match result {
        Ok(ModelInvocation::FinalResponse { text }) => assert_eq!(text.len(), content_len),
        other => panic!("exact-limit plain response was not accepted: {other:?}"),
    }
}

#[tokio::test]
async fn transport_failure_is_attempted_once() {
    let fixture = ChatFixture::closing_and_counting_attempts();
    let mut backend = backend_for(&fixture);

    let result =
        bounded_invocation(backend.invoke(plain_input("hello"), CancellationToken::new())).await;
    let (_, attempts) = fixture.finish_with_attempts().expect("fixture completes");
    assert_eq!(
        result,
        Ok(ModelInvocation::RequestFailure(
            ModelRequestFailure::Transport
        ))
    );
    assert_eq!(attempts, 1);
}

async fn bounded_invocation(invocation: ModelFuture) -> Result<ModelInvocation, &'static str> {
    bounded_future(invocation).await
}

async fn bounded_future<F: Future>(future: F) -> Result<F::Output, &'static str> {
    tokio::pin!(future);
    tokio::select! {
        result = &mut future => Ok(result),
        () = wait_for_deadline() => Err("bounded future timed out"),
    }
}

async fn poll_once<F: Future>(mut future: std::pin::Pin<&mut F>) -> Poll<F::Output> {
    std::future::poll_fn(|context| Poll::Ready(future.as_mut().poll(context))).await
}

async fn await_phase_while_pending(
    fixture: &ChatFixture,
    phase: FixturePhase,
    invocation: &mut ModelFuture,
) -> Result<(), &'static str> {
    tokio::select! {
        biased;
        phase = fixture.wait_for_phase(phase) => phase,
        _ = invocation => Err("invocation completed before fixture phase"),
    }
}

async fn await_json_phase_while_pending(
    fixture: &ChatFixture,
    phase: FixturePhase,
    invocation: &mut ZaiJsonObjectFuture,
) -> Result<(), &'static str> {
    tokio::select! {
        biased;
        phase = fixture.wait_for_phase(phase) => phase,
        _ = invocation => Err("invocation completed before fixture phase"),
    }
}

async fn wait_for_deadline() {
    let deadline = Instant::now() + FIXTURE_TIMEOUT;
    while Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
}

fn backend_for(fixture: &ChatFixture) -> ZaiBackend {
    let config = ZaiConfig::for_endpoint("test-key", fixture.endpoint().clone()).unwrap();
    ZaiBackend::new(config).unwrap()
}

fn plain_input(text: &str) -> ModelInput {
    ModelInput {
        context: vec![ModelItem::UserInput { text: text.into() }],
        tools: Vec::new(),
    }
}

fn json_request() -> ZaiJsonObjectRequest {
    ZaiJsonObjectRequest::new("Return the object.", "input", serde_json::json!({})).unwrap()
}

fn plain_input_for_serialized_size(target: usize) -> ModelInput {
    let base = serde_json::to_vec(
        &translate::plain_request(plain_input(""), ReasoningEffort::High).unwrap(),
    )
    .unwrap()
    .len();
    let input = plain_input(&"x".repeat(target - base));
    let size = serde_json::to_vec(
        &translate::plain_request(input.clone(), ReasoningEffort::High).unwrap(),
    )
    .unwrap()
    .len();
    assert_eq!(size, target);
    input
}

fn json_request_for_serialized_size(target: usize) -> ZaiJsonObjectRequest {
    fn request(input: String) -> ZaiJsonObjectRequest {
        ZaiJsonObjectRequest::new("Return the object.", input, serde_json::json!({})).unwrap()
    }

    let base = serde_json::to_vec(&translate::json_object_request(
        request("x".into()),
        ReasoningEffort::High,
    ))
    .unwrap()
    .len();
    let input = "x".repeat(1 + target - base);
    let size = serde_json::to_vec(&translate::json_object_request(
        request(input.clone()),
        ReasoningEffort::High,
    ))
    .unwrap()
    .len();
    assert_eq!(size, target);
    request(input)
}

fn plain_response_for_size(target: usize) -> (Vec<u8>, usize) {
    let base = serde_json::to_vec(&serde_json::json!({
        "choices": [{
            "message": {"role":"assistant", "content":""},
            "finish_reason":"stop"
        }]
    }))
    .unwrap()
    .len();
    let content_len = target - base;
    let body = serde_json::to_vec(&serde_json::json!({
        "choices": [{
            "message": {"role":"assistant", "content":"x".repeat(content_len)},
            "finish_reason":"stop"
        }]
    }))
    .unwrap();
    assert_eq!(body.len(), target);
    (body, content_len)
}

fn json_response_for_size(target: usize) -> (Vec<u8>, usize) {
    fn body(content: String) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "choices": [{
                "message": {"role":"assistant", "content":content},
                "finish_reason":"stop"
            }]
        }))
        .unwrap()
    }

    let base = body(serde_json::json!({"value":""}).to_string()).len();
    let value_len = target - base;
    let response = body(serde_json::json!({"value":"x".repeat(value_len)}).to_string());
    assert_eq!(response.len(), target);
    (response, value_len)
}

struct ObservedRequest {
    authorization: String,
    body: serde_json::Value,
    body_len: usize,
}

struct ParsedRequest {
    request: ObservedRequest,
    content_type: String,
}

enum FixtureControl {
    Release,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FixturePhase {
    Accepted,
    Held,
    PeerClosed,
}

enum ResponseScript {
    Complete {
        status: u16,
        body: Vec<u8>,
    },
    HoldBeforeHeaders,
    HoldAfterChunks {
        status: u16,
        chunks: Vec<Vec<u8>>,
        released_chunks: Vec<Vec<u8>>,
    },
    CloseAndCountAttempts,
}

struct FixtureWorkerChannels {
    observed: Sender<ParsedRequest>,
    phases: Sender<FixturePhase>,
    attempts: Sender<usize>,
    completed: Sender<()>,
    control: Receiver<FixtureControl>,
}

struct ChatFixture {
    endpoint: Url,
    deadline: Instant,
    observed: Receiver<ParsedRequest>,
    phases: Receiver<FixturePhase>,
    attempts: Receiver<usize>,
    completed: Receiver<()>,
    control: Sender<FixtureControl>,
    server: Option<JoinHandle<()>>,
}

impl ChatFixture {
    fn responding_with(status: u16, body: serde_json::Value) -> Self {
        let bytes = serde_json::to_vec(&body).expect("fixture response serializes");
        Self::responding_with_raw(status, bytes)
    }

    fn responding_with_raw(status: u16, body: Vec<u8>) -> Self {
        Self::start(ResponseScript::Complete { status, body })
    }

    fn holding_before_headers() -> Self {
        Self::start(ResponseScript::HoldBeforeHeaders)
    }

    fn holding_after_headers(status: u16) -> Self {
        Self::holding_after_chunks(
            status,
            Vec::new(),
            vec![b"private-provider-detail".to_vec()],
        )
    }

    fn holding_after_first_chunk(status: u16, chunk: Vec<u8>) -> Self {
        Self::holding_after_chunks(status, vec![chunk], vec![success_body().to_vec()])
    }

    fn holding_after_chunks(
        status: u16,
        chunks: Vec<Vec<u8>>,
        released_chunks: Vec<Vec<u8>>,
    ) -> Self {
        Self::start(ResponseScript::HoldAfterChunks {
            status,
            chunks,
            released_chunks,
        })
    }

    fn closing_and_counting_attempts() -> Self {
        Self::start(ResponseScript::CloseAndCountAttempts)
    }

    fn start(script: ResponseScript) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback fixture binds");
        listener
            .set_nonblocking(true)
            .expect("fixture listener becomes nonblocking");
        let address = listener.local_addr().expect("fixture has an address");
        let endpoint = Url::parse(&format!("http://{address}/chat/completions"))
            .expect("fixture endpoint is valid");
        let (observed_tx, observed) = mpsc::channel();
        let (phase_tx, phases) = mpsc::channel();
        let (attempts_tx, attempts) = mpsc::channel();
        let (completed_tx, completed) = mpsc::channel();
        let (control, control_rx) = mpsc::channel();
        let deadline = Instant::now() + FIXTURE_TIMEOUT;
        let server = thread::spawn(move || {
            run_fixture(
                listener,
                deadline,
                script,
                FixtureWorkerChannels {
                    observed: observed_tx,
                    phases: phase_tx,
                    attempts: attempts_tx,
                    completed: completed_tx,
                    control: control_rx,
                },
            );
        });
        Self {
            endpoint,
            deadline,
            observed,
            phases,
            attempts,
            completed,
            control,
            server: Some(server),
        }
    }

    fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    fn finish(mut self) -> Result<ObservedRequest, &'static str> {
        let (request, _, _) = self.finish_inner()?;
        Ok(request)
    }

    fn finish_with_attempts(mut self) -> Result<(ObservedRequest, usize), &'static str> {
        let (request, attempts, _) = self.finish_inner()?;
        Ok((request, attempts))
    }

    fn finish_with_content_type(mut self) -> Result<(ObservedRequest, String), &'static str> {
        let (request, _, content_type) = self.finish_inner()?;
        Ok((request, content_type))
    }

    fn finish_inner(&mut self) -> Result<(ObservedRequest, usize, String), &'static str> {
        self.control.send(FixtureControl::Release).ok();
        self.control.send(FixtureControl::Stop).ok();
        let request = recv_before(
            &self.observed,
            self.deadline,
            "fixture did not observe one request before the deadline",
        );
        let attempts = self.await_completion();
        let request = request?;
        let attempts = attempts?;
        Ok((request.request, attempts, request.content_type))
    }

    fn await_completion(&mut self) -> Result<usize, &'static str> {
        let completed = recv_before(
            &self.completed,
            self.deadline,
            "fixture server did not confirm completion before the deadline",
        );
        let attempts = recv_before(
            &self.attempts,
            self.deadline,
            "fixture did not report accepted connections before the deadline",
        );
        let server = self.server.take().ok_or("fixture server is not owned")?;
        // Worker I/O shares the fixture deadline, so this join is bounded.
        let joined = server
            .join()
            .map_err(|_| "fixture server panicked before cleanup completed");
        completed?;
        joined?;
        attempts
    }

    fn shutdown_without_connection(mut self) -> Result<(), &'static str> {
        let stopped = self
            .control
            .send(FixtureControl::Stop)
            .map_err(|_| "fixture server did not receive the stop signal");
        let attempts = self.await_completion();
        let no_request = matches!(
            self.observed.try_recv(),
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected)
        );
        stopped?;
        let attempts = attempts?;
        if attempts != 0 {
            return Err("fixture accepted an unexpected connection");
        }
        if !no_request {
            return Err("fixture observed an unexpected request");
        }
        Ok(())
    }

    async fn wait_for_phase(&self, expected: FixturePhase) -> Result<(), &'static str> {
        loop {
            match self.phases.try_recv() {
                Ok(phase) if phase == expected => return Ok(()),
                Ok(_) | Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("fixture ended before reaching expected phase");
                }
            }
            if Instant::now() >= self.deadline {
                return Err("fixture phase wait timed out");
            }
            tokio::task::yield_now().await;
        }
    }
}

fn recv_before<T>(
    receiver: &Receiver<T>,
    deadline: Instant,
    failure: &'static str,
) -> Result<T, &'static str> {
    match receiver.try_recv() {
        Ok(value) => return Ok(value),
        Err(mpsc::TryRecvError::Disconnected) => return Err(failure),
        Err(mpsc::TryRecvError::Empty) => {}
    }
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(failure)?;
    receiver.recv_timeout(remaining).map_err(|_| failure)
}

fn fixture_wait(deadline: Instant, cap: Duration) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .map(|remaining| remaining.min(cap))
}

fn fixture_remaining(deadline: Instant) -> std::io::Result<Duration> {
    fixture_wait(deadline, FIXTURE_TIMEOUT).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::TimedOut, "fixture lifetime expired")
    })
}

fn read_before(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    deadline: Instant,
) -> std::io::Result<usize> {
    loop {
        stream.set_read_timeout(Some(fixture_remaining(deadline)?))?;
        match stream.read(buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            result => return result,
        }
    }
}

fn write_all_before(
    stream: &mut TcpStream,
    mut bytes: &[u8],
    deadline: Instant,
) -> std::io::Result<()> {
    while !bytes.is_empty() {
        stream.set_write_timeout(Some(fixture_remaining(deadline)?))?;
        match stream.write(bytes) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "fixture socket stopped accepting bytes",
                ));
            }
            Ok(written) => bytes = &bytes[written..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn run_fixture(
    listener: TcpListener,
    deadline: Instant,
    script: ResponseScript,
    channels: FixtureWorkerChannels,
) {
    let attempts = match accept_until_stopped(&listener, &channels.control, deadline) {
        Some(mut stream) => {
            channels.phases.send(FixturePhase::Accepted).ok();
            let request = read_request(&mut stream, deadline);
            channels
                .observed
                .send(request)
                .expect("fixture owner receives request");
            match script {
                ResponseScript::Complete { status, body } => {
                    write_response(&mut stream, status, &body, deadline)
                        .expect("fixture writes response");
                    1
                }
                ResponseScript::HoldBeforeHeaders => {
                    if hold_connection(&stream, &channels.control, &channels.phases, deadline) {
                        write_response(&mut stream, 200, success_body(), deadline).ok();
                    }
                    1
                }
                ResponseScript::HoldAfterChunks {
                    status,
                    chunks,
                    released_chunks,
                } => {
                    write_chunked_headers(&mut stream, status, deadline)
                        .expect("fixture writes held response headers");
                    for chunk in chunks {
                        write_chunk(&mut stream, &chunk, deadline).ok();
                    }
                    if hold_connection(&stream, &channels.control, &channels.phases, deadline) {
                        for chunk in released_chunks {
                            write_chunk(&mut stream, &chunk, deadline).ok();
                        }
                        write_all_before(&mut stream, b"0\r\n\r\n", deadline).ok();
                    }
                    1
                }
                ResponseScript::CloseAndCountAttempts => {
                    drop(stream);
                    count_additional_attempts(&listener, &channels.control, deadline)
                }
            }
        }
        None => 0,
    };
    channels.attempts.send(attempts).ok();
    channels.completed.send(()).ok();
}

fn accept_until_stopped(
    listener: &TcpListener,
    control: &Receiver<FixtureControl>,
    deadline: Instant,
) -> Option<TcpStream> {
    loop {
        let wait = fixture_wait(deadline, Duration::from_millis(10))?;
        match listener.accept() {
            Ok((stream, _)) => return Some(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("fixture accept failed: {error}"),
        }
        match control.recv_timeout(wait) {
            Ok(FixtureControl::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            Ok(FixtureControl::Release) | Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn hold_connection(
    stream: &TcpStream,
    control: &Receiver<FixtureControl>,
    phases: &Sender<FixturePhase>,
    deadline: Instant,
) -> bool {
    phases.send(FixturePhase::Held).ok();
    loop {
        let Some(wait) = fixture_wait(deadline, Duration::from_millis(10)) else {
            return false;
        };
        match control.recv_timeout(wait) {
            Ok(FixtureControl::Release) => return true,
            Ok(FixtureControl::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let Some(read_timeout) = fixture_wait(deadline, Duration::from_millis(20)) else {
            return false;
        };
        stream
            .set_read_timeout(Some(read_timeout))
            .expect("held connection read is bounded");
        let mut byte = [0_u8; 1];
        match stream.peek(&mut byte) {
            Ok(0) => {
                phases.send(FixturePhase::PeerClosed).ok();
                return false;
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => {
                phases.send(FixturePhase::PeerClosed).ok();
                return false;
            }
        }
    }
}

fn count_additional_attempts(
    listener: &TcpListener,
    control: &Receiver<FixtureControl>,
    deadline: Instant,
) -> usize {
    let mut attempts = 1;
    loop {
        let Some(wait) = fixture_wait(deadline, Duration::from_millis(10)) else {
            return attempts;
        };
        match listener.accept() {
            Ok((stream, _)) => {
                attempts += 1;
                drop(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("fixture accept failed: {error}"),
        }
        match control.recv_timeout(wait) {
            Ok(FixtureControl::Release | FixtureControl::Stop)
            | Err(mpsc::RecvTimeoutError::Disconnected) => return attempts,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn read_request(stream: &mut TcpStream, deadline: Instant) -> ParsedRequest {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 4096];
        let read = read_before(stream, &mut chunk, deadline).expect("fixture reads request");
        assert_ne!(read, 0, "request ended before headers completed");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end])
        .expect("request headers are UTF-8")
        .to_owned();
    let content_length = header_value(&headers, "content-length")
        .expect("request has content-length")
        .parse::<usize>()
        .expect("content-length is numeric");
    while bytes.len() - header_end < content_length {
        let mut chunk = [0_u8; 4096];
        let read = read_before(stream, &mut chunk, deadline).expect("fixture reads request body");
        assert_ne!(read, 0, "request ended before body completed");
        bytes.extend_from_slice(&chunk[..read]);
    }
    let authorization = header_value(&headers, "authorization")
        .expect("request has authorization")
        .to_owned();
    let content_type = header_value(&headers, "content-type")
        .expect("request has content type")
        .to_owned();
    let body_bytes = &bytes[header_end..header_end + content_length];
    let body = serde_json::from_slice(body_bytes).expect("request body is JSON");
    ParsedRequest {
        request: ObservedRequest {
            authorization,
            body,
            body_len: body_bytes.len(),
        },
        content_type,
    }
}

fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (candidate, value) = line.split_once(':')?;
        candidate.eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    body: &[u8],
    deadline: Instant,
) -> std::io::Result<()> {
    let reason = if status == 200 { "OK" } else { "Fixture" };
    let headers = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    write_all_before(stream, headers.as_bytes(), deadline)?;
    write_all_before(stream, body, deadline)
}

fn write_chunked_headers(
    stream: &mut TcpStream,
    status: u16,
    deadline: Instant,
) -> std::io::Result<()> {
    let reason = if status == 200 { "OK" } else { "Fixture" };
    let headers = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    );
    write_all_before(stream, headers.as_bytes(), deadline)
}

fn write_chunk(stream: &mut TcpStream, chunk: &[u8], deadline: Instant) -> std::io::Result<()> {
    let header = format!("{:x}\r\n", chunk.len());
    write_all_before(stream, header.as_bytes(), deadline)?;
    write_all_before(stream, chunk, deadline)?;
    write_all_before(stream, b"\r\n", deadline)
}

fn success_body() -> &'static [u8] {
    br#"{"choices":[{"message":{"role":"assistant","content":"visible"},"finish_reason":"stop"}]}"#
}

impl Drop for ChatFixture {
    fn drop(&mut self) {
        self.control.send(FixtureControl::Stop).ok();
        if let Some(server) = self.server.take() {
            server.join().ok();
        }
    }
}
