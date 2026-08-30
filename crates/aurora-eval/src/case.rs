use std::collections::BTreeSet;

use aurora_research::SourceId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const IDENTIFIER_LIMIT: usize = 128;
const QUESTION_LIMIT: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EvaluationCaseError {
    #[error("evaluation identifier is blank")]
    BlankIdentifier,
    #[error("evaluation identifier exceeds 128 UTF-8 bytes")]
    IdentifierTooLong,
    #[error("evaluation question is blank")]
    BlankQuestion,
    #[error("evaluation question exceeds 16384 UTF-8 bytes")]
    QuestionTooLong,
    #[error("source snapshot identifier is invalid")]
    InvalidSourceId,
    #[error("source snapshot content is empty")]
    EmptySourceSnapshot,
    #[error("evaluation case repeats source snapshot {0}")]
    DuplicateSourceSnapshot(SourceId),
    #[error("verification expectation has no evidence relations")]
    EmptyVerificationRelations,
    #[error("verification expectation repeats evidence key {0}")]
    DuplicateEvidenceKey(String),
    #[error("evaluation case repeats verification expectation {0}")]
    DuplicateVerificationExpectation(String),
}

macro_rules! evaluation_identifier {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, EvaluationCaseError> {
                let value = value.into();
                validate_identifier(&value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

evaluation_identifier!(EvaluationCaseId);
evaluation_identifier!(EvaluationLabelId);
evaluation_identifier!(EvidenceKey);

fn validate_identifier(value: &str) -> Result<(), EvaluationCaseError> {
    if value.trim().is_empty() {
        return Err(EvaluationCaseError::BlankIdentifier);
    }
    if value.len() > IDENTIFIER_LIMIT {
        return Err(EvaluationCaseError::IdentifierTooLong);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceSnapshotFixture {
    source_id: SourceId,
    content: String,
}

impl SourceSnapshotFixture {
    pub fn new(source_id: impl AsRef<str>, content: String) -> Result<Self, EvaluationCaseError> {
        let source_id = source_id
            .as_ref()
            .parse()
            .map_err(|_| EvaluationCaseError::InvalidSourceId)?;
        if content.is_empty() {
            return Err(EvaluationCaseError::EmptySourceSnapshot);
        }
        Ok(Self { source_id, content })
    }

    pub const fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    pub fn content(&self) -> &str {
        &self.content
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExpectedRelation {
    Supports,
    Contradicts,
    Unclear,
    Irrelevant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExpectedSufficiency {
    Sufficient,
    Insufficient,
    Indeterminate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExpectedTerminalOutcome {
    Completed,
    Failed,
    OperatorStopped,
    BudgetExhausted,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedEvidenceRelation {
    evidence_key: EvidenceKey,
    relation: ExpectedRelation,
}

impl ExpectedEvidenceRelation {
    pub const fn new(evidence_key: EvidenceKey, relation: ExpectedRelation) -> Self {
        Self {
            evidence_key,
            relation,
        }
    }

    pub const fn evidence_key(&self) -> &EvidenceKey {
        &self.evidence_key
    }

    pub const fn relation(&self) -> ExpectedRelation {
        self.relation
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationExpectation {
    id: EvaluationLabelId,
    sufficiency: ExpectedSufficiency,
    relations: Vec<ExpectedEvidenceRelation>,
}

impl VerificationExpectation {
    pub fn new(
        id: EvaluationLabelId,
        sufficiency: ExpectedSufficiency,
        relations: Vec<ExpectedEvidenceRelation>,
    ) -> Result<Self, EvaluationCaseError> {
        if relations.is_empty() {
            return Err(EvaluationCaseError::EmptyVerificationRelations);
        }
        let mut keys = BTreeSet::new();
        for relation in &relations {
            if !keys.insert(relation.evidence_key.clone()) {
                return Err(EvaluationCaseError::DuplicateEvidenceKey(
                    relation.evidence_key.as_str().to_owned(),
                ));
            }
        }
        Ok(Self {
            id,
            sufficiency,
            relations,
        })
    }

    pub const fn id(&self) -> &EvaluationLabelId {
        &self.id
    }

    pub const fn sufficiency(&self) -> ExpectedSufficiency {
        self.sufficiency
    }

    pub fn relations(&self) -> &[ExpectedEvidenceRelation] {
        &self.relations
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluationCase {
    id: EvaluationCaseId,
    question: String,
    source_snapshots: Vec<SourceSnapshotFixture>,
    verification_expectations: Vec<VerificationExpectation>,
    expected_terminal: Option<ExpectedTerminalOutcome>,
    expected_follow_up_tasks: Option<u32>,
}

impl EvaluationCase {
    pub fn new(
        id: EvaluationCaseId,
        question: String,
        source_snapshots: Vec<SourceSnapshotFixture>,
        verification_expectations: Vec<VerificationExpectation>,
        expected_terminal: Option<ExpectedTerminalOutcome>,
        expected_follow_up_tasks: Option<u32>,
    ) -> Result<Self, EvaluationCaseError> {
        if question.trim().is_empty() {
            return Err(EvaluationCaseError::BlankQuestion);
        }
        if question.len() > QUESTION_LIMIT {
            return Err(EvaluationCaseError::QuestionTooLong);
        }
        let mut source_ids = BTreeSet::new();
        for snapshot in &source_snapshots {
            if !source_ids.insert(*snapshot.source_id()) {
                return Err(EvaluationCaseError::DuplicateSourceSnapshot(
                    *snapshot.source_id(),
                ));
            }
        }
        let mut expectation_ids = BTreeSet::new();
        for expectation in &verification_expectations {
            if !expectation_ids.insert(expectation.id().clone()) {
                return Err(EvaluationCaseError::DuplicateVerificationExpectation(
                    expectation.id().as_str().to_owned(),
                ));
            }
        }
        Ok(Self {
            id,
            question,
            source_snapshots,
            verification_expectations,
            expected_terminal,
            expected_follow_up_tasks,
        })
    }

    pub const fn id(&self) -> &EvaluationCaseId {
        &self.id
    }

    pub fn question(&self) -> &str {
        &self.question
    }

    pub fn source_snapshots(&self) -> &[SourceSnapshotFixture] {
        &self.source_snapshots
    }

    pub fn verification_expectations(&self) -> &[VerificationExpectation] {
        &self.verification_expectations
    }

    pub const fn expected_terminal(&self) -> Option<ExpectedTerminalOutcome> {
        self.expected_terminal
    }

    pub const fn expected_follow_up_tasks(&self) -> Option<u32> {
        self.expected_follow_up_tasks
    }
}
