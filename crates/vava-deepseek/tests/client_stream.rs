//! End-to-end tests for [`DeepSeekClient`] against a mock HTTP server — no
//! network, no API key required. The server captures the request (so the
//! serialization can be asserted) and serves a canned response.

use std::net::SocketAddr;
use std::sync::Arc;

use futures::StreamExt;
use secrecy::SecretString;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use vava_core::{Message, ModelClient, ModelEvent, UserMessage};
use vava_deepseek::{DeepSeekClient, DeepSeekError, ModelConfig};

const TOOL_TURN: &str = include_str!("fixtures/tool_turn.stream.txt");

/// A tiny HTTP server that serves one canned response and captures the
/// request it received (headers + body).
async fn mock_server(
    status_line: &'static str,
    content_type: &'static str,
    response_body: &'static str,
) -> (SocketAddr, oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (request_tx, request_rx) = oneshot::channel();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();

        // Read the request: headers up to \r\n\r\n, then the body.
        let mut request = Vec::new();
        let mut tmp = [0u8; 4096];
        let header_end = loop {
            let n = socket.read(&mut tmp).await.unwrap();
            if n == 0 {
                break None;
            }
            request.extend_from_slice(&tmp[..n]);
            if let Some(pos) = find_subsequence(&request, b"\r\n\r\n") {
                break Some(pos);
            }
        };
        let header_end = header_end.expect("client closed before sending headers");

        let headers = String::from_utf8_lossy(&request[..header_end]).to_string();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let lower = line.to_ascii_lowercase();
                lower
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while request.len() < header_end + 4 + content_length {
            let n = socket.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&tmp[..n]);
        }
        let request_body = String::from_utf8_lossy(&request[header_end + 4..]).to_string();
        let full = format!("{headers}\r\n\r\n{request_body}");
        let _ = request_tx.send(full);

        let response = format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: {content_type}\r\n\r\n{response_body}"
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });

    (addr, request_rx)
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn client_at(addr: SocketAddr) -> DeepSeekClient {
    DeepSeekClient::new(
        SecretString::from("sk-test-secret"),
        ModelConfig::default().with_base_url(format!("http://{addr}")),
    )
}

fn user_messages() -> Vec<Message> {
    vec![Message::User(UserMessage {
        content: "run the tests".into(),
    })]
}

#[tokio::test]
async fn streams_events_from_a_mock_server() {
    let (addr, request_rx) = mock_server("200 OK", "text/event-stream", TOOL_TURN).await;
    let client = client_at(addr);

    let stream = client
        .stream(&user_messages(), "You are vava.", &[])
        .await
        .unwrap();
    let events: Vec<ModelEvent> = stream
        .map(|item| item.expect("stream item must be ok"))
        .collect()
        .await;

    // Same event sequence the fixture tests assert, delivered end to end.
    assert_eq!(
        events,
        vec![
            ModelEvent::ReasoningDelta("Let me think about this.".into()),
            ModelEvent::ReasoningDelta(" First, I'll run the tests.".into()),
            ModelEvent::ToolCallStarted {
                index: 0,
                id: "call_1".into(),
                name: "bash".into(),
            },
            ModelEvent::ToolCallArgumentsDelta {
                index: 0,
                delta: "cargo test".into(),
            },
            ModelEvent::Finished,
            ModelEvent::Usage(vava_core::Usage::new(20, 10)),
        ]
    );

    // The request that went over the wire is correct and carries the key
    // only in the Authorization header.
    let request = request_rx.await.unwrap();
    let (headers, body) = request.split_once("\r\n\r\n").unwrap();
    assert!(headers.contains("POST /chat/completions HTTP/1.1"));
    // HTTP header names are case-insensitive; reqwest may send lowercase.
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("authorization: bearer sk-test-secret")
    );
    assert!(!body.contains("sk-test-secret")); // the key is never in the body
    assert!(body.contains(r#""model":"deepseek-chat""#));
    assert!(body.contains(r#""stream":true"#));
    assert!(body.contains(r#"{"role":"system","content":"You are vava."}"#));
    assert!(body.contains(r#"{"role":"user","content":"run the tests"}"#));
}

#[tokio::test]
async fn implements_the_model_client_seam() {
    let (addr, _request_rx) = mock_server("200 OK", "text/event-stream", TOOL_TURN).await;
    let client: Arc<dyn ModelClient> = Arc::new(client_at(addr));

    let stream = client
        .stream(&user_messages(), "sys", &[])
        .await
        .expect("seam setup must succeed");
    let count = stream
        .map(|item| item.expect("stream item must be ok"))
        .count()
        .await;
    assert_eq!(count, 6);
}

#[tokio::test]
async fn api_errors_are_typed() {
    let error_body = r#"{"error":{"message":"Invalid API key","type":"authentication_error","code":"invalid_api_key","param":null}}"#;
    let (addr, _request_rx) = mock_server("401 Unauthorized", "application/json", error_body).await;
    let client = client_at(addr);

    let result = client.stream(&user_messages(), "sys", &[]).await;
    let err = match result {
        Err(err) => err,
        Ok(_) => panic!("expected an Api error"),
    };
    match err {
        DeepSeekError::Api { status, message } => {
            assert_eq!(status, 401);
            assert_eq!(message, "Invalid API key");
        }
        other => panic!("expected Api error, got: {other:?}"),
    }
}

#[tokio::test]
async fn malformed_stream_payloads_are_protocol_errors() {
    let (addr, _request_rx) =
        mock_server("200 OK", "text/event-stream", "data: {not json}\n\n").await;
    let client = client_at(addr);

    let stream = client.stream(&user_messages(), "sys", &[]).await.unwrap();
    let items: Vec<Result<ModelEvent, DeepSeekError>> = stream.collect().await;
    assert_eq!(items.len(), 1);
    assert!(matches!(&items[0], Err(DeepSeekError::Protocol(_))));
}

#[tokio::test]
async fn truncated_stream_still_emits_finished() {
    // No [DONE], no finish_reason: the client must still signal the end.
    let body = "data: {\"id\":\"x\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n";
    let (addr, _request_rx) = mock_server("200 OK", "text/event-stream", body).await;
    let client = client_at(addr);

    let stream = client.stream(&user_messages(), "sys", &[]).await.unwrap();
    let events: Vec<ModelEvent> = stream
        .map(|item| item.expect("stream item must be ok"))
        .collect()
        .await;
    assert_eq!(
        events,
        vec![ModelEvent::TextDelta("hi".into()), ModelEvent::Finished]
    );
}
