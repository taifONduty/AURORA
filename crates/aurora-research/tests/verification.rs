use aurora_research::{
    ClaimId, EvidenceAssessment, EvidenceId, EvidenceRelation, EvidenceSufficiency,
    VerificationAssessment, VerificationId, VerificationValidationError,
};

#[test]
fn support_contradiction_and_insufficiency_coexist_without_a_verdict() {
    let assessment = VerificationAssessment::new(
        verification_id(1),
        claim_id(2),
        vec![
            EvidenceAssessment::new(evidence_id(3), EvidenceRelation::Supports),
            EvidenceAssessment::new(evidence_id(4), EvidenceRelation::Contradicts),
        ],
        EvidenceSufficiency::Insufficient,
    )
    .expect("assessment is valid");

    assert_eq!(assessment.id(), &verification_id(1));
    assert_eq!(assessment.claim_id(), &claim_id(2));
    assert_eq!(
        assessment.relation(&evidence_id(3)),
        Some(EvidenceRelation::Supports)
    );
    assert_eq!(
        assessment.relation(&evidence_id(4)),
        Some(EvidenceRelation::Contradicts)
    );
    assert_eq!(assessment.sufficiency(), EvidenceSufficiency::Insufficient);
}

#[test]
fn every_relation_and_sufficiency_value_is_explicit() {
    let relations = [
        EvidenceRelation::Supports,
        EvidenceRelation::Contradicts,
        EvidenceRelation::Unclear,
        EvidenceRelation::Irrelevant,
    ];
    let sufficiencies = [
        EvidenceSufficiency::Sufficient,
        EvidenceSufficiency::Insufficient,
        EvidenceSufficiency::Indeterminate,
    ];

    for (index, sufficiency) in sufficiencies.into_iter().enumerate() {
        let evidence = relations
            .iter()
            .enumerate()
            .map(|(offset, relation)| {
                EvidenceAssessment::new(evidence_id((index * 10 + offset + 1) as u128), *relation)
            })
            .collect();
        let assessment = VerificationAssessment::new(
            verification_id(index as u128 + 1),
            claim_id(index as u128 + 20),
            evidence,
            sufficiency,
        )
        .expect("assessment is valid");

        assert_eq!(assessment.sufficiency(), sufficiency);
        assert_eq!(assessment.evidence_relations().count(), relations.len());
    }
}

#[test]
fn assessment_requires_distinct_nonempty_evidence() {
    assert_eq!(
        VerificationAssessment::new(
            verification_id(1),
            claim_id(2),
            Vec::new(),
            EvidenceSufficiency::Indeterminate,
        ),
        Err(VerificationValidationError::NoAssessedEvidence)
    );

    let repeated = evidence_id(3);
    assert_eq!(
        VerificationAssessment::new(
            verification_id(1),
            claim_id(2),
            vec![
                EvidenceAssessment::new(repeated, EvidenceRelation::Supports),
                EvidenceAssessment::new(repeated, EvidenceRelation::Unclear),
            ],
            EvidenceSufficiency::Indeterminate,
        ),
        Err(VerificationValidationError::DuplicateAssessedEvidence(
            repeated
        ))
    );
}

#[test]
fn evidence_relations_have_one_deterministic_order() {
    let first = evidence_id(3);
    let second = evidence_id(8);
    let assessment = VerificationAssessment::new(
        verification_id(1),
        claim_id(2),
        vec![
            EvidenceAssessment::new(second, EvidenceRelation::Unclear),
            EvidenceAssessment::new(first, EvidenceRelation::Supports),
        ],
        EvidenceSufficiency::Indeterminate,
    )
    .expect("assessment is valid");

    assert_eq!(
        assessment
            .evidence_relations()
            .map(|(id, relation)| (*id, relation))
            .collect::<Vec<_>>(),
        vec![
            (first, EvidenceRelation::Supports),
            (second, EvidenceRelation::Unclear)
        ]
    );
}

#[test]
fn verification_identity_is_an_opaque_uuid_v4() {
    let generated = VerificationId::generate();
    assert_eq!(
        generated.to_string().parse::<VerificationId>(),
        Ok(generated)
    );
    assert!("not-a-uuid".parse::<VerificationId>().is_err());
    assert!(
        "00000000-0000-0000-8000-000000000001"
            .parse::<VerificationId>()
            .is_err()
    );
}

fn verification_id(value: u128) -> VerificationId {
    uuid(value)
        .parse()
        .expect("verification identifier is valid")
}

fn claim_id(value: u128) -> ClaimId {
    uuid(value).parse().expect("claim identifier is valid")
}

fn evidence_id(value: u128) -> EvidenceId {
    uuid(value).parse().expect("evidence identifier is valid")
}

fn uuid(value: u128) -> String {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).hyphenated().to_string()
}
