use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

use crate::{
    Claim, ClaimId, Evidence, EvidenceId, EvidenceRelation, EvidenceSufficiency, IdentityError,
    ResearchControlState, ResearchControlStatus, ResearchGapCause, ResearchGapState,
    ResearchGapStatus, ResearchStopReason, Source, SourceId, VerificationAssessment,
    VerificationId,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SynthesisReportScope {
    Complete,
    Partial(ResearchStopReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimPresentation {
    Established,
    Unresolved,
    Contested,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroundingCitation {
    claim_id: ClaimId,
    provenance: Vec<(VerificationId, EvidenceRelation)>,
    evidence: Evidence,
    source: Source,
}

impl GroundingCitation {
    pub const fn claim_id(&self) -> &ClaimId {
        &self.claim_id
    }

    pub fn provenance(&self) -> impl Iterator<Item = (&VerificationId, EvidenceRelation)> {
        self.provenance
            .iter()
            .map(|(verification_id, relation)| (verification_id, *relation))
    }

    pub const fn is_fallback(&self) -> bool {
        self.provenance.is_empty()
    }

    pub const fn evidence(&self) -> &Evidence {
        &self.evidence
    }

    pub const fn source(&self) -> &Source {
        &self.source
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SynthesisClaimBasis {
    claim: Claim,
    presentation: ClaimPresentation,
    assessments: Vec<VerificationAssessment>,
    gaps: Vec<ResearchGapState>,
    evidence: BTreeMap<EvidenceId, Evidence>,
    sources: BTreeMap<SourceId, Source>,
    citations: Vec<GroundingCitation>,
}

impl SynthesisClaimBasis {
    pub const fn claim(&self) -> &Claim {
        &self.claim
    }

    pub const fn presentation(&self) -> ClaimPresentation {
        self.presentation
    }

    pub fn assessments(&self) -> impl Iterator<Item = &VerificationAssessment> {
        self.assessments.iter()
    }

    pub fn gaps(&self) -> impl Iterator<Item = &ResearchGapState> {
        self.gaps.iter()
    }

    pub fn evidence(&self, id: &EvidenceId) -> Option<&Evidence> {
        self.evidence.get(id)
    }

    pub fn evidence_items(&self) -> impl Iterator<Item = &Evidence> {
        self.evidence.values()
    }

    pub fn source(&self, id: &SourceId) -> Option<&Source> {
        self.sources.get(id)
    }

    pub fn sources(&self) -> impl Iterator<Item = &Source> {
        self.sources.values()
    }

    pub fn citations(&self) -> impl Iterator<Item = &GroundingCitation> {
        self.citations.iter()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SynthesisBasis {
    question: String,
    scope: SynthesisReportScope,
    known_claim_ids: BTreeSet<ClaimId>,
    claims: BTreeMap<ClaimId, SynthesisClaimBasis>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SynthesisValidationError {
    #[error("research is not terminal")]
    ResearchNotTerminal,
    #[error("failed research cannot be synthesized")]
    FailedResearch,
    #[error("research has no reportable assessed claims")]
    NoReportableClaims,
    #[error("assertion contains an invalid claim identifier: {0}")]
    InvalidClaimIdentifier(IdentityError),
    #[error("draft has no sections")]
    DraftHasNoSections,
    #[error("draft has more than eight sections")]
    TooManyDraftSections,
    #[error("draft has more than sixty-four assertions")]
    TooManyDraftAssertions,
    #[error("section has no assertions")]
    SectionHasNoAssertions,
    #[error("section has more than sixteen assertions")]
    TooManySectionAssertions,
    #[error("assertion exceeds 4096 UTF-8 bytes")]
    AssertionTooLong,
    #[error("assertion text is blank")]
    BlankAssertion,
    #[error("assertion has no claim references")]
    AssertionHasNoClaims,
    #[error("assertion has more than eight claim references")]
    TooManyAssertionClaims,
    #[error("assertion repeats claim identifier {0}")]
    DuplicateAssertionClaim(ClaimId),
    #[error("assertion references unknown claim identifier {0}")]
    UnknownClaim(ClaimId),
    #[error("assertion references unassessed claim identifier {0}")]
    UnassessedClaim(ClaimId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SynthesisAssertionDraft {
    text: String,
    claim_ids: Vec<ClaimId>,
}

impl SynthesisAssertionDraft {
    pub fn new(text: String, claim_ids: Vec<ClaimId>) -> Result<Self, SynthesisValidationError> {
        if text.trim().is_empty() {
            return Err(SynthesisValidationError::BlankAssertion);
        }
        if text.len() > 4_096 {
            return Err(SynthesisValidationError::AssertionTooLong);
        }
        if claim_ids.is_empty() {
            return Err(SynthesisValidationError::AssertionHasNoClaims);
        }
        if claim_ids.len() > 8 {
            return Err(SynthesisValidationError::TooManyAssertionClaims);
        }
        let mut distinct = BTreeSet::new();
        for id in &claim_ids {
            if !distinct.insert(*id) {
                return Err(SynthesisValidationError::DuplicateAssertionClaim(*id));
            }
        }
        Ok(Self { text, claim_ids })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn claim_ids(&self) -> impl Iterator<Item = &ClaimId> {
        self.claim_ids.iter()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SynthesisSectionDraft {
    assertions: Vec<SynthesisAssertionDraft>,
}

impl SynthesisSectionDraft {
    pub fn new(assertions: Vec<SynthesisAssertionDraft>) -> Result<Self, SynthesisValidationError> {
        if assertions.is_empty() {
            return Err(SynthesisValidationError::SectionHasNoAssertions);
        }
        if assertions.len() > 16 {
            return Err(SynthesisValidationError::TooManySectionAssertions);
        }
        Ok(Self { assertions })
    }

    pub fn assertions(&self) -> impl Iterator<Item = &SynthesisAssertionDraft> {
        self.assertions.iter()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SynthesisDraft {
    sections: Vec<SynthesisSectionDraft>,
}

impl SynthesisDraft {
    pub fn new(sections: Vec<SynthesisSectionDraft>) -> Result<Self, SynthesisValidationError> {
        if sections.is_empty() {
            return Err(SynthesisValidationError::DraftHasNoSections);
        }
        if sections.len() > 8 {
            return Err(SynthesisValidationError::TooManyDraftSections);
        }
        if sections
            .iter()
            .map(|section| section.assertions.len())
            .sum::<usize>()
            > 64
        {
            return Err(SynthesisValidationError::TooManyDraftAssertions);
        }
        Ok(Self { sections })
    }

    pub fn sections(&self) -> impl Iterator<Item = &SynthesisSectionDraft> {
        self.sections.iter()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroundedAssertion {
    text: String,
    claim_ids: Vec<ClaimId>,
    presentation: ClaimPresentation,
    citations: Vec<GroundingCitation>,
}

impl GroundedAssertion {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn claim_ids(&self) -> impl Iterator<Item = &ClaimId> {
        self.claim_ids.iter()
    }

    pub const fn presentation(&self) -> ClaimPresentation {
        self.presentation
    }

    pub fn citations(&self) -> impl Iterator<Item = &GroundingCitation> {
        self.citations.iter()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroundedReportSection {
    assertions: Vec<GroundedAssertion>,
}

impl GroundedReportSection {
    pub fn assertions(&self) -> impl Iterator<Item = &GroundedAssertion> {
        self.assertions.iter()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroundedReport {
    question: String,
    scope: SynthesisReportScope,
    sections: Vec<GroundedReportSection>,
    citations: Vec<GroundingCitation>,
}

impl GroundedReport {
    pub fn from_basis(
        basis: &SynthesisBasis,
        draft: SynthesisDraft,
    ) -> Result<Self, SynthesisValidationError> {
        let sections = draft
            .sections
            .into_iter()
            .map(|section| ground_section(basis, section))
            .collect::<Result<Vec<_>, _>>()?;
        let mut citations = Vec::new();
        for citation in sections
            .iter()
            .flat_map(|section| section.assertions.iter())
            .flat_map(|assertion| assertion.citations.iter())
        {
            if citations.iter().all(|existing: &GroundingCitation| {
                existing.claim_id() != citation.claim_id()
                    || existing.evidence().id() != citation.evidence().id()
            }) {
                citations.push(citation.clone());
            }
        }
        Ok(Self {
            question: basis.question.clone(),
            scope: basis.scope.clone(),
            sections,
            citations,
        })
    }

    pub fn question(&self) -> &str {
        &self.question
    }

    pub const fn scope(&self) -> &SynthesisReportScope {
        &self.scope
    }

    pub fn sections(&self) -> impl Iterator<Item = &GroundedReportSection> {
        self.sections.iter()
    }

    pub fn citations(&self) -> impl Iterator<Item = &GroundingCitation> {
        self.citations.iter()
    }

    pub fn render(&self) -> String {
        let source_registry = evidence_registry(&self.citations);
        let mut output = format!(
            "Research report\nQuestion: {}\nStatus: {}\nLimitation: {}\n",
            json_string(&self.question),
            status_label(&self.scope),
            limitation(&self.scope),
        );
        for (section_number, section) in self.sections.iter().enumerate() {
            output.push_str(&format!("Section {}\n", section_number + 1));
            for assertion in &section.assertions {
                output.push_str(presentation_label(assertion.presentation));
                output.push_str(": ");
                output.push_str(&json_string(&assertion.text));
                let mut marked_evidence = BTreeSet::new();
                for citation in &assertion.citations {
                    if !marked_evidence.insert(*citation.evidence().id()) {
                        continue;
                    }
                    let number = source_registry
                        .iter()
                        .position(|candidate| candidate.evidence().id() == citation.evidence().id())
                        .expect("grounded assertion citations are present in the report")
                        + 1;
                    output.push_str(&format!(" [{number}]"));
                }
                output.push('\n');
            }
        }
        output.push_str("Sources\n");
        for (number, citation) in source_registry.iter().enumerate() {
            output.push_str(&format!(
                "[{}] title={} locator={} sha256={} source={} evidence={}\n",
                number + 1,
                citation
                    .source()
                    .title()
                    .map_or_else(|| "null".to_owned(), json_string),
                json_string(citation.source().locator()),
                digest_hex(citation.source().content_digest()),
                citation.source().id(),
                citation.evidence().id(),
            ));
        }
        output
    }
}

fn evidence_registry(citations: &[GroundingCitation]) -> Vec<&GroundingCitation> {
    let mut evidence_ids = BTreeSet::new();
    citations
        .iter()
        .filter(|citation| evidence_ids.insert(*citation.evidence().id()))
        .collect()
}

fn ground_section(
    basis: &SynthesisBasis,
    section: SynthesisSectionDraft,
) -> Result<GroundedReportSection, SynthesisValidationError> {
    let assertions = section
        .assertions
        .into_iter()
        .map(|assertion| ground_assertion(basis, assertion))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(GroundedReportSection { assertions })
}

fn ground_assertion(
    basis: &SynthesisBasis,
    assertion: SynthesisAssertionDraft,
) -> Result<GroundedAssertion, SynthesisValidationError> {
    let mut presentation = ClaimPresentation::Established;
    let mut citations = Vec::new();
    for claim_id in &assertion.claim_ids {
        let Some(claim) = basis.claim(claim_id) else {
            return Err(if basis.is_known_claim(claim_id) {
                SynthesisValidationError::UnassessedClaim(*claim_id)
            } else {
                SynthesisValidationError::UnknownClaim(*claim_id)
            });
        };
        presentation = weaker_presentation(presentation, claim.presentation());
        for citation in claim.citations() {
            if !citations.contains(citation) {
                citations.push(citation.clone());
            }
        }
    }
    citations.sort_by_key(|citation| (*citation.claim_id(), *citation.evidence().id()));
    Ok(GroundedAssertion {
        text: assertion.text,
        claim_ids: assertion.claim_ids,
        presentation,
        citations,
    })
}

fn weaker_presentation(left: ClaimPresentation, right: ClaimPresentation) -> ClaimPresentation {
    match (left, right) {
        (ClaimPresentation::Contested, _) | (_, ClaimPresentation::Contested) => {
            ClaimPresentation::Contested
        }
        (ClaimPresentation::Unresolved, _) | (_, ClaimPresentation::Unresolved) => {
            ClaimPresentation::Unresolved
        }
        _ => ClaimPresentation::Established,
    }
}

fn status_label(scope: &SynthesisReportScope) -> &'static str {
    match scope {
        SynthesisReportScope::Complete => "complete",
        SynthesisReportScope::Partial(_) => "partial",
    }
}

fn limitation(scope: &SynthesisReportScope) -> String {
    match scope {
        SynthesisReportScope::Complete => "null".to_owned(),
        SynthesisReportScope::Partial(ResearchStopReason::OperatorStopped) => {
            json_string("operator stopped")
        }
        SynthesisReportScope::Partial(ResearchStopReason::BudgetExhausted) => {
            json_string("budget exhausted")
        }
        SynthesisReportScope::Partial(ResearchStopReason::Blocked(reason)) => {
            json_string(reason.as_str())
        }
    }
}

fn presentation_label(presentation: ClaimPresentation) -> &'static str {
    match presentation {
        ClaimPresentation::Established => "Established",
        ClaimPresentation::Unresolved => "Unresolved",
        ClaimPresentation::Contested => "Contested",
    }
}

fn json_string(value: &str) -> String {
    let encoded = serde_json::to_string(value).expect("strings always serialize as JSON");
    let mut neutral = String::with_capacity(encoded.len());
    for character in encoded.chars() {
        if matches!(
            character,
            '\u{0080}'..='\u{009f}'
                | '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{206f}'
        ) {
            write!(neutral, "\\u{:04x}", character as u32)
                .expect("writing to a string cannot fail");
        } else {
            neutral.push(character);
        }
    }
    neutral
}

fn digest_hex(digest: &crate::ContentDigest) -> String {
    digest
        .as_sha256()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl SynthesisBasis {
    pub fn from_state(state: &ResearchControlState) -> Result<Self, SynthesisValidationError> {
        let scope = match state.status() {
            ResearchControlStatus::Completed => SynthesisReportScope::Complete,
            ResearchControlStatus::Stopped(reason) => SynthesisReportScope::Partial(reason),
            ResearchControlStatus::Failed(_) => {
                return Err(SynthesisValidationError::FailedResearch);
            }
            ResearchControlStatus::AwaitingLimits
            | ResearchControlStatus::Researching
            | ResearchControlStatus::AwaitingNextStep => {
                return Err(SynthesisValidationError::ResearchNotTerminal);
            }
        };
        let investigation = state.investigation();
        let question = investigation
            .request()
            .expect("terminal research always records a request")
            .question()
            .to_owned();
        let research = investigation.research();
        let known_claim_ids = research.claims().map(|claim| *claim.id()).collect();
        let claims = research
            .claims()
            .filter_map(|claim| claim_basis(state, claim))
            .map(|basis| (*basis.claim().id(), basis))
            .collect::<BTreeMap<_, _>>();
        if claims.is_empty() {
            return Err(SynthesisValidationError::NoReportableClaims);
        }
        Ok(Self {
            question,
            scope,
            known_claim_ids,
            claims,
        })
    }

    pub fn question(&self) -> &str {
        &self.question
    }

    pub const fn scope(&self) -> &SynthesisReportScope {
        &self.scope
    }

    pub fn claim(&self, id: &ClaimId) -> Option<&SynthesisClaimBasis> {
        self.claims.get(id)
    }

    pub fn claims(&self) -> impl Iterator<Item = &SynthesisClaimBasis> {
        self.claims.values()
    }

    pub fn is_known_claim(&self, id: &ClaimId) -> bool {
        self.known_claim_ids.contains(id)
    }
}

fn claim_basis(state: &ResearchControlState, claim: &Claim) -> Option<SynthesisClaimBasis> {
    let assessments = state
        .verification()
        .assessments()
        .filter(|assessment| assessment.claim_id() == claim.id())
        .cloned()
        .collect::<Vec<_>>();
    if assessments.is_empty() {
        return None;
    }
    let assessment_ids = assessments
        .iter()
        .map(|assessment| *assessment.id())
        .collect::<BTreeSet<_>>();
    let gaps = state
        .gaps()
        .filter(|gap| {
            matches!(
                gap.gap().cause(),
                ResearchGapCause::Verification(id) if assessment_ids.contains(id)
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let research = state.investigation().research();
    let presentation = classify(&assessments, &gaps);
    let evidence_ids = assessments
        .iter()
        .flat_map(VerificationAssessment::evidence_relations)
        .map(|(evidence_id, _)| *evidence_id)
        .chain(claim.evidence_ids().iter().copied())
        .collect::<BTreeSet<_>>();
    let evidence = evidence_ids
        .iter()
        .filter_map(|id| research.evidence(id).cloned().map(|item| (*id, item)))
        .collect::<BTreeMap<_, _>>();
    let source_ids = evidence
        .values()
        .map(|item| *item.source_id())
        .collect::<BTreeSet<_>>();
    let sources = source_ids
        .iter()
        .filter_map(|id| research.source(id).cloned().map(|source| (*id, source)))
        .collect::<BTreeMap<_, _>>();
    let mut citation_paths = BTreeMap::<EvidenceId, GroundingCitation>::new();
    for assessment in &assessments {
        for (evidence_id, relation) in assessment.evidence_relations() {
            if !is_citation_relation(presentation, relation) {
                continue;
            }
            let Some(evidence) = evidence.get(evidence_id) else {
                continue;
            };
            let Some(source) = sources.get(evidence.source_id()) else {
                continue;
            };
            let citation =
                citation_paths
                    .entry(*evidence_id)
                    .or_insert_with(|| GroundingCitation {
                        claim_id: *claim.id(),
                        provenance: Vec::new(),
                        evidence: evidence.clone(),
                        source: source.clone(),
                    });
            let provenance = (*assessment.id(), relation);
            if !citation.provenance.contains(&provenance) {
                citation.provenance.push(provenance);
            }
        }
    }
    let mut citations = citation_paths.into_values().collect::<Vec<_>>();
    for citation in &mut citations {
        citation
            .provenance
            .sort_by_key(|(verification_id, _)| *verification_id);
    }
    if citations.is_empty() && presentation == ClaimPresentation::Unresolved {
        citations = claim
            .evidence_ids()
            .iter()
            .filter_map(|evidence_id| {
                let evidence = evidence.get(evidence_id)?.clone();
                let source = sources.get(evidence.source_id())?.clone();
                Some(GroundingCitation {
                    claim_id: *claim.id(),
                    provenance: Vec::new(),
                    evidence,
                    source,
                })
            })
            .collect();
    }
    Some(SynthesisClaimBasis {
        claim: claim.clone(),
        presentation,
        assessments,
        gaps,
        evidence,
        sources,
        citations,
    })
}

fn is_citation_relation(presentation: ClaimPresentation, relation: EvidenceRelation) -> bool {
    match presentation {
        ClaimPresentation::Established => relation == EvidenceRelation::Supports,
        ClaimPresentation::Contested => {
            matches!(
                relation,
                EvidenceRelation::Supports | EvidenceRelation::Contradicts
            )
        }
        ClaimPresentation::Unresolved => !matches!(relation, EvidenceRelation::Irrelevant),
    }
}

fn classify(
    assessments: &[VerificationAssessment],
    gaps: &[ResearchGapState],
) -> ClaimPresentation {
    let has_contradiction = assessments.iter().any(|assessment| {
        assessment
            .evidence_relations()
            .any(|(_, relation)| relation == EvidenceRelation::Contradicts)
    });
    if has_contradiction {
        return ClaimPresentation::Contested;
    }
    let has_open_gap = gaps
        .iter()
        .any(|gap| matches!(gap.status(), ResearchGapStatus::Open));
    let has_sufficient_support = assessments.iter().any(|assessment| {
        assessment.sufficiency() == EvidenceSufficiency::Sufficient
            && assessment
                .evidence_relations()
                .any(|(_, relation)| relation == EvidenceRelation::Supports)
    });
    if has_open_gap || !has_sufficient_support {
        ClaimPresentation::Unresolved
    } else {
        ClaimPresentation::Established
    }
}
