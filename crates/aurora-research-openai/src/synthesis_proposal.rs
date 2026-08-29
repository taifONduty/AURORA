#![allow(dead_code)]

use std::str::FromStr;

use aurora_research::{
    ClaimId, SynthesisAssertionDraft, SynthesisDraft, SynthesisSectionDraft,
    SynthesisValidationError,
};
use serde::Deserialize;
use serde_json::{Value, json};

const SECTION_LIMIT: usize = 8;
const ASSERTION_LIMIT: usize = 16;
const CLAIM_REFERENCE_LIMIT: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum SynthesisProposalError {
    #[error("synthesis proposal is not valid JSON")]
    InvalidJson,
    #[error("synthesis proposal violates its required shape")]
    InvalidShape,
    #[error("synthesis proposal contains blank assertion text")]
    BlankAssertion,
    #[error("synthesis proposal is invalid for the research domain: {0}")]
    InvalidReport(SynthesisValidationError),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SynthesisProposal {
    sections: Vec<SectionProposal>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SectionProposal {
    assertions: Vec<AssertionProposal>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssertionProposal {
    text: String,
    claim_ids: Vec<String>,
}

pub(super) fn decode_synthesis(input: &str) -> Result<SynthesisDraft, SynthesisProposalError> {
    let proposal: SynthesisProposal = serde_json::from_str(input).map_err(|error| {
        if error.is_syntax() || error.is_eof() {
            SynthesisProposalError::InvalidJson
        } else {
            SynthesisProposalError::InvalidShape
        }
    })?;
    let sections = proposal
        .sections
        .into_iter()
        .map(decode_section)
        .collect::<Result<Vec<_>, _>>()?;
    SynthesisDraft::new(sections).map_err(SynthesisProposalError::InvalidReport)
}

fn decode_section(
    section: SectionProposal,
) -> Result<SynthesisSectionDraft, SynthesisProposalError> {
    let assertions = section
        .assertions
        .into_iter()
        .map(decode_assertion)
        .collect::<Result<Vec<_>, _>>()?;
    SynthesisSectionDraft::new(assertions).map_err(SynthesisProposalError::InvalidReport)
}

fn decode_assertion(
    assertion: AssertionProposal,
) -> Result<SynthesisAssertionDraft, SynthesisProposalError> {
    if assertion.text.trim().is_empty() {
        return Err(SynthesisProposalError::BlankAssertion);
    }
    let claim_ids = assertion
        .claim_ids
        .into_iter()
        .map(|raw| {
            ClaimId::from_str(&raw).map_err(|error| {
                SynthesisProposalError::InvalidReport(
                    SynthesisValidationError::InvalidClaimIdentifier(error),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    SynthesisAssertionDraft::new(assertion.text, claim_ids)
        .map_err(SynthesisProposalError::InvalidReport)
}

pub(super) fn synthesis_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["sections"],
        "properties": {
            "sections": {
                "type": "array",
                "minItems": 1,
                "maxItems": SECTION_LIMIT,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["assertions"],
                    "properties": {
                        "assertions": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": ASSERTION_LIMIT,
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["text", "claim_ids"],
                                "properties": {
                                    "text": {"type": "string", "minLength": 1},
                                    "claim_ids": {
                                        "type": "array",
                                        "minItems": 1,
                                        "maxItems": CLAIM_REFERENCE_LIMIT,
                                        "items": {"type": "string"}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}
