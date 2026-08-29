#![allow(dead_code)]

use std::collections::BTreeSet;

use aurora_research::{EvidenceRelation, EvidenceSufficiency};
use serde::Deserialize;
use serde_json::{Value, json};

const INITIAL_TASK_LIMIT: usize = 3;
pub(super) const SNAPSHOT_TEXT_LIMIT: usize = 1024 * 1024;
const EXTRACTED_EVIDENCE_LIMIT: usize = 8;
const CLAIM_LIMIT: usize = 4;
pub(super) const EXCERPT_TEXT_LIMIT: usize = 16 * 1024;
pub(super) const STATEMENT_TEXT_LIMIT: usize = 4 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum ProposalError {
    #[error("proposal is not valid JSON")]
    InvalidJson,
    #[error("proposal violates its required shape")]
    InvalidShape,
    #[error("proposal contains too many initial tasks")]
    TooManyInitialTasks,
    #[error("proposal contains a blank task objective")]
    BlankTaskObjective,
    #[error("proposal repeats a task objective")]
    DuplicateTaskObjective,
    #[error("proposal contains too many evidence excerpts")]
    TooManyEvidence,
    #[error("proposal contains too many claims")]
    TooManyClaims,
    #[error("proposal contains a blank excerpt")]
    BlankExcerpt,
    #[error("proposal excerpt exceeds the text limit")]
    OversizedExcerpt,
    #[error("proposal contains a blank claim statement")]
    BlankStatement,
    #[error("proposal claim statement exceeds the text limit")]
    OversizedStatement,
    #[error("proposal claim has no evidence references")]
    ClaimHasNoEvidence,
    #[error("proposal repeats an evidence reference")]
    DuplicateEvidenceReference,
    #[error("proposal verification has no evidence relations")]
    VerificationHasNoRelations,
    #[error("proposal repeats a verification evidence reference")]
    DuplicateVerificationEvidenceReference,
    #[error("proposal contains a blank follow-up objective")]
    BlankFollowUpObjective,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct InitialPlanProposal {
    pub(super) tasks: Vec<InitialTaskProposal>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct InitialTaskProposal {
    pub(super) objective: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ExtractionProposal {
    pub(super) evidence: Vec<EvidenceProposal>,
    pub(super) claims: Vec<ClaimProposal>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct EvidenceProposal {
    pub(super) source_index: usize,
    pub(super) excerpt: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct ClaimProposal {
    pub(super) statement: String,
    pub(super) evidence_indices: Vec<usize>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct VerificationProposal {
    pub(super) relations: Vec<VerificationRelationProposal>,
    pub(super) sufficiency: ProposalSufficiency,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct VerificationRelationProposal {
    pub(super) evidence_index: usize,
    pub(super) relation: ProposalRelation,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProposalRelation {
    Supports,
    Contradicts,
    Unclear,
    Irrelevant,
}

impl From<ProposalRelation> for EvidenceRelation {
    fn from(value: ProposalRelation) -> Self {
        match value {
            ProposalRelation::Supports => Self::Supports,
            ProposalRelation::Contradicts => Self::Contradicts,
            ProposalRelation::Unclear => Self::Unclear,
            ProposalRelation::Irrelevant => Self::Irrelevant,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ProposalSufficiency {
    Sufficient,
    Insufficient,
    Indeterminate,
}

impl From<ProposalSufficiency> for EvidenceSufficiency {
    fn from(value: ProposalSufficiency) -> Self {
        match value {
            ProposalSufficiency::Sufficient => Self::Sufficient,
            ProposalSufficiency::Insufficient => Self::Insufficient,
            ProposalSufficiency::Indeterminate => Self::Indeterminate,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct FollowUpProposal {
    pub(super) objective: Option<String>,
}

pub(super) fn decode_initial_plan(input: &str) -> Result<InitialPlanProposal, ProposalError> {
    let proposal: InitialPlanProposal = decode(input)?;
    if proposal.tasks.len() > INITIAL_TASK_LIMIT {
        return Err(ProposalError::TooManyInitialTasks);
    }
    let mut objectives = BTreeSet::new();
    for task in &proposal.tasks {
        if task.objective.trim().is_empty() {
            return Err(ProposalError::BlankTaskObjective);
        }
        if !objectives.insert(&task.objective) {
            return Err(ProposalError::DuplicateTaskObjective);
        }
    }
    Ok(proposal)
}

pub(super) fn decode_extraction(input: &str) -> Result<ExtractionProposal, ProposalError> {
    let proposal: ExtractionProposal = decode(input)?;
    if proposal.evidence.len() > EXTRACTED_EVIDENCE_LIMIT {
        return Err(ProposalError::TooManyEvidence);
    }
    if proposal.claims.len() > CLAIM_LIMIT {
        return Err(ProposalError::TooManyClaims);
    }
    for evidence in &proposal.evidence {
        validate_excerpt(&evidence.excerpt)?;
    }
    for claim in &proposal.claims {
        validate_statement(&claim.statement)?;
        if claim.evidence_indices.is_empty() {
            return Err(ProposalError::ClaimHasNoEvidence);
        }
        let mut indexes = BTreeSet::new();
        if claim
            .evidence_indices
            .iter()
            .any(|index| !indexes.insert(index))
        {
            return Err(ProposalError::DuplicateEvidenceReference);
        }
    }
    Ok(proposal)
}

pub(super) fn decode_verification(input: &str) -> Result<VerificationProposal, ProposalError> {
    let proposal: VerificationProposal = decode(input)?;
    if proposal.relations.is_empty() {
        return Err(ProposalError::VerificationHasNoRelations);
    }
    let mut indexes = BTreeSet::new();
    if proposal
        .relations
        .iter()
        .any(|relation| !indexes.insert(relation.evidence_index))
    {
        return Err(ProposalError::DuplicateVerificationEvidenceReference);
    }
    Ok(proposal)
}

pub(super) fn decode_follow_up(input: &str) -> Result<FollowUpProposal, ProposalError> {
    let proposal: FollowUpProposal = decode(input)?;
    if proposal
        .objective
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(ProposalError::BlankFollowUpObjective);
    }
    Ok(proposal)
}

fn decode<T>(input: &str) -> Result<T, ProposalError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(input).map_err(|error| {
        if error.is_syntax() || error.is_eof() {
            ProposalError::InvalidJson
        } else {
            ProposalError::InvalidShape
        }
    })
}

fn validate_excerpt(value: &str) -> Result<(), ProposalError> {
    if value.trim().is_empty() {
        return Err(ProposalError::BlankExcerpt);
    }
    if value.len() > EXCERPT_TEXT_LIMIT {
        return Err(ProposalError::OversizedExcerpt);
    }
    Ok(())
}

fn validate_statement(value: &str) -> Result<(), ProposalError> {
    if value.trim().is_empty() {
        return Err(ProposalError::BlankStatement);
    }
    if value.len() > STATEMENT_TEXT_LIMIT {
        return Err(ProposalError::OversizedStatement);
    }
    Ok(())
}

pub(super) fn initial_plan_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["tasks"],"properties":{"tasks":{"type":"array","maxItems":INITIAL_TASK_LIMIT,"items":{"type":"object","additionalProperties":false,"required":["objective"],"properties":{"objective":{"type":"string","minLength":1}}}}}})
}

pub(super) fn extraction_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["evidence","claims"],"properties":{"evidence":{"type":"array","maxItems":EXTRACTED_EVIDENCE_LIMIT,"items":{"type":"object","additionalProperties":false,"required":["source_index","excerpt"],"properties":{"source_index":{"type":"integer","minimum":0},"excerpt":{"type":"string","minLength":1}}}},"claims":{"type":"array","maxItems":CLAIM_LIMIT,"items":{"type":"object","additionalProperties":false,"required":["statement","evidence_indices"],"properties":{"statement":{"type":"string","minLength":1},"evidence_indices":{"type":"array","minItems":1,"items":{"type":"integer","minimum":0}}}}}}})
}

pub(super) fn verification_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["relations","sufficiency"],"properties":{"relations":{"type":"array","minItems":1,"items":{"type":"object","additionalProperties":false,"required":["evidence_index","relation"],"properties":{"evidence_index":{"type":"integer","minimum":0},"relation":{"type":"string","enum":["supports","contradicts","unclear","irrelevant"]}}}},"sufficiency":{"type":"string","enum":["sufficient","insufficient","indeterminate"]}}})
}

pub(super) fn follow_up_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["objective"],"properties":{"objective":{"type":["string","null"],"minLength":1}}})
}

#[cfg(test)]
mod tests {
    use super::{
        EXCERPT_TEXT_LIMIT, ProposalError, STATEMENT_TEXT_LIMIT, decode_extraction,
        decode_follow_up, decode_initial_plan, decode_verification, extraction_schema,
    };

    #[test]
    fn rejects_missing_and_unknown_initial_plan_fields() {
        assert_eq!(
            decode_initial_plan(r#"{"tasks":[{}]}"#),
            Err(ProposalError::InvalidShape)
        );
        assert_eq!(
            decode_initial_plan(r#"{"tasks":[],"extra":true}"#),
            Err(ProposalError::InvalidShape)
        );
    }

    #[test]
    fn rejects_too_many_blank_and_duplicate_initial_tasks() {
        assert_eq!(
            decode_initial_plan(
                r#"{"tasks":[{"objective":"a"},{"objective":"b"},{"objective":"c"},{"objective":"d"}]}"#
            ),
            Err(ProposalError::TooManyInitialTasks)
        );
        assert_eq!(
            decode_initial_plan(r#"{"tasks":[{"objective":" "}]}"#),
            Err(ProposalError::BlankTaskObjective)
        );
        assert_eq!(
            decode_initial_plan(r#"{"tasks":[{"objective":"same"},{"objective":"same"}]}"#),
            Err(ProposalError::DuplicateTaskObjective)
        );
    }

    #[test]
    fn accepts_an_empty_initial_plan_as_no_useful_action() {
        assert!(decode_initial_plan(r#"{"tasks":[]}"#).is_ok());
    }

    #[test]
    fn rejects_bad_extraction_shapes_and_limits() {
        assert_eq!(
            decode_extraction(r#"{"evidence":[]}"#),
            Err(ProposalError::InvalidShape)
        );
        assert_eq!(
            decode_extraction(r#"{"evidence":[],"claims":[],"extra":true}"#),
            Err(ProposalError::InvalidShape)
        );
        let evidence = (0..9)
            .map(|index| format!(r#"{{"source_index":{index},"excerpt":"x"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            decode_extraction(&format!(r#"{{"evidence":[{evidence}],"claims":[]}}"#)),
            Err(ProposalError::TooManyEvidence)
        );
        assert_eq!(
            decode_extraction(&format!(
                r#"{{"evidence":[{{"source_index":0,"excerpt":"{}"}}],"claims":[]}}"#,
                "x".repeat(EXCERPT_TEXT_LIMIT + 1)
            )),
            Err(ProposalError::OversizedExcerpt)
        );
        assert_eq!(
            decode_extraction(&format!(
                r#"{{"evidence":[],"claims":[{{"statement":"{}","evidence_indices":[0]}}]}}"#,
                "x".repeat(STATEMENT_TEXT_LIMIT + 1)
            )),
            Err(ProposalError::OversizedStatement)
        );
        let claims = (0..5)
            .map(|index| format!(r#"{{"statement":"claim {index}","evidence_indices":[0]}}"#))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            decode_extraction(&format!(
                r#"{{"evidence":[{{"source_index":0,"excerpt":"x"}}],"claims":[{claims}]}}"#
            )),
            Err(ProposalError::TooManyClaims)
        );
    }

    #[test]
    fn rejects_empty_or_duplicate_claim_evidence_references() {
        assert_eq!(
            decode_extraction(r#"{"evidence":[{"source_index":0,"excerpt":" "}],"claims":[]}"#),
            Err(ProposalError::BlankExcerpt)
        );
        assert_eq!(
            decode_extraction(
                r#"{"evidence":[],"claims":[{"statement":" ","evidence_indices":[0]}]}"#
            ),
            Err(ProposalError::BlankStatement)
        );
        assert_eq!(
            decode_extraction(
                r#"{"evidence":[],"claims":[{"statement":"claim","evidence_indices":[]}]}"#
            ),
            Err(ProposalError::ClaimHasNoEvidence)
        );
        assert_eq!(
            decode_extraction(
                r#"{"evidence":[],"claims":[{"statement":"claim","evidence_indices":[0,0]}]}"#
            ),
            Err(ProposalError::DuplicateEvidenceReference)
        );
    }

    #[test]
    fn rejects_unknown_verification_enums_and_duplicate_indexes() {
        assert_eq!(
            decode_verification(
                r#"{"relations":[{"evidence_index":0,"relation":"invented"}],"sufficiency":"sufficient"}"#
            ),
            Err(ProposalError::InvalidShape)
        );
        assert_eq!(
            decode_verification(
                r#"{"relations":[{"evidence_index":0,"relation":"supports"},{"evidence_index":0,"relation":"contradicts"}],"sufficiency":"sufficient"}"#
            ),
            Err(ProposalError::DuplicateVerificationEvidenceReference)
        );
    }

    #[test]
    fn rejects_non_object_roots_and_accepts_empty_or_null_no_useful_actions() {
        assert_eq!(decode_extraction("[]"), Err(ProposalError::InvalidShape));
        assert!(decode_extraction(r#"{"evidence":[],"claims":[]}"#).is_ok());
        assert!(decode_follow_up(r#"{"objective":null}"#).is_ok());
    }

    #[test]
    fn excerpt_and_statement_limits_count_utf8_bytes() {
        let accepted_excerpt = "é".repeat(EXCERPT_TEXT_LIMIT / 2);
        assert!(decode_extraction(&format!(
            r#"{{"evidence":[{{"source_index":0,"excerpt":"{accepted_excerpt}"}}],"claims":[]}}"#
        ))
        .is_ok());
        let oversized_excerpt = format!("{accepted_excerpt}é");
        assert_eq!(
            decode_extraction(&format!(
                r#"{{"evidence":[{{"source_index":0,"excerpt":"{oversized_excerpt}"}}],"claims":[]}}"#
            )),
            Err(ProposalError::OversizedExcerpt)
        );

        let accepted_statement = "é".repeat(STATEMENT_TEXT_LIMIT / 2);
        assert!(decode_extraction(&format!(
            r#"{{"evidence":[],"claims":[{{"statement":"{accepted_statement}","evidence_indices":[0]}}]}}"#
        ))
        .is_ok());
        let oversized_statement = format!("{accepted_statement}é");
        assert_eq!(
            decode_extraction(&format!(
                r#"{{"evidence":[],"claims":[{{"statement":"{oversized_statement}","evidence_indices":[0]}}]}}"#
            )),
            Err(ProposalError::OversizedStatement)
        );
    }

    #[test]
    fn extraction_schema_does_not_claim_character_based_text_limits() {
        assert!(!extraction_schema().to_string().contains("maxLength"));
    }
}
