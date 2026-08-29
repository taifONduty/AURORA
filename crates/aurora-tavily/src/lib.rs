use std::time::Duration;

use aurora_research::{InvestigationFailure, InvestigationResult, InvestigationTask, RetrievedAt};
use reqwest::{
    Url,
    header::{AUTHORIZATION, HeaderValue},
    redirect::Policy,
};

mod admission;
mod wire;

const DEFAULT_SEARCH_ENDPOINT: &str = "https://api.tavily.com/search";
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_BODY_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TavilyConfigError {
    #[error("Tavily API key is empty")]
    EmptyApiKey,
    #[error("Tavily API key cannot form an authorization header")]
    InvalidApiKey,
    #[error("Tavily search endpoint is invalid")]
    InvalidEndpoint,
    #[error("Tavily request timeout must be nonzero")]
    ZeroTimeout,
}

#[derive(Clone, PartialEq, Eq)]
pub struct TavilyConfig {
    authorization: HeaderValue,
    endpoint: Url,
    request_timeout: Duration,
}

struct RedactedAuthorization<'a>(&'a HeaderValue);

impl std::fmt::Debug for RedactedAuthorization<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = self.0;
        formatter.write_str("[REDACTED]")
    }
}

struct SanitizedEndpoint<'a>(&'a Url);

impl std::fmt::Debug for SanitizedEndpoint<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Endpoint")
            .field("scheme", &self.0.scheme())
            .field("host", &self.0.host_str())
            .field("port", &self.0.port())
            .field("path", &self.0.path())
            .finish()
    }
}

impl std::fmt::Debug for TavilyConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TavilyConfig")
            .field("authorization", &RedactedAuthorization(&self.authorization))
            .field("endpoint", &SanitizedEndpoint(&self.endpoint))
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

impl TavilyConfig {
    pub fn new(api_key: impl Into<String>) -> Result<Self, TavilyConfigError> {
        Self::for_endpoint(api_key, DEFAULT_SEARCH_ENDPOINT, DEFAULT_REQUEST_TIMEOUT)
    }

    pub fn for_endpoint(
        api_key: impl Into<String>,
        endpoint: impl AsRef<str>,
        timeout: Duration,
    ) -> Result<Self, TavilyConfigError> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(TavilyConfigError::EmptyApiKey);
        }
        if timeout.is_zero() {
            return Err(TavilyConfigError::ZeroTimeout);
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|_| TavilyConfigError::InvalidApiKey)?;
        authorization.set_sensitive(true);
        let endpoint =
            Url::parse(endpoint.as_ref()).map_err(|_| TavilyConfigError::InvalidEndpoint)?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(TavilyConfigError::InvalidEndpoint);
        }

        Ok(Self {
            authorization,
            endpoint,
            request_timeout: timeout,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Tavily HTTP client could not be constructed")]
pub struct TavilyBuildError;

pub struct TavilyInvestigator {
    config: TavilyConfig,
    client: reqwest::Client,
}

impl std::fmt::Debug for TavilyInvestigator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = &self.client;
        formatter
            .debug_struct("TavilyInvestigator")
            .field("config", &self.config)
            .field("client", &"<reqwest client>")
            .finish()
    }
}

impl TavilyInvestigator {
    pub fn new(config: TavilyConfig) -> Result<Self, TavilyBuildError> {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .timeout(config.request_timeout)
            .build()
            .map_err(|_| TavilyBuildError)?;
        Ok(Self { config, client })
    }

    pub async fn investigate(
        &self,
        task: &InvestigationTask,
        next_sequence: u64,
        retrieved_at: RetrievedAt,
    ) -> Result<InvestigationResult, TavilyFailure> {
        let response = self
            .client
            .post(self.config.endpoint.clone())
            .header(AUTHORIZATION, self.config.authorization.clone())
            .json(&wire::SearchRequest::for_objective(task.objective()))
            .send()
            .await
            .map_err(request_failure)?;
        let status = response.status();
        if !status.is_success() {
            return Err(failure_for_status(status.as_u16()));
        }

        let mut bytes = Vec::new();
        let mut response = response;
        while let Some(chunk) = response.chunk().await.map_err(request_failure)? {
            if chunk.len() > RESPONSE_BODY_LIMIT - bytes.len() {
                return Err(TavilyFailure::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }

        admission::admit_response(&bytes, next_sequence, retrieved_at)
    }
}

fn request_failure(error: reqwest::Error) -> TavilyFailure {
    if error.is_timeout() {
        TavilyFailure::Timeout
    } else {
        TavilyFailure::Transport
    }
}

fn failure_for_status(status: u16) -> TavilyFailure {
    match status {
        400 => TavilyFailure::Rejected,
        401 | 403 => TavilyFailure::Authentication,
        429 => TavilyFailure::RateLimited,
        500..=599 => TavilyFailure::Unavailable,
        _ => TavilyFailure::UnexpectedStatus(status),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TavilyFailure {
    Timeout,
    Transport,
    Authentication,
    RateLimited,
    Rejected,
    Unavailable,
    UnexpectedStatus(u16),
    ResponseTooLarge,
    MalformedResponse,
    InvalidResult,
    InvalidResearchSequence,
    ResearchSequenceExhausted,
}

impl TavilyFailure {
    pub fn into_investigation_failure(self) -> InvestigationFailure {
        let message = match self {
            Self::Timeout => "tavily request timed out",
            Self::Transport => "tavily transport failure",
            Self::Authentication => "tavily authentication failed",
            Self::RateLimited => "tavily rate limited",
            Self::Rejected => "tavily request rejected",
            Self::Unavailable => "tavily service unavailable",
            Self::UnexpectedStatus(_) => "tavily unexpected HTTP status",
            Self::ResponseTooLarge => "tavily response exceeded limit",
            Self::MalformedResponse => "tavily response malformed",
            Self::InvalidResult => "tavily result invalid",
            Self::InvalidResearchSequence => "tavily research sequence invalid",
            Self::ResearchSequenceExhausted => "tavily research sequence exhausted",
        };
        InvestigationFailure::new(message.to_owned())
            .unwrap_or_else(|_| unreachable!("frozen Tavily failure message is nonblank"))
    }
}
