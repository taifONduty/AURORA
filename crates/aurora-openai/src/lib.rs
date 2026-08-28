mod translate;
mod wire;

use aurora_core::{ModelBackend, ModelFuture, ModelInput, ModelInvocation, ModelRequestFailure};
use reqwest::{
    Url,
    header::{AUTHORIZATION, HeaderValue},
    redirect::Policy,
};
use tokio_util::sync::CancellationToken;

use crate::translate::{
    body_independent_failure, classify_http_response, request_from_model_input,
};

const DEFAULT_RESPONSES_ENDPOINT: &str = "https://api.openai.com/v1/responses";

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
pub struct OpenAiConfig {
    api_key: ApiKey,
    model: String,
    endpoint: Url,
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

impl std::fmt::Debug for OpenAiConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenAiConfig")
            .field("api_key", &self.api_key)
            .field("model", &self.model)
            .field("endpoint", &SanitizedEndpoint(&self.endpoint))
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("OpenAI API key is empty")]
    EmptyApiKey,
    #[error("OpenAI API key cannot form an authorization header")]
    InvalidApiKey,
    #[error("OpenAI model name is empty")]
    EmptyModel,
    #[error("OpenAI Responses endpoint is invalid")]
    InvalidEndpoint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("OpenAI HTTP client could not be constructed")]
pub struct BackendBuildError;

pub struct OpenAiBackend {
    config: OpenAiConfig,
    client: reqwest::Client,
}

impl std::fmt::Debug for OpenAiBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = &self.client;
        formatter
            .debug_struct("OpenAiBackend")
            .field("config", &self.config)
            .field("client", &"<reqwest client>")
            .finish()
    }
}

impl OpenAiConfig {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Result<Self, ConfigError> {
        Self::for_endpoint(api_key, model, DEFAULT_RESPONSES_ENDPOINT)
    }

    pub fn for_endpoint(
        api_key: impl Into<String>,
        model: impl Into<String>,
        endpoint: impl AsRef<str>,
    ) -> Result<Self, ConfigError> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(ConfigError::EmptyApiKey);
        }
        let model = model.into();
        if model.is_empty() {
            return Err(ConfigError::EmptyModel);
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|_| ConfigError::InvalidApiKey)?;
        authorization.set_sensitive(true);
        let endpoint = Url::parse(endpoint.as_ref()).map_err(|_| ConfigError::InvalidEndpoint)?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(ConfigError::InvalidEndpoint);
        }
        Ok(Self {
            api_key: ApiKey { authorization },
            model,
            endpoint,
        })
    }
}

impl OpenAiBackend {
    pub fn new(config: OpenAiConfig) -> Result<Self, BackendBuildError> {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| BackendBuildError)?;
        Ok(Self { config, client })
    }
}

impl ModelBackend for OpenAiBackend {
    fn invoke(&mut self, input: ModelInput, cancellation: CancellationToken) -> ModelFuture {
        let client = self.client.clone();
        let config = self.config.clone();
        Box::pin(async move { invoke_owned(client, config, input, cancellation).await })
    }
}

async fn invoke_owned(
    client: reqwest::Client,
    config: OpenAiConfig,
    input: ModelInput,
    cancellation: CancellationToken,
) -> ModelInvocation {
    if cancellation.is_cancelled() {
        return ModelInvocation::Cancelled;
    }
    let request = match request_from_model_input(config.model, input) {
        Ok(request) => request,
        Err(category) => return ModelInvocation::RequestFailure(category),
    };
    let pending = client
        .post(config.endpoint)
        .header(AUTHORIZATION, config.api_key.authorization)
        .json(&request)
        .send();
    tokio::pin!(pending);
    let response = tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            return ModelInvocation::Cancelled;
        }
        result = &mut pending => match result {
            Ok(response) => response,
            Err(_) => {
                return ModelInvocation::RequestFailure(
                    ModelRequestFailure::Transport,
                );
            }
        }
    };
    let status = response.status();
    if let Some(category) = body_independent_failure(status) {
        return ModelInvocation::RequestFailure(category);
    }
    let body_failure = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        ModelRequestFailure::RateLimited
    } else {
        ModelRequestFailure::Transport
    };
    let pending_body = response.bytes();
    tokio::pin!(pending_body);
    let body = tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            return ModelInvocation::Cancelled;
        }
        result = &mut pending_body => match result {
            Ok(body) => body,
            Err(_) => {
                return ModelInvocation::RequestFailure(body_failure);
            }
        }
    };
    classify_http_response(status, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_rejects_empty_or_unusable_values() {
        assert!(matches!(
            OpenAiConfig::new("", "model"),
            Err(ConfigError::EmptyApiKey)
        ));
        assert!(matches!(
            OpenAiConfig::new("key", ""),
            Err(ConfigError::EmptyModel)
        ));
        assert!(matches!(
            OpenAiConfig::new("bad\nkey", "model"),
            Err(ConfigError::InvalidApiKey)
        ));
        assert!(matches!(
            OpenAiConfig::for_endpoint("key", "model", "not a URL"),
            Err(ConfigError::InvalidEndpoint)
        ));
        assert!(matches!(
            OpenAiConfig::for_endpoint("key", "model", "file:///tmp/response"),
            Err(ConfigError::InvalidEndpoint)
        ));
    }

    #[test]
    fn configuration_rejects_endpoint_userinfo_and_fragments() {
        for endpoint in [
            "https://user@example.test/v1/responses",
            "https://user:password@example.test/v1/responses",
            "https://:password@example.test/v1/responses",
            "https://example.test/v1/responses#private-fragment",
        ] {
            assert!(matches!(
                OpenAiConfig::for_endpoint("key", "model", endpoint),
                Err(ConfigError::InvalidEndpoint)
            ));
        }
    }

    #[test]
    fn configuration_and_backend_debug_omit_endpoint_query_values() {
        let query_secret = "query-secret-visible-only-to-the-test";
        let endpoint =
            format!("https://example.test:8443/v1/responses?endpoint_token={query_secret}");
        let config = OpenAiConfig::for_endpoint("key", "fixture-model", endpoint)
            .expect("query-bearing endpoint is supported");
        let config_debug = format!("{config:?}");
        assert!(!config_debug.contains(query_secret));
        assert!(!config_debug.contains("endpoint_token"));
        assert!(config_debug.contains("scheme"));
        assert!(config_debug.contains("example.test"));
        assert!(config_debug.contains("8443"));
        assert!(config_debug.contains("/v1/responses"));

        let backend = OpenAiBackend::new(config).expect("client builds");
        let backend_debug = format!("{backend:?}");
        assert!(!backend_debug.contains(query_secret));
        assert!(!backend_debug.contains("endpoint_token"));
    }

    #[test]
    fn configuration_and_backend_debug_output_redact_the_api_key() {
        let key = "sk-visible-only-to-the-test";
        let config = OpenAiConfig::new(key, "fixture-model").expect("valid configuration");
        let config_debug = format!("{config:?}");
        assert!(!config_debug.contains(key));
        assert!(config_debug.contains("[REDACTED]"));
        assert!(config_debug.contains("fixture-model"));

        let backend = OpenAiBackend::new(config).expect("client builds");
        let backend_debug = format!("{backend:?}");
        assert!(!backend_debug.contains(key));
        assert!(backend_debug.contains("[REDACTED]"));
    }

    #[test]
    fn default_configuration_uses_the_responses_endpoint() {
        let config = OpenAiConfig::new("key", "fixture-model").expect("valid configuration");
        let config_debug = format!("{config:?}");
        assert!(config_debug.contains("scheme: \"https\""));
        assert!(config_debug.contains("host: Some(\"api.openai.com\")"));
        assert!(config_debug.contains("port: None"));
        assert!(config_debug.contains("path: \"/v1/responses\""));
    }

    #[tokio::test]
    async fn cancelled_before_first_poll_returns_without_http() {
        let config =
            OpenAiConfig::for_endpoint("key", "fixture-model", "http://127.0.0.1:9/v1/responses")
                .expect("test endpoint is syntactically valid");
        let mut backend = OpenAiBackend::new(config).expect("client builds");
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();
        let invocation = aurora_core::ModelBackend::invoke(
            &mut backend,
            aurora_core::ModelInput {
                context: Vec::new(),
                tools: Vec::new(),
            },
            cancellation,
        )
        .await;

        assert_eq!(invocation, aurora_core::ModelInvocation::Cancelled);
    }

    #[tokio::test]
    async fn invoke_constructor_does_not_connect_before_future_is_polled() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("test listener binds to a dynamic loopback port");
        listener
            .set_nonblocking(true)
            .expect("test listener becomes nonblocking");
        let endpoint = format!(
            "http://{}/v1/responses",
            listener.local_addr().expect("test listener has an address")
        );
        let config = OpenAiConfig::for_endpoint("key", "fixture-model", endpoint)
            .expect("test endpoint is syntactically valid");
        let mut backend = OpenAiBackend::new(config).expect("client builds");
        let cancellation = tokio_util::sync::CancellationToken::new();

        let future = aurora_core::ModelBackend::invoke(
            &mut backend,
            aurora_core::ModelInput {
                context: Vec::new(),
                tools: Vec::new(),
            },
            cancellation,
        );

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let error = listener
            .accept()
            .expect_err("an unpolled invocation cannot connect");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);

        drop(future);
    }
}
