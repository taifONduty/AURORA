use aurora_core::{ModelBackend, ModelFuture, ModelInput, ModelInvocation, ModelRequestFailure};
use reqwest::{
    Url,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue},
    redirect::Policy,
};
use tokio_util::sync::CancellationToken;

mod json_object;
mod translate;
mod wire;

pub use json_object::{
    ZaiJsonObjectFuture, ZaiJsonObjectInvocation, ZaiJsonObjectRequest,
    ZaiJsonObjectValidationError,
};

const MAX_OUTPUT_TOKENS: u32 = 4096;
const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

const CODING_CHAT_COMPLETIONS: &str = "https://api.z.ai/api/coding/paas/v4/chat/completions";
const GLM_5_3: &str = "glm-5.3";

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    High,
    Max,
}

#[derive(Clone)]
struct ApiKey {
    authorization: HeaderValue,
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = &self.authorization;
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Clone)]
pub struct ZaiConfig {
    api_key: ApiKey,
    endpoint: Url,
    reasoning_effort: ReasoningEffort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("Z.AI API key is empty")]
    EmptyApiKey,
    #[error("Z.AI API key cannot form an authorization header")]
    InvalidApiKey,
}

impl ZaiConfig {
    pub fn new(api_key: impl Into<String>) -> Result<Self, ConfigError> {
        let endpoint =
            Url::parse(CODING_CHAT_COMPLETIONS).expect("the fixed Coding Plan endpoint is valid");
        Self::for_endpoint(api_key, endpoint)
    }

    fn for_endpoint(api_key: impl Into<String>, endpoint: Url) -> Result<Self, ConfigError> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(ConfigError::EmptyApiKey);
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|_| ConfigError::InvalidApiKey)?;
        authorization.set_sensitive(true);
        Ok(Self {
            api_key: ApiKey { authorization },
            endpoint,
            reasoning_effort: ReasoningEffort::High,
        })
    }

    pub const fn with_reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = effort;
        self
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

impl std::fmt::Debug for ZaiConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ZaiConfig")
            .field("api_key", &self.api_key)
            .field("model", &GLM_5_3)
            .field("endpoint", &SanitizedEndpoint(&self.endpoint))
            .field("reasoning_effort", &self.reasoning_effort)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Z.AI HTTP client could not be constructed")]
pub struct BackendBuildError;

pub struct ZaiBackend {
    config: ZaiConfig,
    client: reqwest::Client,
}

impl ZaiBackend {
    pub fn new(config: ZaiConfig) -> Result<Self, BackendBuildError> {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| BackendBuildError)?;
        Ok(Self { config, client })
    }

    pub fn invoke_json_object(
        &mut self,
        request: ZaiJsonObjectRequest,
        cancellation: CancellationToken,
    ) -> ZaiJsonObjectFuture {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(
            async move { invoke_json_object_owned(client, config, request, cancellation).await },
        )
    }
}

impl std::fmt::Debug for ZaiBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = &self.client;
        formatter
            .debug_struct("ZaiBackend")
            .field("config", &self.config)
            .field("client", &"<reqwest client>")
            .finish()
    }
}

impl ModelBackend for ZaiBackend {
    fn invoke(&mut self, input: ModelInput, cancellation: CancellationToken) -> ModelFuture {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move { invoke_owned(client, config, input, cancellation).await })
    }
}

async fn invoke_owned(
    client: reqwest::Client,
    config: ZaiConfig,
    input: ModelInput,
    cancellation: CancellationToken,
) -> ModelInvocation {
    if cancellation.is_cancelled() {
        return ModelInvocation::Cancelled;
    }
    let request = match translate::plain_request(input, config.reasoning_effort) {
        Ok(request) => request,
        Err(failure) => return ModelInvocation::RequestFailure(failure),
    };
    if cancellation.is_cancelled() {
        return ModelInvocation::Cancelled;
    }
    let body = match serde_json::to_vec(&request) {
        Ok(body) if body.len() <= MAX_REQUEST_BYTES => body,
        Ok(_) | Err(_) => {
            return ModelInvocation::RequestFailure(ModelRequestFailure::RequestRejected);
        }
    };
    if cancellation.is_cancelled() {
        return ModelInvocation::Cancelled;
    }
    send_plain(client, config, body, cancellation).await
}

async fn invoke_json_object_owned(
    client: reqwest::Client,
    config: ZaiConfig,
    request: ZaiJsonObjectRequest,
    cancellation: CancellationToken,
) -> ZaiJsonObjectInvocation {
    if cancellation.is_cancelled() {
        return ZaiJsonObjectInvocation::Cancelled;
    }
    let request = translate::json_object_request(request, config.reasoning_effort);
    if cancellation.is_cancelled() {
        return ZaiJsonObjectInvocation::Cancelled;
    }
    let body = match serde_json::to_vec(&request) {
        Ok(body) if body.len() <= MAX_REQUEST_BYTES => body,
        Ok(_) | Err(_) => return ZaiJsonObjectInvocation::RequestTooLarge,
    };
    if cancellation.is_cancelled() {
        return ZaiJsonObjectInvocation::Cancelled;
    }
    send_json_object(client, config, body, cancellation).await
}

fn classify_status(status: reqwest::StatusCode) -> Option<ModelRequestFailure> {
    if status.is_success() {
        None
    } else if status == reqwest::StatusCode::UNAUTHORIZED {
        Some(ModelRequestFailure::Authentication)
    } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        Some(ModelRequestFailure::RateLimited)
    } else if status.is_client_error() {
        Some(ModelRequestFailure::RequestRejected)
    } else if status.is_server_error() {
        Some(ModelRequestFailure::ServiceUnavailable)
    } else {
        Some(ModelRequestFailure::Transport)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoundedBodyFailure {
    Cancelled,
    TooLarge,
    Transport,
}

async fn send_plain(
    client: reqwest::Client,
    config: ZaiConfig,
    body: Vec<u8>,
    cancellation: CancellationToken,
) -> ModelInvocation {
    let pending = client
        .post(config.endpoint)
        .header(AUTHORIZATION, config.api_key.authorization)
        .header(CONTENT_TYPE, "application/json")
        .body(body)
        .send();
    tokio::pin!(pending);
    let response = tokio::select! {
        biased;
        () = cancellation.cancelled() => return ModelInvocation::Cancelled,
        result = &mut pending => match result {
            Ok(response) => response,
            Err(_) => {
                return ModelInvocation::RequestFailure(ModelRequestFailure::Transport);
            }
        }
    };
    if let Some(failure) = classify_status(response.status()) {
        return ModelInvocation::RequestFailure(failure);
    }
    match read_bounded_body(response, cancellation).await {
        Ok(body) => translate::classify_plain_response(&body),
        Err(failure) => plain_body_failure_outcome(failure),
    }
}

async fn send_json_object(
    client: reqwest::Client,
    config: ZaiConfig,
    body: Vec<u8>,
    cancellation: CancellationToken,
) -> ZaiJsonObjectInvocation {
    let pending = client
        .post(config.endpoint)
        .header(AUTHORIZATION, config.api_key.authorization)
        .header(CONTENT_TYPE, "application/json")
        .body(body)
        .send();
    tokio::pin!(pending);
    let response = tokio::select! {
        biased;
        () = cancellation.cancelled() => return ZaiJsonObjectInvocation::Cancelled,
        result = &mut pending => match result {
            Ok(response) => response,
            Err(_) => {
                return ZaiJsonObjectInvocation::RequestFailure(ModelRequestFailure::Transport);
            }
        }
    };
    if let Some(failure) = classify_status(response.status()) {
        return ZaiJsonObjectInvocation::RequestFailure(failure);
    }
    match read_bounded_body(response, cancellation).await {
        Ok(body) => translate::classify_json_object_response(&body),
        Err(failure) => json_object_body_failure_outcome(failure),
    }
}

fn plain_body_failure_outcome(failure: BoundedBodyFailure) -> ModelInvocation {
    match failure {
        BoundedBodyFailure::Cancelled => ModelInvocation::Cancelled,
        BoundedBodyFailure::TooLarge => {
            ModelInvocation::RequestFailure(ModelRequestFailure::UnsupportedResponse)
        }
        BoundedBodyFailure::Transport => {
            ModelInvocation::RequestFailure(ModelRequestFailure::Transport)
        }
    }
}

fn json_object_body_failure_outcome(failure: BoundedBodyFailure) -> ZaiJsonObjectInvocation {
    match failure {
        BoundedBodyFailure::Cancelled => ZaiJsonObjectInvocation::Cancelled,
        BoundedBodyFailure::TooLarge => ZaiJsonObjectInvocation::ResponseTooLarge,
        BoundedBodyFailure::Transport => {
            ZaiJsonObjectInvocation::RequestFailure(ModelRequestFailure::Transport)
        }
    }
}

async fn read_bounded_body(
    mut response: reqwest::Response,
    cancellation: CancellationToken,
) -> Result<Vec<u8>, BoundedBodyFailure> {
    let mut body = Vec::new();
    loop {
        let pending = response.chunk();
        tokio::pin!(pending);
        let chunk = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Err(BoundedBodyFailure::Cancelled),
            result = &mut pending => result.map_err(|_| BoundedBodyFailure::Transport)?,
        };
        let Some(chunk) = chunk else {
            return Ok(body);
        };
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(body.len()) {
            return Err(BoundedBodyFailure::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
}

#[cfg(test)]
mod tests;
