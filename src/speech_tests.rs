#![allow(clippy::unwrap_used)]

use super::*;
use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn service(status: u16, delay: Duration) -> (String, tokio::task::JoinHandle<Vec<i16>>) {
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
        let body = r#"{"text":"Keep the final word: café."}"#;
        let response = format!(
            "HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
        samples
    });
    (url, task)
}

#[tokio::test]
async fn stopping_uploads_every_sample_including_the_final_frame() {
    let (url, server) = service(200, Duration::ZERO).await;
    let (tx, rx) = mpsc::channel(2);
    tx.send(vec![0, 0, 255, 127]).await.unwrap();
    tx.send(vec![0, 128, 1, 0]).await.unwrap();
    drop(tx);
    let transcript = transcribe_at(&Config::default(), "test", 16000, rx, &url)
        .await
        .unwrap();
    assert_eq!(transcript.text, "Keep the final word: café.");
    assert_eq!(server.await.unwrap(), vec![0, 32767, -32768, 1]);
}

#[tokio::test]
async fn rejected_and_stalled_uploads_never_become_successful_dictations() {
    for (status, delay) in [(402, Duration::ZERO), (200, Duration::from_secs(5))] {
        let (url, server) = service(status, delay).await;
        let (tx, rx) = mpsc::channel(1);
        tx.send(vec![0, 0]).await.unwrap();
        drop(tx);
        let config = Config {
            transcription_timeout_ms: 50,
            ..Config::default()
        };
        assert!(
            transcribe_at(&config, "test", 16000, rx, &url)
                .await
                .is_err()
        );
        server.abort();
    }
}

#[tokio::test]
async fn crossing_the_recording_limit_keeps_the_allowed_audio() {
    let (url, server) = service(200, Duration::ZERO).await;
    let (tx, rx) = mpsc::channel(1);
    tx.send(vec![0; 32004]).await.unwrap();
    drop(tx);
    let config = Config {
        max_recording_seconds: 1,
        ..Config::default()
    };
    let transcript = transcribe_at(&config, "test", 16000, rx, &url)
        .await
        .unwrap();
    assert_eq!(transcript.text, "Keep the final word: café.");
    assert_eq!(server.await.unwrap().len(), 16000);
}
