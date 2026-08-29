use aurora_research::{
    EvidenceAssessment, EvidenceRelation, EvidenceSufficiency, IdentifiedResearchGap,
    InvestigationEvent, InvestigationRecord, InvestigationTask, InvestigationTaskId,
    PlanningValidationError, ResearchControlCodecError, ResearchControlEvent,
    ResearchControlLimits, ResearchControlRecord, ResearchControlValidationError, ResearchFailure,
    ResearchGap, ResearchGapCause, ResearchGapId, VerificationAssessment, VerificationId,
    VerificationRecord, decode_research_control_record, encode_research_control_record,
};
use serde_json::{Value, json};
use uuid::Uuid;

#[test]
fn every_control_event_round_trips_canonically() {
    let events = vec![
        ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(3)),
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            1,
            InvestigationEvent::TaskStarted {
                task_id: task_id(1),
            },
        )),
        ResearchControlEvent::VerificationRecorded(verification_record(1, 1)),
        ResearchControlEvent::GapIdentified(identified_gap()),
        ResearchControlEvent::GapFollowUpRecorded {
            gap_id: gap_id(1),
            investigation_record: follow_up_record(2),
        },
        ResearchControlEvent::GapResolved {
            gap_id: gap_id(1),
            verification_id: verification_id(2),
        },
        ResearchControlEvent::ResearchCompleted,
        ResearchControlEvent::ResearchFailed(
            ResearchFailure::new("Research control failed".to_owned()).expect("failure is valid"),
        ),
    ];

    for (index, event) in events.into_iter().enumerate() {
        let record = control_record(index as u64 + 1, event);
        let encoded = encode_research_control_record(&record).expect("record encodes");
        let decoded = decode_research_control_record(&encoded).expect("record decodes");
        assert_eq!(decoded, record);
        assert_eq!(
            encode_research_control_record(&decoded).expect("decoded record encodes"),
            encoded
        );
    }
}

#[test]
fn limits_and_gap_wire_shapes_are_explicit() {
    let limits = encode_research_control_record(&control_record(
        1,
        ResearchControlEvent::LimitsRecorded(ResearchControlLimits::new(2)),
    ))
    .expect("limits encode");
    assert_eq!(
        serde_json::from_slice::<Value>(&limits).expect("JSON is valid"),
        json!({
            "schema_version": 1,
            "sequence": 1,
            "event": {
                "type": "limits_recorded",
                "limits": { "max_follow_up_tasks": 2 }
            }
        })
    );

    let gap = encode_research_control_record(&control_record(
        2,
        ResearchControlEvent::GapIdentified(identified_gap()),
    ))
    .expect("gap encodes");
    assert_eq!(
        serde_json::from_slice::<Value>(&gap).expect("JSON is valid"),
        json!({
            "schema_version": 1,
            "sequence": 2,
            "event": {
                "type": "gap_identified",
                "gap": {
                    "id": uuid(1),
                    "cause": {
                        "type": "verification",
                        "verification_id": uuid(1)
                    },
                    "description": "Evidence is unresolved"
                }
            }
        })
    );
}

#[test]
fn nested_records_use_their_existing_canonical_wire_shapes() {
    let encoded = encode_research_control_record(&control_record(
        1,
        ResearchControlEvent::InvestigationAdvanced(investigation_record(
            1,
            InvestigationEvent::TaskStarted {
                task_id: task_id(7),
            },
        )),
    ))
    .expect("record encodes");
    let value: Value = serde_json::from_slice(&encoded).expect("JSON is valid");

    assert_eq!(
        value["event"]["record"],
        json!({
            "schema_version": 1,
            "sequence": 1,
            "event": {
                "type": "task_started",
                "task_id": uuid(7)
            }
        })
    );
}

#[test]
fn malformed_and_unsupported_records_are_distinct_without_echoing_input() {
    let malformed = br#"{"failure":"secret-payload""#;
    let error = decode_research_control_record(malformed).expect_err("malformed JSON fails");
    assert_eq!(error, ResearchControlCodecError::MalformedJson);
    assert!(!error.to_string().contains("secret-payload"));

    let unsupported = serde_json::to_vec(&json!({
        "schema_version": 9,
        "sequence": 0,
        "event": { "type": "research_failed", "failure": "" }
    }))
    .expect("fixture encodes");
    assert_eq!(
        decode_research_control_record(&unsupported),
        Err(ResearchControlCodecError::UnsupportedSchema(9))
    );
}

#[test]
fn unsupported_schema_precedes_future_event_shape_validation() {
    let future_event = serde_json::to_vec(&json!({
        "schema_version": 9,
        "sequence": 1,
        "event": { "type": "future_control_event", "new_field": true }
    }))
    .expect("fixture encodes");
    assert_eq!(
        decode_research_control_record(&future_event),
        Err(ResearchControlCodecError::UnsupportedSchema(9))
    );

    let incomplete_event = serde_json::to_vec(&json!({
        "schema_version": 9,
        "sequence": 1,
        "event": { "new_field": true }
    }))
    .expect("fixture encodes");
    assert_eq!(
        decode_research_control_record(&incomplete_event),
        Err(ResearchControlCodecError::UnsupportedSchema(9))
    );
}

#[test]
fn zero_sequence_empty_failure_and_invalid_identity_are_rejected() {
    let zero = serde_json::to_vec(&json!({
        "schema_version": 1,
        "sequence": 0,
        "event": { "type": "research_completed" }
    }))
    .expect("fixture encodes");
    assert_eq!(
        decode_research_control_record(&zero),
        Err(ResearchControlCodecError::InvalidRecord(
            ResearchControlValidationError::ZeroResearchControlSequence
        ))
    );

    let blank = serde_json::to_vec(&json!({
        "schema_version": 1,
        "sequence": 1,
        "event": { "type": "research_failed", "failure": "  " }
    }))
    .expect("fixture encodes");
    assert_eq!(
        decode_research_control_record(&blank),
        Err(ResearchControlCodecError::InvalidRecord(
            ResearchControlValidationError::EmptyResearchFailure
        ))
    );

    let invalid_id = serde_json::to_vec(&json!({
        "schema_version": 1,
        "sequence": 1,
        "event": {
            "type": "gap_resolved",
            "gap_id": "not-a-uuid",
            "verification_id": uuid(2)
        }
    }))
    .expect("fixture encodes");
    assert!(matches!(
        decode_research_control_record(&invalid_id),
        Err(ResearchControlCodecError::InvalidIdentity(_))
    ));
}

#[test]
fn invalid_nested_records_keep_their_codec_boundary() {
    let bytes = serde_json::to_vec(&json!({
        "schema_version": 1,
        "sequence": 1,
        "event": {
            "type": "investigation_advanced",
            "record": {
                "schema_version": 99,
                "sequence": 1,
                "event": { "type": "task_started", "task_id": uuid(1) }
            }
        }
    }))
    .expect("fixture encodes");

    assert!(matches!(
        decode_research_control_record(&bytes),
        Err(ResearchControlCodecError::InvalidInvestigationRecord(_))
    ));
}

#[test]
fn invalid_gap_text_has_a_local_categorical_error() {
    let bytes = serde_json::to_vec(&json!({
        "schema_version": 1,
        "sequence": 1,
        "event": {
            "type": "gap_identified",
            "gap": {
                "id": uuid(1),
                "cause": {
                    "type": "verification",
                    "verification_id": uuid(1)
                },
                "description": "  "
            }
        }
    }))
    .expect("fixture encodes");

    assert_eq!(
        decode_research_control_record(&bytes),
        Err(ResearchControlCodecError::InvalidGap(
            PlanningValidationError::EmptyResearchGap
        ))
    );
}

#[test]
fn isolated_decode_defers_cross_record_gap_validation() {
    let record = control_record(
        1,
        ResearchControlEvent::GapIdentified(IdentifiedResearchGap::new(
            gap_id(1),
            ResearchGapCause::Verification(verification_id(99)),
            ResearchGap::new("Missing verification".to_owned()).expect("gap is valid"),
        )),
    );
    let bytes = encode_research_control_record(&record).expect("record encodes");

    assert_eq!(
        decode_research_control_record(&bytes).expect("isolated record decodes"),
        record
    );
}

fn identified_gap() -> IdentifiedResearchGap {
    IdentifiedResearchGap::new(
        gap_id(1),
        ResearchGapCause::Verification(verification_id(1)),
        ResearchGap::new("Evidence is unresolved".to_owned()).expect("gap is valid"),
    )
}

fn follow_up_record(sequence: u64) -> InvestigationRecord {
    investigation_record(
        sequence,
        InvestigationEvent::FollowUpRecorded(
            InvestigationTask::follow_up(
                task_id(2),
                task_id(1),
                "Gather more evidence".to_owned(),
                ResearchGap::new("Evidence is unresolved".to_owned()).expect("gap is valid"),
            )
            .expect("follow-up is valid"),
        ),
    )
}

fn verification_record(sequence: u64, id: u128) -> VerificationRecord {
    VerificationRecord::new(
        sequence,
        VerificationAssessment::new(
            verification_id(id),
            uuid(20).parse().expect("claim identifier is valid"),
            vec![EvidenceAssessment::new(
                uuid(30).parse().expect("evidence identifier is valid"),
                EvidenceRelation::Supports,
            )],
            EvidenceSufficiency::Insufficient,
        )
        .expect("assessment is valid"),
    )
    .expect("verification record is valid")
}

fn control_record(sequence: u64, event: ResearchControlEvent) -> ResearchControlRecord {
    ResearchControlRecord::new(sequence, event).expect("control record is valid")
}

fn investigation_record(sequence: u64, event: InvestigationEvent) -> InvestigationRecord {
    InvestigationRecord::new(sequence, event).expect("investigation record is valid")
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
