use std::{future::Future, pin::Pin, time::Duration};

use aurora_core::ModelRequestFailure;
use aurora_openai::{
    OpenAiBackend, StructuredOutputInvocation, StructuredOutputRequest,
    StructuredOutputValidationError,
};
use aurora_research::{
    GroundedReport, ResearchControlState, SynthesisBasis, SynthesisDraft, SynthesisValidationError,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    synthesis_context::{SynthesisContextError, synthesis_context},
    synthesis_proposal::{SynthesisProposalError, decode_synthesis, synthesis_schema},
};

const MODEL_CALL_LIMIT: Duration = Duration::from_secs(60);
const SYNTHESIS_INSTRUCTIONS: &str = "Write a concise research report from the supplied research basis. Do not introduce new facts. Use ordered sections. Do not write section headings. Assertions are the only substantive units. Each assertion must contain one factual unit. Every assertion must cite one or more claim identifiers from the supplied basis. Return only the required structured proposal.";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ModelBackedSynthesisError {
    #[error("research is not terminal")]
    ResearchNotTerminal,
    #[error("failed research cannot be synthesized")]
    FailedResearch,
    #[error("research has no reportable assessed claims")]
    NoReportableClaims,
    #[error("synthesis model input exceeds the context limit")]
    ModelInputTooLarge,
    #[error("synthesis model provider request failed: {0:?}")]
    ProviderFailure(ModelRequestFailure),
    #[error("synthesis model call timed out")]
    ModelTimeout,
    #[error("synthesis was cancelled")]
    Cancelled,
    #[error("synthesis model output was malformed")]
    MalformedModelOutput,
    #[error("synthesis adapter request exceeds its limit")]
    ModelRequestTooLarge,
    #[error("synthesis adapter response exceeds its limit")]
    ModelOutputTooLarge,
    #[error("synthesis draft is invalid for the research domain: {0}")]
    InvalidReport(SynthesisValidationError),
}

pub struct OpenAiResearchSynthesizer {
    model: OpenAiBackend,
}

impl OpenAiResearchSynthesizer {
    pub const fn new(model: OpenAiBackend) -> Self {
        Self { model }
    }

    pub async fn synthesize(
        &mut self,
        state: &ResearchControlState,
        cancellation: CancellationToken,
    ) -> Result<GroundedReport, ModelBackedSynthesisError> {
        synthesize(&mut self.model, state, cancellation).await
    }
}

pub(super) struct SynthesisModelRequest {
    pub(super) name: String,
    pub(super) instructions: String,
    pub(super) input: String,
    pub(super) schema: Value,
}

impl SynthesisModelRequest {
    fn new(input: String) -> Result<Self, ModelBackedSynthesisError> {
        let name = "research_synthesis".to_owned();
        let instructions = SYNTHESIS_INSTRUCTIONS.to_owned();
        let schema = synthesis_schema();
        StructuredOutputRequest::new(
            name.clone(),
            instructions.clone(),
            input.clone(),
            schema.clone(),
        )
        .map_err(map_request_error)?;
        Ok(Self {
            name,
            instructions,
            input,
            schema,
        })
    }
}

type SynthesisFuture = Pin<Box<dyn Future<Output = StructuredOutputInvocation> + Send + 'static>>;

pub(super) trait SynthesisModel {
    fn propose(
        &mut self,
        request: SynthesisModelRequest,
        cancellation: CancellationToken,
    ) -> SynthesisFuture;
}

impl SynthesisModel for OpenAiBackend {
    fn propose(
        &mut self,
        request: SynthesisModelRequest,
        cancellation: CancellationToken,
    ) -> SynthesisFuture {
        let request = StructuredOutputRequest::new(
            request.name,
            request.instructions,
            request.input,
            request.schema,
        )
        .expect("validated synthesis request remains valid for the adapter");
        self.invoke_structured(request, cancellation)
    }
}

pub(super) async fn synthesize<M>(
    model: &mut M,
    state: &ResearchControlState,
    cancellation: CancellationToken,
) -> Result<GroundedReport, ModelBackedSynthesisError>
where
    M: SynthesisModel,
{
    let (basis, request) = prepare_synthesis(
        state,
        &cancellation,
        SynthesisBasis::from_state,
        synthesis_context,
        SynthesisModelRequest::new,
    )?;

    let pending = model.propose(request, cancellation.clone());
    let waited = tokio::select! {
        biased;
        () = cancellation.cancelled() => return Err(ModelBackedSynthesisError::Cancelled),
        result = tokio::time::timeout(MODEL_CALL_LIMIT, pending) => {
            result
        }
    };
    check_cancelled(&cancellation)?;
    let invocation = waited.map_err(|_| ModelBackedSynthesisError::ModelTimeout)?;
    let proposal = match invocation {
        StructuredOutputInvocation::Output(value) => value,
        StructuredOutputInvocation::RequestFailure(failure) => {
            return Err(ModelBackedSynthesisError::ProviderFailure(failure));
        }
        StructuredOutputInvocation::MalformedOutput => {
            return Err(ModelBackedSynthesisError::MalformedModelOutput);
        }
        StructuredOutputInvocation::ResponseTooLarge => {
            return Err(ModelBackedSynthesisError::ModelOutputTooLarge);
        }
        StructuredOutputInvocation::RequestTooLarge => {
            return Err(ModelBackedSynthesisError::ModelRequestTooLarge);
        }
        StructuredOutputInvocation::Cancelled => return Err(ModelBackedSynthesisError::Cancelled),
    };
    finish_synthesis(
        &basis,
        &proposal,
        &cancellation,
        decode_synthesis,
        GroundedReport::from_basis,
    )
}

fn prepare_synthesis(
    state: &ResearchControlState,
    cancellation: &CancellationToken,
    build_basis: impl FnOnce(&ResearchControlState) -> Result<SynthesisBasis, SynthesisValidationError>,
    build_context: impl FnOnce(&SynthesisBasis) -> Result<String, SynthesisContextError>,
    build_request: impl FnOnce(String) -> Result<SynthesisModelRequest, ModelBackedSynthesisError>,
) -> Result<(SynthesisBasis, SynthesisModelRequest), ModelBackedSynthesisError> {
    check_cancelled(cancellation)?;
    let basis = build_basis(state);
    check_cancelled(cancellation)?;
    let basis = basis.map_err(map_basis_error)?;

    let context = build_context(&basis);
    check_cancelled(cancellation)?;
    let context = context.map_err(|error| match error {
        SynthesisContextError::TooLarge => ModelBackedSynthesisError::ModelInputTooLarge,
    })?;

    let request = build_request(context);
    check_cancelled(cancellation)?;
    let request = request?;
    Ok((basis, request))
}

pub(super) fn finish_synthesis(
    basis: &SynthesisBasis,
    proposal: &Value,
    cancellation: &CancellationToken,
    decode: impl FnOnce(&str) -> Result<SynthesisDraft, SynthesisProposalError>,
    ground: impl FnOnce(
        &SynthesisBasis,
        SynthesisDraft,
    ) -> Result<GroundedReport, SynthesisValidationError>,
) -> Result<GroundedReport, ModelBackedSynthesisError> {
    let serialized = proposal.to_string();
    let decoded = decode(&serialized);
    check_cancelled(cancellation)?;
    let draft = decoded.map_err(map_proposal_error)?;
    let grounded = ground(basis, draft);
    check_cancelled(cancellation)?;
    let report = grounded.map_err(ModelBackedSynthesisError::InvalidReport)?;
    check_cancelled(cancellation)?;
    Ok(report)
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), ModelBackedSynthesisError> {
    if cancellation.is_cancelled() {
        Err(ModelBackedSynthesisError::Cancelled)
    } else {
        Ok(())
    }
}

fn map_basis_error(error: SynthesisValidationError) -> ModelBackedSynthesisError {
    match error {
        SynthesisValidationError::ResearchNotTerminal => {
            ModelBackedSynthesisError::ResearchNotTerminal
        }
        SynthesisValidationError::FailedResearch => ModelBackedSynthesisError::FailedResearch,
        SynthesisValidationError::NoReportableClaims => {
            ModelBackedSynthesisError::NoReportableClaims
        }
        other => ModelBackedSynthesisError::InvalidReport(other),
    }
}

fn map_request_error(error: StructuredOutputValidationError) -> ModelBackedSynthesisError {
    match error {
        StructuredOutputValidationError::RequestTooLarge => {
            ModelBackedSynthesisError::ModelRequestTooLarge
        }
        StructuredOutputValidationError::BlankName
        | StructuredOutputValidationError::BlankInstructions
        | StructuredOutputValidationError::BlankInput
        | StructuredOutputValidationError::SchemaMustBeObject => {
            unreachable!("synthesis constructs a nonblank request with an object schema")
        }
    }
}

fn map_proposal_error(error: SynthesisProposalError) -> ModelBackedSynthesisError {
    match error {
        SynthesisProposalError::InvalidReport(error) => {
            ModelBackedSynthesisError::InvalidReport(error)
        }
        SynthesisProposalError::InvalidJson
        | SynthesisProposalError::InvalidShape
        | SynthesisProposalError::BlankAssertion => ModelBackedSynthesisError::MalformedModelOutput,
    }
}

#[cfg(test)]
mod tests;
