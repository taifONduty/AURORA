use aurora_research::{
    BlockedReason, CodecError, ContentDigest, INVESTIGATION_SCHEMA_VERSION, IdentityError,
    InvestigationCodecError, InvestigationEvent, InvestigationFailure, InvestigationRecord,
    InvestigationResult, InvestigationTask, InvestigationTaskId, MediaType,
    PlanningValidationError, RESEARCH_SCHEMA_VERSION, ResearchEvent, ResearchGap, ResearchPlan,
    ResearchRecord, ResearchRequest, ResearchStopReason, RetrievedAt, Source, SourceId,
    decode_investigation_record, encode_investigation_record,
};
use serde_json::{Value, json};

#[test]
fn every_investigation_event_round_trips_canonically() {
    for record in event_records() {
        let encoded = encode_investigation_record(&record).expect("record encodes");
        let decoded = decode_investigation_record(&encoded).expect("record decodes");

        assert_eq!(decoded, record);
        assert_eq!(
            encode_investigation_record(&decoded).expect("decoded record encodes"),
            encoded
        );
    }
}

#[test]
fn plan_and_follow_up_wire_shapes_are_explicit() {
    let plan = encoded_value(&event_records()[1]);
    assert_eq!(plan["schema_version"], INVESTIGATION_SCHEMA_VERSION);
    assert_eq!(plan["sequence"], 2);
    assert_eq!(plan["event"]["type"], "plan_recorded");
    assert_eq!(
        plan["event"]["plan"]["tasks"][0]["id"],
        "00000000-0000-4000-8000-000000000001"
    );
    assert_eq!(
        plan["event"]["plan"]["tasks"][0]["origin"]["type"],
        "initial"
    );

    let follow_up = encoded_value(&event_records()[5]);
    assert_eq!(follow_up["event"]["type"], "follow_up_recorded");
    assert_eq!(follow_up["event"]["task"]["origin"]["type"], "follow_up");
    assert_eq!(
        follow_up["event"]["task"]["origin"]["parent_task_id"],
        "00000000-0000-4000-8000-000000000001"
    );
    assert_eq!(
        follow_up["event"]["task"]["origin"]["gap"],
        "Unresolved wavelength dependence"
    );
}

#[test]
fn completed_result_embeds_the_existing_research_record_shape() {
    let completed = encoded_value(&event_records()[3]);
    let nested = &completed["event"]["result"]["research_records"][0];

    assert_eq!(nested["schema_version"], RESEARCH_SCHEMA_VERSION);
    assert_eq!(nested["event"]["type"], "source_recorded");
    assert_eq!(
        nested["event"]["source"]["id"],
        "00000000-0000-4000-8000-00000000000b"
    );
}

#[test]
fn stop_reasons_have_distinct_wire_forms() {
    let cases = [
        (ResearchStopReason::OperatorStopped, "operator_stopped"),
        (ResearchStopReason::BudgetExhausted, "budget_exhausted"),
        (
            ResearchStopReason::Blocked(
                BlockedReason::new("Archive unavailable".to_owned()).expect("reason is valid"),
            ),
            "blocked",
        ),
    ];

    for (reason, expected) in cases {
        let value = encoded_value(&record(1, InvestigationEvent::ResearchStopped(reason)));
        assert_eq!(value["event"]["reason"]["type"], expected);
    }
}

#[test]
fn malformed_and_unsupported_records_are_distinct_without_echoing_input() {
    let malformed = b"{private-invalid-json";
    let error = decode_investigation_record(malformed).expect_err("malformed JSON is rejected");
    assert_eq!(error, InvestigationCodecError::MalformedJson);
    assert!(!error.to_string().contains("private-invalid-json"));

    let mut value = encoded_value(&event_records()[0]);
    value["schema_version"] = json!(99);
    value["event"]["request"]["question"] = json!("");
    assert_eq!(
        decode_value(value),
        Err(InvestigationCodecError::UnsupportedSchema(99))
    );
}

#[test]
fn zero_sequence_and_invalid_identity_are_rejected() {
    let mut zero = encoded_value(&event_records()[0]);
    zero["sequence"] = json!(0);
    assert_eq!(
        decode_value(zero),
        Err(InvestigationCodecError::InvalidRecord(
            PlanningValidationError::ZeroInvestigationSequence
        ))
    );

    let mut malformed = encoded_value(&event_records()[1]);
    malformed["event"]["plan"]["tasks"][0]["id"] = json!("not-an-id");
    assert_eq!(
        decode_value(malformed),
        Err(InvestigationCodecError::InvalidIdentity(
            IdentityError::InvalidUuid
        ))
    );
}

#[test]
fn invalid_domain_text_is_rejected_without_echoing_payloads() {
    let mut request = encoded_value(&event_records()[0]);
    request["event"]["request"]["question"] = json!("private-question");
    request["event"]["request"]["question"] = json!(" \n");
    let error = decode_value(request).expect_err("blank question is rejected");
    assert_eq!(
        error,
        InvestigationCodecError::InvalidRecord(PlanningValidationError::EmptyResearchQuestion)
    );
    assert!(!error.to_string().contains("private-question"));

    let mut blocked = encoded_value(&record(
        1,
        InvestigationEvent::ResearchStopped(ResearchStopReason::Blocked(
            BlockedReason::new("private-blocked-reason".to_owned()).expect("reason is valid"),
        )),
    ));
    blocked["event"]["reason"]["reason"] = json!("\t");
    assert_eq!(
        decode_value(blocked),
        Err(InvestigationCodecError::InvalidRecord(
            PlanningValidationError::EmptyBlockedReason
        ))
    );
}

#[test]
fn invalid_nested_research_record_uses_the_frozen_codec_error() {
    let mut completed = encoded_value(&event_records()[3]);
    completed["event"]["result"]["research_records"][0]["schema_version"] = json!(88);

    assert_eq!(
        decode_value(completed),
        Err(InvestigationCodecError::InvalidResearchRecord(
            CodecError::UnsupportedSchema(88)
        ))
    );
}

fn event_records() -> Vec<InvestigationRecord> {
    let initial = InvestigationTask::initial(task_id(1), "Explain scattering".to_owned())
        .expect("task is valid");
    let follow_up = InvestigationTask::follow_up(
        task_id(2),
        task_id(1),
        "Resolve wavelength dependence".to_owned(),
        ResearchGap::new("Unresolved wavelength dependence".to_owned()).expect("gap is valid"),
    )
    .expect("follow-up is valid");
    vec![
        record(
            1,
            InvestigationEvent::RequestRecorded(
                ResearchRequest::new("Why is the sky blue?".to_owned()).expect("request is valid"),
            ),
        ),
        record(
            2,
            InvestigationEvent::PlanRecorded(
                ResearchPlan::new(vec![initial]).expect("plan is valid"),
            ),
        ),
        record(
            3,
            InvestigationEvent::TaskStarted {
                task_id: task_id(1),
            },
        ),
        record(
            4,
            InvestigationEvent::TaskCompleted {
                task_id: task_id(1),
                result: InvestigationResult::new(vec![research_record(
                    1,
                    ResearchEvent::SourceRecorded(source()),
                )]),
            },
        ),
        record(
            5,
            InvestigationEvent::TaskFailed {
                task_id: task_id(1),
                failure: InvestigationFailure::new("No result".to_owned())
                    .expect("failure is valid"),
            },
        ),
        record(6, InvestigationEvent::FollowUpRecorded(follow_up)),
        record(
            7,
            InvestigationEvent::ResearchStopped(ResearchStopReason::Blocked(
                BlockedReason::new("Archive unavailable".to_owned()).expect("reason is valid"),
            )),
        ),
    ]
}

fn source() -> Source {
    Source::new(
        source_id(11),
        ContentDigest::sha256([5; 32]),
        "https://example.test/source".to_owned(),
        None,
        RetrievedAt::new("2026-08-29T10:00:00Z").expect("time is valid"),
        MediaType::new("text/plain").expect("media type is valid"),
    )
    .expect("source is valid")
}

fn encoded_value(record: &InvestigationRecord) -> Value {
    serde_json::from_slice(
        &encode_investigation_record(record).expect("investigation record encodes"),
    )
    .expect("encoded record is JSON")
}

fn decode_value(value: Value) -> Result<InvestigationRecord, InvestigationCodecError> {
    decode_investigation_record(&serde_json::to_vec(&value).expect("value encodes"))
}

fn record(sequence: u64, event: InvestigationEvent) -> InvestigationRecord {
    InvestigationRecord::new(sequence, event).expect("record is valid")
}

fn research_record(sequence: u64, event: ResearchEvent) -> ResearchRecord {
    ResearchRecord::new(sequence, event).expect("research record is valid")
}

fn task_id(value: u128) -> InvestigationTaskId {
    uuid(value).parse().expect("task identifier is valid")
}

fn source_id(value: u128) -> SourceId {
    uuid(value).parse().expect("source identifier is valid")
}

fn uuid(value: u128) -> String {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).hyphenated().to_string()
}
