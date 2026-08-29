use std::str::FromStr;

use aurora_research::{
    Claim, ClaimId, ContentDigest, Evidence, EvidenceAssessment, EvidenceId, EvidenceRelation,
    EvidenceSufficiency, GroundedReport, InvestigationEvent, InvestigationRecord,
    InvestigationResult, InvestigationTask, InvestigationTaskId, MediaType, ResearchControlEvent,
    ResearchControlLimits, ResearchControlRecord, ResearchControlState, ResearchEvent,
    ResearchPlan, ResearchRecord, ResearchRequest, ResearchStopReason, RetrievedAt, Source,
    SourceId, SynthesisAssertionDraft, SynthesisBasis, SynthesisDraft, SynthesisSectionDraft,
    SynthesisValidationError, VerificationAssessment, VerificationId, VerificationRecord,
};

#[test]
fn draft_constructors_enforce_utf8_and_cardinality_boundaries() {
    let claim = claim_id(1);
    let assertion = SynthesisAssertionDraft::new("é".repeat(2_048), vec![claim])
        .expect("four kibibyte assertion is allowed");
    assert_eq!(assertion.text(), "é".repeat(2_048));
    assert_eq!(assertion.claim_ids().collect::<Vec<_>>(), vec![&claim]);

    assert!(matches!(
        SynthesisAssertionDraft::new("é".repeat(2_049), vec![claim]),
        Err(SynthesisValidationError::AssertionTooLong)
    ));
    assert!(matches!(
        SynthesisAssertionDraft::new("text".to_owned(), Vec::new()),
        Err(SynthesisValidationError::AssertionHasNoClaims)
    ));
    assert!(matches!(
        SynthesisAssertionDraft::new("text".to_owned(), vec![claim; 9]),
        Err(SynthesisValidationError::TooManyAssertionClaims)
    ));
    assert!(matches!(
        SynthesisAssertionDraft::new("text".to_owned(), vec![claim, claim]),
        Err(SynthesisValidationError::DuplicateAssertionClaim(id)) if id == claim
    ));
    assert!(matches!(
        SynthesisAssertionDraft::new(" \t\n".to_owned(), vec![claim]),
        Err(SynthesisValidationError::BlankAssertion)
    ));

    let section = SynthesisSectionDraft::new(vec![assertion]).expect("section is valid");
    assert_eq!(section.assertions().count(), 1);
    assert!(matches!(
        SynthesisSectionDraft::new(Vec::new()),
        Err(SynthesisValidationError::SectionHasNoAssertions)
    ));
    assert!(matches!(
        SynthesisSectionDraft::new((1..=17).map(draft_assertion).collect()),
        Err(SynthesisValidationError::TooManySectionAssertions)
    ));

    let draft = SynthesisDraft::new(vec![section]).expect("one section is allowed");
    assert_eq!(draft.sections().count(), 1);
    assert!(matches!(
        SynthesisDraft::new(Vec::new()),
        Err(SynthesisValidationError::DraftHasNoSections)
    ));
    assert!(matches!(
        SynthesisDraft::new((1..=9).map(|id| section_with(id, 1)).collect()),
        Err(SynthesisValidationError::TooManyDraftSections)
    ));
    assert!(matches!(
        SynthesisDraft::new((1..=8).map(|id| section_with(id, 9)).collect()),
        Err(SynthesisValidationError::TooManyDraftAssertions)
    ));
}

#[test]
fn grounding_is_atomic_and_preserves_claim_citation_paths() {
    let basis = fixture_basis();
    let before = basis.clone();
    let report = GroundedReport::from_basis(
        &basis,
        SynthesisDraft::new(vec![
            SynthesisSectionDraft::new(vec![
                SynthesisAssertionDraft::new("supported".to_owned(), vec![claim_id(1)])
                    .expect("draft is valid"),
                SynthesisAssertionDraft::new(
                    "multiple claims".to_owned(),
                    vec![claim_id(1), claim_id(2), claim_id(3)],
                )
                .expect("draft is valid"),
            ])
            .expect("section is valid"),
        ])
        .expect("draft is valid"),
    )
    .expect("known assessed claims ground");

    let assertions = report
        .sections()
        .next()
        .expect("section exists")
        .assertions()
        .collect::<Vec<_>>();
    assert_eq!(
        assertions[0].presentation(),
        aurora_research::ClaimPresentation::Established
    );
    assert_eq!(
        assertions[1].presentation(),
        aurora_research::ClaimPresentation::Contested
    );
    assert_eq!(
        assertions[1].claim_ids().collect::<Vec<_>>(),
        vec![&claim_id(1), &claim_id(2), &claim_id(3)]
    );
    let paths = assertions[1]
        .citations()
        .map(|citation| {
            (
                *citation.claim_id(),
                *citation.evidence().id(),
                *citation.source().id(),
                *citation.source().content_digest().as_sha256(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![
            (claim_id(1), evidence_id(1), source_id(1), [1; 32]),
            (claim_id(2), evidence_id(1), source_id(1), [1; 32]),
            (claim_id(2), evidence_id(2), source_id(2), [2; 32]),
            (claim_id(3), evidence_id(3), source_id(3), [3; 32]),
        ]
    );
    let contested = assertions[1]
        .citations()
        .find(|citation| citation.claim_id() == &claim_id(3))
        .expect("contested path exists");
    assert_eq!(
        contested
            .provenance()
            .map(|(verification_id, relation)| (*verification_id, relation))
            .collect::<Vec<_>>(),
        vec![
            (verification_id(3), EvidenceRelation::Contradicts),
            (verification_id(4), EvidenceRelation::Supports),
        ]
    );
    assert!(!contested.is_fallback());
    assert_eq!(
        report
            .citations()
            .map(|citation| (*citation.claim_id(), *citation.evidence().id()))
            .collect::<Vec<_>>(),
        vec![
            (claim_id(1), evidence_id(1)),
            (claim_id(2), evidence_id(1)),
            (claim_id(2), evidence_id(2)),
            (claim_id(3), evidence_id(3)),
        ]
    );
    assert_eq!(basis, before);

    for (id, expected) in [
        (
            claim_id(99),
            SynthesisValidationError::UnknownClaim(claim_id(99)),
        ),
        (
            claim_id(4),
            SynthesisValidationError::UnassessedClaim(claim_id(4)),
        ),
    ] {
        let draft = one_assertion_draft(id);
        assert_eq!(GroundedReport::from_basis(&basis, draft), Err(expected));
    }
}

#[test]
fn grounding_canonicalizes_citations_without_reordering_draft_claim_references() {
    let basis = fixture_basis();
    let report = GroundedReport::from_basis(
        &basis,
        SynthesisDraft::new(vec![
            SynthesisSectionDraft::new(vec![
                SynthesisAssertionDraft::new(
                    "reversed references".to_owned(),
                    vec![claim_id(3), claim_id(2), claim_id(1)],
                )
                .expect("draft is valid"),
            ])
            .expect("section is valid"),
        ])
        .expect("draft is valid"),
    )
    .expect("draft grounds");

    let assertion = report
        .sections()
        .next()
        .expect("section exists")
        .assertions()
        .next()
        .expect("assertion exists");
    assert_eq!(
        assertion.claim_ids().collect::<Vec<_>>(),
        vec![&claim_id(3), &claim_id(2), &claim_id(1)]
    );
    assert_eq!(
        assertion
            .citations()
            .map(|citation| (*citation.claim_id(), *citation.evidence().id()))
            .collect::<Vec<_>>(),
        vec![
            (claim_id(1), evidence_id(1)),
            (claim_id(2), evidence_id(1)),
            (claim_id(2), evidence_id(2)),
            (claim_id(3), evidence_id(3)),
        ]
    );
    let rendered = report.render();
    let first = rendered
        .find("[1] title=")
        .expect("first source row exists");
    let second = rendered
        .find("[2] title=")
        .expect("second source row exists");
    let third = rendered
        .find("[3] title=")
        .expect("third source row exists");
    assert!(first < second && second < third);
    assert!(rendered[first..second].contains(&format!("evidence={}", evidence_id(1))));
    assert!(rendered[second..third].contains(&format!("evidence={}", evidence_id(2))));
    assert!(rendered[third..].contains(&format!("evidence={}", evidence_id(3))));
    let assertion_line = rendered
        .lines()
        .find(|line| line.contains("reversed references"))
        .expect("assertion renders");
    assert_eq!(assertion_line.matches("[1]").count(), 1);
    assert_eq!(assertion_line.matches("[2]").count(), 1);
    assert_eq!(assertion_line.matches("[3]").count(), 1);
    assert!(!assertion_line.contains("[4]"));
    assert!(!rendered.contains("\n[4] title="));
}

#[test]
fn rendering_covers_partial_limitations_and_conservative_claim_sets() {
    for (reason, expected) in [
        (ResearchStopReason::BudgetExhausted, "budget exhausted"),
        (ResearchStopReason::OperatorStopped, "operator stopped"),
    ] {
        let report = GroundedReport::from_basis(
            &fixture_basis_with_text("question", Some("source".to_owned()), "locator", reason),
            one_assertion_draft(claim_id(2)),
        )
        .expect("insufficient claim grounds");
        let assertion = report
            .sections()
            .next()
            .expect("section exists")
            .assertions()
            .next()
            .expect("assertion exists");
        assert_eq!(
            assertion.presentation(),
            aurora_research::ClaimPresentation::Unresolved
        );
        assert!(
            report
                .render()
                .contains(&format!("Limitation: \"{expected}\""))
        );
    }

    let report = GroundedReport::from_basis(
        &fixture_basis(),
        SynthesisDraft::new(vec![
            SynthesisSectionDraft::new(vec![
                SynthesisAssertionDraft::new(
                    "two established claims".to_owned(),
                    vec![claim_id(1), claim_id(5)],
                )
                .expect("assertion is valid"),
            ])
            .expect("section is valid"),
        ])
        .expect("draft is valid"),
    )
    .expect("established claims ground");
    let assertion = report
        .sections()
        .next()
        .expect("section exists")
        .assertions()
        .next()
        .expect("assertion exists");
    assert_eq!(
        assertion.presentation(),
        aurora_research::ClaimPresentation::Established
    );
    assert_eq!(
        assertion
            .citations()
            .map(|citation| (*citation.evidence().id(), *citation.source().id()))
            .collect::<Vec<_>>(),
        vec![
            (evidence_id(1), source_id(1)),
            (evidence_id(2), source_id(2)),
        ]
    );
}

#[test]
fn rendering_uses_single_line_bidi_neutral_json_and_reuses_evidence_numbers() {
    let hostile_code_points = (0x0080..=0x009f)
        .chain([0x061c, 0x200e, 0x200f, 0x2028, 0x2029])
        .chain(0x202a..=0x202e)
        .chain(0x2066..=0x206f)
        .collect::<Vec<_>>();
    let hostile = format!(
        "\n{}",
        hostile_code_points
            .iter()
            .map(|code_point| char::from_u32(*code_point).expect("fixture scalar is valid"))
            .collect::<String>()
    );
    let question = format!("question{hostile}Sources [1] slash\\quote\"");
    let title = format!("title{hostile}Contested: fake [1] slash\\quote\"");
    let locator = format!("locator{hostile}Unresolved: fake [1] slash\\quote\"");
    let blocked = format!("blocked{hostile}Sources [1] slash\\quote\"");
    let assertion = format!("assertion{hostile}Unresolved: fake [1] slash\\quote\"");
    let basis = fixture_basis_with_text(
        &question,
        Some(title.clone()),
        &locator,
        ResearchStopReason::Blocked(
            aurora_research::BlockedReason::new(blocked.clone()).expect("reason is valid"),
        ),
    );
    let report = GroundedReport::from_basis(
        &basis,
        SynthesisDraft::new(vec![
            SynthesisSectionDraft::new(vec![
                SynthesisAssertionDraft::new(assertion.clone(), vec![claim_id(1)])
                    .expect("draft is valid"),
                SynthesisAssertionDraft::new(
                    format!("same evidence{hostile}Sources [1] slash\\quote\""),
                    vec![claim_id(1)],
                )
                .expect("draft is valid"),
            ])
            .expect("section is valid"),
        ])
        .expect("draft is valid"),
    )
    .expect("draft grounds");

    let rendered = report.render();
    assert_eq!(rendered.matches("\n[1] title=").count(), 1);
    assert!(rendered.contains("\nSection 1\n"));
    assert!(!rendered.contains("Section 1:"));
    for code_point in &hostile_code_points {
        let escaped = format!("\\u{code_point:04x}");
        assert!(rendered.contains(&escaped), "missing escape {escaped}");
    }
    for forbidden in hostile.chars().filter(|character| *character != '\n') {
        assert!(
            !rendered.contains(forbidden),
            "raw hostile character remained"
        );
    }
    assert_eq!(rendered.lines().count(), 9);
    assert!(rendered.contains("Question: \"question\\n\\u0080"));
    assert!(rendered.contains("Limitation: \"blocked\\n\\u0080"));
    assert!(rendered.contains("Established: \"assertion\\n\\u0080"));
    assert!(rendered.contains("title=\"title\\n\\u0080"));
    assert!(rendered.contains("locator=\"locator\\n\\u0080"));

    let untitled = GroundedReport::from_basis(
        &fixture_basis_with_text(
            "question",
            None,
            "locator",
            ResearchStopReason::OperatorStopped,
        ),
        one_assertion_draft(claim_id(1)),
    )
    .expect("draft grounds");
    assert!(untitled.render().contains("title=null locator=\"locator\""));
}

fn draft_assertion(value: u128) -> SynthesisAssertionDraft {
    SynthesisAssertionDraft::new("assertion".to_owned(), vec![claim_id(value)])
        .expect("draft assertion is valid")
}

fn section_with(value: u128, assertions: u128) -> SynthesisSectionDraft {
    SynthesisSectionDraft::new(
        (1..=assertions)
            .map(|offset| draft_assertion(value * 100 + offset))
            .collect(),
    )
    .expect("section is individually valid")
}

pub fn one_assertion_draft(claim: ClaimId) -> SynthesisDraft {
    SynthesisDraft::new(vec![
        SynthesisSectionDraft::new(vec![
            SynthesisAssertionDraft::new("assertion".to_owned(), vec![claim])
                .expect("draft assertion is valid"),
        ])
        .expect("section is valid"),
    ])
    .expect("draft is valid")
}

pub fn fixture_basis() -> SynthesisBasis {
    fixture_basis_with_text(
        "What does the evidence establish?",
        Some("Source".to_owned()),
        "https://example.test/source/1",
        ResearchStopReason::OperatorStopped,
    )
}

fn fixture_basis_with_text(
    question: &str,
    first_title: Option<String>,
    first_locator: &str,
    stop_reason: ResearchStopReason,
) -> SynthesisBasis {
    let source = |value, title: Option<String>, locator: String| {
        Source::new(
            source_id(value),
            ContentDigest::sha256([value as u8; 32]),
            locator,
            title,
            RetrievedAt::new("2026-08-29T00:00:00Z").expect("time is valid"),
            MediaType::new("text/plain").expect("media type is valid"),
        )
        .expect("source is valid")
    };
    let evidence = |value| {
        Evidence::new(
            evidence_id(value),
            source_id(value),
            format!("Evidence {value}"),
        )
        .expect("evidence is valid")
    };
    let claim = |value| {
        Claim::new(
            claim_id(value),
            format!("Claim {value}"),
            vec![evidence_id(value.min(3))],
        )
        .expect("claim is valid")
    };
    let research_records = vec![
        ResearchRecord::new(
            1,
            ResearchEvent::SourceRecorded(source(1, first_title, first_locator.to_owned())),
        )
        .expect("record is valid"),
        ResearchRecord::new(2, ResearchEvent::EvidenceRecorded(evidence(1)))
            .expect("record is valid"),
        ResearchRecord::new(
            3,
            ResearchEvent::SourceRecorded(source(
                2,
                Some("Second source".to_owned()),
                "https://example.test/source/2".to_owned(),
            )),
        )
        .expect("record is valid"),
        ResearchRecord::new(4, ResearchEvent::EvidenceRecorded(evidence(2)))
            .expect("record is valid"),
        ResearchRecord::new(
            5,
            ResearchEvent::SourceRecorded(source(
                3,
                Some("Third source".to_owned()),
                "https://example.test/source/3".to_owned(),
            )),
        )
        .expect("record is valid"),
        ResearchRecord::new(6, ResearchEvent::EvidenceRecorded(evidence(3)))
            .expect("record is valid"),
        ResearchRecord::new(7, ResearchEvent::ClaimProposed(claim(1))).expect("record is valid"),
        ResearchRecord::new(8, ResearchEvent::ClaimProposed(claim(2))).expect("record is valid"),
        ResearchRecord::new(9, ResearchEvent::ClaimProposed(claim(3))).expect("record is valid"),
        ResearchRecord::new(10, ResearchEvent::ClaimProposed(claim(4))).expect("record is valid"),
        ResearchRecord::new(
            11,
            ResearchEvent::ClaimProposed(
                Claim::new(claim_id(5), "Claim 5".to_owned(), vec![evidence_id(2)])
                    .expect("claim is valid"),
            ),
        )
        .expect("record is valid"),
    ];
    let request = ResearchRequest::new(question.to_owned()).expect("question is valid");
    let plan = ResearchPlan::new(vec![
        InvestigationTask::initial(task_id(1), "Investigate".to_owned()).expect("task is valid"),
    ])
    .expect("plan is valid");
    let assessment = |value, claim, relation, sufficiency| {
        VerificationAssessment::new(
            verification_id(value),
            claim_id(claim),
            vec![EvidenceAssessment::new(evidence_id(claim), relation)],
            sufficiency,
        )
        .expect("assessment is valid")
    };
    ResearchControlState::reconstruct([
        control_record(
            1,
            ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(1)),
        ),
        control_record(
            2,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                1,
                InvestigationEvent::RequestRecorded(request),
            )),
        ),
        control_record(
            3,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                2,
                InvestigationEvent::PlanRecorded(plan),
            )),
        ),
        control_record(
            4,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                3,
                InvestigationEvent::TaskStarted {
                    task_id: task_id(1),
                },
            )),
        ),
        control_record(
            5,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                4,
                InvestigationEvent::TaskCompleted {
                    task_id: task_id(1),
                    result: InvestigationResult::new(research_records),
                },
            )),
        ),
        control_record(
            6,
            ResearchControlEvent::VerificationRecorded(verification_record(
                1,
                assessment(
                    1,
                    1,
                    EvidenceRelation::Supports,
                    EvidenceSufficiency::Sufficient,
                ),
            )),
        ),
        control_record(
            7,
            ResearchControlEvent::VerificationRecorded(verification_record(
                2,
                assessment(
                    2,
                    2,
                    EvidenceRelation::Supports,
                    EvidenceSufficiency::Insufficient,
                ),
            )),
        ),
        control_record(
            8,
            ResearchControlEvent::VerificationRecorded(verification_record(
                3,
                assessment(
                    3,
                    3,
                    EvidenceRelation::Contradicts,
                    EvidenceSufficiency::Sufficient,
                ),
            )),
        ),
        control_record(
            9,
            ResearchControlEvent::VerificationRecorded(verification_record(
                4,
                assessment(
                    4,
                    3,
                    EvidenceRelation::Supports,
                    EvidenceSufficiency::Sufficient,
                ),
            )),
        ),
        control_record(
            10,
            ResearchControlEvent::VerificationRecorded(verification_record(
                5,
                VerificationAssessment::new(
                    verification_id(5),
                    claim_id(2),
                    vec![EvidenceAssessment::new(
                        evidence_id(1),
                        EvidenceRelation::Unclear,
                    )],
                    EvidenceSufficiency::Insufficient,
                )
                .expect("assessment is valid"),
            )),
        ),
        control_record(
            11,
            ResearchControlEvent::VerificationRecorded(verification_record(
                6,
                VerificationAssessment::new(
                    verification_id(6),
                    claim_id(5),
                    vec![EvidenceAssessment::new(
                        evidence_id(2),
                        EvidenceRelation::Supports,
                    )],
                    EvidenceSufficiency::Sufficient,
                )
                .expect("assessment is valid"),
            )),
        ),
        control_record(
            12,
            ResearchControlEvent::InvestigationAdvanced(investigation_record(
                5,
                InvestigationEvent::ResearchStopped(stop_reason),
            )),
        ),
    ])
    .map(|state| SynthesisBasis::from_state(&state).expect("basis is valid"))
    .expect("history reconstructs")
}

fn control_record(sequence: u64, event: ResearchControlEvent) -> ResearchControlRecord {
    ResearchControlRecord::new(sequence, event).expect("control record is valid")
}

fn investigation_record(sequence: u64, event: InvestigationEvent) -> InvestigationRecord {
    InvestigationRecord::new(sequence, event).expect("investigation record is valid")
}

fn verification_record(sequence: u64, assessment: VerificationAssessment) -> VerificationRecord {
    VerificationRecord::new(sequence, assessment).expect("verification record is valid")
}

fn claim_id(value: u128) -> ClaimId {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    ClaimId::from_str(&uuid::Uuid::from_bytes(bytes).to_string()).expect("claim id is valid")
}

fn source_id(value: u128) -> SourceId {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    SourceId::from_str(&uuid::Uuid::from_bytes(bytes).to_string()).expect("source id is valid")
}

fn evidence_id(value: u128) -> EvidenceId {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    EvidenceId::from_str(&uuid::Uuid::from_bytes(bytes).to_string()).expect("evidence id is valid")
}

fn task_id(value: u128) -> InvestigationTaskId {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    InvestigationTaskId::from_str(&uuid::Uuid::from_bytes(bytes).to_string())
        .expect("task id is valid")
}

fn verification_id(value: u128) -> VerificationId {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    VerificationId::from_str(&uuid::Uuid::from_bytes(bytes).to_string())
        .expect("verification id is valid")
}
