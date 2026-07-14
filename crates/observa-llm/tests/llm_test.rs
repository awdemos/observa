use axum::{routing::post, Json, Router};
use observa_llm::LlmClient;
use observa_shared::{ChatMessage, Role};
use serde_json::{json, Value};
use tokio_stream::StreamExt;

async fn mock_complete(Json(body): Json<Value>) -> Json<Value> {
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("test-model")
        .to_string();
    let content = body
        .get("messages")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.last())
        .and_then(|msg| msg.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or("hello")
        .to_string();

    Json(json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "created": 0,
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": format!("Echo: {content}")
            },
            "finish_reason": "stop"
        }]
    }))
}

async fn spawn_mock() -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new().route("/v1/chat/completions", post(mock_complete));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}/v1");
    let server = axum::serve(listener, app);

    let handle = tokio::spawn(async move {
        server.await.unwrap();
    });

    (base, handle)
}

#[tokio::test]
async fn complete_parses_assistant_message() {
    let (base, handle) = spawn_mock().await;

    let client = LlmClient::new(base, None, "test-model".to_string(), None);
    let messages = vec![ChatMessage {
        role: Role::User,
        content: "What is Observa?".to_string(),
    }];
    let reply = client.complete(&messages).await.unwrap();

    assert_eq!(reply.role, Role::Assistant);
    assert!(
        reply.content.contains("Echo: What is Observa?"),
        "unexpected reply content: {}",
        reply.content
    );

    handle.abort();
}

#[tokio::test]
async fn complete_stream_emits_tokens() {
    let app = Router::new().route("/v1/chat/completions", post(stream_handler));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}/v1");
    let server = axum::serve(listener, app);

    let handle = tokio::spawn(async move {
        server.await.unwrap();
    });

    let client = LlmClient::new(base, None, "test-model".to_string(), None);
    let messages = vec![ChatMessage {
        role: Role::User,
        content: "Say hi".to_string(),
    }];

    let tokens: Vec<String> = client
        .complete_stream(&messages)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let joined = tokens.join("");
    assert_eq!(joined, "Hello world");

    handle.abort();
}

async fn stream_handler() -> axum::response::Response<axum::body::Body> {
    use axum::response::IntoResponse;

    let deltas = vec![
        json!({"choices": [{"delta": {"content": "Hello "}}]}),
        json!({"choices": [{"delta": {"content": "world"}}]}),
        json!({"choices": [{"delta": {}}]}),
    ];

    let stream = tokio_stream::iter(deltas.into_iter().map(|delta| {
        let line = format!("data: {}\n\n", serde_json::to_string(&delta).unwrap());
        Ok::<_, std::convert::Infallible>(line)
    }));

    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}
