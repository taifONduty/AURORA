use std::{future::Future, pin::Pin, time::Duration};

use aurora_core::ModelRequestFailure;
use aurora_openai::{OpenAiBackend, StructuredOutputInvocation, StructuredOutputRequest};
use aurora_research::{
    BlockedReason, Claim, ClaimId, EvidenceAssessment, IdentifiedResearchGap, InvestigationEvent,
    InvestigationFailure, InvestigationRecord, InvestigationResult, InvestigationTask,
    InvestigationTaskId, InvestigationTaskStatus, ResearchControlEvent, ResearchControlLimits,
    ResearchControlRecord, ResearchControlState, ResearchControlStatus,
    ResearchControlTransitionError, ResearchEvent, ResearchFailure, ResearchGap, ResearchGapCause,
    ResearchGapId, ResearchGapStatus, ResearchPlan, ResearchRequest, ResearchStopReason,
    RetrievedAt, VerificationAssessment, VerificationId, VerificationRecord,
};
use aurora_tavily::{TavilyFailure, TavilyInvestigator};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    admission::{AdmissionError, admit_extraction, snapshot_context},
    context::{
        ModelContextError, follow_up_context, initial_plan_context, sourced_evidence,
        verification_context,
    },
    proposal::{
        decode_extraction, decode_follow_up, decode_initial_plan, decode_verification,
        extraction_schema, follow_up_schema, initial_plan_schema, verification_schema,
    },
};

const MODEL_CALL_LIMIT: Duration = Duration::from_secs(60);
const BLOCKED_REASON: &str = "model-driven research cannot continue";
const RESEARCH_FAILURE: &str = "model-driven research failed";
const CANCELLED_TASK_FAILURE: &str = "research cancelled while task was active";
const EXTRACTION_TASK_FAILURE: &str = "task extraction failed";
const VERIFICATION_GAP: &str = "verification requires additional evidence";

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ModelDrivenResearchIssue {
    #[error("model provider request failed: {0:?}")]
    ProviderFailure(ModelRequestFailure),
    #[error("model call timed out")]
    ModelTimeout,
    #[error("research was cancelled")]
    Cancelled,
    #[error("model output was malformed")]
    MalformedModelOutput,
    #[error("model proposal was invalid for the research domain")]
    DomainInvalidProposal,
    #[error("proposed evidence was absent from the selected source")]
    EvidenceAbsent,
    #[error("acquired source content exceeded the model input limit")]
    SourceContentTooLarge,
    #[error("serialized research model input exceeded the byte limit")]
    ModelInputTooLarge,
    #[error("model produced no useful research action")]
    NoUsefulAction,
    #[error("Tavily retrieval failed: {0:?}")]
    TavilyFailure(TavilyFailure),
    #[error("follow-up allowance was exhausted")]
    FollowUpAllowanceExhausted,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ModelDrivenResearchExecutionError {
    #[error("generated record sequence was exhausted")]
    SequenceExhausted,
    #[error("generated domain value was rejected")]
    GeneratedDomainValue,
    #[error("generated control transition was rejected: {0}")]
    ControlTransition(ResearchControlTransitionError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelDrivenResearchRun {
    records: Vec<ResearchControlRecord>,
    state: ResearchControlState,
    issue: Option<ModelDrivenResearchIssue>,
}

impl ModelDrivenResearchRun {
    pub fn records(&self) -> &[ResearchControlRecord] {
        &self.records
    }

    pub const fn state(&self) -> &ResearchControlState {
        &self.state
    }

    pub const fn issue(&self) -> Option<&ModelDrivenResearchIssue> {
        self.issue.as_ref()
    }

    pub fn into_parts(
        self,
    ) -> (
        Vec<ResearchControlRecord>,
        ResearchControlState,
        Option<ModelDrivenResearchIssue>,
    ) {
        (self.records, self.state, self.issue)
    }
}

#[derive(Debug)]
pub struct OpenAiTavilyResearcher {
    model: OpenAiBackend,
    retrieval: TavilyInvestigator,
}

impl OpenAiTavilyResearcher {
    pub const fn new(model: OpenAiBackend, retrieval: TavilyInvestigator) -> Self {
        Self { model, retrieval }
    }

    pub async fn run(
        &mut self,
        request: ResearchRequest,
        limits: ResearchControlLimits,
        retrieved_at: RetrievedAt,
        cancellation: CancellationToken,
    ) -> Result<ModelDrivenResearchRun, ModelDrivenResearchExecutionError> {
        execute(
            &mut self.model,
            &mut self.retrieval,
            request,
            limits,
            retrieved_at,
            cancellation,
        )
        .await
    }
}

struct ModelProposalRequest {
    name: String,
    instructions: String,
    input: String,
    schema: Value,
}

impl ModelProposalRequest {
    fn new(name: &str, instructions: &str, input: String, schema: Value) -> Self {
        Self {
            name: name.to_owned(),
            instructions: instructions.to_owned(),
            input,
            schema,
        }
    }
}

type ProposalFuture = Pin<Box<dyn Future<Output = StructuredOutputInvocation> + Send + 'static>>;

trait ProposalModel {
    fn propose(
        &mut self,
        request: ModelProposalRequest,
        cancellation: CancellationToken,
    ) -> ProposalFuture;
}

impl ProposalModel for OpenAiBackend {
    fn propose(
        &mut self,
        request: ModelProposalRequest,
        cancellation: CancellationToken,
    ) -> ProposalFuture {
        let request = StructuredOutputRequest::new(
            request.name,
            request.instructions,
            request.input,
            request.schema,
        )
        .unwrap_or_else(|_| unreachable!("runner constructs nonblank object-schema requests"));
        self.invoke_structured(request, cancellation)
    }
}

type RetrievalFuture<'a> =
    Pin<Box<dyn Future<Output = Result<InvestigationResult, TavilyFailure>> + Send + 'a>>;

trait ResearchRetrieval {
    fn retrieve<'a>(
        &'a mut self,
        task: &'a InvestigationTask,
        next_sequence: u64,
        retrieved_at: RetrievedAt,
    ) -> RetrievalFuture<'a>;
}

impl ResearchRetrieval for TavilyInvestigator {
    fn retrieve<'a>(
        &'a mut self,
        task: &'a InvestigationTask,
        next_sequence: u64,
        retrieved_at: RetrievedAt,
    ) -> RetrievalFuture<'a> {
        Box::pin(self.investigate(task, next_sequence, retrieved_at))
    }
}

async fn execute<M, R>(
    model: &mut M,
    retrieval: &mut R,
    request: ResearchRequest,
    limits: ResearchControlLimits,
    retrieved_at: RetrievedAt,
    cancellation: CancellationToken,
) -> Result<ModelDrivenResearchRun, ModelDrivenResearchExecutionError>
where
    M: ProposalModel,
    R: ResearchRetrieval,
{
    let mut owner = SequentialRun::default();
    owner.append(ResearchControlEvent::LimitsRecorded(limits))?;
    owner.append_investigation(InvestigationEvent::RequestRecorded(request.clone()))?;
    if owner.stop_if_cancelled(&cancellation)? {
        return Ok(owner.finish());
    }

    let planning_context = match initial_plan_context(&request) {
        Ok(context) => context,
        Err(ModelContextError::TooLarge) => {
            if owner.stop_if_cancelled(&cancellation)? {
                return Ok(owner.finish());
            }
            return owner
                .stop(ModelDrivenResearchIssue::ModelInputTooLarge)
                .map(|()| owner.finish());
        }
    };
    let planning = invoke_model(
        model,
        ModelProposalRequest::new(
            "initial_plan",
            "Propose up to three distinct research search objectives.",
            planning_context,
            initial_plan_schema(),
        ),
        &cancellation,
    )
    .await;
    let planning = match planning {
        Ok(value) => value,
        Err(issue) => return owner.stop(issue).map(|()| owner.finish()),
    };
    let proposal = match decode_initial_plan(&planning.to_string()) {
        Ok(proposal) => proposal,
        Err(_) => {
            return owner
                .stop(ModelDrivenResearchIssue::DomainInvalidProposal)
                .map(|()| owner.finish());
        }
    };
    if proposal.tasks.is_empty() {
        return owner
            .stop(ModelDrivenResearchIssue::NoUsefulAction)
            .map(|()| owner.finish());
    }
    let tasks = proposal
        .tasks
        .into_iter()
        .map(|proposal| {
            InvestigationTask::initial(InvestigationTaskId::generate(), proposal.objective)
                .map_err(|_| ModelDrivenResearchExecutionError::GeneratedDomainValue)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let plan = ResearchPlan::new(tasks)
        .map_err(|_| ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
    owner.append_investigation(InvestigationEvent::PlanRecorded(plan))?;

    let mut task_issue = None;
    while let Some(task) = owner.state.investigation().next_pending_task().cloned() {
        owner.append_investigation(InvestigationEvent::TaskStarted {
            task_id: *task.id(),
        })?;
        let task_result = investigate_task(
            model,
            retrieval,
            &owner.state,
            &task,
            retrieved_at.clone(),
            &cancellation,
        )
        .await;
        match task_result {
            Ok(result) => owner.append_investigation(InvestigationEvent::TaskCompleted {
                task_id: *task.id(),
                result,
            })?,
            Err(TaskFailure::Domain(issue, failure)) => {
                if task_issue.is_none() || issue == ModelDrivenResearchIssue::Cancelled {
                    task_issue = Some(issue);
                }
                owner.append_investigation(InvestigationEvent::TaskFailed {
                    task_id: *task.id(),
                    failure,
                })?;
            }
            Err(TaskFailure::Execution(error)) => return Err(error),
        }
    }

    if let Some(issue) = task_issue {
        if issue == ModelDrivenResearchIssue::Cancelled {
            owner.stop(issue)?;
        } else {
            owner.fail(issue)?;
        }
        return Ok(owner.finish());
    }
    if owner.state.investigation().research().claim_count() == 0 {
        owner.stop(ModelDrivenResearchIssue::NoUsefulAction)?;
        return Ok(owner.finish());
    }

    let claims = owner.recorded_claims();
    let mut pending_gap_causes = Vec::with_capacity(claims.len());
    for claim in claims {
        let verification = verify_claim(model, &owner.state, &request, &claim, &cancellation).await;
        if owner.stop_if_cancelled(&cancellation)? {
            return Ok(owner.finish());
        }
        match verification {
            Ok(record) => {
                pending_gap_causes
                    .push((*record.assessment().claim_id(), *record.assessment().id()));
                owner.append(ResearchControlEvent::VerificationRecorded(record))?;
            }
            Err(VerificationFailure::Domain(ModelDrivenResearchIssue::Cancelled)) => {
                owner.stop(ModelDrivenResearchIssue::Cancelled)?;
                return Ok(owner.finish());
            }
            Err(VerificationFailure::Domain(issue)) => {
                owner.fail(issue)?;
                return Ok(owner.finish());
            }
            Err(VerificationFailure::Execution(error)) => return Err(error),
        }
    }

    loop {
        if owner.stop_if_cancelled(&cancellation)? {
            return Ok(owner.finish());
        }
        let completion = owner.probe_completion(&pending_gap_causes)?;
        if owner.stop_if_cancelled(&cancellation)? {
            return Ok(owner.finish());
        }
        match completion {
            CompletionProbe::Ready => {
                owner.append(ResearchControlEvent::ResearchCompleted)?;
                return Ok(owner.finish());
            }
            CompletionProbe::NeedsGap(verification_id) => {
                let gap_id = ResearchGapId::generate();
                let gap = ResearchGap::new(VERIFICATION_GAP.to_owned())
                    .map_err(|_| ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
                owner.append(ResearchControlEvent::GapIdentified(
                    IdentifiedResearchGap::new(
                        gap_id,
                        ResearchGapCause::Verification(verification_id),
                        gap,
                    ),
                ))?;
                pending_gap_causes.retain(|(_, candidate)| candidate != &verification_id);
                if owner.stop_if_cancelled(&cancellation)? {
                    return Ok(owner.finish());
                }
                if owner.state.follow_up_count() >= limits.max_follow_up_tasks() {
                    owner.stop(ModelDrivenResearchIssue::FollowUpAllowanceExhausted)?;
                    return Ok(owner.finish());
                }

                let assessment = owner
                    .state
                    .verification()
                    .assessment(&verification_id)
                    .cloned()
                    .ok_or(ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
                let claim = owner
                    .state
                    .investigation()
                    .research()
                    .claim(assessment.claim_id())
                    .cloned()
                    .ok_or(ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
                let gap = owner
                    .state
                    .gap(&gap_id)
                    .map(|state| state.gap().description().clone())
                    .ok_or(ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
                let evidence = sourced_evidence(owner.state.investigation().research())
                    .ok_or(ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
                let completed_objectives = owner.completed_objectives();
                let remaining_follow_ups = limits
                    .max_follow_up_tasks()
                    .checked_sub(owner.state.follow_up_count())
                    .ok_or(ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
                let proposed = invoke_model(
                    model,
                    ModelProposalRequest::new(
                        "follow_up_planning",
                        "Propose one concrete Tavily query that addresses the recorded research gap.",
                        match follow_up_context(
                            &request,
                            &gap,
                            &claim,
                            &assessment,
                            &evidence,
                            &completed_objectives,
                            remaining_follow_ups,
                        ) {
                            Ok(context) => context,
                            Err(ModelContextError::TooLarge) => {
                                if owner.stop_if_cancelled(&cancellation)? {
                                    return Ok(owner.finish());
                                }
                                owner.fail(ModelDrivenResearchIssue::ModelInputTooLarge)?;
                                return Ok(owner.finish());
                            }
                        },
                        follow_up_schema(),
                    ),
                    &cancellation,
                )
                .await;
                if owner.stop_if_cancelled(&cancellation)? {
                    return Ok(owner.finish());
                }
                let proposed = match proposed {
                    Ok(value) => value,
                    Err(ModelDrivenResearchIssue::Cancelled) => {
                        owner.stop(ModelDrivenResearchIssue::Cancelled)?;
                        return Ok(owner.finish());
                    }
                    Err(issue) => {
                        owner.fail(issue)?;
                        return Ok(owner.finish());
                    }
                };
                let proposal = decode_follow_up(&proposed.to_string());
                if owner.stop_if_cancelled(&cancellation)? {
                    return Ok(owner.finish());
                }
                let proposal = match proposal {
                    Ok(proposal) => proposal,
                    Err(_) => {
                        owner.fail(ModelDrivenResearchIssue::DomainInvalidProposal)?;
                        return Ok(owner.finish());
                    }
                };
                if owner.stop_if_cancelled(&cancellation)? {
                    return Ok(owner.finish());
                }
                let Some(objective) = proposal.objective else {
                    owner.stop(ModelDrivenResearchIssue::NoUsefulAction)?;
                    return Ok(owner.finish());
                };
                let parent_task_id = owner
                    .most_recent_completed_task_id()
                    .ok_or(ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
                let follow_up = InvestigationTask::follow_up(
                    InvestigationTaskId::generate(),
                    parent_task_id,
                    objective,
                    gap,
                )
                .map_err(|_| ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
                owner.append_gap_follow_up(gap_id, follow_up.clone())?;
                owner.append_investigation(InvestigationEvent::TaskStarted {
                    task_id: *follow_up.id(),
                })?;
                match investigate_task(
                    model,
                    retrieval,
                    &owner.state,
                    &follow_up,
                    retrieved_at.clone(),
                    &cancellation,
                )
                .await
                {
                    Ok(result) => {
                        owner.append_investigation(InvestigationEvent::TaskCompleted {
                            task_id: *follow_up.id(),
                            result,
                        })?;
                    }
                    Err(TaskFailure::Domain(issue, failure)) => {
                        owner.append_investigation(InvestigationEvent::TaskFailed {
                            task_id: *follow_up.id(),
                            failure,
                        })?;
                        if owner.stop_if_cancelled(&cancellation)? {
                            return Ok(owner.finish());
                        }
                        if issue == ModelDrivenResearchIssue::Cancelled {
                            owner.stop(issue)?;
                        } else {
                            owner.fail(issue)?;
                        }
                        return Ok(owner.finish());
                    }
                    Err(TaskFailure::Execution(error)) => return Err(error),
                }

                if owner.stop_if_cancelled(&cancellation)? {
                    return Ok(owner.finish());
                }

                let verification =
                    verify_claim(model, &owner.state, &request, &claim, &cancellation).await;
                if owner.stop_if_cancelled(&cancellation)? {
                    return Ok(owner.finish());
                }
                let verification = match verification {
                    Ok(record) => record,
                    Err(VerificationFailure::Domain(ModelDrivenResearchIssue::Cancelled)) => {
                        owner.stop(ModelDrivenResearchIssue::Cancelled)?;
                        return Ok(owner.finish());
                    }
                    Err(VerificationFailure::Domain(issue)) => {
                        owner.fail(issue)?;
                        return Ok(owner.finish());
                    }
                    Err(VerificationFailure::Execution(error)) => return Err(error),
                };
                let resolution_id = *verification.assessment().id();
                pending_gap_causes.push((*claim.id(), resolution_id));
                owner.append(ResearchControlEvent::VerificationRecorded(verification))?;
                if owner.stop_if_cancelled(&cancellation)? {
                    return Ok(owner.finish());
                }
                if owner.resolve_eligible_gaps(*claim.id(), resolution_id, &cancellation)? {
                    return Ok(owner.finish());
                }

                let unassessed_claims = owner
                    .recorded_claims()
                    .into_iter()
                    .filter(|candidate| {
                        !owner
                            .state
                            .verification()
                            .assessments()
                            .any(|assessment| assessment.claim_id() == candidate.id())
                    })
                    .collect::<Vec<_>>();
                for unassessed in unassessed_claims {
                    let verification =
                        verify_claim(model, &owner.state, &request, &unassessed, &cancellation)
                            .await;
                    if owner.stop_if_cancelled(&cancellation)? {
                        return Ok(owner.finish());
                    }
                    let verification = match verification {
                        Ok(record) => record,
                        Err(VerificationFailure::Domain(ModelDrivenResearchIssue::Cancelled)) => {
                            owner.stop(ModelDrivenResearchIssue::Cancelled)?;
                            return Ok(owner.finish());
                        }
                        Err(VerificationFailure::Domain(issue)) => {
                            owner.fail(issue)?;
                            return Ok(owner.finish());
                        }
                        Err(VerificationFailure::Execution(error)) => return Err(error),
                    };
                    let verification_id = *verification.assessment().id();
                    pending_gap_causes.push((*unassessed.id(), verification_id));
                    owner.append(ResearchControlEvent::VerificationRecorded(verification))?;
                }
            }
        }
    }
}

async fn investigate_task<M, R>(
    model: &mut M,
    retrieval: &mut R,
    state: &ResearchControlState,
    task: &InvestigationTask,
    retrieved_at: RetrievedAt,
    cancellation: &CancellationToken,
) -> Result<InvestigationResult, TaskFailure>
where
    M: ProposalModel,
    R: ResearchRetrieval,
{
    if cancellation.is_cancelled() {
        return Err(TaskFailure::cancelled());
    }
    let next_sequence =
        next(state.investigation().research().last_sequence()).map_err(TaskFailure::Execution)?;
    let acquired = retrieval.retrieve(task, next_sequence, retrieved_at).await;
    if cancellation.is_cancelled() {
        return Err(TaskFailure::cancelled());
    }
    let acquired = acquired.map_err(|failure| {
        TaskFailure::Domain(
            ModelDrivenResearchIssue::TavilyFailure(failure),
            failure.into_investigation_failure(),
        )
    })?;
    let context = snapshot_context(&acquired);
    if cancellation.is_cancelled() {
        return Err(TaskFailure::cancelled());
    }
    let context = context.map_err(TaskFailure::from_admission)?;
    let output = invoke_model(
        model,
        ModelProposalRequest::new(
            "evidence_extraction",
            "Extract exact evidence excerpts and grounded claims from the supplied snapshots.",
            context,
            extraction_schema(),
        ),
        cancellation,
    )
    .await
    .map_err(TaskFailure::from_issue)?;
    let proposal = decode_extraction(&output.to_string())
        .map_err(|_| TaskFailure::from_issue(ModelDrivenResearchIssue::DomainInvalidProposal))?;
    admit_extraction(state.investigation().research(), acquired, proposal)
        .map_err(TaskFailure::from_admission)
}

async fn verify_claim<M>(
    model: &mut M,
    state: &ResearchControlState,
    request: &ResearchRequest,
    claim: &Claim,
    cancellation: &CancellationToken,
) -> Result<VerificationRecord, VerificationFailure>
where
    M: ProposalModel,
{
    let research = state.investigation().research();
    let evidence = sourced_evidence(research).ok_or(VerificationFailure::Execution(
        ModelDrivenResearchExecutionError::GeneratedDomainValue,
    ))?;
    let output = invoke_model(
        model,
        ModelProposalRequest::new(
            "claim_verification",
            "Assess each referenced evidence item against the claim and decide sufficiency.",
            verification_context(request, claim, &evidence).map_err(|error| match error {
                ModelContextError::TooLarge => {
                    VerificationFailure::Domain(ModelDrivenResearchIssue::ModelInputTooLarge)
                }
            })?,
            verification_schema(),
        ),
        cancellation,
    )
    .await
    .map_err(VerificationFailure::Domain)?;
    let proposal = decode_verification(&output.to_string()).map_err(|_| {
        VerificationFailure::Domain(ModelDrivenResearchIssue::DomainInvalidProposal)
    })?;
    let relations = proposal
        .relations
        .into_iter()
        .map(|relation| {
            let (item, _) =
                evidence
                    .get(relation.evidence_index)
                    .ok_or(VerificationFailure::Domain(
                        ModelDrivenResearchIssue::DomainInvalidProposal,
                    ))?;
            Ok(EvidenceAssessment::new(
                *item.id(),
                relation.relation.into(),
            ))
        })
        .collect::<Result<Vec<_>, VerificationFailure>>()?;
    let assessment = VerificationAssessment::new(
        VerificationId::generate(),
        *claim.id(),
        relations,
        proposal.sufficiency.into(),
    )
    .map_err(|_| {
        VerificationFailure::Execution(ModelDrivenResearchExecutionError::GeneratedDomainValue)
    })?;
    let sequence =
        next(state.verification().last_sequence()).map_err(VerificationFailure::Execution)?;
    VerificationRecord::new(sequence, assessment).map_err(|_| {
        VerificationFailure::Execution(ModelDrivenResearchExecutionError::GeneratedDomainValue)
    })
}

async fn invoke_model<M>(
    model: &mut M,
    request: ModelProposalRequest,
    cancellation: &CancellationToken,
) -> Result<Value, ModelDrivenResearchIssue>
where
    M: ProposalModel,
{
    if cancellation.is_cancelled() {
        return Err(ModelDrivenResearchIssue::Cancelled);
    }
    let pending = model.propose(request, cancellation.clone());
    let invocation = tokio::select! {
        biased;
        () = cancellation.cancelled() => return Err(ModelDrivenResearchIssue::Cancelled),
        result = tokio::time::timeout(MODEL_CALL_LIMIT, pending) => {
            result.map_err(|_| ModelDrivenResearchIssue::ModelTimeout)?
        }
    };
    if cancellation.is_cancelled() {
        return Err(ModelDrivenResearchIssue::Cancelled);
    }
    match invocation {
        StructuredOutputInvocation::Output(value) => Ok(value),
        StructuredOutputInvocation::RequestFailure(failure) => {
            Err(ModelDrivenResearchIssue::ProviderFailure(failure))
        }
        StructuredOutputInvocation::MalformedOutput
        | StructuredOutputInvocation::ResponseTooLarge => {
            Err(ModelDrivenResearchIssue::MalformedModelOutput)
        }
        StructuredOutputInvocation::RequestTooLarge => {
            Err(ModelDrivenResearchIssue::ModelInputTooLarge)
        }
        StructuredOutputInvocation::Cancelled => Err(ModelDrivenResearchIssue::Cancelled),
    }
}

#[derive(Default)]
struct SequentialRun {
    state: ResearchControlState,
    records: Vec<ResearchControlRecord>,
    issue: Option<ModelDrivenResearchIssue>,
}

impl SequentialRun {
    fn append(
        &mut self,
        event: ResearchControlEvent,
    ) -> Result<(), ModelDrivenResearchExecutionError> {
        let sequence = next(self.state.last_sequence())?;
        let record = ResearchControlRecord::new(sequence, event)
            .map_err(|_| ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
        self.state
            .apply(record.clone())
            .map_err(ModelDrivenResearchExecutionError::ControlTransition)?;
        self.records.push(record);
        Ok(())
    }

    fn append_investigation(
        &mut self,
        event: InvestigationEvent,
    ) -> Result<(), ModelDrivenResearchExecutionError> {
        let sequence = next(self.state.investigation().last_sequence())?;
        let record = InvestigationRecord::new(sequence, event)
            .map_err(|_| ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
        self.append(ResearchControlEvent::InvestigationAdvanced(record))
    }

    fn append_gap_follow_up(
        &mut self,
        gap_id: ResearchGapId,
        task: InvestigationTask,
    ) -> Result<(), ModelDrivenResearchExecutionError> {
        let sequence = next(self.state.investigation().last_sequence())?;
        let investigation_record =
            InvestigationRecord::new(sequence, InvestigationEvent::FollowUpRecorded(task))
                .map_err(|_| ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
        self.append(ResearchControlEvent::GapFollowUpRecorded {
            gap_id,
            investigation_record,
        })
    }

    fn stop(
        &mut self,
        issue: ModelDrivenResearchIssue,
    ) -> Result<(), ModelDrivenResearchExecutionError> {
        let reason = match issue {
            ModelDrivenResearchIssue::Cancelled => ResearchStopReason::OperatorStopped,
            ModelDrivenResearchIssue::FollowUpAllowanceExhausted => {
                ResearchStopReason::BudgetExhausted
            }
            _ => ResearchStopReason::Blocked(
                BlockedReason::new(BLOCKED_REASON.to_owned())
                    .map_err(|_| ModelDrivenResearchExecutionError::GeneratedDomainValue)?,
            ),
        };
        self.append_investigation(InvestigationEvent::ResearchStopped(reason))?;
        self.issue = Some(issue);
        Ok(())
    }

    fn stop_if_cancelled(
        &mut self,
        cancellation: &CancellationToken,
    ) -> Result<bool, ModelDrivenResearchExecutionError> {
        if !cancellation.is_cancelled() {
            return Ok(false);
        }
        self.stop(ModelDrivenResearchIssue::Cancelled)?;
        Ok(true)
    }

    fn fail(
        &mut self,
        issue: ModelDrivenResearchIssue,
    ) -> Result<(), ModelDrivenResearchExecutionError> {
        let failure = ResearchFailure::new(RESEARCH_FAILURE.to_owned())
            .map_err(|_| ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
        self.append(ResearchControlEvent::ResearchFailed(failure))?;
        self.issue = Some(issue);
        Ok(())
    }

    fn recorded_claims(&self) -> Vec<Claim> {
        self.records
            .iter()
            .filter_map(|record| match record.event() {
                ResearchControlEvent::InvestigationAdvanced(record) => match record.event() {
                    InvestigationEvent::TaskCompleted { result, .. } => Some(result),
                    _ => None,
                },
                _ => None,
            })
            .flat_map(|result| result.research_records())
            .filter_map(|record| match record.event() {
                ResearchEvent::ClaimProposed(claim) => Some(claim.clone()),
                _ => None,
            })
            .collect()
    }

    fn completed_objectives(&self) -> Vec<String> {
        self.state
            .investigation()
            .tasks()
            .filter(|task| matches!(task.status(), InvestigationTaskStatus::Completed))
            .map(|task| task.task().objective().to_owned())
            .collect()
    }

    fn most_recent_completed_task_id(&self) -> Option<InvestigationTaskId> {
        self.state
            .investigation()
            .tasks()
            .filter(|task| matches!(task.status(), InvestigationTaskStatus::Completed))
            .map(|task| *task.task().id())
            .last()
    }

    fn resolve_eligible_gaps(
        &mut self,
        claim_id: ClaimId,
        verification_id: VerificationId,
        cancellation: &CancellationToken,
    ) -> Result<bool, ModelDrivenResearchExecutionError> {
        let gap_ids = self
            .state
            .gaps()
            .filter(|gap| matches!(gap.status(), ResearchGapStatus::Open))
            .filter_map(|gap| {
                let ResearchGapCause::Verification(cause_id) = gap.gap().cause() else {
                    return None;
                };
                let cause = self.state.verification().assessment(cause_id)?;
                if cause.claim_id() != &claim_id {
                    return None;
                }
                let follow_up_id = gap.follow_up_task_id()?;
                let follow_up = self.state.investigation().task(follow_up_id)?;
                matches!(follow_up.status(), InvestigationTaskStatus::Completed)
                    .then_some(*gap.gap().id())
            })
            .collect::<Vec<_>>();

        for gap_id in gap_ids {
            if self.stop_if_cancelled(cancellation)? {
                return Ok(true);
            }
            let event = ResearchControlEvent::GapResolved {
                gap_id,
                verification_id,
            };
            let sequence = next(self.state.last_sequence())?;
            let record = ResearchControlRecord::new(sequence, event.clone())
                .map_err(|_| ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
            let mut candidate = self.state.clone();
            match candidate.apply(record) {
                Ok(()) => self.append(event)?,
                Err(ResearchControlTransitionError::VerificationDoesNotResolveGap(id))
                    if id == verification_id =>
                {
                    return Ok(false);
                }
                Err(error) => {
                    return Err(ModelDrivenResearchExecutionError::ControlTransition(error));
                }
            }
        }
        Ok(false)
    }

    fn probe_completion(
        &self,
        pending_gap_causes: &[(ClaimId, VerificationId)],
    ) -> Result<CompletionProbe, ModelDrivenResearchExecutionError> {
        let sequence = next(self.state.last_sequence())?;
        let record = ResearchControlRecord::new(sequence, ResearchControlEvent::ResearchCompleted)
            .map_err(|_| ModelDrivenResearchExecutionError::GeneratedDomainValue)?;
        let mut candidate = self.state.clone();
        match candidate.apply(record) {
            Ok(()) => Ok(CompletionProbe::Ready),
            Err(ResearchControlTransitionError::VerificationNeedsGap(id)) => {
                Ok(CompletionProbe::NeedsGap(id))
            }
            Err(error @ ResearchControlTransitionError::ClaimNeedsAssessment(claim_id)) => {
                pending_gap_causes
                    .iter()
                    .rev()
                    .find(|(candidate, _)| candidate == &claim_id)
                    .map(|(_, verification_id)| CompletionProbe::NeedsGap(*verification_id))
                    .ok_or(ModelDrivenResearchExecutionError::ControlTransition(error))
            }
            Err(error) => Err(ModelDrivenResearchExecutionError::ControlTransition(error)),
        }
    }

    fn finish(self) -> ModelDrivenResearchRun {
        debug_assert_eq!(
            self.issue.is_none(),
            self.state.status() == ResearchControlStatus::Completed
        );
        ModelDrivenResearchRun {
            records: self.records,
            state: self.state,
            issue: self.issue,
        }
    }
}

enum CompletionProbe {
    Ready,
    NeedsGap(VerificationId),
}

enum TaskFailure {
    Domain(ModelDrivenResearchIssue, InvestigationFailure),
    Execution(ModelDrivenResearchExecutionError),
}

impl TaskFailure {
    fn from_issue(issue: ModelDrivenResearchIssue) -> Self {
        let failure = InvestigationFailure::new(EXTRACTION_TASK_FAILURE.to_owned())
            .unwrap_or_else(|_| unreachable!("runner task failure is nonblank"));
        Self::Domain(issue, failure)
    }

    fn cancelled() -> Self {
        let failure = InvestigationFailure::new(CANCELLED_TASK_FAILURE.to_owned())
            .unwrap_or_else(|_| unreachable!("runner cancellation failure is nonblank"));
        Self::Domain(ModelDrivenResearchIssue::Cancelled, failure)
    }

    fn from_admission(error: AdmissionError) -> Self {
        match error {
            AdmissionError::SnapshotTextTooLarge => {
                Self::from_issue(ModelDrivenResearchIssue::SourceContentTooLarge)
            }
            AdmissionError::ModelInputTooLarge => {
                Self::from_issue(ModelDrivenResearchIssue::ModelInputTooLarge)
            }
            AdmissionError::ExcerptAbsent => {
                Self::from_issue(ModelDrivenResearchIssue::EvidenceAbsent)
            }
            AdmissionError::UnknownSourceIndex | AdmissionError::UnknownEvidenceIndex => {
                Self::from_issue(ModelDrivenResearchIssue::DomainInvalidProposal)
            }
            AdmissionError::InvalidSnapshotPairing => Self::from_issue(
                ModelDrivenResearchIssue::TavilyFailure(TavilyFailure::InvalidResult),
            ),
            AdmissionError::SequenceExhausted => {
                Self::Execution(ModelDrivenResearchExecutionError::SequenceExhausted)
            }
            AdmissionError::InvalidGeneratedEntity | AdmissionError::InvalidResearchState(_) => {
                Self::Execution(ModelDrivenResearchExecutionError::GeneratedDomainValue)
            }
        }
    }
}

enum VerificationFailure {
    Domain(ModelDrivenResearchIssue),
    Execution(ModelDrivenResearchExecutionError),
}

fn next(sequence: u64) -> Result<u64, ModelDrivenResearchExecutionError> {
    sequence
        .checked_add(1)
        .ok_or(ModelDrivenResearchExecutionError::SequenceExhausted)
}

#[cfg(test)]
mod tests;
