#![allow(clippy::unwrap_used)]

use super::*;
use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const REPLY: &str = r#"{"text":"Keep the final word: café."}"#;

struct Received {
    body: serde_json::Value,
    samples: Vec<i16>,
}

async fn service(
    status: u16,
    delay: Duration,
    reply: &'static str,
) -> (String, tokio::task::JoinHandle<Received>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/transcribe", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let body = loop {
            let mut bytes = [0; 4096];
            let n = socket.read(&mut bytes).await.unwrap();
            assert_ne!(n, 0);
            request.extend_from_slice(&bytes[..n]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break serde_json::from_slice::<serde_json::Value>(
                        &request[end + 4..end + 4 + length],
                    )
                    .unwrap();
                }
            }
        };
        let wav = base64::engine::general_purpose::STANDARD
            .decode(body["input_audio"]["data"].as_str().unwrap())
            .unwrap();
        let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        assert_eq!(reader.spec().channels, 1);
        let samples = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        tokio::time::sleep(delay).await;
        let response = format!(
            "HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
        Received { body, samples }
    });
    (url, task)
}

#[tokio::test]
async fn stopping_uploads_every_sample_including_the_final_frame() {
    let (url, server) = service(200, Duration::ZERO, REPLY).await;
    let (tx, rx) = mpsc::channel(2);
    tx.send(vec![0, 0, 255, 127]).await.unwrap();
    tx.send(vec![0, 128, 1, 0]).await.unwrap();
    drop(tx);
    let transcript = transcribe_at(
        &reqwest::Client::new(),
        &Config::default(),
        "test",
        16000,
        rx,
        &url,
    )
    .await
    .unwrap();
    assert_eq!(transcript.text, "Keep the final word: café.");
    assert_eq!(server.await.unwrap().samples, vec![0, 32767, -32768, 1]);
}

#[tokio::test]
async fn rejected_and_stalled_uploads_never_become_successful_dictations() {
    for (status, delay) in [(402, Duration::ZERO), (200, Duration::from_secs(5))] {
        let (url, server) = service(status, delay, REPLY).await;
        let (tx, rx) = mpsc::channel(1);
        tx.send(vec![0, 0]).await.unwrap();
        drop(tx);
        let config = Config {
            transcription_timeout_ms: 50,
            ..Config::default()
        };
        assert!(
            transcribe_at(&reqwest::Client::new(), &config, "test", 16000, rx, &url)
                .await
                .is_err()
        );
        server.abort();
    }
}

#[tokio::test]
async fn crossing_the_recording_limit_keeps_the_allowed_audio() {
    let (url, server) = service(200, Duration::ZERO, REPLY).await;
    let (tx, rx) = mpsc::channel(1);
    tx.send(vec![0; 32004]).await.unwrap();
    drop(tx);
    let config = Config {
        max_recording_seconds: 1,
        ..Config::default()
    };
    let transcript = transcribe_at(&reqwest::Client::new(), &config, "test", 16000, rx, &url)
        .await
        .unwrap();
    assert_eq!(transcript.text, "Keep the final word: café.");
    assert_eq!(server.await.unwrap().samples.len(), 16000);
}

#[tokio::test]
async fn vocabulary_language_and_model_reach_openrouter_as_sent() {
    let (url, server) = service(200, Duration::ZERO, REPLY).await;
    let (tx, rx) = mpsc::channel(1);
    tx.send(vec![0, 0]).await.unwrap();
    drop(tx);
    let config = Config {
        vocabulary: vec![
            "roblox-ts".into(),
            "foo&model=other".into(),
            "two words".into(),
        ],
        ..Config::default()
    };
    transcribe_at(&reqwest::Client::new(), &config, "test", 16000, rx, &url)
        .await
        .unwrap();
    let body = server.await.unwrap().body;
    let prompt = "roblox-ts, foo&model=other, two words";
    assert_eq!(body["provider"]["options"]["groq"]["prompt"], prompt);
    assert_eq!(
        body["provider"]["options"]["deepinfra/us"]["prompt"],
        prompt
    );
    assert_eq!(body["model"], config.transcription_model.as_str());
    assert_eq!(body["language"], "en");
}

#[tokio::test]
async fn an_empty_vocabulary_sends_no_provider_options() {
    let (url, server) = service(200, Duration::ZERO, REPLY).await;
    let (tx, rx) = mpsc::channel(1);
    tx.send(vec![0, 0]).await.unwrap();
    drop(tx);
    transcribe_at(
        &reqwest::Client::new(),
        &Config::default(),
        "test",
        16000,
        rx,
        &url,
    )
    .await
    .unwrap();
    assert!(server.await.unwrap().body.get("provider").is_none());
}

async fn upload_with_reply(status: u16, reply: &'static str) -> Result<Transcript, ApiError> {
    let (url, server) = service(status, Duration::ZERO, reply).await;
    let (tx, rx) = mpsc::channel(1);
    tx.send(vec![0, 0]).await.unwrap();
    drop(tx);
    let result = transcribe_at(
        &reqwest::Client::new(),
        &Config::default(),
        "test",
        16000,
        rx,
        &url,
    )
    .await;
    server.abort();
    result
}

#[tokio::test]
async fn whitespace_only_transcripts_are_empty() {
    let transcript = upload_with_reply(200, r#"{"text":"  \n"}"#).await.unwrap();
    assert_eq!(transcript.text, "");
}

#[tokio::test]
async fn transcripts_lose_whisper_s_leading_space() {
    let transcript = upload_with_reply(200, r#"{"text":" Hello there. "}"#)
        .await
        .unwrap();
    assert_eq!(transcript.text, "Hello there.");
}

#[tokio::test]
async fn rejections_carry_openrouter_s_reason() {
    let credits = upload_with_reply(402, r#"{"error":{"message":"Insufficient credits"}}"#).await;
    assert!(matches!(credits, Err(ApiError::NoCredits)));
    let key = upload_with_reply(401, r#"{"error":{"message":"No auth"}}"#).await;
    assert!(matches!(key, Err(ApiError::Unauthorized)));
}

#[tokio::test]
async fn language_auto_omits_the_field() {
    for language in ["auto", "AUTO", ""] {
        let (url, server) = service(200, Duration::ZERO, REPLY).await;
        let (tx, rx) = mpsc::channel(1);
        tx.send(vec![0, 0]).await.unwrap();
        drop(tx);
        let config = Config {
            language: language.into(),
            ..Config::default()
        };
        transcribe_at(&reqwest::Client::new(), &config, "test", 16000, rx, &url)
            .await
            .unwrap();
        assert!(server.await.unwrap().body.get("language").is_none());
    }
}
