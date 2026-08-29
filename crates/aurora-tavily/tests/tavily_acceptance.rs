use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use aurora_research::{InvestigationTask, InvestigationTaskId, ResearchEvent, RetrievedAt, Source};
use aurora_tavily::{TavilyConfig, TavilyFailure, TavilyInvestigator};
use serde_json::{Value, json};

#[derive(Debug)]
struct CapturedRequest {
    line: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

enum Reply {
    Json {
        status: &'static str,
        body: Value,
    },
    Bytes {
        status: &'static str,
        body: Vec<u8>,
    },
    Oversized {
        status: &'static str,
    },
    HoldBodyAfterHeaders {
        status: &'static str,
        declared_length: usize,
        headers_sent: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    },
    Redirect,
    Disconnect,
}

struct ProbeServer {
    endpoint: String,
    requests: mpsc::Receiver<CapturedRequest>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ProbeServer {
    fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener binds");
        listener
            .set_nonblocking(true)
            .expect("fixture listener becomes nonblocking");
        let address = listener
            .local_addr()
            .expect("fixture listener address is available");
        let (sender, requests) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut replies = VecDeque::from(replies);
            while let Some(reply) = replies.pop_front() {
                let mut stream = accept_fixture_connection(&listener);
                if matches!(&reply, Reply::Disconnect) {
                    continue;
                }
                let request = read_request(&mut stream);
                sender
                    .send(request)
                    .expect("test still receives observations");
                match reply {
                    Reply::Json { status, body } => {
                        let body = serde_json::to_vec(&body).expect("fixture JSON serializes");
                        write_response(&mut stream, status, &body);
                    }
                    Reply::Bytes { status, body } => write_response(&mut stream, status, &body),
                    Reply::Oversized { status } => write_oversized_response(&mut stream, status),
                    Reply::HoldBodyAfterHeaders {
                        status,
                        declared_length,
                        headers_sent,
                        release,
                    } => {
                        write_response_headers(&mut stream, status, declared_length);
                        headers_sent
                            .send(())
                            .expect("test observes fixture response headers");
                        release
                            .recv_timeout(Duration::from_secs(5))
                            .expect("test releases held fixture response");
                    }
                    Reply::Redirect => write_redirect(&mut stream),
                    Reply::Disconnect => unreachable!("disconnects skip request reading"),
                }
            }
        });
        Self {
            endpoint: format!("http://{address}/search"),
            requests,
            worker: Some(worker),
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn request(&self) -> CapturedRequest {
        self.requests
            .recv_timeout(Duration::from_secs(5))
            .expect("fixture observed request")
    }

    fn finish(mut self) {
        self.worker
            .take()
            .expect("fixture owns its worker")
            .join()
            .expect("fixture worker joins");
    }
}

fn accept_fixture_connection(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .expect("fixture stream becomes blocking");
                return stream;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "fixture connection did not arrive before its deadline"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("fixture accepts a request: {error}"),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("fixture configures read deadline");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let (headers_end, content_length) = loop {
        let count = stream
            .read(&mut buffer)
            .expect("fixture reads request bytes");
        assert_ne!(count, 0, "request completes before the connection closes");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(headers_end) = header_end(&bytes) {
            let header_text =
                std::str::from_utf8(&bytes[..headers_end]).expect("request headers are UTF-8");
            let content_length = header_text
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
                .expect("client sends content length");
            if bytes.len() >= headers_end + 4 + content_length {
                break (headers_end, content_length);
            }
        }
    };
    let header_text =
        std::str::from_utf8(&bytes[..headers_end]).expect("request headers are UTF-8");
    let mut lines = header_text.lines();
    let line = lines.next().expect("request line is present").to_owned();
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.to_ascii_lowercase(), value.trim().to_owned()))
        })
        .collect();
    let body_start = headers_end + 4;
    let body = serde_json::from_slice(&bytes[body_start..body_start + content_length])
        .expect("request body is JSON");
    CapturedRequest {
        line,
        headers,
        body,
    }
}

fn header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn write_response(stream: &mut TcpStream, status: &str, body: &[u8]) {
    write_response_headers(stream, status, body.len());
    stream.write_all(body).expect("fixture writes body");
    stream.flush().expect("fixture flushes response");
}

fn write_response_headers(stream: &mut TcpStream, status: &str, declared_length: usize) {
    let headers = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        declared_length
    );
    stream
        .write_all(headers.as_bytes())
        .expect("fixture writes headers");
    stream.flush().expect("fixture flushes headers");
}

fn write_oversized_response(stream: &mut TcpStream, status: &str) {
    const RESPONSE_LIMIT: usize = 8 * 1024 * 1024;
    const CHUNK_LENGTH: usize = 64 * 1024;

    let response_length = RESPONSE_LIMIT + CHUNK_LENGTH;
    write_response_headers(stream, status, response_length);

    let chunk = [b'x'; CHUNK_LENGTH];
    let mut sent = 0;
    while sent < response_length {
        let remaining = response_length - sent;
        let write_length = remaining.min(chunk.len());
        match stream.write(&chunk[..write_length]) {
            Ok(0) => return,
            Ok(count) => sent += count,
            Err(error)
                if sent > RESPONSE_LIMIT
                    && matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    ) =>
            {
                return;
            }
            Err(_) => return,
        }
    }

    let _ = stream.flush();
}

fn write_redirect(stream: &mut TcpStream) {
    stream
        .write_all(
            b"HTTP/1.1 302 Found\r\nlocation: /search\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        )
        .expect("fixture writes redirect");
    stream.flush().expect("fixture flushes redirect");
}

fn investigator(endpoint: &str, timeout: Duration) -> TavilyInvestigator {
    let config = TavilyConfig::for_endpoint("fixture-key", endpoint, timeout)
        .expect("fixture configuration is valid");
    TavilyInvestigator::new(config).expect("fixture client builds")
}

fn task(objective: &str) -> InvestigationTask {
    InvestigationTask::initial(InvestigationTaskId::generate(), objective.to_owned())
        .expect("fixture task is valid")
}

fn retrieved_at() -> RetrievedAt {
    RetrievedAt::new("2026-08-29T12:34:56Z").expect("fixture retrieval time is valid")
}

fn response_with(content: &str) -> Value {
    json!({
        "results": [{
            "title": "Fixture source",
            "url": "https://source.example/article",
            "raw_content": content,
        }]
    })
}

fn only_source(result: &aurora_research::InvestigationResult) -> &Source {
    let ResearchEvent::SourceRecorded(source) = result.research_records()[0].event() else {
        panic!("first record is a source");
    };
    source
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sends_the_fixed_search_request_and_admits_a_successful_response() {
    let server = ProbeServer::start(vec![Reply::Json {
        status: "200 OK",
        body: response_with("exact source text"),
    }]);
    let result = investigator(server.endpoint(), Duration::from_secs(1))
        .investigate(&task("objective kept verbatim"), 17, retrieved_at())
        .await
        .expect("successful response admits");
    let request = server.request();
    server.finish();

    assert_eq!(request.line, "POST /search HTTP/1.1");
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer fixture-key")
    );
    assert_eq!(
        request.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert_eq!(request.body["query"], "objective kept verbatim");
    assert_eq!(request.body["search_depth"], "basic");
    assert_eq!(request.body["max_results"], 3);
    assert_eq!(request.body["include_answer"], false);
    assert_eq!(request.body["include_raw_content"], "text");
    assert_eq!(request.body["auto_parameters"], false);
    assert_eq!(result.research_records().len(), 2);
    assert_eq!(result.research_records()[0].sequence(), 17);
    assert_eq!(result.research_records()[1].sequence(), 18);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn classifies_http_failures_without_decoding_their_bodies() {
    let cases = [
        ("401 Unauthorized", TavilyFailure::Authentication),
        ("403 Forbidden", TavilyFailure::Authentication),
        ("429 Too Many Requests", TavilyFailure::RateLimited),
        ("503 Service Unavailable", TavilyFailure::Unavailable),
        ("418 Teapot", TavilyFailure::UnexpectedStatus(418)),
    ];

    for (status, expected) in cases {
        let server = ProbeServer::start(vec![Reply::Bytes {
            status,
            body: b"provider body must stay unread".to_vec(),
        }]);
        let actual = investigator(server.endpoint(), Duration::from_secs(1))
            .investigate(&task("status classification"), 1, retrieved_at())
            .await;
        let _request = server.request();
        server.finish();

        assert_eq!(actual, Err(expected));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn classifies_a_non_success_status_before_its_provider_body_arrives() {
    let (headers_sender, headers_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let server = ProbeServer::start(vec![Reply::HoldBodyAfterHeaders {
        status: "400 Bad Request",
        declared_length: 128,
        headers_sent: headers_sender,
        release: release_receiver,
    }]);
    let result = investigator(server.endpoint(), Duration::from_secs(1))
        .investigate(&task("held provider body"), 1, retrieved_at())
        .await;
    let _request = server.request();
    headers_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("fixture sends rejected-response headers");
    release_sender
        .send(())
        .expect("fixture provider body is released");
    server.finish();

    assert_eq!(result, Err(TavilyFailure::Rejected));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_malformed_or_oversized_success_bodies() {
    let malformed = ProbeServer::start(vec![Reply::Bytes {
        status: "200 OK",
        body: b"not JSON".to_vec(),
    }]);
    let malformed_result = investigator(malformed.endpoint(), Duration::from_secs(1))
        .investigate(&task("malformed response"), 1, retrieved_at())
        .await;
    let _request = malformed.request();
    malformed.finish();
    assert_eq!(malformed_result, Err(TavilyFailure::MalformedResponse));

    let oversized = ProbeServer::start(vec![Reply::Oversized { status: "200 OK" }]);
    let oversized_result = investigator(oversized.endpoint(), Duration::from_secs(5))
        .investigate(&task("oversized response"), 1, retrieved_at())
        .await;
    let _request = oversized.request();
    oversized.finish();
    assert_eq!(oversized_result, Err(TavilyFailure::ResponseTooLarge));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn classifies_a_held_body_after_success_headers_as_timeout() {
    let (headers_sender, headers_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let server = ProbeServer::start(vec![Reply::HoldBodyAfterHeaders {
        status: "200 OK",
        declared_length: 128,
        headers_sent: headers_sender,
        release: release_receiver,
    }]);
    let investigator = investigator(server.endpoint(), Duration::from_secs(1));
    let held_task = task("held response body");
    let investigation = investigator.investigate(&held_task, 1, retrieved_at());
    tokio::pin!(investigation);
    let headers_deadline = tokio::time::sleep(Duration::from_millis(500));
    tokio::pin!(headers_deadline);

    loop {
        match headers_receiver.try_recv() {
            Ok(()) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                panic!("fixture stopped before sending successful-response headers");
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        tokio::select! {
            _ = &mut headers_deadline => {
                panic!("fixture did not send headers before the total request deadline");
            }
            result = &mut investigation => {
                panic!("investigation completed before fixture headers arrived: {result:?}");
            }
            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
    }

    let result = investigation.await;
    let _request = server.request();
    release_sender
        .send(())
        .expect("fixture response is released");
    server.finish();

    assert_eq!(result, Err(TavilyFailure::Timeout));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn does_not_follow_redirects() {
    let server = ProbeServer::start(vec![Reply::Redirect]);
    let result = investigator(server.endpoint(), Duration::from_secs(1))
        .investigate(&task("redirect response"), 1, retrieved_at())
        .await;
    let _request = server.request();
    server.finish();

    assert_eq!(result, Err(TavilyFailure::UnexpectedStatus(302)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_content_at_one_locator_creates_new_snapshot_identities_and_digests() {
    let server = ProbeServer::start(vec![
        Reply::Json {
            status: "200 OK",
            body: response_with("first acquired bytes"),
        },
        Reply::Json {
            status: "200 OK",
            body: response_with("second acquired bytes"),
        },
    ]);
    let investigator = investigator(server.endpoint(), Duration::from_secs(1));
    let first = investigator
        .investigate(&task("first acquisition"), 1, retrieved_at())
        .await
        .expect("first response admits");
    let second = investigator
        .investigate(&task("second acquisition"), 3, retrieved_at())
        .await
        .expect("second response admits");
    let _first_request = server.request();
    let _second_request = server.request();
    server.finish();

    let first_source = only_source(&first);
    let second_source = only_source(&second);
    assert_eq!(first_source.locator(), second_source.locator());
    assert_ne!(first_source.id(), second_source.id());
    assert_ne!(
        first_source.content_digest(),
        second_source.content_digest()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn distinguishes_empty_success_from_transport_failure() {
    let empty = ProbeServer::start(vec![Reply::Json {
        status: "200 OK",
        body: json!({"results": []}),
    }]);
    let empty_result = investigator(empty.endpoint(), Duration::from_secs(1))
        .investigate(&task("empty response"), 1, retrieved_at())
        .await;
    let _request = empty.request();
    empty.finish();
    assert!(
        empty_result
            .expect("valid empty response succeeds")
            .research_records()
            .is_empty()
    );

    let disconnected = ProbeServer::start(vec![Reply::Disconnect]);
    let transport_result = investigator(disconnected.endpoint(), Duration::from_secs(1))
        .investigate(&task("transport failure"), 1, retrieved_at())
        .await;
    disconnected.finish();
    assert_eq!(transport_result, Err(TavilyFailure::Transport));
}
