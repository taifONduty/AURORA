use aurora_research::{
    ContentDigest, Evidence, EvidenceId, InvestigationResult, MediaType, ResearchEvent,
    ResearchRecord, RetrievedAt, Source, SourceId,
};
use reqwest::Url;
use ring::digest;

use crate::{TavilyFailure, wire::SearchResponse};

pub(super) fn admit_response(
    bytes: &[u8],
    next_sequence: u64,
    retrieved_at: RetrievedAt,
) -> Result<InvestigationResult, TavilyFailure> {
    let response = SearchResponse::decode(bytes)?;

    if next_sequence == 0 {
        return Err(TavilyFailure::InvalidResearchSequence);
    }

    let media_type =
        MediaType::new("text/plain".to_owned()).map_err(|_| TavilyFailure::InvalidResult)?;
    let mut records = Vec::new();
    let mut next_source_sequence = Some(next_sequence);

    for result in response.results {
        let Some(content) = result.raw_content else {
            continue;
        };
        if content.trim().is_empty() {
            continue;
        }

        let source_sequence =
            next_source_sequence.ok_or(TavilyFailure::ResearchSequenceExhausted)?;
        let evidence_sequence = source_sequence
            .checked_add(1)
            .ok_or(TavilyFailure::ResearchSequenceExhausted)?;
        next_source_sequence = evidence_sequence.checked_add(1);

        let locator = result.url.ok_or(TavilyFailure::InvalidResult)?;
        let url = Url::parse(&locator).map_err(|_| TavilyFailure::InvalidResult)?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(TavilyFailure::InvalidResult);
        }

        let title = result.title.filter(|value| !value.trim().is_empty());
        let digest = digest::digest(&digest::SHA256, content.as_bytes());
        let mut digest_bytes = [0_u8; 32];
        digest_bytes.copy_from_slice(digest.as_ref());
        let source_id = SourceId::generate();
        let evidence_id = EvidenceId::generate();
        let source = Source::new(
            source_id,
            ContentDigest::sha256(digest_bytes),
            locator,
            title,
            retrieved_at.clone(),
            media_type.clone(),
        )
        .map_err(|_| TavilyFailure::InvalidResult)?;
        let evidence = Evidence::new(evidence_id, source_id, content)
            .map_err(|_| TavilyFailure::InvalidResult)?;
        let source_record =
            ResearchRecord::new(source_sequence, ResearchEvent::SourceRecorded(source))
                .map_err(|_| TavilyFailure::InvalidResearchSequence)?;
        let evidence_record =
            ResearchRecord::new(evidence_sequence, ResearchEvent::EvidenceRecorded(evidence))
                .map_err(|_| TavilyFailure::InvalidResearchSequence)?;

        records.push(source_record);
        records.push(evidence_record);
    }

    Ok(InvestigationResult::new(records))
}

#[cfg(test)]
mod tests {
    use aurora_research::{MediaType, ResearchEvent, RetrievedAt};

    use super::admit_response;
    use crate::TavilyFailure;

    const RETRIEVED_AT: &str = "2026-08-29T12:34:56Z";

    #[test]
    fn admits_usable_results_in_provider_order_with_exact_snapshot_bytes() {
        let fixture = r#"{
                "results": [
                    {
                        "title": "  First result  ",
                        "url": "https://first.example/article",
                        "raw_content": " \nExact body \t"
                    },
                    {
                        "title": "Second result",
                        "url": "https://second.example/article",
                        "raw_content": "Second body"
                    }
                ]
            }"#;
        let retrieved_at = retrieved_at();

        let admitted = admit_response(fixture.as_bytes(), 41, retrieved_at.clone())
            .expect("admission succeeds");
        let records = admitted.research_records();

        assert_eq!(records.len(), 4);
        assert_eq!(records[0].sequence(), 41);
        assert_eq!(records[1].sequence(), 42);
        assert_eq!(records[2].sequence(), 43);
        assert_eq!(records[3].sequence(), 44);

        let ResearchEvent::SourceRecorded(first_source) = records[0].event() else {
            panic!("first record is a source");
        };
        assert_eq!(first_source.locator(), "https://first.example/article");
        assert_eq!(first_source.title(), Some("  First result  "));
        assert_eq!(first_source.retrieved_at(), &retrieved_at);
        assert_eq!(
            first_source.media_type(),
            &MediaType::new("text/plain").expect("valid media type")
        );
        assert_eq!(
            first_source.content_digest().as_sha256(),
            &[
                0x44, 0x24, 0x49, 0x56, 0xfb, 0xb8, 0x24, 0xfb, 0xd7, 0xa7, 0x8f, 0x8b, 0x06, 0x12,
                0x89, 0x84, 0xd4, 0xf9, 0xf1, 0xcf, 0x60, 0x9e, 0x95, 0x50, 0x47, 0x24, 0xd6, 0x51,
                0x50, 0x0c, 0x50, 0x23,
            ],
        );

        let ResearchEvent::EvidenceRecorded(first_evidence) = records[1].event() else {
            panic!("second record is evidence");
        };
        assert_eq!(first_evidence.source_id(), first_source.id());
        assert_eq!(first_evidence.excerpt(), " \nExact body \t");

        let ResearchEvent::SourceRecorded(second_source) = records[2].event() else {
            panic!("third record is a source");
        };
        assert_eq!(second_source.locator(), "https://second.example/article");
        assert_eq!(second_source.title(), Some("Second result"));
        assert_eq!(second_source.retrieved_at(), &retrieved_at);
        assert_eq!(
            second_source.media_type(),
            &MediaType::new("text/plain").expect("valid media type")
        );

        let ResearchEvent::EvidenceRecorded(second_evidence) = records[3].event() else {
            panic!("fourth record is evidence");
        };
        assert_eq!(second_evidence.source_id(), second_source.id());
        assert_eq!(second_evidence.excerpt(), "Second body");
    }

    #[test]
    fn skips_absent_null_and_blank_raw_content() {
        let fixture = r#"{
                "results": [
                    { "title": "Absent", "url": "https://absent.example/article" },
                    { "title": "Null", "url": "https://null.example/article", "raw_content": null },
                    { "title": "Blank", "url": "https://blank.example/article", "raw_content": " \n\t " }
                ]
            }"#;

        let admitted =
            admit_response(fixture.as_bytes(), 1, retrieved_at()).expect("admission succeeds");

        assert!(admitted.research_records().is_empty());
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(
            admit_response(b"{not json", 1, retrieved_at()),
            Err(TavilyFailure::MalformedResponse)
        ));
    }

    #[test]
    fn requires_a_results_array_but_accepts_an_explicit_empty_array() {
        assert!(matches!(
            admit_response(b"{}", 1, retrieved_at()),
            Err(TavilyFailure::MalformedResponse)
        ));

        let admitted = admit_response(br#"{"results":[]}"#, 1, retrieved_at())
            .expect("an explicit empty result array admits");

        assert!(admitted.research_records().is_empty());
    }

    #[test]
    fn rejects_non_http_result_urls_without_partial_admission() {
        let fixture = r#"{
                "results": [
                    { "url": "https://valid.example/article", "raw_content": "valid" },
                    { "url": "file:///tmp/article", "raw_content": "invalid" }
                ]
            }"#;

        assert_eq!(
            admit_response(fixture.as_bytes(), 1, retrieved_at()),
            Err(TavilyFailure::InvalidResult)
        );
    }

    #[test]
    fn rejects_zero_research_sequence() {
        assert_eq!(
            admit_response(single_usable_result().as_bytes(), 0, retrieved_at()),
            Err(TavilyFailure::InvalidResearchSequence)
        );
    }

    #[test]
    fn rejects_research_sequence_overflow() {
        assert_eq!(
            admit_response(single_usable_result().as_bytes(), u64::MAX, retrieved_at()),
            Err(TavilyFailure::ResearchSequenceExhausted),
        );
    }

    #[test]
    fn admits_one_usable_result_at_the_last_two_sequences() {
        let admitted = admit_response(
            single_usable_result().as_bytes(),
            u64::MAX - 1,
            retrieved_at(),
        )
        .expect("the last source and evidence pair fit");
        let records = admitted.research_records();

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].sequence(), u64::MAX - 1);
        assert_eq!(records[1].sequence(), u64::MAX);
    }

    #[test]
    fn rejects_two_usable_results_that_overflow_without_exposing_a_result() {
        let fixture = r#"{
            "results": [
                { "url": "https://first.example/article", "raw_content": "first" },
                { "url": "https://second.example/article", "raw_content": "second" }
            ]
        }"#;

        assert_eq!(
            admit_response(fixture.as_bytes(), u64::MAX - 1, retrieved_at()),
            Err(TavilyFailure::ResearchSequenceExhausted),
        );
    }

    #[test]
    fn skipped_results_do_not_consume_research_sequences() {
        let fixture = r#"{
            "results": [
                { "url": "https://blank.example/article", "raw_content": " \n " },
                { "url": "https://first.example/article", "raw_content": "first" },
                { "url": "https://absent.example/article" },
                { "url": "https://second.example/article", "raw_content": "second" }
            ]
        }"#;
        let admitted = admit_response(fixture.as_bytes(), 50, retrieved_at())
            .expect("only usable results consume sequences");
        let sequences: Vec<_> = admitted
            .research_records()
            .iter()
            .map(|record| record.sequence())
            .collect();

        assert_eq!(sequences, [50, 51, 52, 53]);
    }

    fn retrieved_at() -> RetrievedAt {
        RetrievedAt::new(RETRIEVED_AT).expect("fixture timestamp is valid")
    }

    fn single_usable_result() -> &'static str {
        r#"{
            "results": [
                { "url": "https://result.example/article", "raw_content": "content" }
            ]
        }"#
    }
}
