#![allow(clippy::unwrap_used)]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};
use whisper_bro::{audio::Encoder, config::Config, hotkey::Hotkey, speech, storage::Dictation};

async fn pair() -> (
    WebSocketStream<tokio::io::DuplexStream>,
    WebSocketStream<tokio::io::DuplexStream>,
) {
    let (client, server) = tokio::io::duplex(65_536);
    (
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
        WebSocketStream::from_raw_socket(server, Role::Server, None).await,
    )
}

fn result(start: f64, final_result: bool, text: &str) -> Message {
    Message::Text(
        serde_json::json!({
            "type": "Results", "start": start, "duration": 1.0, "is_final": final_result,
            "speech_final": true, "channel": { "alternatives": [{ "transcript": text }] }
        })
        .to_string()
        .into(),
    )
}

async fn finish_request(server: &mut WebSocketStream<tokio::io::DuplexStream>) -> Vec<u8> {
    let mut audio = Vec::new();
    while let Some(Ok(message)) = server.next().await {
        match message {
            Message::Binary(bytes) => audio.extend(bytes),
            Message::Text(text) if text.contains("CloseStream") => return audio,
            _ => {}
        }
    }
    panic!("Client disconnected before finishing the recording");
}

#[tokio::test]
async fn final_words_arriving_after_stop_are_not_lost_or_duplicated() {
    let (client, mut server) = pair().await;
    let (tx, rx) = mpsc::channel(2);
    tx.send(vec![0, 0, 255, 127]).await.unwrap();
    drop(tx);
    let service = tokio::spawn(async move {
        server
            .send(result(0.0, false, "Wrong preliminary words"))
            .await
            .unwrap();
        server.send(result(0.0, true, "Use Luau.")).await.unwrap();
        server.send(result(0.0, true, "Use Luau.")).await.unwrap();
        let audio = finish_request(&mut server).await;
        server
            .send(result(1.0, true, "Keep the type annotations."))
            .await
            .unwrap();
        server
            .send(Message::Text(r#"{"type":"Metadata"}"#.into()))
            .await
            .unwrap();
        audio
    });
    let output = speech::stream(client, rx, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(output.text, "Use Luau. Keep the type annotations.");
    assert_eq!(service.await.unwrap(), [0, 0, 255, 127]);
}

#[tokio::test]
async fn stopping_after_an_already_finalized_utterance_does_not_wait_for_from_finalize() {
    // Deepgram documents that Finalize need not produce a from_finalize response.
    let (client, mut server) = pair().await;
    let (tx, rx) = mpsc::channel(1);
    drop(tx);
    let service = tokio::spawn(async move {
        server
            .send(result(0.0, true, "Already complete."))
            .await
            .unwrap();
        finish_request(&mut server).await;
        server
            .send(Message::Text(r#"{"type":"Metadata"}"#.into()))
            .await
            .unwrap();
    });
    let output = speech::stream(client, rx, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(output.text, "Already complete.");
    service.await.unwrap();
}

#[tokio::test]
async fn a_disconnect_preserves_confirmed_words_but_does_not_claim_success() {
    let (client, mut server) = pair().await;
    let (_tx, rx) = mpsc::channel(1);
    let service = tokio::spawn(async move {
        server
            .send(result(0.0, true, "Save these words."))
            .await
            .unwrap();
        server
            .send(result(1.0, false, "An unconfirmed guess"))
            .await
            .unwrap();
        server.close(None).await.unwrap();
    });
    let error = speech::stream(client, rx, Duration::from_secs(1))
        .await
        .err()
        .unwrap();
    assert_eq!(error.partial, "Save these words.");
    service.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_stalled_final_response_has_a_deadline_and_recoverable_text() {
    let (client, mut server) = pair().await;
    let (tx, rx) = mpsc::channel(1);
    drop(tx);
    let service = tokio::spawn(async move {
        finish_request(&mut server).await;
        server
            .send(result(0.0, true, "Recover this."))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(60)).await;
    });
    let started = tokio::time::Instant::now();
    let error = speech::stream(client, rx, Duration::from_millis(500))
        .await
        .err()
        .unwrap();
    assert_eq!(error.partial, "Recover this.");
    assert!(started.elapsed() < Duration::from_secs(1));
    service.abort();
}

#[tokio::test(start_paused = true)]
async fn pauses_do_not_end_a_long_dictation() {
    let (client, mut server) = pair().await;
    let (tx, rx) = mpsc::channel(2);
    let recording = tokio::spawn(async move {
        tx.send(vec![1, 0]).await.unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
        tx.send(vec![2, 0]).await.unwrap();
    });
    let service = tokio::spawn(async move {
        let audio = finish_request(&mut server).await;
        server
            .send(result(0.0, true, "Before the pause. After the pause."))
            .await
            .unwrap();
        server
            .send(Message::Text(r#"{"type":"Metadata"}"#.into()))
            .await
            .unwrap();
        audio
    });
    let transcript = speech::stream(client, rx, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(transcript.text, "Before the pause. After the pause.");
    assert_eq!(service.await.unwrap(), [1, 0, 2, 0]);
    recording.await.unwrap();
}

#[test]
fn stereo_audio_becomes_mono_pcm_without_distorting_silence_or_clipping() {
    let (tx, mut rx) = mpsc::channel(2);
    let mut encoder = Encoder::new(2, 16_000, tx);
    encoder.push(&[-1.0f32, -1.0, 1.0, 1.0, -1.0, 1.0, 0.5]);
    encoder.push(&[0.5f32]);
    encoder.flush();
    assert_eq!(rx.try_recv().unwrap(), [0, 128, 255, 127, 0, 0, 0, 64]);
    assert!(!encoder.overflowed);
}

#[test]
fn audio_backpressure_is_reported_instead_of_silently_losing_words() {
    let (tx, _rx) = mpsc::channel(1);
    let mut encoder = Encoder::new(1, 16_000, tx);
    encoder.push(&[0.0f32; 640]);
    assert!(encoder.overflowed);
}

#[test]
fn special_characters_in_vocabulary_cannot_change_speech_request_parameters() {
    let config = Config {
        vocabulary: vec![
            "roblox-ts".into(),
            "foo&model=other".into(),
            "two words".into(),
        ],
        ..Config::default()
    };
    let url = speech::request_url(&config, 48_000).unwrap();
    let terms: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key == "keyterm")
        .map(|(_, value)| value.into_owned())
        .collect();
    let models: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| key == "model")
        .map(|(_, value)| value.into_owned())
        .collect();
    assert_eq!(terms, ["roblox-ts", "foo&model=other", "two words"]);
    assert_eq!(models, ["nova-3"]);
}

#[test]
fn recovery_preserves_unicode_newlines_and_incomplete_status_across_restarts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("last.json");
    Dictation {
        text: "First message".into(),
        raw: "First message".into(),
        complete: true,
        cleanup: "disabled".into(),
        recorded_at_unix: 1,
    }
    .save(&path)
    .unwrap();
    Dictation {
        text: "Résumé\n你好 🦀".into(),
        raw: "um Résumé 你好 🦀".into(),
        complete: false,
        cleanup: "disabled".into(),
        recorded_at_unix: 2,
    }
    .save(&path)
    .unwrap();
    let recovered = Dictation::load(&path).unwrap();
    assert_eq!(recovered.text, "Résumé\n你好 🦀");
    assert!(!recovered.complete);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn invalid_shortcuts_and_misspelled_configuration_are_rejected() {
    assert!("Ctrl+Alt".parse::<Hotkey>().is_err());
    assert!("Ctrl+A+B".parse::<Hotkey>().is_err());
    assert!("A".parse::<Hotkey>().is_err());
    assert!("Ctrl+F99".parse::<Hotkey>().is_err());
    assert!(toml::from_str::<Config>("microhpone = 'Headset'").is_err());
    let config = Config {
        shortcut: "Escape".into(),
        ..Config::default()
    };
    assert!(config.validate().is_err());
}

#[test]
fn holding_the_shortcut_cannot_toggle_recording_on_and_off_due_to_key_repeat() {
    let mut key = whisper_bro::hotkey::PressLatch::default();
    assert!(key.press());
    assert!(!key.press());
    assert!(!key.press());
    key.release();
    assert!(key.press());
}
