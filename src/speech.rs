use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
    time::{MissedTickBehavior, timeout},
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};
use url::Url;

use crate::config::Config;

#[derive(thiserror::Error, Debug)]
#[error("{message}")]
pub struct SpeechFailure {
    pub message: String,
    pub partial: String,
}

pub struct Transcript {
    pub text: String,
    pub connect_ms: u128,
    pub finalize_ms: u128,
}

pub fn request_url(config: &Config, sample_rate: u32) -> Result<Url, url::ParseError> {
    let mut url = Url::parse("wss://api.deepgram.com/v1/listen")?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("model", "nova-3")
            .append_pair("language", &config.language)
            .append_pair("encoding", "linear16")
            .append_pair("sample_rate", &sample_rate.to_string())
            .append_pair("channels", "1")
            .append_pair("interim_results", "true")
            .append_pair("smart_format", "true")
            .append_pair("filler_words", "false")
            .append_pair("endpointing", "false")
            .append_pair("mip_opt_out", "true");
        for term in &config.vocabulary {
            query.append_pair("keyterm", term);
        }
    }
    Ok(url)
}

pub async fn transcribe(
    config: &Config,
    key: &str,
    sample_rate: u32,
    audio: mpsc::Receiver<Vec<u8>>,
) -> Result<Transcript, SpeechFailure> {
    let started = Instant::now();
    let connect = async {
        let mut request = request_url(config, sample_rate)?
            .as_str()
            .into_client_request()?;
        let mut authorization = format!("Token {key}").parse::<reqwest::header::HeaderValue>()?;
        authorization.set_sensitive(true);
        request.headers_mut().insert("Authorization", authorization);
        let (socket, _) = tokio_tungstenite::connect_async(request).await?;
        Ok::<_, anyhow::Error>(socket)
    };
    let socket = match timeout(Duration::from_secs(5), connect).await {
        Ok(Ok(socket)) => socket,
        Ok(Err(error)) => {
            return Err(SpeechFailure {
                message: format!(
                    "Could not connect to Deepgram: {error}. Check the API key, balance, and connection"
                ),
                partial: String::new(),
            });
        }
        Err(_) => {
            return Err(SpeechFailure {
                message: "Deepgram connection timed out after 5 seconds".into(),
                partial: String::new(),
            });
        }
    };
    let connect_ms = started.elapsed().as_millis();
    let mut transcript = stream(
        socket,
        audio,
        Duration::from_millis(config.finalize_timeout_ms),
    )
    .await?;
    transcript.connect_ms = connect_ms;
    Ok(transcript)
}

pub async fn stream<S>(
    mut socket: WebSocketStream<S>,
    mut audio: mpsc::Receiver<Vec<u8>>,
    finalize_timeout: Duration,
) -> Result<Transcript, SpeechFailure>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut transcript = TranscriptBuffer::default();
    let mut closing = None;
    let mut keepalive = tokio::time::interval(Duration::from_secs(3));
    keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        let deadline = closing
            .map(|time: tokio::time::Instant| time + finalize_timeout)
            .unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(86_400));
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline), if closing.is_some() => {
                return Err(transcript.failure("Timed out waiting for the final words. The saved result is incomplete"));
            }
            incoming = socket.next() => {
                let message = match incoming {
                    Some(Ok(message)) => message,
                    Some(Err(_)) | None => return Err(transcript.failure("Speech connection closed before transcription finished")),
                };
                match message {
                    Message::Text(text) => {
                        let event = serde_json::from_str::<Event>(&text)
                            .map_err(|_| transcript.failure("Invalid response from the speech service"))?;
                        match event {
                            Event::Results { start, duration, is_final, channel } => {
                                if let Some(alternative) = channel.alternatives.first() {
                                    transcript.add(start, duration, is_final, &alternative.transcript);
                                }
                            }
                            Event::Metadata => {
                                if let Some(stopped) = closing {
                                    return Ok(Transcript { text: transcript.text(), connect_ms: 0, finalize_ms: stopped.elapsed().as_millis() });
                                }
                                return Err(transcript.failure("Speech service ended the session while still recording"));
                            }
                            Event::Error { description } => return Err(transcript.failure(&format!("Speech service: {description}"))),
                            Event::Other => {}
                        }
                    }
                    Message::Close(_) => return Err(transcript.failure("Speech connection closed before the final transcript was acknowledged")),
                    Message::Ping(payload) => send(&mut socket, Message::Pong(payload)).await
                        .map_err(|_| transcript.failure("Speech connection stalled"))?,
                    _ => {}
                }
            }
            frame = audio.recv(), if closing.is_none() => {
                let message = match frame {
                    Some(bytes) => Message::Binary(bytes.into()),
                    None => {
                        closing = Some(tokio::time::Instant::now());
                        // CloseStream flushes cached audio and ends with Metadata; Finalize alone
                        // does not guarantee an acknowledgement when the buffer is already empty.
                        Message::Text(r#"{"type":"CloseStream"}"#.into())
                    }
                };
                send(&mut socket, message).await
                    .map_err(|_| transcript.failure("Audio upload stalled or disconnected"))?;
            }
            _ = keepalive.tick(), if closing.is_none() => {
                send(&mut socket, Message::Text(r#"{"type":"KeepAlive"}"#.into())).await
                    .map_err(|_| transcript.failure("Speech connection stalled"))?;
            }
        }
    }
}

async fn send<S>(socket: &mut WebSocketStream<S>, message: Message) -> Result<(), ()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    timeout(Duration::from_secs(2), socket.send(message))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Event {
    Results {
        start: f64,
        duration: f64,
        is_final: bool,
        channel: Channel,
    },
    Metadata,
    Error {
        #[serde(default)]
        description: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct Channel {
    alternatives: Vec<Alternative>,
}

#[derive(Deserialize)]
struct Alternative {
    transcript: String,
}

#[derive(Default)]
pub struct TranscriptBuffer {
    segments: BTreeMap<u64, (u64, String)>,
}

impl TranscriptBuffer {
    pub fn add(&mut self, start: f64, duration: f64, is_final: bool, text: &str) {
        if !is_final || !start.is_finite() || !duration.is_finite() {
            return;
        }
        let start = (start.max(0.0) * 1_000_000.0).round() as u64;
        let end = start.saturating_add((duration.max(0.0) * 1_000_000.0).round() as u64);
        self.segments.insert(start, (end, text.trim().to_owned()));
    }

    pub fn text(&self) -> String {
        self.segments
            .values()
            .map(|(_, text)| text.as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn failure(&self, message: &str) -> SpeechFailure {
        SpeechFailure {
            message: message.into(),
            partial: self.text(),
        }
    }
}
