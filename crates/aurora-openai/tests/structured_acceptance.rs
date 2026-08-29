use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::Duration,
};

use aurora_core::ModelRequestFailure;
use aurora_openai::{
    OpenAiBackend, OpenAiConfig, StructuredOutputInvocation, StructuredOutputRequest,
    StructuredOutputValidationError,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const MEBIBYTE: usize = 1024 * 1024;
const MAX_STRUCTURED_REQUEST_BYTES: usize = 4 * MEBIBYTE;

#[derive(Debug)]
struct ObservedRequest {
    request_line: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

enum ResponseScript {
    Json { status: &'static str, body: Value },
    Raw { status: &'static str, body: Vec<u8> },
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
        let (sender, requests) = mpsc::channel();
        let join = thread::spawn(move || {
            let mut scripts = VecDeque::from(scripts);
            while let Some(script) = scripts.pop_front() {
                let (mut stream, _) = listener.accept().expect("scripted request arrives");
                let request = read_request(&mut stream);
                sender.send(request).expect("test observes request");
                match script {
                    ResponseScript::Json { status, body } => {
                        let body = serde_json::to_vec(&body).expect("fixture serializes");
                        write_response(&mut stream, status, &body);
                    }
                    ResponseScript::Raw { status, body } => {
                        write_response(&mut stream, status, &body)
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
            let headers = std::str::from_utf8(&bytes[..header_end]).expect("headers are UTF-8");
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
    let header_text = std::str::from_utf8(&bytes[..header_end]).expect("headers are UTF-8");
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
    let headers = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(headers.as_bytes())
        .expect("response headers write");
    stream.write_all(body).expect("response body writes");
    stream.flush().expect("response flushes");
}

fn backend(endpoint: &str) -> OpenAiBackend {
    let config = OpenAiConfig::for_endpoint("local-test-secret", "fixture-model", endpoint)
        .expect("local endpoint configuration is valid");
    OpenAiBackend::new(config).expect("HTTP client builds")
}

fn request() -> StructuredOutputRequest {
    StructuredOutputRequest::new(
        "fixture.result",
        "Return the fixture result.",
        "Read fixture alpha.",
        json!({
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"],
            "additionalProperties": false
        }),
    )
    .expect("fixture request is valid")
}

fn completed_output(text: &str) -> Value {
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

#[test]
fn structured_request_rejects_blank_prompt_values_and_non_object_schema() {
    for (name, instructions, input, schema, expected) in [
        (
            "",
            "instructions",
            "input",
            json!({}),
            StructuredOutputValidationError::BlankName,
        ),
        (
            "name",
            "",
            "input",
            json!({}),
            StructuredOutputValidationError::BlankInstructions,
        ),
        (
            "name",
            "instructions",
            "",
            json!({}),
            StructuredOutputValidationError::BlankInput,
        ),
        (
            "name",
            "instructions",
            "input",
            json!(true),
            StructuredOutputValidationError::SchemaMustBeObject,
        ),
    ] {
        assert!(matches!(
            StructuredOutputRequest::new(name, instructions, input, schema),
            Err(actual) if actual == expected
        ));
    }
}

#[test]
fn structured_request_debug_redacts_prompt_bearing_values() {
    let request = StructuredOutputRequest::new(
        "private schema name",
        "private instructions",
        "private input",
        json!({"private_schema": true}),
    )
    .expect("request is valid");
    let debug = format!("{request:?}");
    for private in [
        "private schema name",
        "private instructions",
        "private input",
        "private_schema",
    ] {
        assert!(!debug.contains(private));
    }
}

#[test]
fn structured_request_rejects_obviously_oversized_private_components() {
    let oversized = "x".repeat(MAX_STRUCTURED_REQUEST_BYTES + 1);
    for result in [
        StructuredOutputRequest::new(oversized.clone(), "instructions", "input", json!({})),
        StructuredOutputRequest::new("name", oversized.clone(), "input", json!({})),
        StructuredOutputRequest::new("name", "instructions", oversized.clone(), json!({})),
        StructuredOutputRequest::new(
            "name",
            "instructions",
            "input",
            json!({"private": oversized}),
        ),
    ] {
        let error = result.expect_err("an obviously oversized component is rejected");
        assert_eq!(error, StructuredOutputValidationError::RequestTooLarge);
        assert_eq!(format!("{error:?}"), "RequestTooLarge");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invoke_structured_sends_a_fresh_strict_json_schema_request() {
    let endpoint = ScriptedEndpoint::spawn(vec![ResponseScript::Json {
        status: "200 OK",
        body: completed_output(r#"{"value":"fixture"}"#),
    }]);
    let mut backend = backend(endpoint.endpoint());

    let invocation = backend
        .invoke_structured(request(), CancellationToken::new())
        .await;

    let observed = endpoint.next_request();
    endpoint.finish();
    assert_eq!(
        invocation,
        StructuredOutputInvocation::Output(json!({"value": "fixture"}))
    );
    assert_eq!(observed.request_line, "POST /v1/responses HTTP/1.1");
    assert_eq!(
        observed.headers.get("authorization").map(String::as_str),
        Some("Bearer local-test-secret")
    );
    assert_eq!(
        observed.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert_eq!(
        observed.body,
        json!({
            "model": "fixture-model",
            "store": false,
            "stream": false,
            "instructions": "Return the fixture result.",
            "input": "Read fixture alpha.",
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "fixture.result",
                    "strict": true,
                    "schema": {
                        "type": "object",
                        "properties": {"value": {"type": "string"}},
                        "required": ["value"],
                        "additionalProperties": false
                    }
                }
            }
        })
    );
    let object = observed.body.as_object().expect("request is an object");
    for forbidden in [
        "previous_response_id",
        "conversation",
        "tools",
        "parallel_tool_calls",
        "reasoning",
        "include",
        "prompt_cache_key",
    ] {
        assert!(
            !object.contains_key(forbidden),
            "request must omit {forbidden}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invoke_structured_accepts_reasoning_before_the_final_json_object() {
    let endpoint = ScriptedEndpoint::spawn(vec![ResponseScript::Json {
        status: "200 OK",
        body: json!({
            "status": "completed",
            "output": [
                {"type": "reasoning", "summary": []},
                {
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": "{\"answer\":true}"}]
                }
            ]
        }),
    }]);
    let mut backend = backend(endpoint.endpoint());

    let invocation = backend
        .invoke_structured(request(), CancellationToken::new())
        .await;

    let _request = endpoint.next_request();
    endpoint.finish();
    assert_eq!(
        invocation,
        StructuredOutputInvocation::Output(json!({"answer": true}))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invoke_structured_categorizes_provider_and_invalid_output_failures() {
    let endpoint = ScriptedEndpoint::spawn(vec![
        ResponseScript::Json {
            status: "401 Unauthorized",
            body: json!({"error": {"code": "invalid_api_key"}}),
        },
        ResponseScript::Json {
            status: "200 OK",
            body: completed_output("not JSON"),
        },
        ResponseScript::Json {
            status: "200 OK",
            body: completed_output("[]"),
        },
        ResponseScript::Json {
            status: "200 OK",
            body: json!({
                "status": "completed",
                "output": [{
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "refusal", "refusal": "cannot comply"}]
                }]
            }),
        },
        ResponseScript::Json {
            status: "200 OK",
            body: json!({"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}}),
        },
        ResponseScript::Json {
            status: "200 OK",
            body: json!({
                "status": "completed",
                "output": [
                    {
                        "type": "message",
                        "role": "assistant",
                        "status": "completed",
                        "content": [{"type": "output_text", "text": "{\"answer\":true}"}]
                    },
                    {"type": "function_call", "call_id": "call-1", "name": "private.tool", "arguments": "{}", "status": "completed"}
                ]
            }),
        },
    ]);
    let mut backend = backend(endpoint.endpoint());

    let expected = [
        StructuredOutputInvocation::RequestFailure(ModelRequestFailure::Authentication),
        StructuredOutputInvocation::MalformedOutput,
        StructuredOutputInvocation::MalformedOutput,
        StructuredOutputInvocation::MalformedOutput,
        StructuredOutputInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse),
        StructuredOutputInvocation::MalformedOutput,
    ];
    for expected in expected {
        assert_eq!(
            backend
                .invoke_structured(request(), CancellationToken::new())
                .await,
            expected
        );
        let _request = endpoint.next_request();
    }
    endpoint.finish();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invoke_structured_preserves_categories_for_success_status_error_envelopes() {
    let cases = [
        ("invalid_api_key", ModelRequestFailure::Authentication),
        ("authentication_error", ModelRequestFailure::Authentication),
        ("rate_limit_exceeded", ModelRequestFailure::RateLimited),
        ("server_error", ModelRequestFailure::ServiceUnavailable),
        (
            "invalid_request_error",
            ModelRequestFailure::RequestRejected,
        ),
        ("insufficient_quota", ModelRequestFailure::RequestRejected),
        ("billing_not_active", ModelRequestFailure::RequestRejected),
    ];
    let private_message = "provider-private-message-must-not-escape";
    let endpoint = ScriptedEndpoint::spawn(
        cases
            .iter()
            .map(|(code, _)| ResponseScript::Json {
                status: "200 OK",
                body: json!({
                    "status": "failed",
                    "error": {"code": code, "message": private_message},
                    "output": []
                }),
            })
            .collect(),
    );
    let mut backend = backend(endpoint.endpoint());

    for (_, category) in cases {
        let invocation = backend
            .invoke_structured(request(), CancellationToken::new())
            .await;
        assert_eq!(
            invocation,
            StructuredOutputInvocation::RequestFailure(category)
        );
        assert!(!format!("{invocation:?}").contains(private_message));
        let _request = endpoint.next_request();
    }
    endpoint.finish();
}

#[tokio::test]
async fn invoke_structured_rejects_final_escaped_wire_size_before_http() {
    let config =
        OpenAiConfig::for_endpoint("key", "fixture-model", "http://127.0.0.1:9/v1/responses")
            .expect("test endpoint is syntactically valid");
    let mut backend = OpenAiBackend::new(config).expect("client builds");
    let mut escaping_input = String::with_capacity(MAX_STRUCTURED_REQUEST_BYTES / 2);
    escaping_input.push('x');
    escaping_input.extend(std::iter::repeat_n(
        '\n',
        MAX_STRUCTURED_REQUEST_BYTES / 2 - 1,
    ));
    let request = StructuredOutputRequest::new(
        "fixture.result",
        "Return the fixture result.",
        escaping_input,
        json!({"type": "object"}),
    )
    .expect("raw components alone remain below the exact wire limit");

    let invocation = backend
        .invoke_structured(request, CancellationToken::new())
        .await;

    assert_eq!(invocation, StructuredOutputInvocation::RequestTooLarge);
    assert_eq!(format!("{invocation:?}"), "RequestTooLarge");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invoke_structured_enforces_the_fixed_one_mebibyte_response_limit() {
    let endpoint = ScriptedEndpoint::spawn(vec![
        ResponseScript::Raw {
            status: "200 OK",
            body: vec![b' '; MEBIBYTE],
        },
        ResponseScript::Raw {
            status: "200 OK",
            body: vec![b' '; MEBIBYTE + 1],
        },
    ]);
    let mut backend = backend(endpoint.endpoint());

    assert_eq!(
        backend
            .invoke_structured(request(), CancellationToken::new())
            .await,
        StructuredOutputInvocation::MalformedOutput
    );
    let _request = endpoint.next_request();
    assert_eq!(
        backend
            .invoke_structured(request(), CancellationToken::new())
            .await,
        StructuredOutputInvocation::ResponseTooLarge
    );
    let _request = endpoint.next_request();
    endpoint.finish();
}

#[tokio::test]
async fn invoke_structured_returns_cancelled_without_connecting_when_already_cancelled() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test listener binds");
    listener
        .set_nonblocking(true)
        .expect("listener becomes nonblocking");
    let config = OpenAiConfig::for_endpoint(
        "key",
        "fixture-model",
        format!(
            "http://{}/v1/responses",
            listener.local_addr().expect("listener has address")
        ),
    )
    .expect("test endpoint is syntactically valid");
    let mut backend = OpenAiBackend::new(config).expect("client builds");
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    assert_eq!(
        backend.invoke_structured(request(), cancellation).await,
        StructuredOutputInvocation::Cancelled
    );
    tokio::time::sleep(Duration::from_millis(10)).await;
    let error = listener
        .accept()
        .expect_err("an already-cancelled structured invocation cannot connect");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invoke_structured_cancellation_during_a_request_is_not_a_provider_failure() {
    let (release_sender, release_receiver) = mpsc::channel();
    let endpoint = ScriptedEndpoint::spawn(vec![ResponseScript::Hold(release_receiver)]);
    let cancellation = CancellationToken::new();
    let mut backend = backend(endpoint.endpoint());
    let operation = backend.invoke_structured(request(), cancellation.clone());
    tokio::pin!(operation);

    let _request = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match endpoint.requests.try_recv() {
                Ok(request) => return request,
                Err(mpsc::TryRecvError::Disconnected) => {
                    panic!("structured fixture stopped before observing the request")
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            tokio::select! {
                result = &mut operation => {
                    panic!("held structured request completed before cancellation: {result:?}")
                }
                () = tokio::time::sleep(Duration::from_millis(1)) => {}
            }
        }
    })
    .await
    .expect("structured request reaches the owned server fixture");
    cancellation.cancel();
    let completion = tokio::time::timeout(Duration::from_millis(200), &mut operation).await;
    let release = release_sender.send(());
    endpoint.finish();
    release.expect("held server is released");
    assert_eq!(
        completion.expect("structured cancellation completes promptly"),
        StructuredOutputInvocation::Cancelled
    );
}

#[tokio::test]
async fn invoke_structured_does_not_connect_before_its_future_is_polled() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test listener binds");
    listener
        .set_nonblocking(true)
        .expect("listener becomes nonblocking");
    let config = OpenAiConfig::for_endpoint(
        "key",
        "fixture-model",
        format!(
            "http://{}/v1/responses",
            listener.local_addr().expect("listener has address")
        ),
    )
    .expect("test endpoint is syntactically valid");
    let mut backend = OpenAiBackend::new(config).expect("client builds");

    let future = backend.invoke_structured(request(), CancellationToken::new());
    tokio::time::sleep(Duration::from_millis(10)).await;
    let error = listener
        .accept()
        .expect_err("an unpolled structured invocation cannot connect");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    drop(future);
}
