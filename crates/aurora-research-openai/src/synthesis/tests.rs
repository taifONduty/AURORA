use std::{
    collections::VecDeque,
    future::{Future, ready},
    pin::Pin,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use aurora_openai::StructuredOutputInvocation;
use aurora_research::{
    Claim, ClaimId, ContentDigest, Evidence, EvidenceAssessment, EvidenceId, EvidenceRelation,
    EvidenceSufficiency, InvestigationEvent, InvestigationRecord, InvestigationResult,
    InvestigationTask, InvestigationTaskId, MediaType, ResearchControlEvent, ResearchControlLimits,
    ResearchControlRecord, ResearchControlState, ResearchControlStatus, ResearchEvent,
    ResearchFailure, ResearchPlan, ResearchRecord, ResearchRequest, ResearchStopReason,
    RetrievedAt, Source, SourceId, SynthesisValidationError, VerificationAssessment,
    VerificationId, VerificationRecord,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    ModelBackedSynthesisError, SynthesisModel, SynthesisModelRequest, finish_synthesis,
    prepare_synthesis, synthesize,
};

#[tokio::test]
async fn completed_research_uses_one_basis_for_stateless_grounded_synthesis() {
    let state = completed_state();
    let mut model = ScriptedSynthesisModel::new([output(valid_proposal())]);

    let report = synthesize(&mut model, &state, CancellationToken::new())
        .await
        .expect("completed assessed research synthesizes");

    assert_eq!(model.calls.len(), 1);
    let request = &model.calls[0];
    assert_eq!(request.name, "research_synthesis");
    assert!(request.instructions.contains("Do not introduce new facts"));
    assert!(
        request
            .instructions
            .contains("Every assertion must cite one or more claim identifiers")
    );
    assert!(
        request
            .instructions
            .contains("Each assertion must contain one factual unit")
    );
    assert!(
        request
            .instructions
            .contains("Do not write section headings")
    );
    assert!(
        request
            .instructions
            .contains("Assertions are the only substantive units")
    );
    let context: Value = serde_json::from_str(&request.input).expect("context is JSON");
    assert_eq!(
        context["research_question"],
        "What does the source establish?"
    );
    assert_eq!(context["claims"][0]["claim_id"], claim_id().to_string());
    assert!(context.get("response_id").is_none());
    assert!(context.get("continuation").is_none());
    assert!(!request.input.contains("initial_plan"));
    assert_eq!(state.status(), ResearchControlStatus::Completed);
    assert_eq!(
        report.render(),
        format!(
            "Research report\nQuestion: \"What does the source establish?\"\nStatus: complete\nLimitation: null\nSection 1\nEstablished: \"The source establishes the claim.\" [1]\nSources\n[1] title=\"Primary source\" locator=\"https://example.test/source\" sha256={} source={} evidence={}\n",
            "01".repeat(32),
            source_id(),
            evidence_id(),
        )
    );
}

#[tokio::test]
async fn synthesis_maps_preconditions_and_model_categories_without_mutating_research() {
    let mut model = ScriptedSynthesisModel::new([]);
    for (state, expected_status) in [
        (
            ResearchControlState::default(),
            ResearchControlStatus::AwaitingLimits,
        ),
        (
            awaiting_next_step_state(),
            ResearchControlStatus::AwaitingNextStep,
        ),
        (researching_state(), ResearchControlStatus::Researching),
    ] {
        assert_eq!(state.status(), expected_status);
        assert_eq!(
            synthesize(&mut model, &state, CancellationToken::new()).await,
            Err(ModelBackedSynthesisError::ResearchNotTerminal)
        );
        assert!(model.calls.is_empty());
    }

    let no_reportable = no_reportable_state();
    assert_eq!(
        synthesize(&mut model, &no_reportable, CancellationToken::new()).await,
        Err(ModelBackedSynthesisError::NoReportableClaims)
    );
    assert!(model.calls.is_empty());

    let failed = failed_state();
    assert_eq!(
        synthesize(&mut model, &failed, CancellationToken::new()).await,
        Err(ModelBackedSynthesisError::FailedResearch)
    );
    assert!(model.calls.is_empty());

    for (invocation, expected) in [
        (
            StructuredOutputInvocation::RequestFailure(
                aurora_core::ModelRequestFailure::RateLimited,
            ),
            ModelBackedSynthesisError::ProviderFailure(
                aurora_core::ModelRequestFailure::RateLimited,
            ),
        ),
        (
            StructuredOutputInvocation::MalformedOutput,
            ModelBackedSynthesisError::MalformedModelOutput,
        ),
        (
            StructuredOutputInvocation::ResponseTooLarge,
            ModelBackedSynthesisError::ModelOutputTooLarge,
        ),
        (
            StructuredOutputInvocation::RequestTooLarge,
            ModelBackedSynthesisError::ModelRequestTooLarge,
        ),
    ] {
        let state = completed_state();
        let before = state.clone();
        let mut model = ScriptedSynthesisModel::new([invocation]);
        assert_eq!(
            synthesize(&mut model, &state, CancellationToken::new()).await,
            Err(expected)
        );
        assert_eq!(state, before);
    }
}

#[test]
fn cancellation_after_each_preparation_stage_error_precedes_that_error() {
    let state = completed_state();

    let cancellation = CancellationToken::new();
    let stage_cancellation = cancellation.clone();
    let result = prepare_synthesis(
        &state,
        &cancellation,
        |_| {
            stage_cancellation.cancel();
            Err(SynthesisValidationError::FailedResearch)
        },
        crate::synthesis_context::synthesis_context,
        SynthesisModelRequest::new,
    );
    assert!(matches!(result, Err(ModelBackedSynthesisError::Cancelled)));

    let cancellation = CancellationToken::new();
    let stage_cancellation = cancellation.clone();
    let result = prepare_synthesis(
        &state,
        &cancellation,
        aurora_research::SynthesisBasis::from_state,
        |_| {
            stage_cancellation.cancel();
            Err(crate::synthesis_context::SynthesisContextError::TooLarge)
        },
        SynthesisModelRequest::new,
    );
    assert!(matches!(result, Err(ModelBackedSynthesisError::Cancelled)));

    let cancellation = CancellationToken::new();
    let stage_cancellation = cancellation.clone();
    let result = prepare_synthesis(
        &state,
        &cancellation,
        aurora_research::SynthesisBasis::from_state,
        |_| Ok("context".to_owned()),
        |_| {
            stage_cancellation.cancel();
            Err(ModelBackedSynthesisError::ModelRequestTooLarge)
        },
    );
    assert!(matches!(result, Err(ModelBackedSynthesisError::Cancelled)));
}

#[tokio::test]
async fn synthesis_maps_decode_and_grounding_rejections_to_their_public_categories() {
    let state = completed_state();
    let invalid_identifier = json!({
        "sections": [{"assertions": [{
            "text": "claim", "claim_ids": ["not-a-uuid"]
        }]}]
    });
    let unknown_claim = json!({
        "sections": [{"assertions": [{
            "text": "claim", "claim_ids": ["123e4567-e89b-42d3-a456-426614174099"]
        }]}]
    });
    for (proposal, expected) in [
        (
            json!({"sections": "not-an-array"}),
            ModelBackedSynthesisError::MalformedModelOutput,
        ),
        (
            invalid_identifier,
            ModelBackedSynthesisError::InvalidReport(
                SynthesisValidationError::InvalidClaimIdentifier(
                    aurora_research::IdentityError::InvalidUuid,
                ),
            ),
        ),
        (
            unknown_claim,
            ModelBackedSynthesisError::InvalidReport(SynthesisValidationError::UnknownClaim(
                ClaimId::from_str("123e4567-e89b-42d3-a456-426614174099").expect("fixture ID"),
            )),
        ),
        (
            json!({"sections": []}),
            ModelBackedSynthesisError::InvalidReport(SynthesisValidationError::DraftHasNoSections),
        ),
        (
            json!({"sections": [{"heading": "model-authored", "assertions": [{"text": "claim", "claim_ids": [claim_id().to_string()]}]}]}),
            ModelBackedSynthesisError::MalformedModelOutput,
        ),
        (
            json!({"sections": [{"assertions": [{"text": " ", "claim_ids": [claim_id().to_string()]}]}]}),
            ModelBackedSynthesisError::MalformedModelOutput,
        ),
        (
            json!({"sections": [{"assertions": [{"text": "claim", "claim_ids": [claim_id().to_string()], "citations": []}]}]}),
            ModelBackedSynthesisError::MalformedModelOutput,
        ),
        (
            json!({"sections": [{"assertions": [{"text": "claim", "claim_ids": [claim_id().to_string()], "qualification": "established"}]}]}),
            ModelBackedSynthesisError::MalformedModelOutput,
        ),
        (
            json!({"sections": [{"assertions": [{"text": "claim", "claim_ids": [claim_id().to_string()]}]}], "sources": []}),
            ModelBackedSynthesisError::MalformedModelOutput,
        ),
    ] {
        let before = state.clone();
        let mut model = ScriptedSynthesisModel::new([output(proposal)]);
        assert_eq!(
            synthesize(&mut model, &state, CancellationToken::new()).await,
            Err(expected)
        );
        assert_eq!(state, before);
    }

    let state = stopped_state_with_unassessed_claim();
    let before = state.clone();
    let mut model = ScriptedSynthesisModel::new([output(json!({
        "sections": [{"assertions": [{
            "text": "claim", "claim_ids": [unassessed_claim_id().to_string()]
        }]}]
    }))]);
    assert_eq!(
        synthesize(&mut model, &state, CancellationToken::new()).await,
        Err(ModelBackedSynthesisError::InvalidReport(
            SynthesisValidationError::UnassessedClaim(unassessed_claim_id()),
        ))
    );
    assert_eq!(state, before);
}

#[tokio::test]
async fn entry_and_simultaneous_provider_cancellation_win_without_a_report() {
    let state = completed_state();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let mut model = ScriptedSynthesisModel::new([output(valid_proposal())]);
    assert_eq!(
        synthesize(&mut model, &state, cancellation).await,
        Err(ModelBackedSynthesisError::Cancelled)
    );
    assert!(model.calls.is_empty());

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(
        synthesize(&mut model, &ResearchControlState::default(), cancellation).await,
        Err(ModelBackedSynthesisError::Cancelled)
    );
    assert!(model.calls.is_empty());

    for output in [
        output(valid_proposal()),
        StructuredOutputInvocation::RequestFailure(aurora_core::ModelRequestFailure::Transport),
        output(json!({"sections": "invalid"})),
    ] {
        let cancellation = CancellationToken::new();
        let mut model = CancellingSynthesisModel::new(cancellation.clone(), output);
        assert_eq!(
            synthesize(&mut model, &state, cancellation).await,
            Err(ModelBackedSynthesisError::Cancelled)
        );
        assert_eq!(model.calls, 1);
    }
}

#[tokio::test]
async fn oversized_context_fails_before_synthesis_invocation() {
    let state = completed_state_with_excerpt(
        &"x".repeat(crate::synthesis_context::MAX_SYNTHESIS_CONTEXT_BYTES),
    );
    let mut model = ScriptedSynthesisModel::new([]);
    assert_eq!(
        synthesize(&mut model, &state, CancellationToken::new()).await,
        Err(ModelBackedSynthesisError::ModelInputTooLarge)
    );
    assert!(model.calls.is_empty());
}

#[tokio::test(start_paused = true)]
async fn timeout_drops_the_caller_owned_provider_future() {
    let state = completed_state();
    let dropped = Arc::new(AtomicBool::new(false));
    let mut model = PendingSynthesisModel::new(dropped.clone());
    let future = synthesize(&mut model, &state, CancellationToken::new());
    tokio::pin!(future);

    assert!(
        tokio::time::timeout(Duration::ZERO, &mut future)
            .await
            .is_err()
    );
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(future.await, Err(ModelBackedSynthesisError::ModelTimeout));
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn cancellation_wins_when_timeout_becomes_ready_at_the_same_boundary() {
    let state = completed_state();
    let cancellation = CancellationToken::new();
    let dropped = Arc::new(AtomicBool::new(false));
    let mut model = PendingSynthesisModel::new(dropped.clone());
    let future = synthesize(&mut model, &state, cancellation.clone());
    tokio::pin!(future);

    assert!(
        tokio::time::timeout(Duration::ZERO, &mut future)
            .await
            .is_err()
    );
    cancellation.cancel();
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(future.await, Err(ModelBackedSynthesisError::Cancelled));
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn cancellation_observed_when_timeout_drops_the_provider_future_wins() {
    let state = completed_state();
    let cancellation = CancellationToken::new();
    let dropped = Arc::new(AtomicBool::new(false));
    let mut model = CancellationOnDropSynthesisModel::new(cancellation.clone(), dropped.clone());
    let future = synthesize(&mut model, &state, cancellation);
    tokio::pin!(future);

    assert!(
        tokio::time::timeout(Duration::ZERO, &mut future)
            .await
            .is_err()
    );
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(future.await, Err(ModelBackedSynthesisError::Cancelled));
    assert!(dropped.load(Ordering::SeqCst));
}

#[test]
fn cancellation_after_decoder_or_grounding_failure_precedes_the_stage_error() {
    let state = completed_state();
    let basis = aurora_research::SynthesisBasis::from_state(&state).expect("basis is valid");
    let cancellation = CancellationToken::new();
    assert_eq!(
        finish_synthesis(
            &basis,
            &valid_proposal(),
            &cancellation,
            |_| {
                cancellation.cancel();
                Err(crate::synthesis_proposal::SynthesisProposalError::InvalidJson)
            },
            aurora_research::GroundedReport::from_basis,
        ),
        Err(ModelBackedSynthesisError::Cancelled)
    );

    let cancellation = CancellationToken::new();
    assert_eq!(
        finish_synthesis(
            &basis,
            &valid_proposal(),
            &cancellation,
            crate::synthesis_proposal::decode_synthesis,
            |_, _| {
                cancellation.cancel();
                Err(SynthesisValidationError::UnknownClaim(claim_id()))
            },
        ),
        Err(ModelBackedSynthesisError::Cancelled)
    );
}

#[tokio::test]
async fn dropping_the_caller_owned_synthesis_future_drops_its_provider_future() {
    let state = completed_state();
    let dropped = Arc::new(AtomicBool::new(false));
    let mut model = PendingSynthesisModel::new(dropped.clone());
    let mut future = Box::pin(synthesize(&mut model, &state, CancellationToken::new()));

    assert!(
        tokio::time::timeout(Duration::ZERO, &mut future)
            .await
            .is_err()
    );
    drop(future);
    assert!(dropped.load(Ordering::SeqCst));
}

type SynthesisFuture = Pin<Box<dyn Future<Output = StructuredOutputInvocation> + Send + 'static>>;

struct ScriptedSynthesisModel {
    calls: Vec<SynthesisModelRequest>,
    outputs: VecDeque<StructuredOutputInvocation>,
}

struct CancellingSynthesisModel {
    cancellation: CancellationToken,
    output: Option<StructuredOutputInvocation>,
    calls: usize,
}

struct PendingSynthesisModel {
    dropped: Arc<AtomicBool>,
}

struct CancellationOnDropSynthesisModel {
    cancellation: CancellationToken,
    dropped: Arc<AtomicBool>,
}

impl CancellationOnDropSynthesisModel {
    fn new(cancellation: CancellationToken, dropped: Arc<AtomicBool>) -> Self {
        Self {
            cancellation,
            dropped,
        }
    }
}

impl SynthesisModel for CancellationOnDropSynthesisModel {
    fn propose(&mut self, _: SynthesisModelRequest, _: CancellationToken) -> SynthesisFuture {
        Box::pin(CancellationOnDropPending {
            cancellation: self.cancellation.clone(),
            dropped: self.dropped.clone(),
        })
    }
}

struct CancellationOnDropPending {
    cancellation: CancellationToken,
    dropped: Arc<AtomicBool>,
}

impl Future for CancellationOnDropPending {
    type Output = StructuredOutputInvocation;

    fn poll(self: Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        std::task::Poll::Pending
    }
}

impl Drop for CancellationOnDropPending {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
        self.cancellation.cancel();
    }
}

impl PendingSynthesisModel {
    fn new(dropped: Arc<AtomicBool>) -> Self {
        Self { dropped }
    }
}

impl SynthesisModel for PendingSynthesisModel {
    fn propose(&mut self, _: SynthesisModelRequest, _: CancellationToken) -> SynthesisFuture {
        Box::pin(DropTrackedPending {
            dropped: self.dropped.clone(),
        })
    }
}

struct DropTrackedPending {
    dropped: Arc<AtomicBool>,
}

impl Future for DropTrackedPending {
    type Output = StructuredOutputInvocation;

    fn poll(self: Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        std::task::Poll::Pending
    }
}

impl Drop for DropTrackedPending {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl CancellingSynthesisModel {
    fn new(cancellation: CancellationToken, output: StructuredOutputInvocation) -> Self {
        Self {
            cancellation,
            output: Some(output),
            calls: 0,
        }
    }
}

impl SynthesisModel for CancellingSynthesisModel {
    fn propose(&mut self, _: SynthesisModelRequest, _: CancellationToken) -> SynthesisFuture {
        self.calls += 1;
        self.cancellation.cancel();
        Box::pin(ready(self.output.take().expect("fixture has one output")))
    }
}

impl ScriptedSynthesisModel {
    fn new(outputs: impl IntoIterator<Item = StructuredOutputInvocation>) -> Self {
        Self {
            calls: Vec::new(),
            outputs: outputs.into_iter().collect(),
        }
    }
}

impl SynthesisModel for ScriptedSynthesisModel {
    fn propose(&mut self, request: SynthesisModelRequest, _: CancellationToken) -> SynthesisFuture {
        self.calls.push(request);
        Box::pin(ready(
            self.outputs
                .pop_front()
                .expect("fixture has one provider output"),
        ))
    }
}

fn output(value: Value) -> StructuredOutputInvocation {
    StructuredOutputInvocation::Output(value)
}

fn valid_proposal() -> Value {
    json!({
        "sections": [{
            "assertions": [{
                "text": "The source establishes the claim.",
                "claim_ids": [claim_id().to_string()]
            }]
        }]
    })
}

fn completed_state() -> ResearchControlState {
    completed_state_with_excerpt("The source establishes the claim.")
}

fn awaiting_next_step_state() -> ResearchControlState {
    ResearchControlState::reconstruct([control(
        1,
        ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(0)),
    )])
    .expect("configured research awaits its next step")
}

fn researching_state() -> ResearchControlState {
    let task =
        InvestigationTask::initial(task_id(), "Assess source".to_owned()).expect("task is valid");
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
                    ResearchRequest::new("What does the source establish?".to_owned())
                        .expect("request is valid"),
                ),
            )),
        ),
        control(
            3,
            ResearchControlEvent::InvestigationAdvanced(investigation(
                2,
                InvestigationEvent::PlanRecorded(
                    ResearchPlan::new(vec![task]).expect("plan is valid"),
                ),
            )),
        ),
    ])
    .expect("planned research is active")
}

fn completed_state_with_excerpt(excerpt: &str) -> ResearchControlState {
    ResearchControlState::reconstruct(completed_records(excerpt))
        .expect("fixture history reconstructs")
}

fn completed_records(excerpt: &str) -> Vec<ResearchControlRecord> {
    synthesis_records(excerpt, false, true)
}

fn stopped_state_with_unassessed_claim() -> ResearchControlState {
    ResearchControlState::reconstruct(synthesis_records(
        "The source establishes the claim.",
        true,
        false,
    ))
    .expect("stopped fixture history reconstructs")
}

fn synthesis_records(
    excerpt: &str,
    include_unassessed: bool,
    complete: bool,
) -> Vec<ResearchControlRecord> {
    let source = Source::new(
        source_id(),
        ContentDigest::sha256([1; 32]),
        "https://example.test/source".to_owned(),
        Some("Primary source".to_owned()),
        RetrievedAt::new("2026-08-29T00:00:00Z".to_owned()).expect("timestamp is valid"),
        MediaType::new("text/plain".to_owned()).expect("media type is valid"),
    )
    .expect("source is valid");
    let evidence =
        Evidence::new(evidence_id(), source_id(), excerpt.to_owned()).expect("evidence is valid");
    let claim = Claim::new(
        claim_id(),
        "The source establishes the claim.".to_owned(),
        vec![evidence_id()],
    )
    .expect("claim is valid");
    let mut research_records = vec![
        research(1, ResearchEvent::SourceRecorded(source)),
        research(2, ResearchEvent::EvidenceRecorded(evidence)),
        research(3, ResearchEvent::ClaimProposed(claim)),
    ];
    if include_unassessed {
        let unassessed = Claim::new(
            unassessed_claim_id(),
            "The unassessed claim is not reportable.".to_owned(),
            vec![evidence_id()],
        )
        .expect("claim is valid");
        research_records.push(research(4, ResearchEvent::ClaimProposed(unassessed)));
    }
    let task =
        InvestigationTask::initial(task_id(), "Assess source".to_owned()).expect("task is valid");
    vec![
        control(
            1,
            ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(0)),
        ),
        control(
            2,
            ResearchControlEvent::InvestigationAdvanced(investigation(
                1,
                InvestigationEvent::RequestRecorded(
                    ResearchRequest::new("What does the source establish?".to_owned())
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
                            evidence_id(),
                            EvidenceRelation::Supports,
                        )],
                        EvidenceSufficiency::Sufficient,
                    )
                    .expect("assessment is valid"),
                )
                .expect("record is valid"),
            ),
        ),
        control(
            7,
            if complete {
                ResearchControlEvent::ResearchCompleted
            } else {
                ResearchControlEvent::InvestigationAdvanced(investigation(
                    5,
                    InvestigationEvent::ResearchStopped(ResearchStopReason::OperatorStopped),
                ))
            },
        ),
    ]
}

fn failed_state() -> ResearchControlState {
    let mut records = completed_records("The source establishes the claim.");
    records.pop();
    records.push(control(
        7,
        ResearchControlEvent::ResearchFailed(
            ResearchFailure::new("model failure".to_owned()).expect("failure is valid"),
        ),
    ));
    ResearchControlState::reconstruct(records).expect("failed fixture history reconstructs")
}

fn no_reportable_state() -> ResearchControlState {
    let task =
        InvestigationTask::initial(task_id(), "Assess source".to_owned()).expect("task is valid");
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
                    ResearchRequest::new("What does the source establish?".to_owned())
                        .expect("request is valid"),
                ),
            )),
        ),
        control(
            3,
            ResearchControlEvent::InvestigationAdvanced(investigation(
                2,
                InvestigationEvent::PlanRecorded(
                    ResearchPlan::new(vec![task]).expect("plan is valid"),
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
                    result: InvestigationResult::new(vec![]),
                },
            )),
        ),
        control(
            6,
            ResearchControlEvent::InvestigationAdvanced(investigation(
                5,
                InvestigationEvent::ResearchStopped(ResearchStopReason::OperatorStopped),
            )),
        ),
    ])
    .expect("no-reportable fixture history reconstructs")
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
    SourceId::from_str("123e4567-e89b-42d3-a456-426614174001").expect("fixture ID is v4")
}

fn evidence_id() -> EvidenceId {
    EvidenceId::from_str("123e4567-e89b-42d3-a456-426614174002").expect("fixture ID is v4")
}

fn claim_id() -> ClaimId {
    ClaimId::from_str("123e4567-e89b-42d3-a456-426614174003").expect("fixture ID is v4")
}

fn unassessed_claim_id() -> ClaimId {
    ClaimId::from_str("123e4567-e89b-42d3-a456-426614174006").expect("fixture ID is v4")
}

fn task_id() -> InvestigationTaskId {
    InvestigationTaskId::from_str("123e4567-e89b-42d3-a456-426614174004").expect("fixture ID is v4")
}

fn verification_id() -> VerificationId {
    VerificationId::from_str("123e4567-e89b-42d3-a456-426614174005").expect("fixture ID is v4")
}
