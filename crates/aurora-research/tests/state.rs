use aurora_research::{
    Claim, ClaimId, ContentDigest, Evidence, EvidenceId, MediaType, ResearchEvent, ResearchRecord,
    ResearchState, RetrievedAt, Source, SourceId, TransitionError, ValidationError,
};

#[test]
fn source_evidence_claim_history_reconstructs_expected_state() {
    let source = source(source_id(1), [1; 32], "https://example.test/paper");
    let evidence = evidence(evidence_id(2), *source.id(), "Quoted result.");
    let claim = claim(
        claim_id(3),
        "The result was observed.",
        vec![*evidence.id()],
    );
    let records = vec![
        record(1, ResearchEvent::SourceRecorded(source.clone())),
        record(2, ResearchEvent::EvidenceRecorded(evidence.clone())),
        record(3, ResearchEvent::ClaimProposed(claim.clone())),
    ];

    let state = ResearchState::reconstruct(records).expect("valid history reconstructs");

    assert_eq!(state.last_sequence(), 3);
    assert_eq!(state.source_count(), 1);
    assert_eq!(state.evidence_count(), 1);
    assert_eq!(state.claim_count(), 1);
    assert_eq!(state.source(source.id()), Some(&source));
    assert_eq!(state.evidence(evidence.id()), Some(&evidence));
    assert_eq!(state.claim(claim.id()), Some(&claim));
}

#[test]
fn empty_history_reconstructs_empty_state() {
    let state = ResearchState::reconstruct(Vec::new()).expect("empty history is valid");

    assert_eq!(state.last_sequence(), 0);
    assert_eq!(state.source_count(), 0);
    assert_eq!(state.evidence_count(), 0);
    assert_eq!(state.claim_count(), 0);
    assert_eq!(state.sources().count(), 0);
    assert_eq!(state.evidence_items().count(), 0);
    assert_eq!(state.claims().count(), 0);
}

#[test]
fn record_rejects_zero_sequence() {
    let result = ResearchRecord::new(
        0,
        ResearchEvent::SourceRecorded(source(source_id(1), [1; 32], "https://example.test/paper")),
    );

    assert_eq!(result, Err(ValidationError::ZeroSequence));
}

#[test]
fn sequence_mismatch_is_rejected_without_mutation() {
    let mut state = ResearchState::default();
    let before = state.clone();

    let error = state
        .apply(record(
            2,
            ResearchEvent::SourceRecorded(source(
                source_id(1),
                [1; 32],
                "https://example.test/paper",
            )),
        ))
        .expect_err("history must start at sequence one");

    assert_eq!(
        error,
        TransitionError::Sequence {
            expected: 1,
            actual: 2,
        }
    );
    assert_eq!(state, before);
}

#[test]
fn evidence_requires_an_existing_source_without_mutating_state() {
    let mut state = ResearchState::default();
    let before = state.clone();
    let missing_source = source_id(9);

    let error = state
        .apply(record(
            1,
            ResearchEvent::EvidenceRecorded(evidence(
                evidence_id(2),
                missing_source,
                "Quoted result.",
            )),
        ))
        .expect_err("evidence cannot precede its source");

    assert_eq!(error, TransitionError::UnknownSource(missing_source));
    assert_eq!(state, before);
}

#[test]
fn claim_requires_every_evidence_item_without_mutating_state() {
    let source = source(source_id(1), [1; 32], "https://example.test/paper");
    let evidence = evidence(evidence_id(2), *source.id(), "Quoted result.");
    let mut state = ResearchState::reconstruct(vec![
        record(1, ResearchEvent::SourceRecorded(source)),
        record(2, ResearchEvent::EvidenceRecorded(evidence)),
    ])
    .expect("fixture history is valid");
    let before = state.clone();
    let missing_evidence = evidence_id(4);

    let error = state
        .apply(record(
            3,
            ResearchEvent::ClaimProposed(claim(
                claim_id(3),
                "The result was observed.",
                vec![evidence_id(2), missing_evidence],
            )),
        ))
        .expect_err("claim cannot precede any evidence item");

    assert_eq!(error, TransitionError::UnknownEvidence(missing_evidence));
    assert_eq!(state, before);
}

#[test]
fn duplicate_identifiers_are_rejected_without_mutation() {
    let source = source(source_id(1), [1; 32], "https://example.test/paper");
    let mut source_state = ResearchState::reconstruct(vec![record(
        1,
        ResearchEvent::SourceRecorded(source.clone()),
    )])
    .expect("source history is valid");
    let source_before = source_state.clone();
    assert_eq!(
        source_state.apply(record(2, ResearchEvent::SourceRecorded(source.clone()))),
        Err(TransitionError::DuplicateSource(*source.id()))
    );
    assert_eq!(source_state, source_before);

    let evidence = evidence(evidence_id(2), *source.id(), "Quoted result.");
    let mut evidence_state = ResearchState::reconstruct(vec![
        record(1, ResearchEvent::SourceRecorded(source.clone())),
        record(2, ResearchEvent::EvidenceRecorded(evidence.clone())),
    ])
    .expect("evidence history is valid");
    let evidence_before = evidence_state.clone();
    assert_eq!(
        evidence_state.apply(record(3, ResearchEvent::EvidenceRecorded(evidence.clone()))),
        Err(TransitionError::DuplicateEvidence(*evidence.id()))
    );
    assert_eq!(evidence_state, evidence_before);

    let claim = claim(
        claim_id(3),
        "The result was observed.",
        vec![*evidence.id()],
    );
    let mut claim_state = ResearchState::reconstruct(vec![
        record(1, ResearchEvent::SourceRecorded(source)),
        record(2, ResearchEvent::EvidenceRecorded(evidence)),
        record(3, ResearchEvent::ClaimProposed(claim.clone())),
    ])
    .expect("claim history is valid");
    let claim_before = claim_state.clone();
    assert_eq!(
        claim_state.apply(record(4, ResearchEvent::ClaimProposed(claim.clone()))),
        Err(TransitionError::DuplicateClaim(*claim.id()))
    );
    assert_eq!(claim_state, claim_before);
}

#[test]
fn locator_and_digest_do_not_define_source_identity() {
    let first = source(source_id(1), [1; 32], "https://example.test/paper");
    let changed = source(source_id(2), [2; 32], "https://example.test/paper");
    let mirrored = source(source_id(3), [1; 32], "local:paper-copy");

    let state = ResearchState::reconstruct(vec![
        record(1, ResearchEvent::SourceRecorded(first)),
        record(2, ResearchEvent::SourceRecorded(changed)),
        record(3, ResearchEvent::SourceRecorded(mirrored)),
    ])
    .expect("identity alone controls source uniqueness");

    assert_eq!(state.source_count(), 3);
}

fn record(sequence: u64, event: ResearchEvent) -> ResearchRecord {
    ResearchRecord::new(sequence, event).expect("fixture record is valid")
}

fn source(id: SourceId, digest: [u8; 32], locator: &str) -> Source {
    Source::new(
        id,
        ContentDigest::sha256(digest),
        locator.to_owned(),
        Some("Research source".to_owned()),
        RetrievedAt::new("2026-08-29T10:00:00Z").expect("fixture time is valid"),
        MediaType::new("text/plain").expect("fixture media type is valid"),
    )
    .expect("source fixture is valid")
}

fn evidence(id: EvidenceId, source_id: SourceId, excerpt: &str) -> Evidence {
    Evidence::new(id, source_id, excerpt.to_owned()).expect("evidence fixture is valid")
}

fn claim(id: ClaimId, statement: &str, evidence_ids: Vec<EvidenceId>) -> Claim {
    Claim::new(id, statement.to_owned(), evidence_ids).expect("claim fixture is valid")
}

fn source_id(suffix: u8) -> SourceId {
    format!("00000000-0000-4000-8000-{suffix:012}")
        .parse()
        .expect("fixture source identifier is valid")
}

fn evidence_id(suffix: u8) -> EvidenceId {
    format!("00000000-0000-4000-8000-{suffix:012}")
        .parse()
        .expect("fixture evidence identifier is valid")
}

fn claim_id(suffix: u8) -> ClaimId {
    format!("00000000-0000-4000-8000-{suffix:012}")
        .parse()
        .expect("fixture claim identifier is valid")
}
