use std::time::Duration;

use observa_shared::{ChatMessage, ObservaError, Result, Role};
use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};

use crate::stream::{send_json_request, token_stream};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// OpenAI-compatible chat completion client.
#[derive(Debug, Clone)]
pub struct LlmClient {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ApiMessage>,
    stream: bool,
    max_tokens: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApiMessage {
    role: String,
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
}

impl ApiMessage {
    fn visible_content(&self) -> String {
        if !self.content.is_empty() {
            return self.content.clone();
        }
        self.reasoning_content.clone().unwrap_or_default()
    }
}

impl From<&ChatMessage> for ApiMessage {
    fn from(value: &ChatMessage) -> Self {
        Self {
            role: role_to_string(value.role),
            content: value.content.clone(),
            reasoning_content: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Option<ApiMessage>,
}

impl LlmClient {
    /// Create a new client.
    ///
    /// `base_url` is the OpenAI-compatible API root (e.g. `https://api.openai.com/v1`
    /// or `http://localhost:8080/v1`). It should include the `/v1` path prefix.
    /// `timeout` controls the overall request limit; pass `None` for the default 60 s.
    pub fn new(
        base_url: String,
        api_key: Option<String>,
        model: String,
        timeout: Option<Duration>,
    ) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let client = reqwest::Client::builder()
            .timeout(timeout.unwrap_or(DEFAULT_TIMEOUT))
            .connect_timeout(CONNECT_TIMEOUT)
            .default_headers(headers)
            .build()
            .expect("reqwest client built from constants should not fail");

        Self {
            client,
            base_url,
            api_key,
            model,
        }
    }

    fn url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        if let Some(key) = &self.api_key {
            if let Ok(value) = HeaderValue::from_str(&format!("Bearer {key}")) {
                headers.insert(header::AUTHORIZATION, value);
            }
        }
        headers
    }

    fn body(&self, messages: &[ChatMessage], stream: bool) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: self.model.clone(),
            messages: messages.iter().map(ApiMessage::from).collect(),
            stream,
            max_tokens: 512,
        }
    }

    /// Send a non-streaming chat completion request and return the assistant
    /// message.
    pub async fn complete(&self, messages: &[ChatMessage]) -> Result<ChatMessage> {
        let response = send_json_request(
            &self.client,
            &self.url(),
            self.headers(),
            &self.body(messages, false),
        )
        .await?;

        let payload: ChatCompletionResponse = response
            .json()
            .await
            .map_err(|e| ObservaError::Llm(format!("failed to parse completion: {e}")))?;

        payload
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message)
            .map(|message| ChatMessage {
                role: parse_role(&message.role),
                content: message.visible_content(),
            })
            .ok_or_else(|| ObservaError::Llm("completion missing assistant message".to_string()))
    }

    /// Send a streaming chat completion request and yield assistant content
    /// tokens as they arrive.
    pub async fn complete_stream(
        &self,
        messages: &[ChatMessage],
    ) -> Result<impl tokio_stream::Stream<Item = Result<String>> + Send + 'static> {
        token_stream(
            &self.client,
            &self.url(),
            self.headers(),
            &self.body(messages, true),
        )
        .await
    }
}

fn role_to_string(role: Role) -> String {
    match role {
        Role::System => "system".to_string(),
        Role::User => "user".to_string(),
        Role::Assistant => "assistant".to_string(),
    }
}

fn parse_role(role: &str) -> Role {
    match role {
        "system" => Role::System,
        "assistant" => Role::Assistant,
        _ => Role::User,
    }
}
