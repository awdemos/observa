use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_core::Stream;
use observa_shared::{ObservaError, Result};
use reqwest::header::HeaderMap;
use serde::Deserialize;

pub async fn token_stream(
    client: &reqwest::Client,
    url: &str,
    headers: HeaderMap,
    body: &impl serde::Serialize,
) -> Result<TokenStream> {
    let response = send_json_request(client, url, headers, body).await?;
    Ok(TokenStream::new(response.bytes_stream()))
}

/// Send a JSON POST request and return the response if the status is success.
/// This centralizes the network-error and non-2xx handling shared by the
/// streaming and non-streaming completion paths.
pub(crate) async fn send_json_request(
    client: &reqwest::Client,
    url: &str,
    headers: HeaderMap,
    body: &impl serde::Serialize,
) -> Result<reqwest::Response> {
    let response = client
        .post(url)
        .headers(headers)
        .json(body)
        .send()
        .await
        .map_err(|e| ObservaError::Llm(format!("request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ObservaError::Llm(format!(
            "upstream returned {status}: {body}"
        )));
    }

    Ok(response)
}

pub struct TokenStream {
    inner: Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send>>,
    buffer: String,
}

impl TokenStream {
    fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
    {
        Self {
            inner: Box::pin(stream),
            buffer: String::new(),
        }
    }
}

impl Stream for TokenStream {
    type Item = Result<String>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(nl) = self.buffer.find('\n') {
                let line = self.buffer.drain(..=nl).collect::<String>();
                if let Some(result) = parse_stream_line(&line) {
                    return Poll::Ready(Some(result));
                }
                continue;
            }

            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    self.buffer.push_str(&String::from_utf8_lossy(&chunk));
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(ObservaError::Llm(format!("stream error: {e}")))));
                }
                Poll::Ready(None) => {
                    if self.buffer.is_empty() {
                        return Poll::Ready(None);
                    }
                    let line = std::mem::take(&mut self.buffer);
                    if let Some(result) = parse_stream_line(&line) {
                        return Poll::Ready(Some(result));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

fn parse_stream_line(line: &str) -> Option<Result<String>> {
    let trimmed = line.strip_prefix("data: ").unwrap_or(line).trim();
    if trimmed == "[DONE]" || trimmed.is_empty() {
        return None;
    }

    let event: StreamEvent = match serde_json::from_str(trimmed) {
        Ok(e) => e,
        Err(e) => return Some(Err(ObservaError::Llm(format!("invalid stream event: {e}")))),
    };

    event
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.delta.visible_content())
        .map(Ok)
}

#[derive(Debug, Deserialize)]
struct StreamEvent {
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: StreamDelta,
}

#[derive(Debug, Default, Deserialize)]
struct StreamDelta {
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
}

impl StreamDelta {
    fn visible_content(&self) -> Option<String> {
        let text = self
            .content
            .as_ref()
            .filter(|s| !s.is_empty())
            .cloned()
            .or_else(|| self.reasoning_content.clone())?;
        if text.is_empty() {
            return None;
        }
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_stream_line;

    #[test]
    fn parses_content_delta() {
        let line = r#"data: {"choices":[{"delta":{"content":"Hello"}}]}"#;
        assert_eq!(parse_stream_line(line).unwrap().unwrap(), "Hello");
    }

    #[test]
    fn ignores_done_marker() {
        assert!(parse_stream_line("data: [DONE]").is_none());
    }

    #[test]
    fn ignores_empty_delta() {
        let line = r#"data: {"choices":[{"delta":{}}]}"#;
        assert!(parse_stream_line(line).is_none());
    }
}
