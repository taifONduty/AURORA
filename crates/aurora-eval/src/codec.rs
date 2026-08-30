use serde::{Deserialize, Serialize};

use crate::{
    EvaluationCase, EvaluationCaseError, EvaluationCaseId, EvaluationLabelId, EvidenceKey,
    ExpectedEvidenceRelation, ExpectedRelation, ExpectedSufficiency, ExpectedTerminalOutcome,
    SourceSnapshotFixture, VerificationExpectation,
};

const EVALUATION_CASE_SCHEMA_VERSION: u32 = 1;
const CASE_RESULT_SCHEMA_VERSION: u32 = 1;
const EVALUATION_REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EvaluationCodecError {
    #[error("evaluation JSON is malformed")]
    MalformedJson,
    #[error("evaluation schema version is unsupported")]
    UnsupportedSchemaVersion,
    #[error("evaluation value is invalid")]
    InvalidValue,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseWire {
    schema_version: u32,
    id: String,
    question: String,
    source_snapshots: Vec<SourceSnapshotWire>,
    verification_expectations: Vec<VerificationExpectationWire>,
    expected_terminal: Option<TerminalWire>,
    expected_follow_up_tasks: Option<u32>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceSnapshotWire {
    source_id: String,
    content: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VerificationExpectationWire {
    id: String,
    sufficiency: SufficiencyWire,
    relations: Vec<EvidenceRelationWire>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceRelationWire {
    evidence_key: String,
    relation: RelationWire,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RelationWire {
    Supports,
    Contradicts,
    Unclear,
    Irrelevant,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SufficiencyWire {
    Sufficient,
    Insufficient,
    Indeterminate,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalWire {
    Completed,
    Failed,
    OperatorStopped,
    BudgetExhausted,
    Blocked,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseResultEnvelope {
    schema_version: u32,
    result: crate::CaseEvaluationResult,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportEnvelope {
    schema_version: u32,
    cases: Vec<crate::CaseEvaluationResult>,
    aggregate: crate::AggregateEvaluation,
}

pub fn encode_case(case: &EvaluationCase) -> Result<Vec<u8>, EvaluationCodecError> {
    serde_json::to_vec(&CaseWire::from(case)).map_err(|_| EvaluationCodecError::InvalidValue)
}

pub fn decode_case(bytes: &[u8]) -> Result<EvaluationCase, EvaluationCodecError> {
    let wire: CaseWire =
        serde_json::from_slice(bytes).map_err(|_| EvaluationCodecError::MalformedJson)?;
    if wire.schema_version != EVALUATION_CASE_SCHEMA_VERSION {
        return Err(EvaluationCodecError::UnsupportedSchemaVersion);
    }
    EvaluationCase::try_from(wire).map_err(|_| EvaluationCodecError::InvalidValue)
}

pub fn encode_case_result(
    result: &crate::CaseEvaluationResult,
) -> Result<Vec<u8>, EvaluationCodecError> {
    if !result.is_valid() {
        return Err(EvaluationCodecError::InvalidValue);
    }
    serde_json::to_vec(&CaseResultEnvelope {
        schema_version: CASE_RESULT_SCHEMA_VERSION,
        result: result.clone(),
    })
    .map_err(|_| EvaluationCodecError::InvalidValue)
}

pub fn decode_case_result(
    bytes: &[u8],
) -> Result<crate::CaseEvaluationResult, EvaluationCodecError> {
    let envelope: CaseResultEnvelope =
        serde_json::from_slice(bytes).map_err(|_| EvaluationCodecError::MalformedJson)?;
    if envelope.schema_version != CASE_RESULT_SCHEMA_VERSION {
        return Err(EvaluationCodecError::UnsupportedSchemaVersion);
    }
    if !envelope.result.is_valid() {
        return Err(EvaluationCodecError::InvalidValue);
    }
    Ok(envelope.result)
}

pub fn encode_report(report: &crate::EvaluationReport) -> Result<Vec<u8>, EvaluationCodecError> {
    if !report
        .cases()
        .iter()
        .all(crate::CaseEvaluationResult::is_valid)
        || !report.aggregate().is_valid()
    {
        return Err(EvaluationCodecError::InvalidValue);
    }
    serde_json::to_vec(&ReportEnvelope {
        schema_version: EVALUATION_REPORT_SCHEMA_VERSION,
        cases: report.cases().to_vec(),
        aggregate: report.aggregate().clone(),
    })
    .map_err(|_| EvaluationCodecError::InvalidValue)
}

pub fn decode_report(bytes: &[u8]) -> Result<crate::EvaluationReport, EvaluationCodecError> {
    let envelope: ReportEnvelope =
        serde_json::from_slice(bytes).map_err(|_| EvaluationCodecError::MalformedJson)?;
    if envelope.schema_version != EVALUATION_REPORT_SCHEMA_VERSION {
        return Err(EvaluationCodecError::UnsupportedSchemaVersion);
    }
    if !envelope
        .cases
        .iter()
        .all(crate::CaseEvaluationResult::is_valid)
        || !envelope.aggregate.is_valid()
    {
        return Err(EvaluationCodecError::InvalidValue);
    }
    let report = crate::EvaluationReport::new(envelope.cases)
        .map_err(|_| EvaluationCodecError::InvalidValue)?;
    if report.aggregate() != &envelope.aggregate {
        return Err(EvaluationCodecError::InvalidValue);
    }
    Ok(report)
}

impl From<&EvaluationCase> for CaseWire {
    fn from(case: &EvaluationCase) -> Self {
        Self {
            schema_version: EVALUATION_CASE_SCHEMA_VERSION,
            id: case.id().as_str().to_owned(),
            question: case.question().to_owned(),
            source_snapshots: case
                .source_snapshots()
                .iter()
                .map(|snapshot| SourceSnapshotWire {
                    source_id: snapshot.source_id().to_string(),
                    content: snapshot.content().to_owned(),
                })
                .collect(),
            verification_expectations: case
                .verification_expectations()
                .iter()
                .map(VerificationExpectationWire::from)
                .collect(),
            expected_terminal: case.expected_terminal().map(TerminalWire::from),
            expected_follow_up_tasks: case.expected_follow_up_tasks(),
        }
    }
}

impl TryFrom<CaseWire> for EvaluationCase {
    type Error = EvaluationCaseError;

    fn try_from(wire: CaseWire) -> Result<Self, Self::Error> {
        let snapshots = wire
            .source_snapshots
            .into_iter()
            .map(|snapshot| SourceSnapshotFixture::new(snapshot.source_id, snapshot.content))
            .collect::<Result<Vec<_>, _>>()?;
        let expectations = wire
            .verification_expectations
            .into_iter()
            .map(VerificationExpectation::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(
            EvaluationCaseId::new(wire.id)?,
            wire.question,
            snapshots,
            expectations,
            wire.expected_terminal.map(ExpectedTerminalOutcome::from),
            wire.expected_follow_up_tasks,
        )
    }
}

impl From<&VerificationExpectation> for VerificationExpectationWire {
    fn from(expectation: &VerificationExpectation) -> Self {
        Self {
            id: expectation.id().as_str().to_owned(),
            sufficiency: SufficiencyWire::from(expectation.sufficiency()),
            relations: expectation
                .relations()
                .iter()
                .map(|relation| EvidenceRelationWire {
                    evidence_key: relation.evidence_key().as_str().to_owned(),
                    relation: RelationWire::from(relation.relation()),
                })
                .collect(),
        }
    }
}

impl TryFrom<VerificationExpectationWire> for VerificationExpectation {
    type Error = EvaluationCaseError;

    fn try_from(wire: VerificationExpectationWire) -> Result<Self, Self::Error> {
        let relations = wire
            .relations
            .into_iter()
            .map(|relation| {
                Ok(ExpectedEvidenceRelation::new(
                    EvidenceKey::new(relation.evidence_key)?,
                    ExpectedRelation::from(relation.relation),
                ))
            })
            .collect::<Result<Vec<_>, EvaluationCaseError>>()?;
        Self::new(
            EvaluationLabelId::new(wire.id)?,
            ExpectedSufficiency::from(wire.sufficiency),
            relations,
        )
    }
}

impl From<ExpectedRelation> for RelationWire {
    fn from(value: ExpectedRelation) -> Self {
        match value {
            ExpectedRelation::Supports => Self::Supports,
            ExpectedRelation::Contradicts => Self::Contradicts,
            ExpectedRelation::Unclear => Self::Unclear,
            ExpectedRelation::Irrelevant => Self::Irrelevant,
        }
    }
}

impl From<RelationWire> for ExpectedRelation {
    fn from(value: RelationWire) -> Self {
        match value {
            RelationWire::Supports => Self::Supports,
            RelationWire::Contradicts => Self::Contradicts,
            RelationWire::Unclear => Self::Unclear,
            RelationWire::Irrelevant => Self::Irrelevant,
        }
    }
}

impl From<ExpectedSufficiency> for SufficiencyWire {
    fn from(value: ExpectedSufficiency) -> Self {
        match value {
            ExpectedSufficiency::Sufficient => Self::Sufficient,
            ExpectedSufficiency::Insufficient => Self::Insufficient,
            ExpectedSufficiency::Indeterminate => Self::Indeterminate,
        }
    }
}

impl From<SufficiencyWire> for ExpectedSufficiency {
    fn from(value: SufficiencyWire) -> Self {
        match value {
            SufficiencyWire::Sufficient => Self::Sufficient,
            SufficiencyWire::Insufficient => Self::Insufficient,
            SufficiencyWire::Indeterminate => Self::Indeterminate,
        }
    }
}

impl From<ExpectedTerminalOutcome> for TerminalWire {
    fn from(value: ExpectedTerminalOutcome) -> Self {
        match value {
            ExpectedTerminalOutcome::Completed => Self::Completed,
            ExpectedTerminalOutcome::Failed => Self::Failed,
            ExpectedTerminalOutcome::OperatorStopped => Self::OperatorStopped,
            ExpectedTerminalOutcome::BudgetExhausted => Self::BudgetExhausted,
            ExpectedTerminalOutcome::Blocked => Self::Blocked,
        }
    }
}

impl From<TerminalWire> for ExpectedTerminalOutcome {
    fn from(value: TerminalWire) -> Self {
        match value {
            TerminalWire::Completed => Self::Completed,
            TerminalWire::Failed => Self::Failed,
            TerminalWire::OperatorStopped => Self::OperatorStopped,
            TerminalWire::BudgetExhausted => Self::BudgetExhausted,
            TerminalWire::Blocked => Self::Blocked,
        }
    }
}
