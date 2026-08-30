use std::collections::BTreeSet;

use aurora_research::{
    ClaimPresentation, EvidenceId, GroundedReport, ResearchControlRecord, VerificationId,
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

const METADATA_IDENTIFIER_LIMIT: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EvaluationObservationError {
    #[error("evaluation metadata identifier is blank")]
    BlankMetadataIdentifier,
    #[error("evaluation metadata identifier exceeds 256 UTF-8 bytes")]
    MetadataIdentifierTooLong,
    #[error("evaluation timestamp is not an RFC 3339 UTC value")]
    InvalidEvaluatedAt,
    #[error("verification identifier is invalid")]
    InvalidVerificationId,
    #[error("evidence identifier is invalid")]
    InvalidEvidenceId,
    #[error("verification binding has no evidence bindings")]
    EmptyEvidenceBindings,
    #[error("verification binding repeats evidence key {0}")]
    DuplicateEvidenceBinding(String),
    #[error("evaluation run repeats verification binding {0}")]
    DuplicateVerificationBinding(String),
    #[error("evaluation run repeats semantic adjudication location")]
    DuplicateSemanticAdjudication,
    #[error("evaluation run repeats execution failure")]
    DuplicateExecutionFailure,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluationMetadata {
    aurora_revision: String,
    suite_id: String,
    configuration_id: String,
    evaluated_at: String,
    case_id: Option<crate::EvaluationCaseId>,
    model: Option<ModelConfiguration>,
    retrieval: Option<RetrievalConfiguration>,
    follow_up_limit: Option<u32>,
    repeated_run_seed: Option<u64>,
}

impl EvaluationMetadata {
    pub fn new(
        aurora_revision: String,
        suite_id: String,
        configuration_id: String,
        evaluated_at: String,
    ) -> Result<Self, EvaluationObservationError> {
        for value in [&aurora_revision, &suite_id, &configuration_id] {
            validate_metadata_identifier(value)?;
        }
        let parsed = OffsetDateTime::parse(&evaluated_at, &Rfc3339)
            .map_err(|_| EvaluationObservationError::InvalidEvaluatedAt)?;
        if parsed.offset() != UtcOffset::UTC {
            return Err(EvaluationObservationError::InvalidEvaluatedAt);
        }
        Ok(Self {
            aurora_revision,
            suite_id,
            configuration_id,
            evaluated_at,
            case_id: None,
            model: None,
            retrieval: None,
            follow_up_limit: None,
            repeated_run_seed: None,
        })
    }

    pub fn aurora_revision(&self) -> &str {
        &self.aurora_revision
    }

    pub fn suite_id(&self) -> &str {
        &self.suite_id
    }

    pub fn configuration_id(&self) -> &str {
        &self.configuration_id
    }

    pub fn evaluated_at(&self) -> &str {
        &self.evaluated_at
    }

    pub fn with_model(mut self, model: ModelConfiguration) -> Self {
        self.model = Some(model);
        self
    }

    pub fn with_case_id(mut self, case_id: crate::EvaluationCaseId) -> Self {
        self.case_id = Some(case_id);
        self
    }

    pub fn with_retrieval(mut self, retrieval: RetrievalConfiguration) -> Self {
        self.retrieval = Some(retrieval);
        self
    }

    pub const fn with_follow_up_limit(mut self, limit: u32) -> Self {
        self.follow_up_limit = Some(limit);
        self
    }

    pub const fn with_repeated_run_seed(mut self, seed: u64) -> Self {
        self.repeated_run_seed = Some(seed);
        self
    }

    pub const fn model(&self) -> Option<&ModelConfiguration> {
        self.model.as_ref()
    }
    pub const fn case_id(&self) -> Option<&crate::EvaluationCaseId> {
        self.case_id.as_ref()
    }
    pub const fn retrieval(&self) -> Option<&RetrievalConfiguration> {
        self.retrieval.as_ref()
    }
    pub const fn follow_up_limit(&self) -> Option<u32> {
        self.follow_up_limit
    }
    pub const fn repeated_run_seed(&self) -> Option<u64> {
        self.repeated_run_seed
    }

    pub(crate) fn is_valid(&self) -> bool {
        Self::new(
            self.aurora_revision.clone(),
            self.suite_id.clone(),
            self.configuration_id.clone(),
            self.evaluated_at.clone(),
        )
        .is_ok()
            && self.model.as_ref().is_none_or(ModelConfiguration::is_valid)
            && self
                .retrieval
                .as_ref()
                .is_none_or(RetrievalConfiguration::is_valid)
    }

    pub(crate) fn conflicts_with(
        &self,
        case_id: &crate::EvaluationCaseId,
        follow_up_limit: Option<u32>,
    ) -> bool {
        self.case_id.as_ref().is_some_and(|value| value != case_id)
            || self
                .follow_up_limit
                .is_some_and(|value| Some(value) != follow_up_limit)
    }

    pub(crate) fn bind(
        mut self,
        case_id: crate::EvaluationCaseId,
        follow_up_limit: Option<u32>,
    ) -> Self {
        self.case_id = Some(case_id);
        self.follow_up_limit = follow_up_limit;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfiguration {
    provider_id: String,
    model_id: String,
    configuration_id: String,
}

impl ModelConfiguration {
    pub fn new(
        provider_id: String,
        model_id: String,
        configuration_id: String,
    ) -> Result<Self, EvaluationObservationError> {
        for value in [&provider_id, &model_id, &configuration_id] {
            validate_metadata_identifier(value)?;
        }
        Ok(Self {
            provider_id,
            model_id,
            configuration_id,
        })
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }
    pub fn model_id(&self) -> &str {
        &self.model_id
    }
    pub fn configuration_id(&self) -> &str {
        &self.configuration_id
    }

    fn is_valid(&self) -> bool {
        Self::new(
            self.provider_id.clone(),
            self.model_id.clone(),
            self.configuration_id.clone(),
        )
        .is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetrievalConfiguration {
    provider_id: String,
    configuration_id: String,
}

impl RetrievalConfiguration {
    pub fn new(
        provider_id: String,
        configuration_id: String,
    ) -> Result<Self, EvaluationObservationError> {
        for value in [&provider_id, &configuration_id] {
            validate_metadata_identifier(value)?;
        }
        Ok(Self {
            provider_id,
            configuration_id,
        })
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }
    pub fn configuration_id(&self) -> &str {
        &self.configuration_id
    }

    fn is_valid(&self) -> bool {
        Self::new(self.provider_id.clone(), self.configuration_id.clone()).is_ok()
    }
}

fn validate_metadata_identifier(value: &str) -> Result<(), EvaluationObservationError> {
    if value.trim().is_empty() {
        return Err(EvaluationObservationError::BlankMetadataIdentifier);
    }
    if value.len() > METADATA_IDENTIFIER_LIMIT {
        return Err(EvaluationObservationError::MetadataIdentifierTooLong);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluationRun {
    records: Vec<ResearchControlRecord>,
    metadata: EvaluationMetadata,
    verification_bindings: Vec<VerificationBinding>,
    synthesis: Option<SynthesisObservation>,
    semantic_adjudications: Vec<SemanticAdjudication>,
    usage: Option<ObservedUsage>,
    failures: Vec<ExecutionFailure>,
}

impl EvaluationRun {
    pub const fn new(records: Vec<ResearchControlRecord>, metadata: EvaluationMetadata) -> Self {
        Self {
            records,
            metadata,
            verification_bindings: Vec::new(),
            synthesis: None,
            semantic_adjudications: Vec::new(),
            usage: None,
            failures: Vec::new(),
        }
    }

    pub fn records(&self) -> &[ResearchControlRecord] {
        &self.records
    }

    pub const fn metadata(&self) -> &EvaluationMetadata {
        &self.metadata
    }

    pub fn with_verification_binding(
        mut self,
        binding: VerificationBinding,
    ) -> Result<Self, EvaluationObservationError> {
        if self
            .verification_bindings
            .iter()
            .any(|candidate| candidate.expectation_id() == binding.expectation_id())
        {
            return Err(EvaluationObservationError::DuplicateVerificationBinding(
                binding.expectation_id().as_str().to_owned(),
            ));
        }
        self.verification_bindings.push(binding);
        Ok(self)
    }

    pub fn verification_bindings(&self) -> &[VerificationBinding] {
        &self.verification_bindings
    }

    pub fn with_synthesis(mut self, synthesis: SynthesisObservation) -> Self {
        self.synthesis = Some(synthesis);
        self
    }

    pub fn with_semantic_adjudication(
        mut self,
        adjudication: SemanticAdjudication,
    ) -> Result<Self, EvaluationObservationError> {
        if self.semantic_adjudications.iter().any(|candidate| {
            candidate.location() == adjudication.location()
                && same_adjudication_origin(candidate.origin(), adjudication.origin())
        }) {
            return Err(EvaluationObservationError::DuplicateSemanticAdjudication);
        }
        self.semantic_adjudications.push(adjudication);
        Ok(self)
    }

    pub const fn synthesis(&self) -> Option<&SynthesisObservation> {
        self.synthesis.as_ref()
    }

    pub fn semantic_adjudications(&self) -> &[SemanticAdjudication] {
        &self.semantic_adjudications
    }

    pub fn with_usage(mut self, usage: ObservedUsage) -> Self {
        self.usage = Some(usage);
        self
    }

    pub fn with_failure(
        mut self,
        failure: ExecutionFailure,
    ) -> Result<Self, EvaluationObservationError> {
        if self.failures.contains(&failure) {
            return Err(EvaluationObservationError::DuplicateExecutionFailure);
        }
        self.failures.push(failure);
        Ok(self)
    }

    pub const fn usage(&self) -> Option<&ObservedUsage> {
        self.usage.as_ref()
    }

    pub fn failures(&self) -> &[ExecutionFailure] {
        &self.failures
    }
}

fn same_adjudication_origin(left: &AdjudicationOrigin, right: &AdjudicationOrigin) -> bool {
    matches!(
        (left, right),
        (
            AdjudicationOrigin::LabelledFixture,
            AdjudicationOrigin::LabelledFixture
        ) | (
            AdjudicationOrigin::ModelJudge(_),
            AdjudicationOrigin::ModelJudge(_)
        )
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCost {
    currency: String,
    micros: u64,
}

impl ProviderCost {
    pub fn new(currency: String, micros: u64) -> Result<Self, EvaluationObservationError> {
        validate_metadata_identifier(&currency)?;
        Ok(Self { currency, micros })
    }

    pub fn currency(&self) -> &str {
        &self.currency
    }

    pub const fn micros(&self) -> u64 {
        self.micros
    }

    pub(crate) fn is_valid(&self) -> bool {
        Self::new(self.currency.clone(), self.micros).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedUsage {
    model_invocations: Option<u64>,
    retrieval_calls: Option<u64>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    wall_clock_millis: Option<u64>,
    provider_cost: Option<ProviderCost>,
}

impl ObservedUsage {
    pub const fn new(
        model_invocations: Option<u64>,
        retrieval_calls: Option<u64>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        wall_clock_millis: Option<u64>,
        provider_cost: Option<ProviderCost>,
    ) -> Self {
        Self {
            model_invocations,
            retrieval_calls,
            input_tokens,
            output_tokens,
            wall_clock_millis,
            provider_cost,
        }
    }

    pub const fn model_invocations(&self) -> Option<u64> {
        self.model_invocations
    }

    pub const fn retrieval_calls(&self) -> Option<u64> {
        self.retrieval_calls
    }

    pub const fn input_tokens(&self) -> Option<u64> {
        self.input_tokens
    }

    pub const fn output_tokens(&self) -> Option<u64> {
        self.output_tokens
    }

    pub const fn wall_clock_millis(&self) -> Option<u64> {
        self.wall_clock_millis
    }

    pub const fn provider_cost(&self) -> Option<&ProviderCost> {
        self.provider_cost.as_ref()
    }

    pub(crate) fn is_valid(&self) -> bool {
        self.provider_cost
            .as_ref()
            .is_none_or(ProviderCost::is_valid)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionFailure {
    Provider,
    Retrieval,
    MalformedModelProposal,
    DomainInvalidProposal,
    ResearchExecution,
    Synthesis,
    BenchmarkMapping,
    Scoring,
    InvalidResearchHistory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceBinding {
    evidence_key: crate::EvidenceKey,
    evidence_id: EvidenceId,
}

impl EvidenceBinding {
    pub fn new(
        evidence_key: crate::EvidenceKey,
        evidence_id: impl AsRef<str>,
    ) -> Result<Self, EvaluationObservationError> {
        let evidence_id = evidence_id
            .as_ref()
            .parse()
            .map_err(|_| EvaluationObservationError::InvalidEvidenceId)?;
        Ok(Self {
            evidence_key,
            evidence_id,
        })
    }

    pub const fn evidence_key(&self) -> &crate::EvidenceKey {
        &self.evidence_key
    }

    pub const fn evidence_id(&self) -> &EvidenceId {
        &self.evidence_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationBinding {
    expectation_id: crate::EvaluationLabelId,
    verification_id: VerificationId,
    evidence: Vec<EvidenceBinding>,
}

impl VerificationBinding {
    pub fn new(
        expectation_id: crate::EvaluationLabelId,
        verification_id: impl AsRef<str>,
        evidence: Vec<EvidenceBinding>,
    ) -> Result<Self, EvaluationObservationError> {
        if evidence.is_empty() {
            return Err(EvaluationObservationError::EmptyEvidenceBindings);
        }
        let verification_id = verification_id
            .as_ref()
            .parse()
            .map_err(|_| EvaluationObservationError::InvalidVerificationId)?;
        let mut keys = BTreeSet::new();
        for binding in &evidence {
            if !keys.insert(binding.evidence_key().clone()) {
                return Err(EvaluationObservationError::DuplicateEvidenceBinding(
                    binding.evidence_key().as_str().to_owned(),
                ));
            }
        }
        Ok(Self {
            expectation_id,
            verification_id,
            evidence,
        })
    }

    pub const fn expectation_id(&self) -> &crate::EvaluationLabelId {
        &self.expectation_id
    }

    pub const fn verification_id(&self) -> &VerificationId {
        &self.verification_id
    }

    pub fn evidence(&self) -> &[EvidenceBinding] {
        &self.evidence
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservedPresentation {
    Established,
    Unresolved,
    Contested,
}

impl From<ClaimPresentation> for ObservedPresentation {
    fn from(value: ClaimPresentation) -> Self {
        match value {
            ClaimPresentation::Established => Self::Established,
            ClaimPresentation::Unresolved => Self::Unresolved,
            ClaimPresentation::Contested => Self::Contested,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedCitation {
    claim_id: String,
    evidence_id: String,
    source_id: String,
    source_digest: String,
}

impl ObservedCitation {
    pub const fn new(
        claim_id: String,
        evidence_id: String,
        source_id: String,
        source_digest: String,
    ) -> Self {
        Self {
            claim_id,
            evidence_id,
            source_id,
            source_digest,
        }
    }

    pub fn claim_id(&self) -> &str {
        &self.claim_id
    }

    pub fn evidence_id(&self) -> &str {
        &self.evidence_id
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn source_digest(&self) -> &str {
        &self.source_digest
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedAssertion {
    text: String,
    claim_ids: Vec<String>,
    presentation: ObservedPresentation,
    citations: Vec<ObservedCitation>,
}

impl ObservedAssertion {
    pub const fn new(
        text: String,
        claim_ids: Vec<String>,
        presentation: ObservedPresentation,
        citations: Vec<ObservedCitation>,
    ) -> Self {
        Self {
            text,
            claim_ids,
            presentation,
            citations,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn claim_ids(&self) -> &[String] {
        &self.claim_ids
    }

    pub const fn presentation(&self) -> ObservedPresentation {
        self.presentation
    }

    pub fn citations(&self) -> &[ObservedCitation] {
        &self.citations
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedSection {
    assertions: Vec<ObservedAssertion>,
}

impl ObservedSection {
    pub const fn new(assertions: Vec<ObservedAssertion>) -> Self {
        Self { assertions }
    }

    pub fn assertions(&self) -> &[ObservedAssertion] {
        &self.assertions
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SynthesisObservation {
    sections: Vec<ObservedSection>,
    deterministic_rendering: Option<bool>,
}

impl SynthesisObservation {
    pub const fn new(sections: Vec<ObservedSection>) -> Self {
        Self {
            sections,
            deterministic_rendering: None,
        }
    }

    pub fn from_report(report: &GroundedReport) -> Self {
        let first_render = report.render();
        let deterministic_rendering = Some(first_render == report.render());
        let sections = report
            .sections()
            .map(|section| {
                ObservedSection::new(
                    section
                        .assertions()
                        .map(|assertion| {
                            ObservedAssertion::new(
                                assertion.text().to_owned(),
                                assertion.claim_ids().map(ToString::to_string).collect(),
                                assertion.presentation().into(),
                                assertion
                                    .citations()
                                    .map(|citation| {
                                        ObservedCitation::new(
                                            citation.claim_id().to_string(),
                                            citation.evidence().id().to_string(),
                                            citation.source().id().to_string(),
                                            digest_hex(citation.source().content_digest()),
                                        )
                                    })
                                    .collect(),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        Self {
            sections,
            deterministic_rendering,
        }
    }

    pub fn sections(&self) -> &[ObservedSection] {
        &self.sections
    }

    pub const fn deterministic_rendering(&self) -> Option<bool> {
        self.deterministic_rendering
    }
}

fn digest_hex(digest: &aurora_research::ContentDigest) -> String {
    digest
        .as_sha256()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AssertionLocation {
    section_index: u32,
    assertion_index: u32,
}

impl AssertionLocation {
    pub const fn new(section_index: u32, assertion_index: u32) -> Self {
        Self {
            section_index,
            assertion_index,
        }
    }

    pub const fn section_index(&self) -> u32 {
        self.section_index
    }

    pub const fn assertion_index(&self) -> u32 {
        self.assertion_index
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticGrounding {
    Faithful,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JudgeMetadata {
    provider_id: String,
    model_id: String,
    prompt_version: String,
    configuration_id: String,
}

impl JudgeMetadata {
    pub fn new(
        provider_id: String,
        model_id: String,
        prompt_version: String,
        configuration_id: String,
    ) -> Result<Self, EvaluationObservationError> {
        for value in [&provider_id, &model_id, &prompt_version, &configuration_id] {
            validate_metadata_identifier(value)?;
        }
        Ok(Self {
            provider_id,
            model_id,
            prompt_version,
            configuration_id,
        })
    }

    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub fn prompt_version(&self) -> &str {
        &self.prompt_version
    }

    pub fn configuration_id(&self) -> &str {
        &self.configuration_id
    }

    pub(crate) fn is_valid(&self) -> bool {
        Self::new(
            self.provider_id.clone(),
            self.model_id.clone(),
            self.prompt_version.clone(),
            self.configuration_id.clone(),
        )
        .is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdjudicationOrigin {
    LabelledFixture,
    ModelJudge(JudgeMetadata),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticAdjudication {
    location: AssertionLocation,
    grounding: SemanticGrounding,
    origin: AdjudicationOrigin,
}

impl SemanticAdjudication {
    pub const fn new(
        location: AssertionLocation,
        grounding: SemanticGrounding,
        origin: AdjudicationOrigin,
    ) -> Self {
        Self {
            location,
            grounding,
            origin,
        }
    }

    pub const fn location(&self) -> &AssertionLocation {
        &self.location
    }

    pub const fn grounding(&self) -> SemanticGrounding {
        self.grounding
    }

    pub const fn origin(&self) -> &AdjudicationOrigin {
        &self.origin
    }
}
