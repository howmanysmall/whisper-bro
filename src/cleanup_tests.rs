#![allow(clippy::unwrap_used)]

use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn serve(
    body: &'static str,
    delay: Duration,
) -> (String, tokio::task::JoinHandle<serde_json::Value>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/cleanup", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut bytes = [0; 4096];
        let received = loop {
            let count = socket.read(&mut bytes).await.unwrap();
            if count == 0 {
                return serde_json::Value::Null;
            }
            request.extend_from_slice(&bytes[..count]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&request[..end]);
                let length = header
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break serde_json::from_slice::<serde_json::Value>(
                        &request[end + 4..end + 4 + length],
                    )
                    .unwrap();
                }
            }
        };
        tokio::time::sleep(delay).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
        received
    });
    (endpoint, server)
}

#[tokio::test]
async fn a_cleanup_deadline_returns_the_complete_original_instead_of_truncating_it() {
    let (endpoint, server) = serve(
        r#"{"choices":[{"finish_reason":"stop","message":{"content":"Too late."}}]}"#,
        Duration::from_secs(1),
    )
    .await;
    let config = CleanupConfig {
        enabled: true,
        timeout_ms: 30,
        ..CleanupConfig::default()
    };
    let (text, status) = clean_at(
        &reqwest::Client::new(),
        &config,
        Some("test-key"),
        "Keep every word, even the last one.",
        &endpoint,
    )
    .await;
    assert_eq!(text, "Keep every word, even the last one.");
    assert_eq!(status, CleanupStatus::TimedOut);
    server.abort();
}

#[tokio::test]
async fn truncated_model_output_cannot_replace_a_complete_dictation() {
    let (endpoint, server) = serve(
        r#"{"choices":[{"finish_reason":"length","message":{"content":"Keep every"}}]}"#,
        Duration::ZERO,
    )
    .await;
    let config = CleanupConfig {
        enabled: true,
        ..CleanupConfig::default()
    };
    let (text, status) = clean_at(
        &reqwest::Client::new(),
        &config,
        Some("test-key"),
        "Keep every word.",
        &endpoint,
    )
    .await;
    assert_eq!(text, "Keep every word.");
    assert_eq!(status, CleanupStatus::Unavailable);
    server.await.unwrap();
}

#[tokio::test]
async fn a_complete_edit_replaces_the_unedited_transcript() {
    let (endpoint, server) = serve(
        r#"{"choices":[{"finish_reason":"stop","message":{"content":"Meet Monday."}}]}"#,
        Duration::ZERO,
    )
    .await;
    let config = CleanupConfig {
        enabled: true,
        ..CleanupConfig::default()
    };
    let (text, status) = clean_at(
        &reqwest::Client::new(),
        &config,
        Some("test-key"),
        "Meet Friday, actually Monday.",
        &endpoint,
    )
    .await;
    assert_eq!(text, "Meet Monday.");
    assert_eq!(status, CleanupStatus::Cleaned);
    server.await.unwrap();
}

#[tokio::test]
async fn custom_instructions_are_appended_by_default_and_replace_on_request() {
    for replace in [false, true] {
        let (endpoint, server) = serve(
            r#"{"choices":[{"finish_reason":"stop","message":{"content":"Done."}}]}"#,
            Duration::ZERO,
        )
        .await;
        let config = CleanupConfig {
            enabled: true,
            instructions: Some("Always write numbers as digits.".into()),
            replace_instructions: replace,
            ..CleanupConfig::default()
        };
        clean_at(
            &reqwest::Client::new(),
            &config,
            Some("test-key"),
            "Meet at five.",
            &endpoint,
        )
        .await;
        let body = server.await.unwrap();
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains("Always write numbers as digits."));
        assert_eq!(system.contains("You edit voice dictation"), !replace);
    }
}
