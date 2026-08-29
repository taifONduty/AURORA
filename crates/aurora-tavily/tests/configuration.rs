use std::time::Duration;

use aurora_tavily::{TavilyConfig, TavilyConfigError, TavilyFailure, TavilyInvestigator};

#[test]
fn configuration_redacts_the_api_key() {
    let config = TavilyConfig::new("tvly-secret").expect("config is valid");

    assert!(!format!("{config:?}").contains("tvly-secret"));
    assert!(format!("{config:?}").contains("[REDACTED]"));
}

#[test]
fn configuration_rejects_empty_api_keys() {
    assert_eq!(TavilyConfig::new(""), Err(TavilyConfigError::EmptyApiKey));
}

#[test]
fn configuration_rejects_api_keys_that_cannot_be_sensitive_headers() {
    let error = TavilyConfig::new("key\r\ninjected").expect_err("invalid header is rejected");

    assert_eq!(error, TavilyConfigError::InvalidApiKey);
    assert!(!format!("{error:?}").contains("injected"));
    assert!(!error.to_string().contains("injected"));
}

#[test]
fn configuration_rejects_non_http_endpoints() {
    assert_eq!(
        TavilyConfig::for_endpoint("key", "file:///tmp/search", Duration::from_secs(1)),
        Err(TavilyConfigError::InvalidEndpoint),
    );
}

#[test]
fn configuration_rejects_zero_timeouts() {
    assert_eq!(
        TavilyConfig::for_endpoint("key", "https://example.test/search", Duration::ZERO),
        Err(TavilyConfigError::ZeroTimeout),
    );
}

#[test]
fn configuration_rejects_endpoints_with_userinfo_or_fragments() {
    for endpoint in [
        "https://user:password@example.test/search",
        "https://example.test/search#fragment",
    ] {
        assert_eq!(
            TavilyConfig::for_endpoint("key", endpoint, Duration::from_secs(1)),
            Err(TavilyConfigError::InvalidEndpoint),
        );
    }
}

#[test]
fn configuration_debug_omits_endpoint_query_values() {
    let config = TavilyConfig::for_endpoint(
        "key",
        "https://example.test/search?credential=endpoint-secret",
        Duration::from_secs(1),
    )
    .expect("config is valid");

    let debug = format!("{config:?}");
    assert!(!debug.contains("endpoint-secret"));
    assert!(!debug.contains("credential"));
    assert!(debug.contains("https"));
    assert!(debug.contains("example.test"));
    assert!(debug.contains("/search"));
}

#[test]
fn default_configuration_uses_the_fixed_tavily_search_endpoint() {
    let config = TavilyConfig::new("key").expect("config is valid");
    let debug = format!("{config:?}");

    assert!(debug.contains("https"));
    assert!(debug.contains("api.tavily.com"));
    assert!(debug.contains("/search"));
}

#[test]
fn investigator_construction_preserves_configuration_redaction() {
    let config = TavilyConfig::new("tvly-secret").expect("config is valid");
    let investigator = TavilyInvestigator::new(config).expect("client builds");

    assert!(!format!("{investigator:?}").contains("tvly-secret"));
}

#[test]
fn tavily_failures_convert_to_frozen_nonblank_investigation_failures() {
    let cases = [
        (TavilyFailure::Timeout, "tavily request timed out"),
        (TavilyFailure::Transport, "tavily transport failure"),
        (
            TavilyFailure::Authentication,
            "tavily authentication failed",
        ),
        (TavilyFailure::RateLimited, "tavily rate limited"),
        (TavilyFailure::Rejected, "tavily request rejected"),
        (TavilyFailure::Unavailable, "tavily service unavailable"),
        (
            TavilyFailure::UnexpectedStatus(599),
            "tavily unexpected HTTP status",
        ),
        (
            TavilyFailure::ResponseTooLarge,
            "tavily response exceeded limit",
        ),
        (
            TavilyFailure::MalformedResponse,
            "tavily response malformed",
        ),
        (TavilyFailure::InvalidResult, "tavily result invalid"),
        (
            TavilyFailure::InvalidResearchSequence,
            "tavily research sequence invalid",
        ),
        (
            TavilyFailure::ResearchSequenceExhausted,
            "tavily research sequence exhausted",
        ),
    ];

    for (failure, expected) in cases {
        let investigation_failure = failure.into_investigation_failure();

        assert_eq!(investigation_failure.as_str(), expected);
        assert!(!investigation_failure.as_str().trim().is_empty());
        assert!(!investigation_failure.as_str().contains("599"));
    }
}
