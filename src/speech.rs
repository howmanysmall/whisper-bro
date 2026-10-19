use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::{sync::mpsc, time::timeout};

use crate::{
    config::Config,
    openrouter::{self, ApiError},
};

pub struct Transcript {
    pub text: String,
    pub finalize_ms: u128,
}

pub async fn transcribe(
    client: &reqwest::Client,
    config: &Config,
    key: &str,
    sample_rate: u32,
    audio: mpsc::Receiver<Vec<u8>>,
) -> Result<Transcript, ApiError> {
    transcribe_at(
        client,
        config,
        key,
        sample_rate,
        audio,
        "https://openrouter.ai/api/v1/audio/transcriptions",
    )
    .await
}

async fn transcribe_at(
    client: &reqwest::Client,
    config: &Config,
    key: &str,
    sample_rate: u32,
    mut audio: mpsc::Receiver<Vec<u8>>,
    endpoint: &str,
) -> Result<Transcript, ApiError> {
    use base64::Engine;
    let invalid = |error: hound::Error| ApiError::Invalid(error.to_string());
    let mut pcm = Vec::new();
    let limit = sample_rate as u64 * config.max_recording_seconds * 2;
    while let Some(frame) = audio.recv().await {
        let remaining = limit.saturating_sub(pcm.len() as u64) as usize;
        pcm.extend_from_slice(&frame[..frame.len().min(remaining)]);
    }
    if pcm.is_empty() {
        return Ok(Transcript {
            text: String::new(),
            finalize_ms: 0,
        });
    }
    let stopped = Instant::now();
    let result = async {
        let mut wav = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(
                &mut wav,
                hound::WavSpec {
                    channels: 1,
                    sample_rate,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .map_err(invalid)?;
            for sample in pcm.as_chunks::<2>().0 {
                writer
                    .write_sample(i16::from_le_bytes(*sample))
                    .map_err(invalid)?;
            }
            writer.finalize().map_err(invalid)?;
        }
        drop(pcm);
        let mut body = serde_json::json!({
            "model": config.transcription_model,
            "input_audio": { "data": base64::engine::general_purpose::STANDARD.encode(wav.into_inner()), "format": "wav" },
            "language": config.language,
            "temperature": 0,
        });
        if !config.vocabulary.is_empty() {
            let prompt = config.vocabulary.join(", ");
            body["provider"] = serde_json::json!({"options": {"groq": {"prompt": prompt}, "deepinfra/us": {"prompt": prompt}}});
        }
        #[derive(Deserialize)]
        struct Response {
            text: String,
        }
        let response = client
            .post(endpoint)
            .bearer_auth(key)
            .json(&body)
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
        finalize_ms: stopped.elapsed().as_millis(),
    })
}

#[cfg(test)]
#[path = "speech_tests.rs"]
mod tests;
