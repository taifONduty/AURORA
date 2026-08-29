#[path = "synthesis_report.rs"]
mod synthesis_report;

use aurora_research::{
    ClaimId, GroundedReport, SynthesisAssertionDraft, SynthesisSectionDraft,
    SynthesisValidationError,
};

#[test]
fn changing_only_assertion_prose_preserves_presentation_and_citations() {
    let basis = synthesis_report::fixture_basis();
    let first = GroundedReport::from_basis(&basis, three_claim_draft("first prose"))
        .expect("first draft grounds");
    let second = GroundedReport::from_basis(
        &basis,
        aurora_research::SynthesisDraft::new(vec![
            SynthesisSectionDraft::new(vec![
                SynthesisAssertionDraft::new(
                    "different prose".to_owned(),
                    vec![claim_id(1), claim_id(2), claim_id(3)],
                )
                .expect("assertion is valid"),
            ])
            .expect("section is valid"),
        ])
        .expect("draft is valid"),
    )
    .expect("second draft grounds");

    let first_assertion = first
        .sections()
        .next()
        .expect("section exists")
        .assertions()
        .next()
        .expect("assertion exists");
    let second_assertion = second
        .sections()
        .next()
        .expect("section exists")
        .assertions()
        .next()
        .expect("assertion exists");
    assert_eq!(
        first_assertion.presentation(),
        second_assertion.presentation()
    );
    assert_eq!(
        first_assertion.citations().collect::<Vec<_>>(),
        second_assertion.citations().collect::<Vec<_>>()
    );
    assert_eq!(
        first.citations().collect::<Vec<_>>(),
        second.citations().collect::<Vec<_>>()
    );
}

#[test]
fn grounding_does_not_change_its_basis_and_citation_order_is_repeatable() {
    let basis = synthesis_report::fixture_basis();
    let before = basis.clone();
    let draft = three_claim_draft("same prose");
    let first = GroundedReport::from_basis(&basis, draft.clone()).expect("draft grounds");
    let second = GroundedReport::from_basis(&basis, draft).expect("draft grounds");

    assert_eq!(basis, before);
    assert_eq!(
        first.citations().collect::<Vec<_>>(),
        second.citations().collect::<Vec<_>>()
    );
    assert_eq!(first.render(), second.render());
}

#[test]
fn invalid_drafts_never_return_a_partial_report() {
    let basis = synthesis_report::fixture_basis();
    let unknown = synthesis_report::one_assertion_draft(claim_id(99));
    let unassessed = synthesis_report::one_assertion_draft(claim_id(4));

    assert!(matches!(
        GroundedReport::from_basis(&basis, unknown),
        Err(SynthesisValidationError::UnknownClaim(_))
    ));
    assert!(matches!(
        GroundedReport::from_basis(&basis, unassessed),
        Err(SynthesisValidationError::UnassessedClaim(_))
    ));
}

fn claim_id(value: u128) -> ClaimId {
    use std::str::FromStr;

    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    ClaimId::from_str(&uuid::Uuid::from_bytes(bytes).to_string()).expect("claim id is valid")
}

fn three_claim_draft(prose: &str) -> aurora_research::SynthesisDraft {
    aurora_research::SynthesisDraft::new(vec![
        SynthesisSectionDraft::new(vec![
            SynthesisAssertionDraft::new(
                prose.to_owned(),
                vec![claim_id(1), claim_id(2), claim_id(3)],
            )
            .expect("assertion is valid"),
        ])
        .expect("section is valid"),
    ])
    .expect("draft is valid")
}
