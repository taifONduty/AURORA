use aurora_research::{
    ContentDigest, Evidence, EvidenceId, InvestigationResult, MediaType, ResearchEvent,
    ResearchRecord, ResearchState, RetrievedAt, Source, SourceId,
};

use crate::{
    admission::{AdmissionError, admit_extraction, snapshot_context},
    proposal::decode_extraction,
};

#[test]
fn snapshot_context_pairs_each_source_with_its_following_full_content() {
    let result = tavily_result(&["First complete snapshot", "Second complete snapshot"]);

    let context = snapshot_context(&result).expect("the Tavily-shaped result is paired");

    assert!(context.contains("\"source_index\":0"));
    assert!(context.contains("First complete snapshot"));
    assert!(context.contains("\"source_index\":1"));
    assert!(context.contains("Second complete snapshot"));
    let ResearchEvent::SourceRecorded(source) = result.research_records()[0].event() else {
        panic!("first acquired record is a source");
    };
    assert!(!context.contains(&source.id().to_string()));
}

#[test]
fn snapshot_context_rejects_over_one_mib_without_truncating() {
    let result = tavily_result(&[&"x".repeat(1024 * 1024 + 1)]);

    assert_eq!(
        snapshot_context(&result),
        Err(AdmissionError::SnapshotTextTooLarge)
    );
}

#[test]
fn snapshot_context_rejects_two_individually_valid_snapshots_over_the_aggregate_limit() {
    let first = "a".repeat(700 * 1024);
    let second = "b".repeat(400 * 1024);
    let result = tavily_result(&[&first, &second]);

    assert_eq!(
        snapshot_context(&result),
        Err(AdmissionError::SnapshotTextTooLarge)
    );
}

#[test]
fn snapshot_context_rejects_json_escaping_expansion_without_truncating() {
    let mut text = String::with_capacity(1024 * 1024);
    text.push('x');
    text.extend(std::iter::repeat_n('\n', 1024 * 1024 - 1));
    let result = tavily_result(&[&text]);

    assert_eq!(
        snapshot_context(&result),
        Err(AdmissionError::ModelInputTooLarge)
    );
    let ResearchEvent::EvidenceRecorded(full_content) = result.research_records()[1].event() else {
        panic!("second acquired record is full content");
    };
    assert_eq!(full_content.excerpt(), text);
}

#[test]
fn admits_only_exact_excerpts_and_generates_all_new_identities_in_sequence() {
    let acquired = tavily_result(&["Exact excerpt within full snapshot"]);
    let proposal = decode_extraction(
        r#"{"evidence":[{"source_index":0,"excerpt":"Exact excerpt"}],"claims":[{"statement":"A grounded claim","evidence_indices":[0]}]}"#,
    )
    .expect("proposal is locally valid");

    let enriched = admit_extraction(&ResearchState::default(), acquired, proposal)
        .expect("an exact excerpt is admitted");
    let records = enriched.research_records();

    assert_eq!(records.len(), 4);
    assert_eq!(
        records
            .iter()
            .map(ResearchRecord::sequence)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    let ResearchEvent::EvidenceRecorded(extracted) = records[2].event() else {
        panic!("third record is the AURORA-generated extracted evidence");
    };
    let ResearchEvent::EvidenceRecorded(full_content) = records[1].event() else {
        panic!("second record is the Tavily full-content evidence");
    };
    assert_ne!(extracted.id(), full_content.id());
    let ResearchEvent::ClaimProposed(claim) = records[3].event() else {
        panic!("fourth record is the AURORA-generated claim");
    };
    assert_eq!(
        claim.evidence_ids().iter().copied().collect::<Vec<_>>(),
        [*extracted.id()]
    );
    assert_eq!(
        ResearchState::reconstruct(records.to_vec())
            .unwrap()
            .claim_count(),
        1
    );
}

#[test]
fn rejects_case_whitespace_and_fabricated_excerpts() {
    for excerpt in ["exact excerpt", "Exact  excerpt", "fabricated"] {
        let proposal = decode_extraction(&format!(
            r#"{{"evidence":[{{"source_index":0,"excerpt":"{excerpt}"}}],"claims":[]}}"#
        ))
        .unwrap();

        assert_eq!(
            admit_extraction(
                &ResearchState::default(),
                tavily_result(&["Exact excerpt"]),
                proposal
            ),
            Err(AdmissionError::ExcerptAbsent)
        );
    }
}

#[test]
fn rejects_unknown_source_and_evidence_indexes() {
    let unknown_source =
        decode_extraction(r#"{"evidence":[{"source_index":1,"excerpt":"Exact"}],"claims":[]}"#)
            .unwrap();
    assert_eq!(
        admit_extraction(
            &ResearchState::default(),
            tavily_result(&["Exact"]),
            unknown_source
        ),
        Err(AdmissionError::UnknownSourceIndex)
    );

    let unknown_evidence = decode_extraction(
        r#"{"evidence":[{"source_index":0,"excerpt":"Exact"}],"claims":[{"statement":"claim","evidence_indices":[1]}]}"#,
    )
    .unwrap();
    assert_eq!(
        admit_extraction(
            &ResearchState::default(),
            tavily_result(&["Exact"]),
            unknown_evidence
        ),
        Err(AdmissionError::UnknownEvidenceIndex)
    );
}

#[test]
fn rejects_a_late_invalid_proposal_without_returning_partial_admission() {
    let state = ResearchState::default();
    let proposal = decode_extraction(
        r#"{"evidence":[{"source_index":0,"excerpt":"Exact"},{"source_index":0,"excerpt":"fabricated"}],"claims":[]}"#,
    )
    .unwrap();

    assert_eq!(
        admit_extraction(&state, tavily_result(&["Exact"]), proposal),
        Err(AdmissionError::ExcerptAbsent)
    );
    assert_eq!(state, ResearchState::default());
}

#[test]
fn validates_the_complete_candidate_against_a_cloned_research_state() {
    let acquired = tavily_result(&["Exact"]);
    let state = ResearchState::reconstruct(acquired.research_records().to_vec()).unwrap();
    let proposal = decode_extraction(r#"{"evidence":[],"claims":[]}"#).unwrap();

    assert!(matches!(
        admit_extraction(&state, acquired, proposal),
        Err(AdmissionError::InvalidResearchState(_))
    ));
    assert_eq!(state.source_count(), 1);
    assert_eq!(state.evidence_count(), 1);
}

fn tavily_result(contents: &[&str]) -> InvestigationResult {
    let media_type = MediaType::new("text/plain").unwrap();
    let retrieved_at = RetrievedAt::new("2026-08-29T12:34:56Z").unwrap();
    let mut records = Vec::new();
    for (index, content) in contents.iter().enumerate() {
        let source_id = SourceId::generate();
        let source = Source::new(
            source_id,
            ContentDigest::sha256([index as u8; 32]),
            format!("https://source-{index}.example/article"),
            Some(format!("Source {index}")),
            retrieved_at.clone(),
            media_type.clone(),
        )
        .unwrap();
        let evidence =
            Evidence::new(EvidenceId::generate(), source_id, (*content).to_owned()).unwrap();
        let sequence = (index as u64) * 2 + 1;
        records.push(ResearchRecord::new(sequence, ResearchEvent::SourceRecorded(source)).unwrap());
        records.push(
            ResearchRecord::new(sequence + 1, ResearchEvent::EvidenceRecorded(evidence)).unwrap(),
        );
    }
    InvestigationResult::new(records)
}
