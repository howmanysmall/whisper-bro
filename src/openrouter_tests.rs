#![allow(clippy::unwrap_used)]

use super::*;

const REASON: &str = r#"{"error":{"message":"Provider said no","code":1}}"#;

#[test]
fn statuses_map_to_distinct_actionable_errors() {
    assert!(matches!(classify(401, REASON), ApiError::Unauthorized));
    assert!(matches!(classify(403, REASON), ApiError::Unauthorized));
    assert!(matches!(classify(402, REASON), ApiError::NoCredits));
    assert!(matches!(classify(429, REASON), ApiError::RateLimited));
    match classify(503, REASON) {
        ApiError::Server {
            status: 503,
            message,
        } => assert_eq!(message, "Provider said no"),
        other => panic!("{other:?}"),
    }
    match classify(400, REASON) {
        ApiError::Rejected {
            status: 400,
            message,
        } => assert_eq!(message, "Provider said no"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_non_json_body_still_surfaces_its_text() {
    match classify(502, "Bad gateway from upstream") {
        ApiError::Server { message, .. } => assert_eq!(message, "Bad gateway from upstream"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn long_bodies_are_truncated() {
    match classify(400, &"x".repeat(1000)) {
        ApiError::Rejected { message, .. } => assert_eq!(message.len(), 200),
        other => panic!("{other:?}"),
    }
}

#[test]
fn only_transient_failures_are_retryable() {
    let cases = [
        (classify(401, ""), false),
        (classify(402, ""), false),
        (classify(429, ""), false),
        (classify(400, ""), false),
        (ApiError::Invalid("x".into()), false),
        (classify(503, ""), true),
        (ApiError::Connect("x".into()), true),
        (ApiError::Timeout, true),
    ];
    for (error, expected) in cases {
        assert_eq!(error.retryable(), expected, "{error:?}");
    }
}

#[test]
fn messages_tell_the_user_what_to_do() {
    assert!(ApiError::Unauthorized.to_string().contains("yap setup"));
    assert_eq!(
        ApiError::NoCredits.to_string(),
        "OpenRouter account has no credits"
    );
    assert_eq!(
        classify(400, REASON).to_string(),
        "OpenRouter rejected the request (400): Provider said no"
    );
}

async fn key_service(status: u16, reply: &'static str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/key", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let _ = socket.read(&mut request).await.unwrap();
        let response = format!(
            "HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
    });
    url
}

#[tokio::test]
async fn a_valid_key_reports_its_label_and_spend() {
    let url = key_service(
        200,
        r#"{"data":{"label":"laptop","usage":1.5,"limit":5.0,"limit_remaining":3.5,"is_free_tier":false}}"#,
    )
    .await;
    let info = key_info_at(&reqwest::Client::new(), "test", &url)
        .await
        .unwrap();
    assert_eq!(info.label, "laptop");
    assert_eq!(info.usage, 1.5);
    assert_eq!(info.limit_remaining, Some(3.5));
    assert!(!info.is_free_tier);
}

#[tokio::test]
async fn a_key_response_without_optional_fields_still_parses() {
    let url = key_service(200, r#"{"data":{"label":"bare"}}"#).await;
    let info = key_info_at(&reqwest::Client::new(), "test", &url)
        .await
        .unwrap();
    assert_eq!(info.label, "bare");
    assert_eq!(info.limit_remaining, None);
}

#[tokio::test]
async fn a_rejected_key_is_unauthorized() {
    let url = key_service(401, r#"{"error":{"message":"No auth"}}"#).await;
    let result = key_info_at(&reqwest::Client::new(), "bad", &url).await;
    assert!(matches!(result, Err(ApiError::Unauthorized)));
}
