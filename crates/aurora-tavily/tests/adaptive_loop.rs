use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use aurora_research::{
    Claim, ClaimId, ContentDigest, Evidence, EvidenceAssessment, EvidenceId, EvidenceRelation,
    EvidenceSufficiency, IdentifiedResearchGap, InvestigationEvent, InvestigationRecord,
    InvestigationResult, InvestigationTask, InvestigationTaskId, MediaType, ResearchControlEvent,
    ResearchControlLimits, ResearchControlRecord, ResearchControlState, ResearchControlStatus,
    ResearchControlTransitionError, ResearchEvent, ResearchGap, ResearchGapCause, ResearchGapId,
    ResearchPlan, ResearchRecord, ResearchRequest, RetrievedAt, Source, SourceId,
    VerificationAssessment, VerificationId, VerificationRecord,
};
use aurora_tavily::{TavilyConfig, TavilyFailure, TavilyInvestigator};
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retrieved_follow_up_resolves_its_verification_gap_then_completes_explicitly() {
    let initial_task =
        initial_task("Establish that the 2024 total solar eclipse crossed North America");
    let (initial_result, claim_id, initial_evidence_id) = initial_result();
    let gap_id = ResearchGapId::generate();
    let verification_id = VerificationId::generate();
    let follow_up = InvestigationTask::follow_up(
        InvestigationTaskId::generate(),
        *initial_task.id(),
        "Find independent confirmation that the 2024 total solar eclipse crossed North America"
            .to_owned(),
        ResearchGap::new(
            "Independent confirmation that the 2024 total solar eclipse crossed North America is needed"
                .to_owned(),
        )
        .expect("gap is valid"),
    )
    .expect("follow-up task is valid");
    let mut state = ResearchControlState::default();

    apply(
        &mut state,
        1,
        ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(1)),
    );
    apply(
        &mut state,
        2,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            1,
            InvestigationEvent::RequestRecorded(
                ResearchRequest::new(
                    "Did the 2024 total solar eclipse cross North America?".to_owned(),
                )
                .expect("request is valid"),
            ),
        )),
    );
    apply(
        &mut state,
        3,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            2,
            InvestigationEvent::PlanRecorded(
                ResearchPlan::new(vec![initial_task.clone()]).expect("plan is valid"),
            ),
        )),
    );
    apply(
        &mut state,
        4,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            3,
            InvestigationEvent::TaskStarted {
                task_id: *initial_task.id(),
            },
        )),
    );
    apply(
        &mut state,
        5,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            4,
            InvestigationEvent::TaskCompleted {
                task_id: *initial_task.id(),
                result: initial_result,
            },
        )),
    );
    apply(
        &mut state,
        6,
        ResearchControlEvent::VerificationRecorded(verification_record(
            1,
            verification_id,
            claim_id,
            vec![EvidenceAssessment::new(
                initial_evidence_id,
                EvidenceRelation::Supports,
            )],
            EvidenceSufficiency::Insufficient,
        )),
    );
    apply(
        &mut state,
        7,
        ResearchControlEvent::GapIdentified(IdentifiedResearchGap::new(
            gap_id,
            ResearchGapCause::Verification(verification_id),
            ResearchGap::new(
                "Independent confirmation that the 2024 total solar eclipse crossed North America is needed"
                    .to_owned(),
            )
            .expect("gap is valid"),
        )),
    );
    apply(
        &mut state,
        8,
        ResearchControlEvent::GapFollowUpRecorded {
            gap_id,
            investigation_record: investigation_record(
                5,
                InvestigationEvent::FollowUpRecorded(follow_up.clone()),
            ),
        },
    );
    apply(
        &mut state,
        9,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            6,
            InvestigationEvent::TaskStarted {
                task_id: *follow_up.id(),
            },
        )),
    );

    let response = json!({
        "results": [{
            "title": "Independent observatory fixture",
            "url": "https://independent.example/support",
            "raw_content": "An independent observatory reports that the 2024 total solar eclipse crossed North America.",
        }]
    });
    let (endpoint, requests, worker) = loopback_response(
        "200 OK",
        serde_json::to_vec(&response).expect("fixture response serializes"),
    );
    let result = investigator(&endpoint)
        .investigate(&follow_up, 4, retrieved_at())
        .await
        .expect("fixture response admits");
    let request = requests
        .recv_timeout(Duration::from_secs(5))
        .expect("fixture receives the search request");
    worker.join().expect("fixture worker joins");
    assert_eq!(request["query"], follow_up.objective());

    let acquired_evidence_id = verified_acquired_evidence_id(
        &result,
        "An independent observatory reports that the 2024 total solar eclipse crossed North America.",
    );
    apply(
        &mut state,
        10,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            7,
            InvestigationEvent::TaskCompleted {
                task_id: *follow_up.id(),
                result,
            },
        )),
    );
    let resolving_verification_id = VerificationId::generate();
    apply(
        &mut state,
        11,
        ResearchControlEvent::VerificationRecorded(verification_record(
            2,
            resolving_verification_id,
            claim_id,
            vec![
                EvidenceAssessment::new(initial_evidence_id, EvidenceRelation::Supports),
                EvidenceAssessment::new(acquired_evidence_id, EvidenceRelation::Supports),
            ],
            EvidenceSufficiency::Sufficient,
        )),
    );
    apply(
        &mut state,
        12,
        ResearchControlEvent::GapResolved {
            gap_id,
            verification_id: resolving_verification_id,
        },
    );
    apply(&mut state, 13, ResearchControlEvent::ResearchCompleted);

    assert_eq!(state.status(), ResearchControlStatus::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_failure_records_task_failed_and_rejects_research_completion() {
    let successful_task =
        initial_task("Establish that the 2024 total solar eclipse crossed North America");
    let failed_task = initial_task("Find an independent eclipse record");
    let (initial_result, claim_id, initial_evidence_id) = initial_result();
    let mut state = ResearchControlState::default();

    apply(
        &mut state,
        1,
        ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(0)),
    );
    apply(
        &mut state,
        2,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            1,
            InvestigationEvent::RequestRecorded(
                ResearchRequest::new(
                    "Did the 2024 total solar eclipse cross North America?".to_owned(),
                )
                .expect("request is valid"),
            ),
        )),
    );
    apply(
        &mut state,
        3,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            2,
            InvestigationEvent::PlanRecorded(
                ResearchPlan::new(vec![successful_task.clone(), failed_task.clone()])
                    .expect("plan is valid"),
            ),
        )),
    );
    apply(
        &mut state,
        4,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            3,
            InvestigationEvent::TaskStarted {
                task_id: *successful_task.id(),
            },
        )),
    );
    apply(
        &mut state,
        5,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            4,
            InvestigationEvent::TaskCompleted {
                task_id: *successful_task.id(),
                result: initial_result,
            },
        )),
    );
    apply(
        &mut state,
        6,
        ResearchControlEvent::VerificationRecorded(verification_record(
            1,
            VerificationId::generate(),
            claim_id,
            vec![EvidenceAssessment::new(
                initial_evidence_id,
                EvidenceRelation::Supports,
            )],
            EvidenceSufficiency::Sufficient,
        )),
    );
    apply(
        &mut state,
        7,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            5,
            InvestigationEvent::TaskStarted {
                task_id: *failed_task.id(),
            },
        )),
    );

    let (endpoint, requests, worker) = loopback_response("503 Service Unavailable", Vec::new());
    let failure = investigator(&endpoint)
        .investigate(&failed_task, 4, retrieved_at())
        .await
        .expect_err("provider failure remains distinct from a result");
    let request = requests
        .recv_timeout(Duration::from_secs(5))
        .expect("fixture receives the search request");
    worker.join().expect("fixture worker joins");
    assert_eq!(request["query"], failed_task.objective());
    assert_eq!(failure, TavilyFailure::Unavailable);

    apply(
        &mut state,
        8,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            6,
            InvestigationEvent::TaskFailed {
                task_id: *failed_task.id(),
                failure: failure.into_investigation_failure(),
            },
        )),
    );

    assert_eq!(
        state.apply(control_record(9, ResearchControlEvent::ResearchCompleted)),
        Err(
            ResearchControlTransitionError::InvestigationFailurePreventsCompletion(
                *failed_task.id()
            )
        )
    );
}

fn loopback_response(
    status: &'static str,
    body: Vec<u8>,
) -> (String, mpsc::Receiver<Value>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener binds");
    listener
        .set_nonblocking(true)
        .expect("fixture listener becomes nonblocking");
    let address = listener
        .local_addr()
        .expect("fixture listener address is available");
    let (requests_sender, requests) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut stream = accept_fixture_connection(&listener);
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("fixture configures a read deadline");
        requests_sender
            .send(read_request(&mut stream))
            .expect("test receives the request");
        let headers = format!(
            "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        stream
            .write_all(headers.as_bytes())
            .and_then(|_| stream.write_all(&body))
            .and_then(|_| stream.flush())
            .expect("fixture writes the response");
    });
    (format!("http://{address}/search"), requests, worker)
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
            Err(error) => panic!("fixture accepts the request: {error}"),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Value {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let (headers_end, content_length) = loop {
        let count = stream
            .read(&mut buffer)
            .expect("fixture reads request bytes");
        assert_ne!(count, 0, "request completes before the connection closes");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(headers_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers =
                std::str::from_utf8(&bytes[..headers_end]).expect("request headers are UTF-8");
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
                .expect("client sends content length");
            if bytes.len() >= headers_end + 4 + content_length {
                break (headers_end, content_length);
            }
        }
    };
    serde_json::from_slice(&bytes[headers_end + 4..headers_end + 4 + content_length])
        .expect("request body is JSON")
}

fn initial_task(objective: &str) -> InvestigationTask {
    InvestigationTask::initial(InvestigationTaskId::generate(), objective.to_owned())
        .expect("initial task is valid")
}

fn initial_result() -> (InvestigationResult, ClaimId, EvidenceId) {
    let source_id = SourceId::generate();
    let evidence_id = EvidenceId::generate();
    let claim_id = ClaimId::generate();
    let source = Source::new(
        source_id,
        ContentDigest::sha256([7; 32]),
        "https://nasa.example/eclipse".to_owned(),
        Some("NASA fixture".to_owned()),
        retrieved_at(),
        MediaType::new("text/plain").expect("media type is valid"),
    )
    .expect("source is valid");
    let evidence = Evidence::new(
        evidence_id,
        source_id,
        "NASA reports that the 2024 total solar eclipse crossed North America.".to_owned(),
    )
    .expect("evidence is valid");
    let claim = Claim::new(
        claim_id,
        "The 2024 total solar eclipse crossed North America.".to_owned(),
        vec![evidence_id],
    )
    .expect("claim is valid");

    (
        InvestigationResult::new(vec![
            research_record(1, ResearchEvent::SourceRecorded(source)),
            research_record(2, ResearchEvent::EvidenceRecorded(evidence)),
            research_record(3, ResearchEvent::ClaimProposed(claim)),
        ]),
        claim_id,
        evidence_id,
    )
}

fn verified_acquired_evidence_id(
    result: &InvestigationResult,
    expected_excerpt: &str,
) -> EvidenceId {
    let ResearchEvent::SourceRecorded(source) = result.research_records()[0].event() else {
        panic!("Tavily result records its source first");
    };
    let ResearchEvent::EvidenceRecorded(evidence) = result.research_records()[1].event() else {
        panic!("Tavily result records evidence after its source");
    };
    assert_eq!(evidence.excerpt(), expected_excerpt);
    assert_eq!(evidence.source_id(), source.id());
    *evidence.id()
}

fn investigator(endpoint: &str) -> TavilyInvestigator {
    TavilyInvestigator::new(
        TavilyConfig::for_endpoint("fixture-key", endpoint, Duration::from_secs(1))
            .expect("fixture configuration is valid"),
    )
    .expect("fixture client builds")
}

fn retrieved_at() -> RetrievedAt {
    RetrievedAt::new("2026-08-29T12:34:56Z").expect("fixture timestamp is valid")
}

fn verification_record(
    sequence: u64,
    verification_id: VerificationId,
    claim_id: ClaimId,
    evidence: Vec<EvidenceAssessment>,
    sufficiency: EvidenceSufficiency,
) -> VerificationRecord {
    VerificationRecord::new(
        sequence,
        VerificationAssessment::new(verification_id, claim_id, evidence, sufficiency)
            .expect("verification is valid"),
    )
    .expect("verification record is valid")
}

fn apply(state: &mut ResearchControlState, sequence: u64, event: ResearchControlEvent) {
    state
        .apply(control_record(sequence, event))
        .expect("control record applies");
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
