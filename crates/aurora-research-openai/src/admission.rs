#![allow(dead_code)]

use aurora_research::{
    Evidence, EvidenceId, InvestigationResult, ResearchEvent, ResearchRecord, ResearchState,
    Source, TransitionError,
};

use crate::{
    context::{ModelContextError, extraction_context},
    proposal::{ExtractionProposal, SNAPSHOT_TEXT_LIMIT},
};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum AdmissionError {
    #[error("acquired records do not contain source and full-content pairs")]
    InvalidSnapshotPairing,
    #[error("acquired snapshot text exceeds the extraction input limit")]
    SnapshotTextTooLarge,
    #[error("serialized extraction model context exceeds the model input limit")]
    ModelInputTooLarge,
    #[error("proposal references an unknown source index")]
    UnknownSourceIndex,
    #[error("proposal references an unknown extracted evidence index")]
    UnknownEvidenceIndex,
    #[error("proposal excerpt is absent from its selected full snapshot")]
    ExcerptAbsent,
    #[error("generated extraction record sequence is exhausted")]
    SequenceExhausted,
    #[error("generated extraction entity is invalid")]
    InvalidGeneratedEntity,
    #[error("combined acquisition and extraction records violate research state: {0}")]
    InvalidResearchState(TransitionError),
}

#[derive(Clone, Debug)]
pub(super) struct Snapshot {
    pub(super) source: Source,
    pub(super) full_text: String,
}

pub(super) fn snapshot_context(result: &InvestigationResult) -> Result<String, AdmissionError> {
    let snapshots = snapshots(result)?;
    snapshot_text_len(&snapshots)?;
    extraction_context(&snapshots).map_err(|error| match error {
        ModelContextError::TooLarge => AdmissionError::ModelInputTooLarge,
    })
}

pub(super) fn admit_extraction(
    state: &ResearchState,
    acquired: InvestigationResult,
    proposal: ExtractionProposal,
) -> Result<InvestigationResult, AdmissionError> {
    let snapshots = snapshots(&acquired)?;
    snapshot_text_len(&snapshots)?;
    let mut records = acquired.research_records().to_vec();
    let mut next_sequence = records
        .last()
        .map(ResearchRecord::sequence)
        .unwrap_or_else(|| state.last_sequence());
    let mut extracted_evidence = Vec::with_capacity(proposal.evidence.len());

    for proposed in proposal.evidence {
        let snapshot = snapshots
            .get(proposed.source_index)
            .ok_or(AdmissionError::UnknownSourceIndex)?;
        if !snapshot.full_text.contains(&proposed.excerpt) {
            return Err(AdmissionError::ExcerptAbsent);
        }
        next_sequence = next_sequence
            .checked_add(1)
            .ok_or(AdmissionError::SequenceExhausted)?;
        let evidence_id = EvidenceId::generate();
        let evidence = Evidence::new(evidence_id, *snapshot.source.id(), proposed.excerpt)
            .map_err(|_| AdmissionError::InvalidGeneratedEntity)?;
        records.push(
            ResearchRecord::new(next_sequence, ResearchEvent::EvidenceRecorded(evidence))
                .map_err(|_| AdmissionError::InvalidGeneratedEntity)?,
        );
        extracted_evidence.push(evidence_id);
    }

    for proposed in proposal.claims {
        next_sequence = next_sequence
            .checked_add(1)
            .ok_or(AdmissionError::SequenceExhausted)?;
        let evidence_ids = proposed
            .evidence_indices
            .into_iter()
            .map(|index| {
                extracted_evidence
                    .get(index)
                    .copied()
                    .ok_or(AdmissionError::UnknownEvidenceIndex)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let claim = aurora_research::Claim::new(
            aurora_research::ClaimId::generate(),
            proposed.statement,
            evidence_ids,
        )
        .map_err(|_| AdmissionError::InvalidGeneratedEntity)?;
        records.push(
            ResearchRecord::new(next_sequence, ResearchEvent::ClaimProposed(claim))
                .map_err(|_| AdmissionError::InvalidGeneratedEntity)?,
        );
    }

    let mut candidate = state.clone();
    for record in &records {
        candidate
            .apply(record.clone())
            .map_err(AdmissionError::InvalidResearchState)?;
    }
    Ok(InvestigationResult::new(records))
}

fn snapshots(result: &InvestigationResult) -> Result<Vec<Snapshot>, AdmissionError> {
    let records = result.research_records();
    let (pairs, remainder) = records.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(AdmissionError::InvalidSnapshotPairing);
    }
    pairs
        .iter()
        .map(|pair| {
            let ResearchEvent::SourceRecorded(source) = pair[0].event() else {
                return Err(AdmissionError::InvalidSnapshotPairing);
            };
            let ResearchEvent::EvidenceRecorded(full_content) = pair[1].event() else {
                return Err(AdmissionError::InvalidSnapshotPairing);
            };
            if full_content.source_id() != source.id() {
                return Err(AdmissionError::InvalidSnapshotPairing);
            }
            Ok(Snapshot {
                source: source.clone(),
                full_text: full_content.excerpt().to_owned(),
            })
        })
        .collect()
}

fn snapshot_text_len(snapshots: &[Snapshot]) -> Result<(), AdmissionError> {
    let mut total = 0_usize;
    for snapshot in snapshots {
        total = total
            .checked_add(snapshot.full_text.len())
            .ok_or(AdmissionError::SnapshotTextTooLarge)?;
        if total > SNAPSHOT_TEXT_LIMIT {
            return Err(AdmissionError::SnapshotTextTooLarge);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
