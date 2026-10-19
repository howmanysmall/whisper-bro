#![allow(clippy::unwrap_used)]

use std::collections::HashMap;

use super::*;
use base64::Engine;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};

const REPLY: &str = r#"{"text":"Keep the final word: café."}"#;

/// What the fake OpenRouter saw: the content type, the JSON body (null for
/// multipart), the multipart parts by name, and the uploaded samples.
struct Received {
    content_type: String,
    body: serde_json::Value,
    parts: HashMap<String, Vec<u8>>,
    samples: Vec<i16>,
}

impl Received {
    fn part(&self, name: &str) -> &str {
        std::str::from_utf8(&self.parts[name]).unwrap()
    }
}

fn header<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn multipart(body: &[u8], boundary: &str) -> HashMap<String, Vec<u8>> {
    let delimiter = format!("--{boundary}");
    let mut parts = HashMap::new();
    let mut rest = &body[find(body, delimiter.as_bytes()).unwrap() + delimiter.len()..];
    while !rest.starts_with(b"--") {
        rest = rest.strip_prefix(b"\r\n").unwrap();
        let headers_end = find(rest, b"\r\n\r\n").unwrap();
        let headers = std::str::from_utf8(&rest[..headers_end]).unwrap();
        let disposition = header(headers, "content-disposition").unwrap();
        let name = disposition
            .split(';')
            .find_map(|field| field.trim().strip_prefix("name="))
            .unwrap()
            .trim_matches('"')
            .to_owned();
        let content = &rest[headers_end + 4..];
        let end = find(content, format!("\r\n{delimiter}").as_bytes()).unwrap();
        parts.insert(name, content[..end].to_vec());
        rest = &content[end + 2 + delimiter.len()..];
    }
    parts
}

fn samples(wav: &[u8]) -> Vec<i16> {
    let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).unwrap();
    assert_eq!(reader.spec().sample_rate, 16000);
    assert_eq!(reader.spec().channels, 1);
    reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
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
        let (content_type, body) = loop {
            let mut bytes = [0; 4096];
            let n = socket.read(&mut bytes).await.unwrap();
            assert_ne!(n, 0);
            request.extend_from_slice(&bytes[..n]);
            if let Some(end) = find(&request, b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]).into_owned();
                let length: usize = header(&headers, "content-length").unwrap().parse().unwrap();
                if request.len() >= end + 4 + length {
                    break (
                        header(&headers, "content-type").unwrap().to_owned(),
                        request[end + 4..end + 4 + length].to_vec(),
                    );
                }
            }
        };
        let received = match content_type.split_once("; boundary=") {
            Some(("multipart/form-data", boundary)) => {
                let parts = multipart(&body, boundary);
                Received {
                    samples: samples(&parts["file"]),
                    content_type,
                    body: serde_json::Value::Null,
                    parts,
                }
            }
            _ => {
                let body = serde_json::from_slice::<serde_json::Value>(&body).unwrap();
                let wav = base64::engine::general_purpose::STANDARD
                    .decode(body["input_audio"]["data"].as_str().unwrap())
                    .unwrap();
                Received {
                    samples: samples(&wav),
                    content_type,
                    body,
                    parts: HashMap::new(),
                }
            }
        };
        tokio::time::sleep(delay).await;
        let response = format!(
            "HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
        received
    });
    (url, task)
}

async fn recording(frames: Vec<Vec<u8>>, max_seconds: u64) -> Vec<u8> {
    let (tx, rx) = mpsc::channel(frames.len().max(1));
    for frame in frames {
        tx.send(frame).await.unwrap();
    }
    drop(tx);
    wav(collect(rx, 16000, max_seconds).await, 16000)
}

async fn upload(
    config: &Config,
    frames: Vec<Vec<u8>>,
    url: &str,
    multipart_limit: usize,
) -> Result<Transcript, ApiError> {
    let wav = recording(frames, config.max_recording_seconds).await;
    transcribe_at(
        &reqwest::Client::new(),
        config,
        "test",
        &wav,
        url,
        multipart_limit,
    )
    .await
}

#[tokio::test]
async fn stopping_uploads_every_sample_including_the_final_frame() {
    let (url, server) = service(200, Duration::ZERO, REPLY).await;
    let frames = vec![vec![0, 0, 255, 127], vec![0, 128, 1, 0]];
    let transcript = upload(&Config::default(), frames, &url, 25 << 20)
        .await
        .unwrap();
    assert_eq!(transcript.text, "Keep the final word: café.");
    assert_eq!(server.await.unwrap().samples, vec![0, 32767, -32768, 1]);
}

#[tokio::test]
async fn uploads_under_the_limit_use_multipart() {
    let (url, server) = service(200, Duration::ZERO, REPLY).await;
    let config = Config::default();
    upload(&config, vec![vec![0, 0, 255, 127, 0, 128]], &url, 25 << 20)
        .await
        .unwrap();
    let received = server.await.unwrap();
    assert!(received.content_type.starts_with("multipart/form-data"));
    assert_eq!(received.part("model"), config.transcription_model);
    assert_eq!(received.part("language"), "en");
    assert_eq!(received.part("temperature"), "0");
    assert!(!received.parts.contains_key("input_audio"));
    assert_eq!(received.samples, vec![0, 32767, -32768]);
}

#[tokio::test]
async fn uploads_over_the_limit_fall_back_to_base64_json() {
    let (url, server) = service(200, Duration::ZERO, REPLY).await;
    let config = Config::default();
    upload(&config, vec![vec![0, 0, 255, 127]], &url, 0)
        .await
        .unwrap();
    let received = server.await.unwrap();
    assert!(received.content_type.starts_with("application/json"));
    assert_eq!(received.body["input_audio"]["format"], "wav");
    assert_eq!(received.body["model"], config.transcription_model.as_str());
    assert_eq!(received.body["language"], "en");
    assert_eq!(received.samples, vec![0, 32767]);
}

#[tokio::test]
async fn rejected_and_stalled_uploads_never_become_successful_dictations() {
    for (status, delay) in [(402, Duration::ZERO), (200, Duration::from_secs(5))] {
        let (url, server) = service(status, delay, REPLY).await;
        let config = Config {
            transcription_timeout_ms: 50,
            ..Config::default()
        };
        assert!(
            upload(&config, vec![vec![0, 0]], &url, 25 << 20)
                .await
                .is_err()
        );
        server.abort();
    }
}

#[tokio::test]
async fn crossing_the_recording_limit_keeps_the_allowed_audio() {
    let (tx, rx) = mpsc::channel(2);
    tx.send(vec![1; 32000]).await.unwrap();
    tx.send(vec![2; 4]).await.unwrap();
    drop(tx);
    let pcm = collect(rx, 16000, 1).await;
    assert_eq!(pcm.len(), 44 + 32000);
    assert!(pcm[44..].iter().all(|&byte| byte == 1));
}

#[tokio::test]
async fn an_empty_recording_is_only_the_header_and_uploads_nothing() {
    let (tx, rx) = mpsc::channel::<Vec<u8>>(1);
    drop(tx);
    let pcm = collect(rx, 16000, 60).await;
    assert_eq!(pcm.len(), 44);
    let transcript = transcribe_at(
        &reqwest::Client::new(),
        &Config::default(),
        "test",
        &wav(pcm, 16000),
        "http://127.0.0.1:9/unreachable",
        25 << 20,
    )
    .await
    .unwrap();
    assert_eq!(transcript.text, "");
}

#[test]
fn wav_headers_describe_the_audio_to_a_standard_reader() {
    let mut pcm = vec![0; 44];
    for sample in [0i16, 1, -1, i16::MAX, i16::MIN] {
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    let wav = wav(pcm, 24000);
    let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).unwrap();
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 24000);
    assert_eq!(spec.channels, 1);
    assert_eq!(spec.bits_per_sample, 16);
    assert_eq!(reader.duration(), 5);
    let samples = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(samples, vec![0, 1, -1, i16::MAX, i16::MIN]);
}

#[tokio::test]
async fn vocabulary_language_and_model_reach_openrouter_as_sent() {
    let (url, server) = service(200, Duration::ZERO, REPLY).await;
    let config = Config {
        vocabulary: vec![
            "roblox-ts".into(),
            "foo&model=other".into(),
            "two words".into(),
        ],
        ..Config::default()
    };
    upload(&config, vec![vec![0, 0]], &url, 25 << 20)
        .await
        .unwrap();
    let received = server.await.unwrap();
    let provider: serde_json::Value = serde_json::from_str(received.part("provider")).unwrap();
    let prompt = "roblox-ts, foo&model=other, two words";
    assert_eq!(
        provider,
        serde_json::json!({"options": {"groq": {"prompt": prompt}, "deepinfra/us": {"prompt": prompt}}})
    );
    assert_eq!(received.part("model"), config.transcription_model);
    assert_eq!(received.part("language"), "en");
}

#[tokio::test]
async fn an_empty_vocabulary_sends_no_provider_options() {
    for limit in [25 << 20, 0] {
        let (url, server) = service(200, Duration::ZERO, REPLY).await;
        upload(&Config::default(), vec![vec![0, 0]], &url, limit)
            .await
            .unwrap();
        let received = server.await.unwrap();
        assert!(!received.parts.contains_key("provider"));
        assert!(received.body.get("provider").is_none());
    }
}

async fn upload_with_reply(status: u16, reply: &'static str) -> Result<Transcript, ApiError> {
    let (url, server) = service(status, Duration::ZERO, reply).await;
    let result = upload(&Config::default(), vec![vec![0, 0]], &url, 25 << 20).await;
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
        let config = Config {
            language: language.into(),
            ..Config::default()
        };
        for limit in [25 << 20, 0] {
            let (url, server) = service(200, Duration::ZERO, REPLY).await;
            upload(&config, vec![vec![0, 0]], &url, limit)
                .await
                .unwrap();
            let received = server.await.unwrap();
            assert!(!received.parts.contains_key("language"));
            assert!(received.body.get("language").is_none());
        }
    }
}
