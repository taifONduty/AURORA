use std::str::FromStr;

use aurora_research::{ClaimId, SynthesisValidationError};

use crate::synthesis_proposal::{SynthesisProposalError, decode_synthesis, synthesis_schema};

fn valid_claim_id() -> ClaimId {
    ClaimId::from_str("123e4567-e89b-42d3-a456-426614174000").expect("fixture UUID is v4")
}

fn valid_proposal() -> String {
    format!(
        r#"{{"sections":[{{"assertions":[{{"text":"A grounded finding.","claim_ids":["{}"]}}]}}]}}"#,
        valid_claim_id()
    )
}

#[test]
fn strict_synthesis_proposal_rejects_shape_and_unknown_fields_at_every_level() {
    for input in [
        r#"{}"#.to_owned(),
        r#"{"sections":[],"extra":true}"#.to_owned(),
        r#"{"sections":[{}]}"#.to_owned(),
        r#"{"sections":[{"heading":"model-authored","assertions":[]}]}"#.to_owned(),
        r#"{"sections":[{"assertions":[],"extra":true}]}"#.to_owned(),
        format!(
            r#"{{"sections":[{{"assertions":[{{"text":"t","claim_ids":["{}"],"extra":true}}]}}]}}"#,
            valid_claim_id()
        ),
        format!(
            r#"{{"sections":[{{"assertions":[{{"text":"t","claim_ids":["{}"],"citations":[]}}]}}]}}"#,
            valid_claim_id()
        ),
    ] {
        assert_eq!(
            decode_synthesis(&input),
            Err(SynthesisProposalError::InvalidShape)
        );
    }
}

#[test]
fn strict_synthesis_proposal_maps_local_limits_and_identifier_failures_to_domain_errors() {
    assert_eq!(
        decode_synthesis(r#"{"sections":[]}"#),
        Err(SynthesisProposalError::InvalidReport(
            SynthesisValidationError::DraftHasNoSections
        ))
    );
    let sections = (0..9)
        .map(|_| r#"{"assertions":[{"text":"t","claim_ids":["123e4567-e89b-42d3-a456-426614174000"]}]}"#)
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        decode_synthesis(&format!(r#"{{"sections":[{sections}]}}"#)),
        Err(SynthesisProposalError::InvalidReport(
            SynthesisValidationError::TooManyDraftSections
        ))
    );
    assert_eq!(
        decode_synthesis(r#"{"sections":[{"assertions":[{"text":"t","claim_ids":[]}]}]}"#),
        Err(SynthesisProposalError::InvalidReport(
            SynthesisValidationError::AssertionHasNoClaims
        ))
    );
    assert_eq!(
        decode_synthesis(
            r#"{"sections":[{"assertions":[{"text":"t","claim_ids":["not-a-uuid"]}]}]}"#
        ),
        Err(SynthesisProposalError::InvalidReport(
            SynthesisValidationError::InvalidClaimIdentifier(
                aurora_research::IdentityError::InvalidUuid
            )
        ))
    );
    assert_eq!(
        decode_synthesis(
            r#"{"sections":[{"assertions":[{"text":"t","claim_ids":["123e4567-e89b-12d3-a456-426614174000"]}]}]}"#
        ),
        Err(SynthesisProposalError::InvalidReport(
            SynthesisValidationError::InvalidClaimIdentifier(
                aurora_research::IdentityError::NotVersion4
            )
        ))
    );
    assert_eq!(
        decode_synthesis(
            r#"{"sections":[{"assertions":[{"text":"t","claim_ids":["123e4567-e89b-42d3-c456-426614174000"]}]}]}"#
        ),
        Err(SynthesisProposalError::InvalidReport(
            SynthesisValidationError::InvalidClaimIdentifier(
                aurora_research::IdentityError::NotRfc4122
            )
        ))
    );
}

#[test]
fn strict_synthesis_proposal_rejects_all_local_cardinality_and_text_boundaries() {
    assert_eq!(
        decode_synthesis(
            r#"{"sections":[{"assertions":[{"text":" ","claim_ids":["123e4567-e89b-42d3-a456-426614174000"]}]}]}"#
        ),
        Err(SynthesisProposalError::BlankAssertion)
    );
    assert_eq!(
        decode_synthesis(&format!(
            r#"{{"sections":[{{"assertions":[{{"text":"{}","claim_ids":["123e4567-e89b-42d3-a456-426614174000"]}}]}}]}}"#,
            "t".repeat(4_097)
        )),
        Err(SynthesisProposalError::InvalidReport(
            SynthesisValidationError::AssertionTooLong
        ))
    );
    let assertions = (0..17)
        .map(|_| r#"{"text":"t","claim_ids":["123e4567-e89b-42d3-a456-426614174000"]}"#)
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        decode_synthesis(&format!(
            r#"{{"sections":[{{"assertions":[{assertions}]}}]}}"#
        )),
        Err(SynthesisProposalError::InvalidReport(
            SynthesisValidationError::TooManySectionAssertions
        ))
    );
    let sections = (0..8)
        .map(|_| {
            let assertions = (0..9)
                .map(|_| r#"{"text":"t","claim_ids":["123e4567-e89b-42d3-a456-426614174000"]}"#)
                .collect::<Vec<_>>()
                .join(",");
            format!(r#"{{"assertions":[{assertions}]}}"#)
        })
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        decode_synthesis(&format!(r#"{{"sections":[{sections}]}}"#)),
        Err(SynthesisProposalError::InvalidReport(
            SynthesisValidationError::TooManyDraftAssertions
        ))
    );
    let claim_ids = std::iter::repeat_n("\"123e4567-e89b-42d3-a456-426614174000\"", 9)
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        decode_synthesis(&format!(
            r#"{{"sections":[{{"assertions":[{{"text":"t","claim_ids":[{claim_ids}]}}]}}]}}"#
        )),
        Err(SynthesisProposalError::InvalidReport(
            SynthesisValidationError::TooManyAssertionClaims
        ))
    );
    assert_eq!(
        decode_synthesis(
            r#"{"sections":[{"assertions":[{"text":"t","claim_ids":["123e4567-e89b-42d3-a456-426614174000","123e4567-e89b-42d3-a456-426614174000"]}]}]}"#
        ),
        Err(SynthesisProposalError::InvalidReport(
            SynthesisValidationError::DuplicateAssertionClaim(valid_claim_id())
        ))
    );
}

#[test]
fn strict_synthesis_proposal_preserves_typed_claim_ids_and_schema_shape() {
    let draft = decode_synthesis(&valid_proposal()).expect("valid proposal decodes");
    let assertion = draft
        .sections()
        .next()
        .expect("section exists")
        .assertions()
        .next()
        .expect("assertion exists");
    assert_eq!(
        assertion.claim_ids().collect::<Vec<_>>(),
        vec![&valid_claim_id()]
    );

    let schema = synthesis_schema();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["sections"]["minItems"], 1);
    assert_eq!(schema["properties"]["sections"]["maxItems"], 8);
    assert!(
        schema["properties"]["sections"]["items"]["properties"]
            .get("heading")
            .is_none()
    );
    assert_eq!(
        schema["properties"]["sections"]["items"]["properties"]["assertions"]["items"]["properties"]
            ["claim_ids"]["minItems"],
        1
    );
    assert!(!schema.to_string().contains("maxLength"));
}
