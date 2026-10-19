//! A fake OpenRouter transcription endpoint for tests.

#![allow(clippy::unwrap_used)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use base64::Engine;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinHandle,
};

pub const REPLY: &str = r#"{"text":"Keep the final word: café."}"#;

/// What the fake OpenRouter saw: the content type, the JSON body (null for
/// multipart), the multipart parts by name, and the uploaded samples.
pub struct Received {
    pub content_type: String,
    pub body: serde_json::Value,
    pub parts: HashMap<String, Vec<u8>>,
    pub samples: Vec<i16>,
}

impl Received {
    pub fn part(&self, name: &str) -> &str {
        std::str::from_utf8(&self.parts[name]).unwrap()
    }
}

/// A local server that answers successive connections with the given
/// `(status, reply)` pairs in order, then stops accepting.
pub struct Service {
    pub url: String,
    received: Arc<Mutex<Vec<Received>>>,
    task: JoinHandle<()>,
}

impl Service {
    /// Must be called inside a Tokio runtime; the server runs on it.
    pub async fn start(responses: Vec<(u16, &'static str)>, delay: Duration) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/transcribe", listener.local_addr().unwrap());
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        let task = tokio::spawn(async move {
            for (status, reply) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let request = read_request(&mut socket).await;
                log.lock().unwrap().push(request);
                tokio::time::sleep(delay).await;
                let response = format!(
                    "HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        Self {
            url,
            received,
            task,
        }
    }

    /// The requests served so far, in arrival order.
    pub fn requests(&self) -> MutexGuard<'_, Vec<Received>> {
        self.received.lock().unwrap()
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Received {
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
    match content_type.split_once("; boundary=") {
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
