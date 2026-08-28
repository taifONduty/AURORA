use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::Duration,
};

use aurora_core::{
    AuthorizationDecision, Authorizer, DomainEvent, EventEnvelope, EventStore, FinishReason,
    FixtureTool, FixtureToolBehavior, InMemoryEventStore, ModelBackend, ModelFailure, ModelInput,
    ModelInvocation, ModelRequestFailure, RunDriver, RunId, RunLimits, RunStart, Tool,
    ToolAuthorization, ToolCatalog,
};
use aurora_openai::{OpenAiBackend, OpenAiConfig};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct ObservedRequest {
    request_line: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

enum ResponseScript {
    Json {
        status: &'static str,
        body: Value,
    },
    Raw {
        status: &'static str,
        body: Vec<u8>,
    },
    Truncated {
        status: &'static str,
        declared_length: usize,
        body: Vec<u8>,
    },
    HoldBodyAfterHeaders {
        status: &'static str,
        release: mpsc::Receiver<()>,
    },
    RedirectToSelf,
    Disconnect,
    Hold(mpsc::Receiver<()>),
}

struct ScriptedEndpoint {
    endpoint: String,
    requests: mpsc::Receiver<ObservedRequest>,
    join: Option<thread::JoinHandle<()>>,
}

impl ScriptedEndpoint {
    fn spawn(scripts: Vec<ResponseScript>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("local listener binds");
        let address = listener.local_addr().expect("listener has an address");
        let (request_sender, requests) = mpsc::channel();
        let join = thread::spawn(move || {
            let mut scripts = VecDeque::from(scripts);
            while let Some(script) = scripts.pop_front() {
                let (mut stream, _) = listener.accept().expect("scripted request arrives");
                if matches!(&script, ResponseScript::Disconnect) {
                    continue;
                }
                let request = read_request(&mut stream);
                request_sender
                    .send(request)
                    .expect("test still observes requests");
                match script {
                    ResponseScript::Json { status, body } => {
                        let bytes = serde_json::to_vec(&body).expect("fixture serializes");
                        write_response(&mut stream, status, &bytes);
                    }
                    ResponseScript::Raw { status, body } => {
                        write_response(&mut stream, status, &body);
                    }
                    ResponseScript::Truncated {
                        status,
                        declared_length,
                        body,
                    } => {
                        write_response_with_length(&mut stream, status, declared_length, &body);
                    }
                    ResponseScript::HoldBodyAfterHeaders { status, release } => {
                        write_response_headers(&mut stream, status, 128);
                        release
                            .recv_timeout(Duration::from_secs(5))
                            .expect("test releases held response body");
                    }
                    ResponseScript::RedirectToSelf => {
                        write_redirect(&mut stream);
                    }
                    ResponseScript::Disconnect => {
                        unreachable!("disconnect is handled before request parsing");
                    }
                    ResponseScript::Hold(release) => {
                        release.recv().expect("test releases held response");
                    }
                }
            }
        });
        Self {
            endpoint: format!("http://{address}/v1/responses"),
            requests,
            join: Some(join),
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn next_request(&self) -> ObservedRequest {
        self.requests
            .recv_timeout(Duration::from_secs(5))
            .expect("scripted request is observed")
    }

    fn finish(mut self) {
        self.join
            .take()
            .expect("server thread is owned")
            .join()
            .expect("server thread joins");
    }
}

fn read_request(stream: &mut TcpStream) -> ObservedRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout is configured");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let (header_end, content_length) = loop {
        let count = stream.read(&mut buffer).expect("request bytes read");
        assert_ne!(count, 0, "connection closed before request completed");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(header_end) = find_header_end(&bytes) {
            let headers =
                std::str::from_utf8(&bytes[..header_end]).expect("request headers are UTF-8");
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| {
                        value
                            .trim()
                            .parse::<usize>()
                            .expect("content length is numeric")
                    })
                })
                .expect("reqwest sends Content-Length");
            if bytes.len() >= header_end + 4 + content_length {
                break (header_end, content_length);
            }
        }
    };
    let header_text = std::str::from_utf8(&bytes[..header_end]).expect("request headers are UTF-8");
    let mut lines = header_text.lines();
    let request_line = lines.next().expect("request line is present").to_owned();
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.to_ascii_lowercase(), value.trim().to_owned()))
        })
        .collect();
    let body_start = header_end + 4;
    let body = serde_json::from_slice(&bytes[body_start..body_start + content_length])
        .expect("request body is JSON");
    ObservedRequest {
        request_line,
        headers,
        body,
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn write_response(stream: &mut TcpStream, status: &str, body: &[u8]) {
    write_response_with_length(stream, status, body.len(), body);
}

fn write_response_with_length(
    stream: &mut TcpStream,
    status: &str,
    declared_length: usize,
    body: &[u8],
) {
    write_response_headers(stream, status, declared_length);
    stream.write_all(body).expect("response body writes");
    stream.flush().expect("response flushes");
}

fn write_response_headers(stream: &mut TcpStream, status: &str, declared_length: usize) {
    let headers = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        declared_length
    );
    stream
        .write_all(headers.as_bytes())
        .expect("response headers write");
    stream.flush().expect("response flushes");
}

fn write_redirect(stream: &mut TcpStream) {
    let response = b"HTTP/1.1 302 Found\r\nlocation: /v1/responses\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
    stream.write_all(response).expect("redirect writes");
    stream.flush().expect("redirect flushes");
}

#[derive(Debug)]
struct Allow;

impl Authorizer for Allow {
    fn authorize(&self, _request: &ToolAuthorization<'_>) -> AuthorizationDecision {
        AuthorizationDecision::Allow
    }
}

fn limits() -> RunLimits {
    RunLimits {
        max_model_steps: 3,
        max_tool_executions: 1,
        model_timeout_ms: 2_000,
        tool_timeout_ms: 1_000,
        shutdown_grace_period_ms: 200,
    }
}

fn start(request: &str) -> RunStart {
    RunStart {
        run_id: RunId::new("run-openai-adapter"),
        request: request.to_owned(),
        limits: limits(),
    }
}

fn backend(endpoint: &str) -> OpenAiBackend {
    let config = OpenAiConfig::for_endpoint("local-test-secret", "fixture-model", endpoint)
        .expect("local endpoint configuration is valid");
    OpenAiBackend::new(config).expect("HTTP client builds")
}

fn event_names(events: &[EventEnvelope]) -> Vec<&'static str> {
    events
        .iter()
        .map(|envelope| match envelope.event {
            DomainEvent::RunStarted { .. } => "run_started",
            DomainEvent::ModelRequestStarted { .. } => "model_started",
            DomainEvent::ModelRequestFinished { .. } => "model_finished",
            DomainEvent::ToolExecutionStarted { .. } => "tool_started",
            DomainEvent::ToolCallResolved { .. } => "tool_resolved",
            DomainEvent::RunFinished { .. } => "run_finished",
        })
        .collect()
}

fn final_response(text: &str) -> Value {
    json!({
        "status": "completed",
        "error": null,
        "output": [{
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": text}]
        }]
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_http_final_response_uses_normal_driver_ordering() {
    let endpoint = ScriptedEndpoint::spawn(vec![ResponseScript::Json {
        status: "200 OK",
        body: final_response("done"),
    }]);
    let mut store = InMemoryEventStore::new();
    let mut model = backend(endpoint.endpoint());
    let mut catalog = ToolCatalog::empty();
    let mut observed_at = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &Allow,
        &mut observed_at,
    )
    .run(start("answer"), CancellationToken::new())
    .await
    .expect("local response completes");

    let request = endpoint.next_request();
    endpoint.finish();
    assert_eq!(view.finish_reason, Some(FinishReason::Completed));
    assert_eq!(view.final_response.as_deref(), Some("done"));
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "run_finished",
        ]
    );
    assert_eq!(request.request_line, "POST /v1/responses HTTP/1.1");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer local-test-secret")
    );
    assert_eq!(request.body["store"], false);
    assert_eq!(request.body["stream"], false);
    assert_eq!(request.body["parallel_tool_calls"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_http_tool_round_trip_rebuilds_committed_context() {
    let endpoint = ScriptedEndpoint::spawn(vec![
        ResponseScript::Json {
            status: "200 OK",
            body: json!({
                "status": "completed",
                "error": null,
                "output": [{
                    "type": "function_call",
                    "call_id": "call-1",
                    "name": "fixture.read",
                    "arguments": "{\"key\":\"alpha\"}",
                    "status": "completed"
                }]
            }),
        },
        ResponseScript::Json {
            status: "200 OK",
            body: final_response("fixture is available"),
        },
    ]);
    let mut store = InMemoryEventStore::new();
    let mut model = backend(endpoint.endpoint());
    let tools: Vec<Box<dyn Tool>> = vec![Box::new(FixtureTool::new(
        "fixture.read",
        FixtureToolBehavior::Success(json!({"value": "fixture"})),
    ))];
    let mut catalog = ToolCatalog::new(tools).expect("fixture catalog is valid");
    let mut observed_at = || "2026-01-01T00:00:00Z".to_owned();

    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &Allow,
        &mut observed_at,
    )
    .run(start("look up alpha"), CancellationToken::new())
    .await
    .expect("tool round trip completes");

    let first = endpoint.next_request();
    let second = endpoint.next_request();
    endpoint.finish();
    assert_eq!(view.finish_reason, Some(FinishReason::Completed));
    assert_eq!(
        event_names(store.acknowledged()),
        [
            "run_started",
            "model_started",
            "model_finished",
            "tool_started",
            "tool_resolved",
            "model_started",
            "model_finished",
            "run_finished",
        ]
    );
    assert_eq!(first.body["tools"][0]["strict"], false);
    assert_eq!(
        second.body["input"],
        json!([
            {
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "look up alpha"
                }]
            },
            {
                "type": "function_call",
                "call_id": "call-1",
                "name": "fixture.read",
                "arguments": "{\"key\":\"alpha\"}"
            },
            {
                "type": "function_call_output",
                "call_id": "call-1",
                "output": "{\"type\":\"success\",\"detail\":{\"value\":{\"value\":\"fixture\"}}}"
            }
        ])
    );
    let second_object = second.body.as_object().expect("request is an object");
    assert!(!second_object.contains_key("previous_response_id"));
    assert!(!second_object.contains_key("conversation"));
    assert!(!second_object.contains_key("reasoning"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_of_a_held_http_request_is_not_a_provider_failure() {
    let (release_sender, release_receiver) = mpsc::channel();
    let endpoint = ScriptedEndpoint::spawn(vec![ResponseScript::Hold(release_receiver)]);
    let endpoint_url = endpoint.endpoint().to_owned();
    let user_cancellation = CancellationToken::new();
    let cancellation_for_run = user_cancellation.clone();
    let mut store = InMemoryEventStore::new();
    let mut model = backend(&endpoint_url);
    let mut catalog = ToolCatalog::empty();
    let mut observed_at = || "2026-01-01T00:00:00Z".to_owned();
    let mut observation = tokio::task::spawn_blocking(move || {
        let request = endpoint.next_request();
        (endpoint, request)
    });

    let (view, endpoint) = {
        let mut driver = RunDriver::new(
            &mut store,
            &mut model,
            &mut catalog,
            &Allow,
            &mut observed_at,
        );
        let run = driver.run(start("hold"), cancellation_for_run);
        tokio::pin!(run);

        let (endpoint, _request) = tokio::select! {
            observed = &mut observation => {
                user_cancellation.cancel();
                observed.expect("request observation task joins")
            }
            result = &mut run => {
                panic!("held request completed before cancellation: {result:?}")
            }
        };
        let view = run.await.expect("driver records cancellation");
        (view, endpoint)
    };
    let events = store.acknowledged().to_vec();
    release_sender.send(()).expect("held server is released");
    endpoint.finish();

    assert_eq!(view.finish_reason, Some(FinishReason::Cancelled));
    assert_eq!(
        event_names(&events),
        [
            "run_started",
            "model_started",
            "model_finished",
            "run_finished",
        ]
    );
    assert!(matches!(
        events[2].event,
        DomainEvent::ModelRequestFinished {
            outcome: aurora_core::ModelOutcome::Cancelled,
            ..
        }
    ));
}

async fn assert_durable_failure(script: ResponseScript, expected: ModelRequestFailure) {
    let endpoint = ScriptedEndpoint::spawn(vec![script]);
    let mut store = InMemoryEventStore::new();
    let mut model = backend(endpoint.endpoint());
    let mut catalog = ToolCatalog::empty();
    let mut observed_at = || "2026-01-01T00:00:00Z".to_owned();
    let view = RunDriver::new(
        &mut store,
        &mut model,
        &mut catalog,
        &Allow,
        &mut observed_at,
    )
    .run(start("fail"), CancellationToken::new())
    .await
    .expect("request failure is durable");

    let _request = endpoint.next_request();
    endpoint.finish();
    assert_eq!(
        view.finish_reason,
        Some(FinishReason::Failed(ModelFailure::Request(expected)))
    );
    assert!(matches!(
        store.acknowledged()[2].event,
        DomainEvent::ModelRequestFinished {
            outcome: aurora_core::ModelOutcome::RequestFailure(actual),
            ..
        } if actual == expected
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_status_and_shape_failures_use_durable_core_categories() {
    assert_durable_failure(
        ResponseScript::RedirectToSelf,
        ModelRequestFailure::RequestRejected,
    )
    .await;
    assert_durable_failure(
        ResponseScript::Json {
            status: "401 Unauthorized",
            body: json!({
                "error": {
                    "code": "invalid_api_key",
                    "message": "provider-private"
                }
            }),
        },
        ModelRequestFailure::Authentication,
    )
    .await;
    assert_durable_failure(
        ResponseScript::Json {
            status: "429 Too Many Requests",
            body: json!({
                "error": {
                    "code": "insufficient_quota",
                    "message": "provider-private"
                }
            }),
        },
        ModelRequestFailure::RequestRejected,
    )
    .await;
    assert_durable_failure(
        ResponseScript::Json {
            status: "503 Service Unavailable",
            body: json!({
                "error": {
                    "code": "server_error",
                    "message": "provider-private"
                }
            }),
        },
        ModelRequestFailure::ServiceUnavailable,
    )
    .await;
    assert_durable_failure(
        ResponseScript::Truncated {
            status: "429 Too Many Requests",
            declared_length: 128,
            body: b"{\"error\":{".to_vec(),
        },
        ModelRequestFailure::RateLimited,
    )
    .await;
    assert_durable_failure(
        ResponseScript::Truncated {
            status: "200 OK",
            declared_length: 128,
            body: b"{\"status\":\"completed\"}".to_vec(),
        },
        ModelRequestFailure::Transport,
    )
    .await;
    assert_durable_failure(
        ResponseScript::Raw {
            status: "200 OK",
            body: b"provider-private invalid body".to_vec(),
        },
        ModelRequestFailure::UnsupportedResponse,
    )
    .await;
}

async fn assert_status_precedes_held_body(status: &'static str, expected: ModelRequestFailure) {
    let (release_sender, release_receiver) = mpsc::channel();
    let endpoint = ScriptedEndpoint::spawn(vec![ResponseScript::HoldBodyAfterHeaders {
        status,
        release: release_receiver,
    }]);

    let invocation =
        tokio::time::timeout(Duration::from_secs(2), invoke_at(endpoint.endpoint())).await;
    let _request = endpoint.next_request();
    release_sender
        .send(())
        .expect("held response body is released");
    endpoint.finish();

    assert_eq!(
        invocation,
        Ok(ModelInvocation::RequestFailure(expected)),
        "status {status} must classify before response-body I/O"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn known_error_statuses_do_not_wait_for_held_response_bodies() {
    assert_status_precedes_held_body("401 Unauthorized", ModelRequestFailure::Authentication).await;
    assert_status_precedes_held_body(
        "503 Service Unavailable",
        ModelRequestFailure::ServiceUnavailable,
    )
    .await;
}

async fn invoke_at(endpoint: &str) -> ModelInvocation {
    let mut model = backend(endpoint);
    model
        .invoke(
            ModelInput {
                context: Vec::new(),
                tools: Vec::new(),
            },
            CancellationToken::new(),
        )
        .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_and_tls_failures_are_transport_failures() {
    let unused = TcpListener::bind("127.0.0.1:0").expect("ephemeral address binds");
    let unused_address = unused.local_addr().expect("address is available");
    drop(unused);
    assert_eq!(
        invoke_at(&format!("http://{unused_address}/v1/responses")).await,
        ModelInvocation::RequestFailure(ModelRequestFailure::Transport)
    );

    let endpoint = ScriptedEndpoint::spawn(vec![ResponseScript::Disconnect]);
    let tls_endpoint = endpoint.endpoint().replacen("http://", "https://", 1);
    let invocation = invoke_at(&tls_endpoint).await;
    endpoint.finish();
    assert_eq!(
        invocation,
        ModelInvocation::RequestFailure(ModelRequestFailure::Transport)
    );
}
