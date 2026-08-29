use aurora_research::{
    Claim, ClaimId, ContentDigest, Evidence, EvidenceAssessment, EvidenceId, EvidenceRelation,
    EvidenceSufficiency, MediaType, ResearchEvent, ResearchRecord, ResearchState, RetrievedAt,
    Source, SourceId, VerificationAssessment, VerificationId, VerificationRecord,
    VerificationState, VerificationTransitionError,
};

#[test]
fn mixed_assessment_is_admitted_against_exact_research_inputs() {
    let research = research_state();
    let assessment = assessment(
        1,
        21,
        &[
            (11, EvidenceRelation::Supports),
            (12, EvidenceRelation::Contradicts),
        ],
        EvidenceSufficiency::Sufficient,
    );
    let mut state = VerificationState::default();

    state
        .apply(&research, record(1, assessment.clone()))
        .expect("assessment applies");

    assert_eq!(state.last_sequence(), 1);
    assert_eq!(state.assessment(&verification_id(1)), Some(&assessment));
    assert_eq!(state.assessments().count(), 1);
    assert_eq!(
        state
            .assessment(&verification_id(1))
            .expect("assessment exists")
            .relation(&evidence_id(12)),
        Some(EvidenceRelation::Contradicts)
    );
}

#[test]
fn multiple_assessments_of_one_claim_remain_independent() {
    let research = research_state();
    let records = vec![
        record(
            1,
            assessment(
                1,
                21,
                &[(11, EvidenceRelation::Supports)],
                EvidenceSufficiency::Insufficient,
            ),
        ),
        record(
            2,
            assessment(
                2,
                21,
                &[(12, EvidenceRelation::Contradicts)],
                EvidenceSufficiency::Indeterminate,
            ),
        ),
    ];

    let state = VerificationState::reconstruct(&research, records).expect("history is valid");

    assert_eq!(state.assessments().count(), 2);
    assert_eq!(
        state
            .assessment(&verification_id(1))
            .expect("first assessment exists")
            .sufficiency(),
        EvidenceSufficiency::Insufficient
    );
    assert_eq!(
        state
            .assessment(&verification_id(2))
            .expect("second assessment exists")
            .sufficiency(),
        EvidenceSufficiency::Indeterminate
    );
}

#[test]
fn unknown_claim_is_rejected_without_mutating_either_state() {
    let research = research_state();
    let research_before = research.clone();
    let mut state = VerificationState::default();
    let state_before = state.clone();
    let missing = claim_id(99);

    assert_eq!(
        state.apply(
            &research,
            record(
                1,
                VerificationAssessment::new(
                    verification_id(1),
                    missing,
                    vec![EvidenceAssessment::new(
                        evidence_id(11),
                        EvidenceRelation::Supports,
                    )],
                    EvidenceSufficiency::Indeterminate,
                )
                .expect("assessment shape is valid"),
            ),
        ),
        Err(VerificationTransitionError::UnknownClaim(missing))
    );
    assert_eq!(state, state_before);
    assert_eq!(research, research_before);
}

#[test]
fn unknown_evidence_is_rejected_without_mutating_either_state() {
    let research = research_state();
    let research_before = research.clone();
    let mut state = VerificationState::default();
    let state_before = state.clone();
    let missing = evidence_id(99);

    assert_eq!(
        state.apply(
            &research,
            record(
                1,
                assessment(
                    1,
                    21,
                    &[(99, EvidenceRelation::Unclear)],
                    EvidenceSufficiency::Insufficient,
                ),
            ),
        ),
        Err(VerificationTransitionError::UnknownEvidence(missing))
    );
    assert_eq!(state, state_before);
    assert_eq!(research, research_before);
}

#[test]
fn evidence_outside_the_claim_proposal_basis_may_be_assessed() {
    let research = research_state();
    let claim = research.claim(&claim_id(21)).expect("claim exists");
    assert!(!claim.evidence_ids().contains(&evidence_id(13)));
    let mut state = VerificationState::default();

    state
        .apply(
            &research,
            record(
                1,
                assessment(
                    1,
                    21,
                    &[(13, EvidenceRelation::Contradicts)],
                    EvidenceSufficiency::Insufficient,
                ),
            ),
        )
        .expect("additional evidence may be assessed");

    assert_eq!(
        state
            .assessment(&verification_id(1))
            .expect("assessment exists")
            .relation(&evidence_id(13)),
        Some(EvidenceRelation::Contradicts)
    );
}

#[test]
fn one_evidence_item_can_bear_differently_on_two_claims() {
    let research = research_state();
    let state = VerificationState::reconstruct(
        &research,
        vec![
            record(
                1,
                assessment(
                    1,
                    21,
                    &[(11, EvidenceRelation::Supports)],
                    EvidenceSufficiency::Sufficient,
                ),
            ),
            record(
                2,
                assessment(
                    2,
                    22,
                    &[(11, EvidenceRelation::Contradicts)],
                    EvidenceSufficiency::Sufficient,
                ),
            ),
        ],
    )
    .expect("history is valid");

    assert_eq!(
        state
            .assessment(&verification_id(1))
            .expect("first assessment exists")
            .relation(&evidence_id(11)),
        Some(EvidenceRelation::Supports)
    );
    assert_eq!(
        state
            .assessment(&verification_id(2))
            .expect("second assessment exists")
            .relation(&evidence_id(11)),
        Some(EvidenceRelation::Contradicts)
    );
}

#[test]
fn duplicate_identity_and_sequence_gap_are_non_mutating() {
    let research = research_state();
    let first = assessment(
        1,
        21,
        &[(11, EvidenceRelation::Supports)],
        EvidenceSufficiency::Sufficient,
    );
    let mut state = VerificationState::reconstruct(&research, vec![record(1, first.clone())])
        .expect("first assessment applies");
    let before = state.clone();

    assert_eq!(
        state.apply(&research, record(3, first.clone())),
        Err(VerificationTransitionError::Sequence {
            expected: 2,
            actual: 3,
        })
    );
    assert_eq!(state, before);
    assert_eq!(
        state.apply(&research, record(2, first)),
        Err(VerificationTransitionError::DuplicateVerification(
            verification_id(1)
        ))
    );
    assert_eq!(state, before);
}

#[test]
fn reconstruction_matches_incremental_application() {
    let research = research_state();
    let records = vec![
        record(
            1,
            assessment(
                1,
                21,
                &[(11, EvidenceRelation::Supports)],
                EvidenceSufficiency::Insufficient,
            ),
        ),
        record(
            2,
            assessment(
                2,
                22,
                &[(12, EvidenceRelation::Unclear)],
                EvidenceSufficiency::Indeterminate,
            ),
        ),
    ];
    let replayed =
        VerificationState::reconstruct(&research, records.clone()).expect("history reconstructs");
    let mut incremental = VerificationState::default();
    for record in records {
        incremental
            .apply(&research, record)
            .expect("record applies");
    }

    assert_eq!(replayed, incremental);
}

fn research_state() -> ResearchState {
    ResearchState::reconstruct(vec![
        research_record(1, ResearchEvent::SourceRecorded(source(1))),
        research_record(2, ResearchEvent::EvidenceRecorded(evidence(11, 1))),
        research_record(3, ResearchEvent::EvidenceRecorded(evidence(12, 1))),
        research_record(4, ResearchEvent::EvidenceRecorded(evidence(13, 1))),
        research_record(5, ResearchEvent::ClaimProposed(claim(21, 11))),
        research_record(6, ResearchEvent::ClaimProposed(claim(22, 12))),
    ])
    .expect("research fixture reconstructs")
}

fn source(id: u128) -> Source {
    Source::new(
        source_id(id),
        ContentDigest::sha256([id as u8; 32]),
        format!("https://example.test/source/{id}"),
        None,
        RetrievedAt::new("2026-08-29T10:00:00Z").expect("time is valid"),
        MediaType::new("text/plain").expect("media type is valid"),
    )
    .expect("source is valid")
}

fn evidence(id: u128, source: u128) -> Evidence {
    Evidence::new(
        evidence_id(id),
        source_id(source),
        format!("Evidence excerpt {id}"),
    )
    .expect("evidence is valid")
}

fn claim(id: u128, evidence: u128) -> Claim {
    Claim::new(
        claim_id(id),
        format!("Proposed claim {id}"),
        vec![evidence_id(evidence)],
    )
    .expect("claim is valid")
}

fn assessment(
    verification: u128,
    claim: u128,
    evidence: &[(u128, EvidenceRelation)],
    sufficiency: EvidenceSufficiency,
) -> VerificationAssessment {
    VerificationAssessment::new(
        verification_id(verification),
        claim_id(claim),
        evidence
            .iter()
            .map(|(id, relation)| EvidenceAssessment::new(evidence_id(*id), *relation))
            .collect(),
        sufficiency,
    )
    .expect("assessment is valid")
}

fn record(sequence: u64, assessment: VerificationAssessment) -> VerificationRecord {
    VerificationRecord::new(sequence, assessment).expect("verification record is valid")
}

fn research_record(sequence: u64, event: ResearchEvent) -> ResearchRecord {
    ResearchRecord::new(sequence, event).expect("research record is valid")
}

fn verification_id(value: u128) -> VerificationId {
    uuid(value)
        .parse()
        .expect("verification identifier is valid")
}

fn source_id(value: u128) -> SourceId {
    uuid(value).parse().expect("source identifier is valid")
}

fn evidence_id(value: u128) -> EvidenceId {
    uuid(value).parse().expect("evidence identifier is valid")
}

fn claim_id(value: u128) -> ClaimId {
    uuid(value).parse().expect("claim identifier is valid")
}

fn uuid(value: u128) -> String {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).hyphenated().to_string()
}
