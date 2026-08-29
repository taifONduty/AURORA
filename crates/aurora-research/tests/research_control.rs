use aurora_research::{
    IdentifiedResearchGap, InvestigationEvent, InvestigationRecord, InvestigationTaskId,
    ResearchControlEvent, ResearchControlLimits, ResearchControlRecord,
    ResearchControlValidationError, ResearchFailure, ResearchGap, ResearchGapCause, ResearchGapId,
    VerificationId,
};
use uuid::Uuid;

#[test]
fn control_limits_preserve_zero_and_nonzero_follow_up_bounds() {
    assert_eq!(ResearchControlLimits::new(0).max_follow_up_tasks(), 0);
    assert_eq!(ResearchControlLimits::new(7).max_follow_up_tasks(), 7);
}

#[test]
fn research_failure_requires_non_blank_text() {
    assert_eq!(
        ResearchFailure::new("  \n".to_owned()),
        Err(ResearchControlValidationError::EmptyResearchFailure)
    );

    let failure = ResearchFailure::new("Verifier process failed".to_owned())
        .expect("failure reason is valid");
    assert_eq!(failure.as_str(), "Verifier process failed");
}

#[test]
fn identified_gap_preserves_typed_identity_cause_and_description() {
    let verification = verification_id(2);
    let gap = IdentifiedResearchGap::new(
        gap_id(1),
        ResearchGapCause::Verification(verification),
        ResearchGap::new("Evidence is insufficient".to_owned()).expect("gap text is valid"),
    );

    assert_eq!(gap.id(), &gap_id(1));
    assert_eq!(gap.cause(), &ResearchGapCause::Verification(verification));
    assert_eq!(gap.description().as_str(), "Evidence is insufficient");
}

#[test]
fn gap_causes_keep_verification_and_investigation_distinct() {
    let task = task_id(3);
    let verification = verification_id(4);

    assert_ne!(
        ResearchGapCause::InvestigationFailure(task),
        ResearchGapCause::Verification(verification)
    );
}

#[test]
fn research_gap_identity_is_an_opaque_uuid_v4() {
    let text = uuid(9);
    let id: ResearchGapId = text.parse().expect("UUID v4 is accepted");

    assert_eq!(id.to_string(), text);
    assert_eq!(
        "00000000-0000-0000-0000-000000000009".parse::<ResearchGapId>(),
        Err(aurora_research::IdentityError::NotVersion4)
    );
}

#[test]
fn control_record_is_versioned_nonzero_and_preserves_every_event_shape() {
    assert_eq!(
        ResearchControlRecord::new(0, ResearchControlEvent::ResearchCompleted),
        Err(ResearchControlValidationError::ZeroResearchControlSequence)
    );

    let events = vec![
        ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(2)),
        ResearchControlEvent::InvestigationAdvanced(
            InvestigationRecord::new(
                1,
                InvestigationEvent::TaskStarted {
                    task_id: task_id(1),
                },
            )
            .expect("investigation record is valid"),
        ),
        ResearchControlEvent::GapIdentified(IdentifiedResearchGap::new(
            gap_id(1),
            ResearchGapCause::InvestigationFailure(task_id(1)),
            ResearchGap::new("Task failed".to_owned()).expect("gap text is valid"),
        )),
        ResearchControlEvent::GapResolved {
            gap_id: gap_id(1),
            verification_id: verification_id(2),
        },
        ResearchControlEvent::ResearchCompleted,
        ResearchControlEvent::ResearchFailed(
            ResearchFailure::new("Research process failed".to_owned()).expect("failure is valid"),
        ),
    ];

    for (index, event) in events.into_iter().enumerate() {
        let record = ResearchControlRecord::new(index as u64 + 1, event.clone())
            .expect("control record is valid");
        assert_eq!(record.schema_version(), 1);
        assert_eq!(record.sequence(), index as u64 + 1);
        assert_eq!(record.event(), &event);
    }
}

fn gap_id(value: u128) -> ResearchGapId {
    uuid(value).parse().expect("gap identifier is valid")
}

fn task_id(value: u128) -> InvestigationTaskId {
    uuid(value).parse().expect("task identifier is valid")
}

fn verification_id(value: u128) -> VerificationId {
    uuid(value)
        .parse()
        .expect("verification identifier is valid")
}

fn uuid(value: u128) -> String {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).hyphenated().to_string()
}
