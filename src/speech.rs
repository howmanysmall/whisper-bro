use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::{sync::mpsc::Receiver, time::timeout};

use crate::{
    config::Config,
    openrouter::{self, ApiError},
};

/// Bytes reserved at the front of a recording for the WAV header.
const HEADER: usize = 44;

pub struct Transcript {
    pub text: String,
    pub finalize_ms: u128,
}

/// Gathers PCM frames behind a blank 44-byte prefix, keeping at most
/// `max_seconds` of 16-bit mono audio. A recording with no audio is exactly
/// the prefix.
pub async fn collect(mut audio: Receiver<Vec<u8>>, sample_rate: u32, max_seconds: u64) -> Vec<u8> {
    let limit = sample_rate as u64 * max_seconds * 2;
    let mut buffer = vec![0; HEADER];
    while let Some(frame) = audio.recv().await {
        let remaining = limit.saturating_sub((buffer.len() - HEADER) as u64) as usize;
        buffer.extend_from_slice(&frame[..frame.len().min(remaining)]);
    }
    buffer
}

/// Fills the prefix reserved by [`collect`] with a 16-bit mono PCM WAV header.
pub fn wav(mut pcm_with_prefix: Vec<u8>, sample_rate: u32) -> Vec<u8> {
    if pcm_with_prefix.len() < HEADER {
        pcm_with_prefix.resize(HEADER, 0);
    }
    let data = u32::try_from(pcm_with_prefix.len() - HEADER).unwrap_or(u32::MAX - 36);
    let fields: [&[u8]; 13] = [
        b"RIFF",
        &(data + 36).to_le_bytes(),
        b"WAVE",
        b"fmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &1u16.to_le_bytes(),
        &sample_rate.to_le_bytes(),
        &(sample_rate * 2).to_le_bytes(),
        &2u16.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
    ];
    let mut at = 0;
    for field in fields {
        pcm_with_prefix[at..at + field.len()].copy_from_slice(field);
        at += field.len();
    }
    pcm_with_prefix
}

/// OpenRouter's transcription endpoint.
pub const ENDPOINT: &str = "https://openrouter.ai/api/v1/audio/transcriptions";

/// How long to wait before the one retry of a transient failure.
const RETRY_DELAY: Duration = Duration::from_millis(if cfg!(test) { 10 } else { 1000 });

/// Uploads the WAV to `endpoint` and returns the transcript, trying once more
/// after a short pause when the first attempt fails transiently.
pub async fn transcribe(
    client: &reqwest::Client,
    config: &Config,
    key: &str,
    wav: &[u8],
    endpoint: &str,
) -> Result<Transcript, ApiError> {
    transcribe_at(client, config, key, wav, endpoint, 25 << 20).await
}

async fn transcribe_at(
    client: &reqwest::Client,
    config: &Config,
    key: &str,
    wav: &[u8],
    endpoint: &str,
    multipart_limit: usize,
) -> Result<Transcript, ApiError> {
    if wav.len() <= HEADER {
        return Ok(Transcript {
            text: String::new(),
            finalize_ms: 0,
        });
    }
    match attempt(client, config, key, wav, endpoint, multipart_limit).await {
        Err(error) if error.retryable() => {
            tracing::warn!(%error, "Transcription failed; retrying once");
            tokio::time::sleep(RETRY_DELAY).await;
            attempt(client, config, key, wav, endpoint, multipart_limit).await
        }
        result => result,
    }
}

async fn attempt(
    client: &reqwest::Client,
    config: &Config,
    key: &str,
    wav: &[u8],
    endpoint: &str,
    multipart_limit: usize,
) -> Result<Transcript, ApiError> {
    let started = Instant::now();
    let result = async {
        #[derive(Deserialize)]
        struct Response {
            text: String,
        }
        let response = request(client, endpoint, key, config, wav, multipart_limit)?
            .send()
            .await?;
        let response = openrouter::check(response)
            .await?
            .json::<Response>()
            .await?;
        Ok::<_, ApiError>(response.text)
    };
    let text = timeout(
        Duration::from_millis(config.transcription_timeout_ms),
        result,
    )
    .await
    .map_err(|_| ApiError::Timeout)??;
    Ok(Transcript {
        text: text.trim().to_owned(),
        finalize_ms: started.elapsed().as_millis(),
    })
}

/// Uploads the WAV as a multipart file when it fits under `multipart_limit`,
/// and as base64 inside a JSON body otherwise.
fn request(
    client: &reqwest::Client,
    endpoint: &str,
    key: &str,
    config: &Config,
    wav: &[u8],
    multipart_limit: usize,
) -> reqwest::Result<reqwest::RequestBuilder> {
    use base64::Engine;
    let mut body = serde_json::json!({
        "model": config.transcription_model,
        "temperature": 0,
    });
    if let Some(code) = config.language_code() {
        body["language"] = serde_json::json!(code);
    }
    if !config.vocabulary.is_empty() {
        let prompt = config.vocabulary.join(", ");
        body["provider"] = serde_json::json!({"options": {"groq": {"prompt": prompt}, "deepinfra/us": {"prompt": prompt}}});
    }
    let builder = client.post(endpoint).bearer_auth(key);
    if wav.len() > multipart_limit {
        body["input_audio"] = serde_json::json!({
            "data": base64::engine::general_purpose::STANDARD.encode(wav),
            "format": "wav",
        });
        return Ok(builder.json(&body));
    }
    let mut form = reqwest::multipart::Form::new();
    if let serde_json::Value::Object(fields) = body {
        for (name, value) in fields {
            let value = match value {
                serde_json::Value::String(text) => text,
                other => other.to_string(),
            };
            form = form.text(name, value);
        }
    }
    let file = reqwest::multipart::Part::bytes(wav.to_vec())
        .file_name("recording.wav")
        .mime_str("audio/wav")?;
    Ok(builder.multipart(form.part("file", file)))
}

#[cfg(test)]
#[path = "speech_tests.rs"]
mod tests;
