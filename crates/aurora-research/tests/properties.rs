use aurora_research::{
    Claim, ClaimId, ContentDigest, Evidence, EvidenceId, MediaType, ResearchEvent, ResearchRecord,
    ResearchState, RetrievedAt, Source, SourceId, TransitionError, decode_record, encode_record,
};
use proptest::prelude::*;
use uuid::Uuid;

prop_compose! {
    fn valid_history()
        (
            seed in any::<u128>(),
            digest in any::<[u8; 32]>(),
            locator in nonblank_text(47),
            excerpt in nonblank_text(63),
            statement in nonblank_text(63),
        ) -> Vec<ResearchRecord>
    {
        let source_id = source_id(seed);
        let evidence_id = evidence_id(seed.rotate_left(17));
        let claim_id = claim_id(seed.rotate_left(41));
        let source = Source::new(
            source_id,
            ContentDigest::sha256(digest),
            locator,
            None,
            RetrievedAt::new("2026-08-29T10:00:00Z").expect("fixed time is valid"),
            MediaType::new("text/plain").expect("fixed media type is valid"),
        )
        .expect("generated source is valid");
        let evidence = Evidence::new(evidence_id, source_id, excerpt)
            .expect("generated evidence is valid");
        let claim = Claim::new(claim_id, statement, vec![evidence_id])
            .expect("generated claim is valid");
        vec![
            record(1, ResearchEvent::SourceRecorded(source)),
            record(2, ResearchEvent::EvidenceRecorded(evidence)),
            record(3, ResearchEvent::ClaimProposed(claim)),
        ]
    }
}

proptest! {
    #[test]
    fn every_valid_record_survives_codec_round_trip(records in valid_history()) {
        for record in records {
            let encoded = encode_record(&record).expect("valid record encodes");
            let decoded = decode_record(&encoded).expect("encoded record decodes");
            prop_assert_eq!(decoded, record);
        }
    }

    #[test]
    fn replay_matches_incremental_application(records in valid_history()) {
        let replayed = ResearchState::reconstruct(records.clone())
            .expect("valid history reconstructs");
        let mut incremental = ResearchState::default();
        for record in records {
            incremental.apply(record).expect("valid record applies");
        }
        prop_assert_eq!(replayed, incremental);
    }

    #[test]
    fn codec_replay_preserves_research_state(records in valid_history()) {
        let expected = ResearchState::reconstruct(records.clone())
            .expect("valid history reconstructs");
        let decoded = records
            .iter()
            .map(|record| {
                let encoded = encode_record(record).expect("valid record encodes");
                decode_record(&encoded).expect("encoded record decodes")
            })
            .collect::<Vec<_>>();
        let actual = ResearchState::reconstruct(decoded)
            .expect("codec history reconstructs");
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn rejected_claim_never_changes_state(
        records in valid_history(),
        seed in any::<u128>(),
        statement in nonblank_text(63),
    ) {
        let mut state = ResearchState::reconstruct(records).expect("valid history reconstructs");
        let before = state.clone();
        let mut missing_seed = seed ^ 0xa5a5_a5a5_a5a5_a5a5_u128;
        let missing = loop {
            let candidate = evidence_id(missing_seed);
            if state.evidence(&candidate).is_none() {
                break candidate;
            }
            missing_seed = missing_seed.wrapping_add(1);
        };
        let mut claim_seed = seed ^ 0x5a5a_5a5a_5a5a_5a5a_u128;
        let candidate_id = loop {
            let candidate = claim_id(claim_seed);
            if state.claim(&candidate).is_none() {
                break candidate;
            }
            claim_seed = claim_seed.wrapping_add(1);
        };
        let candidate = Claim::new(
            candidate_id,
            statement,
            vec![missing],
        )
        .expect("generated claim is valid");

        let result = state.apply(record(4, ResearchEvent::ClaimProposed(candidate)));

        prop_assert_eq!(result, Err(TransitionError::UnknownEvidence(missing)));
        prop_assert_eq!(state, before);
    }

    #[test]
    fn claim_input_order_has_one_domain_and_wire_form(seed in any::<u128>()) {
        let first = evidence_id(seed);
        let second = evidence_id(seed ^ 1);
        let id = claim_id(seed.rotate_left(29));
        let forward = Claim::new(
            id,
            "Ordered evidence basis".to_owned(),
            vec![first, second],
        )
        .expect("forward claim is valid");
        let reverse = Claim::new(
            id,
            "Ordered evidence basis".to_owned(),
            vec![second, first],
        )
        .expect("reverse claim is valid");

        prop_assert_eq!(&forward, &reverse);
        let forward = encode_record(&record(1, ResearchEvent::ClaimProposed(forward)))
            .expect("forward claim encodes");
        let reverse = encode_record(&record(1, ResearchEvent::ClaimProposed(reverse)))
            .expect("reverse claim encodes");
        prop_assert_eq!(forward, reverse);
    }
}

fn nonblank_text(max_tail_length: usize) -> impl Strategy<Value = String> {
    (
        any::<char>().prop_filter("first character is not whitespace", |value| {
            !value.is_whitespace()
        }),
        prop::collection::vec(any::<char>(), 0..=max_tail_length),
    )
        .prop_map(|(first, tail)| std::iter::once(first).chain(tail).collect())
}

fn record(sequence: u64, event: ResearchEvent) -> ResearchRecord {
    ResearchRecord::new(sequence, event).expect("generated record is valid")
}

fn source_id(seed: u128) -> SourceId {
    uuid_v4(seed)
        .parse()
        .expect("generated source identifier is valid")
}

fn evidence_id(seed: u128) -> EvidenceId {
    uuid_v4(seed)
        .parse()
        .expect("generated evidence identifier is valid")
}

fn claim_id(seed: u128) -> ClaimId {
    uuid_v4(seed)
        .parse()
        .expect("generated claim identifier is valid")
}

fn uuid_v4(seed: u128) -> String {
    let mut bytes = seed.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).hyphenated().to_string()
}
