#![allow(clippy::unwrap_used)]

use super::*;
use crate::test_support::{REPLY, Service};
use tokio::sync::mpsc;

async fn service(status: u16, delay: Duration) -> Service {
    Service::start(vec![(status, REPLY)], delay).await
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
    let server = service(200, Duration::ZERO).await;
    let frames = vec![vec![0, 0, 255, 127], vec![0, 128, 1, 0]];
    let transcript = upload(&Config::default(), frames, &server.url, 25 << 20)
        .await
        .unwrap();
    assert_eq!(transcript.text, "Keep the final word: café.");
    assert_eq!(server.requests()[0].samples, vec![0, 32767, -32768, 1]);
}

#[tokio::test]
async fn uploads_under_the_limit_use_multipart() {
    let server = service(200, Duration::ZERO).await;
    let config = Config::default();
    upload(
        &config,
        vec![vec![0, 0, 255, 127, 0, 128]],
        &server.url,
        25 << 20,
    )
    .await
    .unwrap();
    let received = &server.requests()[0];
    assert!(received.content_type.starts_with("multipart/form-data"));
    assert_eq!(received.part("model"), config.transcription_model);
    assert_eq!(received.part("language"), "en");
    assert_eq!(received.part("temperature"), "0");
    assert!(!received.parts.contains_key("input_audio"));
    assert_eq!(received.samples, vec![0, 32767, -32768]);
}

#[tokio::test]
async fn uploads_over_the_limit_fall_back_to_base64_json() {
    let server = service(200, Duration::ZERO).await;
    let config = Config::default();
    upload(&config, vec![vec![0, 0, 255, 127]], &server.url, 0)
        .await
        .unwrap();
    let received = &server.requests()[0];
    assert!(received.content_type.starts_with("application/json"));
    assert_eq!(received.body["input_audio"]["format"], "wav");
    assert_eq!(received.body["model"], config.transcription_model.as_str());
    assert_eq!(received.body["language"], "en");
    assert_eq!(received.samples, vec![0, 32767]);
}

#[tokio::test]
async fn rejected_and_stalled_uploads_never_become_successful_dictations() {
    for (status, delay) in [(402, Duration::ZERO), (200, Duration::from_secs(5))] {
        let server = service(status, delay).await;
        let config = Config {
            transcription_timeout_ms: 50,
            ..Config::default()
        };
        assert!(
            upload(&config, vec![vec![0, 0]], &server.url, 25 << 20)
                .await
                .is_err()
        );
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
    let server = service(200, Duration::ZERO).await;
    let config = Config {
        vocabulary: vec![
            "roblox-ts".into(),
            "foo&model=other".into(),
            "two words".into(),
        ],
        ..Config::default()
    };
    upload(&config, vec![vec![0, 0]], &server.url, 25 << 20)
        .await
        .unwrap();
    let received = &server.requests()[0];
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
        let server = service(200, Duration::ZERO).await;
        upload(&Config::default(), vec![vec![0, 0]], &server.url, limit)
            .await
            .unwrap();
        let received = &server.requests()[0];
        assert!(!received.parts.contains_key("provider"));
        assert!(received.body.get("provider").is_none());
    }
}

async fn upload_with_reply(status: u16, reply: &'static str) -> Result<Transcript, ApiError> {
    let server = Service::start(vec![(status, reply)], Duration::ZERO).await;
    upload(&Config::default(), vec![vec![0, 0]], &server.url, 25 << 20).await
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
            let server = service(200, Duration::ZERO).await;
            upload(&config, vec![vec![0, 0]], &server.url, limit)
                .await
                .unwrap();
            let received = &server.requests()[0];
            assert!(!received.parts.contains_key("language"));
            assert!(received.body.get("language").is_none());
        }
    }
}

async fn upload_against(
    responses: Vec<(u16, &'static str)>,
) -> (Result<Transcript, ApiError>, usize) {
    let server = Service::start(responses, Duration::ZERO).await;
    let result = upload(&Config::default(), vec![vec![0, 0]], &server.url, 25 << 20).await;
    let served = server.requests().len();
    (result, served)
}

#[tokio::test]
async fn a_transient_failure_is_retried_once() {
    let (result, served) = upload_against(vec![(503, "busy"), (200, REPLY)]).await;
    assert_eq!(result.unwrap().text, "Keep the final word: café.");
    assert_eq!(served, 2);
}

#[tokio::test]
async fn a_rejection_is_not_retried() {
    let (result, served) = upload_against(vec![(402, "{}"), (200, REPLY)]).await;
    assert!(matches!(result, Err(ApiError::NoCredits)));
    assert_eq!(served, 1);
}

#[tokio::test]
async fn a_rate_limit_is_not_retried() {
    let (result, served) = upload_against(vec![(429, "{}"), (200, REPLY)]).await;
    assert!(matches!(result, Err(ApiError::RateLimited)));
    assert_eq!(served, 1);
}

#[tokio::test]
async fn a_second_failure_is_final() {
    let (result, served) = upload_against(vec![(503, "busy"), (503, "busy"), (200, REPLY)]).await;
    assert!(matches!(result, Err(ApiError::Server { status: 503, .. })));
    assert_eq!(served, 2);
}
