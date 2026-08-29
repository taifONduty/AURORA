#![allow(dead_code)]

use aurora_research::{
    ClaimPresentation, EvidenceRelation, EvidenceSufficiency, ResearchGapCause, ResearchGapStatus,
    ResearchStopReason, SynthesisBasis, SynthesisReportScope,
};
use serde_json::{Value, json};

pub(super) const MAX_SYNTHESIS_CONTEXT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum SynthesisContextError {
    #[error("serialized synthesis context exceeds the byte limit")]
    TooLarge,
}

pub(super) fn synthesis_context(basis: &SynthesisBasis) -> Result<String, SynthesisContextError> {
    let claims = basis
        .claims()
        .map(|claim| {
            let assessments = claim
                .assessments()
                .map(|assessment| {
                    let relations = assessment
                        .evidence_relations()
                        .map(|(evidence_id, relation)| {
                            json!({
                                "evidence_id": evidence_id.to_string(),
                                "relation": relation_name(relation),
                            })
                        })
                        .collect::<Vec<_>>();
                    json!({
                        "verification_id": assessment.id().to_string(),
                        "sufficiency": sufficiency_name(assessment.sufficiency()),
                        "relations": relations,
                    })
                })
                .collect::<Vec<_>>();
            let evidence = claim
                .evidence_items()
                .filter_map(|evidence| {
                    claim.source(evidence.source_id()).map(|source| {
                        json!({
                            "evidence_id": evidence.id().to_string(),
                            "excerpt": evidence.excerpt(),
                            "source": {
                                "source_id": source.id().to_string(),
                                "title": source.title(),
                                "locator": source.locator(),
                                "retrieved_at": source.retrieved_at().as_str(),
                                "media_type": source.media_type().as_str(),
                                "content_digest": digest_hex(source.content_digest().as_sha256()),
                            },
                        })
                    })
                })
                .collect::<Vec<_>>();
            let gaps = claim.gaps().map(gap_context).collect::<Vec<_>>();
            json!({
                "claim_id": claim.claim().id().to_string(),
                "statement": claim.claim().statement(),
                "presentation": presentation_name(claim.presentation()),
                "assessments": assessments,
                "gaps": gaps,
                "evidence": evidence,
            })
        })
        .collect::<Vec<_>>();
    serialize_bounded(json!({
        "research_question": basis.question(),
        "scope": scope_context(basis.scope()),
        "claims": claims,
    }))
}

fn scope_context(scope: &SynthesisReportScope) -> Value {
    match scope {
        SynthesisReportScope::Complete => json!({ "status": "complete" }),
        SynthesisReportScope::Partial(reason) => json!({
            "status": "partial",
            "reason": stop_reason_context(reason),
        }),
    }
}

fn stop_reason_context(reason: &ResearchStopReason) -> Value {
    match reason {
        ResearchStopReason::Blocked(reason) => json!({
            "kind": "blocked",
            "reason": reason.as_str(),
        }),
        ResearchStopReason::BudgetExhausted => json!({ "kind": "budget_exhausted" }),
        ResearchStopReason::OperatorStopped => json!({ "kind": "operator_stopped" }),
    }
}

fn gap_context(gap: &aurora_research::ResearchGapState) -> Value {
    json!({
        "gap_id": gap.gap().id().to_string(),
        "description": gap.gap().description().as_str(),
        "cause": gap_cause_context(gap.gap().cause()),
        "status": gap_status_context(gap.status()),
    })
}

fn gap_cause_context(cause: &ResearchGapCause) -> Value {
    match cause {
        ResearchGapCause::Verification(verification_id) => json!({
            "kind": "verification",
            "verification_id": verification_id.to_string(),
        }),
        ResearchGapCause::InvestigationFailure(task_id) => json!({
            "kind": "investigation_failure",
            "task_id": task_id.to_string(),
        }),
    }
}

fn gap_status_context(status: &ResearchGapStatus) -> Value {
    match status {
        ResearchGapStatus::Open => json!({ "kind": "open" }),
        ResearchGapStatus::Resolved(verification_id) => json!({
            "kind": "resolved",
            "verification_id": verification_id.to_string(),
        }),
    }
}

fn serialize_bounded(value: Value) -> Result<String, SynthesisContextError> {
    let serialized = serde_json::to_string(&value)
        .unwrap_or_else(|_| unreachable!("a JSON value always serializes"));
    if serialized.len() > MAX_SYNTHESIS_CONTEXT_BYTES {
        return Err(SynthesisContextError::TooLarge);
    }
    Ok(serialized)
}

fn presentation_name(presentation: ClaimPresentation) -> &'static str {
    match presentation {
        ClaimPresentation::Established => "established",
        ClaimPresentation::Unresolved => "unresolved",
        ClaimPresentation::Contested => "contested",
    }
}

fn relation_name(relation: EvidenceRelation) -> &'static str {
    match relation {
        EvidenceRelation::Supports => "supports",
        EvidenceRelation::Contradicts => "contradicts",
        EvidenceRelation::Unclear => "unclear",
        EvidenceRelation::Irrelevant => "irrelevant",
    }
}

fn sufficiency_name(sufficiency: EvidenceSufficiency) -> &'static str {
    match sufficiency {
        EvidenceSufficiency::Sufficient => "sufficient",
        EvidenceSufficiency::Insufficient => "insufficient",
        EvidenceSufficiency::Indeterminate => "indeterminate",
    }
}

fn digest_hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
