use aurora_research::{
    Claim, ClaimId, ContentDigest, Evidence, EvidenceAssessment, EvidenceId, EvidenceRelation,
    EvidenceSufficiency, MediaType, ResearchEvent, ResearchRecord, ResearchState, RetrievedAt,
    Source, SourceId, VerificationAssessment, VerificationId, VerificationRecord,
    VerificationState, VerificationTransitionError, decode_verification_record,
    encode_verification_record,
};
use proptest::prelude::*;
use uuid::Uuid;

prop_compose! {
    fn valid_fixture()
        (
            seed in any::<u128>(),
            digest in any::<[u8; 32]>(),
            first_relation in relation(),
            second_relation in relation(),
            first_sufficiency in sufficiency(),
            second_sufficiency in sufficiency(),
        ) -> (ResearchState, Vec<VerificationRecord>)
    {
        let source_id = source_id(seed.rotate_left(7));
        let first_evidence_id = evidence_id(seed ^ 0x101);
        let second_evidence_id = evidence_id(seed ^ 0x202);
        let claim_id = claim_id(seed.rotate_left(31));
        let source = Source::new(
            source_id,
            ContentDigest::sha256(digest),
            "https://example.test/property-source".to_owned(),
            None,
            RetrievedAt::new("2026-08-29T10:00:00Z").expect("fixed time is valid"),
            MediaType::new("text/plain").expect("fixed media type is valid"),
        )
        .expect("source is valid");
        let first_evidence = Evidence::new(
            first_evidence_id,
            source_id,
            "First generated excerpt".to_owned(),
        )
        .expect("evidence is valid");
        let second_evidence = Evidence::new(
            second_evidence_id,
            source_id,
            "Second generated excerpt".to_owned(),
        )
        .expect("evidence is valid");
        let claim = Claim::new(
            claim_id,
            "Generated proposed claim".to_owned(),
            vec![first_evidence_id],
        )
        .expect("claim is valid");
        let research = ResearchState::reconstruct(vec![
            research_record(1, ResearchEvent::SourceRecorded(source)),
            research_record(2, ResearchEvent::EvidenceRecorded(first_evidence)),
            research_record(3, ResearchEvent::EvidenceRecorded(second_evidence)),
            research_record(4, ResearchEvent::ClaimProposed(claim)),
        ])
        .expect("research history reconstructs");
        let first = VerificationAssessment::new(
            verification_id(seed),
            claim_id,
            vec![
                EvidenceAssessment::new(first_evidence_id, first_relation),
                EvidenceAssessment::new(second_evidence_id, second_relation),
            ],
            first_sufficiency,
        )
        .expect("first assessment is valid");
        let second = VerificationAssessment::new(
            verification_id(seed ^ 1),
            claim_id,
            vec![EvidenceAssessment::new(first_evidence_id, second_relation)],
            second_sufficiency,
        )
        .expect("second assessment is valid");
        (
            research,
            vec![verification_record(1, first), verification_record(2, second)],
        )
    }
}

proptest! {
    #[test]
    fn every_valid_verification_record_survives_codec_round_trip(
        fixture in valid_fixture()
    ) {
        for record in fixture.1 {
            let encoded = encode_verification_record(&record).expect("record encodes");
            let decoded = decode_verification_record(&encoded).expect("record decodes");
            prop_assert_eq!(decoded, record);
        }
    }

    #[test]
    fn replay_matches_incremental_verification_application(
        fixture in valid_fixture()
    ) {
        let (research, records) = fixture;
        let replayed = VerificationState::reconstruct(&research, records.clone())
            .expect("history reconstructs");
        let mut incremental = VerificationState::default();
        for record in records {
            incremental.apply(&research, record).expect("record applies");
        }
        prop_assert_eq!(replayed, incremental);
    }

    #[test]
    fn codec_replay_preserves_verification_state(fixture in valid_fixture()) {
        let (research, records) = fixture;
        let expected = VerificationState::reconstruct(&research, records.clone())
            .expect("history reconstructs");
        let decoded = records
            .iter()
            .map(|record| {
                decode_verification_record(
                    &encode_verification_record(record).expect("record encodes")
                )
                .expect("record decodes")
            })
            .collect::<Vec<_>>();
        let actual = VerificationState::reconstruct(&research, decoded)
            .expect("decoded history reconstructs");
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn evidence_input_permutations_have_one_domain_and_wire_form(
        seed in any::<u128>(),
        first_relation in relation(),
        second_relation in relation(),
        sufficiency in sufficiency(),
    ) {
        let first_evidence = evidence_id(seed ^ 0x101);
        let second_evidence = evidence_id(seed ^ 0x202);
        let id = verification_id(seed.rotate_left(11));
        let claim = claim_id(seed.rotate_left(29));
        let forward = VerificationAssessment::new(
            id,
            claim,
            vec![
                EvidenceAssessment::new(first_evidence, first_relation),
                EvidenceAssessment::new(second_evidence, second_relation),
            ],
            sufficiency,
        )
        .expect("forward assessment is valid");
        let reverse = VerificationAssessment::new(
            id,
            claim,
            vec![
                EvidenceAssessment::new(second_evidence, second_relation),
                EvidenceAssessment::new(first_evidence, first_relation),
            ],
            sufficiency,
        )
        .expect("reverse assessment is valid");

        prop_assert_eq!(&forward, &reverse);
        let forward = encode_verification_record(&verification_record(1, forward))
            .expect("forward record encodes");
        let reverse = encode_verification_record(&verification_record(1, reverse))
            .expect("reverse record encodes");
        prop_assert_eq!(forward, reverse);
    }

    #[test]
    fn rejected_unknown_evidence_changes_neither_state(
        fixture in valid_fixture(),
        seed in any::<u128>(),
        relation in relation(),
        sufficiency in sufficiency(),
    ) {
        let (research, records) = fixture;
        let mut state = VerificationState::reconstruct(&research, records)
            .expect("history reconstructs");
        let research_before = research.clone();
        let state_before = state.clone();
        let missing = missing_evidence_id(&research, seed);
        let claim = *research.claims().next().expect("fixture claim exists").id();
        let candidate = VerificationAssessment::new(
            unused_verification_id(&state, seed),
            claim,
            vec![EvidenceAssessment::new(missing, relation)],
            sufficiency,
        )
        .expect("candidate shape is valid");
        let sequence = state.last_sequence() + 1;

        let result = state.apply(&research, verification_record(sequence, candidate));

        prop_assert_eq!(
            result,
            Err(VerificationTransitionError::UnknownEvidence(missing))
        );
        prop_assert_eq!(state, state_before);
        prop_assert_eq!(research, research_before);
    }
}

fn relation() -> impl Strategy<Value = EvidenceRelation> {
    prop_oneof![
        Just(EvidenceRelation::Supports),
        Just(EvidenceRelation::Contradicts),
        Just(EvidenceRelation::Unclear),
        Just(EvidenceRelation::Irrelevant),
    ]
}

fn sufficiency() -> impl Strategy<Value = EvidenceSufficiency> {
    prop_oneof![
        Just(EvidenceSufficiency::Sufficient),
        Just(EvidenceSufficiency::Insufficient),
        Just(EvidenceSufficiency::Indeterminate),
    ]
}

fn missing_evidence_id(research: &ResearchState, seed: u128) -> EvidenceId {
    let mut candidate = seed;
    loop {
        let id = evidence_id(candidate);
        if research.evidence(&id).is_none() {
            return id;
        }
        candidate = candidate.wrapping_add(1);
    }
}

fn unused_verification_id(state: &VerificationState, seed: u128) -> VerificationId {
    let mut candidate = seed;
    loop {
        let id = verification_id(candidate);
        if state.assessment(&id).is_none() {
            return id;
        }
        candidate = candidate.wrapping_add(1);
    }
}

fn verification_record(sequence: u64, assessment: VerificationAssessment) -> VerificationRecord {
    VerificationRecord::new(sequence, assessment).expect("verification record is valid")
}

fn research_record(sequence: u64, event: ResearchEvent) -> ResearchRecord {
    ResearchRecord::new(sequence, event).expect("research record is valid")
}

fn verification_id(seed: u128) -> VerificationId {
    uuid_v4(seed)
        .parse()
        .expect("verification identifier is valid")
}

fn source_id(seed: u128) -> SourceId {
    uuid_v4(seed).parse().expect("source identifier is valid")
}

fn evidence_id(seed: u128) -> EvidenceId {
    uuid_v4(seed).parse().expect("evidence identifier is valid")
}

fn claim_id(seed: u128) -> ClaimId {
    uuid_v4(seed).parse().expect("claim identifier is valid")
}

fn uuid_v4(seed: u128) -> String {
    let mut bytes = seed.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).hyphenated().to_string()
}
