#![allow(dead_code)]

use aurora_research::{
    Claim, Evidence, ResearchGap, ResearchRequest, ResearchState, Source, VerificationAssessment,
};
use serde_json::{Value, json};

use crate::admission::Snapshot;

pub(super) const MAX_MODEL_CONTEXT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum ModelContextError {
    #[error("serialized model context exceeds the byte limit")]
    TooLarge,
}

pub(super) fn extraction_context(snapshots: &[Snapshot]) -> Result<String, ModelContextError> {
    preflight(snapshots.iter().flat_map(|snapshot| {
        [
            snapshot.source.locator().len(),
            snapshot.source.title().map_or(0, str::len),
            snapshot.source.retrieved_at().as_str().len(),
            snapshot.source.media_type().as_str().len(),
            snapshot.full_text.len(),
        ]
    }))?;
    let sources = snapshots
        .iter()
        .enumerate()
        .map(|(source_index, snapshot)| {
            json!({
                "source_index": source_index,
                "locator": snapshot.source.locator(),
                "title": snapshot.source.title(),
                "retrieved_at": snapshot.source.retrieved_at().as_str(),
                "media_type": snapshot.source.media_type().as_str(),
                "content_digest": digest_text(&snapshot.source),
                "full_text": snapshot.full_text,
            })
        })
        .collect::<Vec<_>>();
    serialize_bounded(json!({ "sources": sources }))
}

pub(super) fn initial_plan_context(request: &ResearchRequest) -> Result<String, ModelContextError> {
    preflight([request.question().len()])?;
    serialize_bounded(json!({ "research_question": request.question() }))
}

pub(super) fn verification_context(
    request: &ResearchRequest,
    claim: &Claim,
    evidence: &[(&Evidence, &Source)],
) -> Result<String, ModelContextError> {
    preflight(
        [request.question().len(), claim.statement().len()]
            .into_iter()
            .chain(evidence_raw_bytes(evidence)),
    )?;
    serialize_bounded(json!({
        "research_question": request.question(),
        "claim_statement": claim.statement(),
        "evidence": evidence_context(evidence),
    }))
}

pub(super) fn sourced_evidence(research: &ResearchState) -> Option<Vec<(&Evidence, &Source)>> {
    research
        .evidence_items()
        .map(|evidence| {
            research
                .source(evidence.source_id())
                .map(|source| (evidence, source))
        })
        .collect()
}

pub(super) fn follow_up_context(
    request: &ResearchRequest,
    gap: &ResearchGap,
    claim: &Claim,
    assessment: &VerificationAssessment,
    evidence: &[(&Evidence, &Source)],
    completed_objectives: &[String],
    remaining_follow_ups: u32,
) -> Result<String, ModelContextError> {
    preflight(
        [
            request.question().len(),
            gap.as_str().len(),
            claim.statement().len(),
        ]
        .into_iter()
        .chain(evidence_raw_bytes(evidence))
        .chain(completed_objectives.iter().map(String::len)),
    )?;
    let relations = assessment
        .evidence_relations()
        .filter_map(|(evidence_id, relation)| {
            evidence
                .iter()
                .position(|(item, _)| item.id() == evidence_id)
                .map(|evidence_index| {
                    json!({ "evidence_index": evidence_index, "relation": relation_name(relation) })
                })
        })
        .collect::<Vec<_>>();
    serialize_bounded(json!({
        "research_question": request.question(),
        "gap": gap.as_str(),
        "claim_statement": claim.statement(),
        "evidence": evidence_context(evidence),
        "assessment": {
            "relations": relations,
            "sufficiency": sufficiency_name(assessment.sufficiency()),
        },
        "completed_objectives": completed_objectives,
        "remaining_follow_ups": remaining_follow_ups,
    }))
}

fn evidence_raw_bytes<'a>(
    evidence: &'a [(&'a Evidence, &'a Source)],
) -> impl Iterator<Item = usize> + 'a {
    evidence
        .iter()
        .flat_map(|(item, source)| [source.locator().len(), item.excerpt().len()])
}

fn preflight(lengths: impl IntoIterator<Item = usize>) -> Result<(), ModelContextError> {
    let total = lengths
        .into_iter()
        .try_fold(0_usize, usize::checked_add)
        .ok_or(ModelContextError::TooLarge)?;
    if total > MAX_MODEL_CONTEXT_BYTES {
        return Err(ModelContextError::TooLarge);
    }
    Ok(())
}

fn serialize_bounded(value: Value) -> Result<String, ModelContextError> {
    let serialized = serde_json::to_string(&value)
        .unwrap_or_else(|_| unreachable!("a JSON value always serializes"));
    if serialized.len() > MAX_MODEL_CONTEXT_BYTES {
        return Err(ModelContextError::TooLarge);
    }
    Ok(serialized)
}

fn evidence_context(evidence: &[(&Evidence, &Source)]) -> Vec<Value> {
    evidence
        .iter()
        .enumerate()
        .map(|(evidence_index, (item, source))| {
            json!({
                "evidence_index": evidence_index,
                "locator": source.locator(),
                "content_digest": digest_text(source),
                "excerpt": item.excerpt(),
            })
        })
        .collect()
}

fn digest_text(source: &Source) -> String {
    source
        .content_digest()
        .as_sha256()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn relation_name(relation: aurora_research::EvidenceRelation) -> &'static str {
    match relation {
        aurora_research::EvidenceRelation::Supports => "supports",
        aurora_research::EvidenceRelation::Contradicts => "contradicts",
        aurora_research::EvidenceRelation::Unclear => "unclear",
        aurora_research::EvidenceRelation::Irrelevant => "irrelevant",
    }
}

fn sufficiency_name(sufficiency: aurora_research::EvidenceSufficiency) -> &'static str {
    match sufficiency {
        aurora_research::EvidenceSufficiency::Sufficient => "sufficient",
        aurora_research::EvidenceSufficiency::Insufficient => "insufficient",
        aurora_research::EvidenceSufficiency::Indeterminate => "indeterminate",
    }
}
