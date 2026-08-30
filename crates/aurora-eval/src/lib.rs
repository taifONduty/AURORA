#![forbid(unsafe_code)]

mod aggregate;
mod case;
mod classification;
mod codec;
mod evaluate;
mod observation;
mod result;

pub use aggregate::{
    AggregateEvaluation, AggregateSynthesisMetrics, AggregateUsage, CurrencyTotal,
    DistributionSummary, EvaluationReport, EvaluationReportError, FailureCount, ObservedTotal,
    TerminalCount,
};
pub use case::{
    EvaluationCase, EvaluationCaseError, EvaluationCaseId, EvaluationLabelId, EvidenceKey,
    ExpectedEvidenceRelation, ExpectedRelation, ExpectedSufficiency, ExpectedTerminalOutcome,
    SourceSnapshotFixture, VerificationExpectation,
};
pub use codec::{
    EvaluationCodecError, decode_case, decode_case_result, decode_report, encode_case,
    encode_case_result, encode_report,
};
pub use evaluate::evaluate_case;
pub use observation::{
    AdjudicationOrigin, AssertionLocation, EvaluationMetadata, EvaluationObservationError,
    EvaluationRun, EvidenceBinding, ExecutionFailure, JudgeMetadata, ModelConfiguration,
    ObservedAssertion, ObservedCitation, ObservedPresentation, ObservedSection, ObservedUsage,
    ProviderCost, RetrievalConfiguration, SemanticAdjudication, SemanticGrounding,
    SynthesisObservation, VerificationBinding,
};
pub use result::{
    AdaptiveLoopMetrics, CaseEvaluationResult, ClassMetric, DerivedRunCounts,
    EvidenceGroundingMetrics, GuaranteeAudit, MetricCount, ObservedTerminalOutcome,
    RelationMetrics, SemanticGroundingMetrics, SufficiencyMetrics, SynthesisMetrics,
    VerificationMetrics,
};
