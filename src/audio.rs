use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;

pub const BUFFER_FRAMES: usize = 500;

pub struct Capture {
    stream: Option<cpal::Stream>,
    encoder: Arc<Mutex<Encoder>>,
    fault: Arc<Mutex<Option<String>>>,
    pub sample_rate: u32,
}

pub fn devices() -> Result<Vec<String>> {
    cpal::default_host()
        .input_devices()?
        .map(|device| device.name().map_err(Into::into))
        .collect()
}

impl Capture {
    pub fn start(name: Option<&str>) -> Result<(Self, mpsc::Receiver<Vec<u8>>)> {
        let host = cpal::default_host();
        let device = match name {
            Some(name) => host
                .input_devices()?
                .find(|d| d.name().is_ok_and(|n| n == name))
                .with_context(|| {
                    format!("Microphone '{name}' is unavailable. Run `whisper-bro devices`")
                })?,
            None => host
                .default_input_device()
                .context("No default microphone is available")?,
        };
        let supported = device
            .default_input_config()
            .context("Read microphone format; check microphone permission")?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let sample_rate = config.sample_rate.0;
        let (tx, rx) = mpsc::channel(BUFFER_FRAMES);
        let encoder = Arc::new(Mutex::new(Encoder::new(
            config.channels as usize,
            sample_rate,
            tx,
        )));
        let fault = Arc::new(Mutex::new(None));
        let stream = match format {
            cpal::SampleFormat::F32 => {
                build::<f32>(&device, &config, encoder.clone(), fault.clone())
            }
            cpal::SampleFormat::F64 => {
                build::<f64>(&device, &config, encoder.clone(), fault.clone())
            }
            cpal::SampleFormat::I16 => {
                build::<i16>(&device, &config, encoder.clone(), fault.clone())
            }
            cpal::SampleFormat::I32 => {
                build::<i32>(&device, &config, encoder.clone(), fault.clone())
            }
            cpal::SampleFormat::U16 => {
                build::<u16>(&device, &config, encoder.clone(), fault.clone())
            }
            _ => bail!("Unsupported microphone sample format: {format}"),
        }?;
        stream
            .play()
            .context("Start microphone; check microphone permission")?;
        Ok((
            Self {
                stream: Some(stream),
                encoder,
                fault,
                sample_rate,
            },
            rx,
        ))
    }

    pub fn fault(&self) -> Option<String> {
        if let Some(error) = self.fault.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Some(error);
        }
        let encoder = self.encoder.lock().unwrap_or_else(|e| e.into_inner());
        if encoder.last_data.elapsed() > std::time::Duration::from_secs(5) {
            return Some(
                "The microphone stopped delivering audio. Check the device and try again".into(),
            );
        }
        encoder.overflowed.then(|| "Audio could not be sent fast enough. Recording stopped to avoid silently dropping words".into())
    }

    pub fn is_ready(&self) -> bool {
        self.encoder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .has_data
    }

    pub fn stop(mut self) -> Result<()> {
        // Drop the stream before flushing so no callback can append after the tail.
        self.stream.take();
        let mut encoder = self.encoder.lock().unwrap_or_else(|e| e.into_inner());
        encoder.flush();
        if encoder.overflowed {
            bail!("Audio buffer overflowed; the recording is incomplete");
        }
        Ok(())
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    encoder: Arc<Mutex<Encoder>>,
    fault: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            encoder.lock().unwrap_or_else(|e| e.into_inner()).push(data);
        },
        move |error| {
            *fault.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(format!("Microphone disconnected or failed: {error}"));
        },
        None,
    )?)
}

pub struct Encoder {
    channels: usize,
    frame_bytes: usize,
    pending: Vec<u8>,
    channel_sum: f32,
    channel_count: usize,
    sender: mpsc::Sender<Vec<u8>>,
    pub overflowed: bool,
    last_data: std::time::Instant,
    has_data: bool,
}

impl Encoder {
    pub fn new(channels: usize, sample_rate: u32, sender: mpsc::Sender<Vec<u8>>) -> Self {
        let frame_bytes = (sample_rate as usize / 50).max(1) * 2;
        Self {
            channels: channels.max(1),
            frame_bytes,
            pending: Vec::with_capacity(frame_bytes),
            channel_sum: 0.0,
            channel_count: 0,
            sender,
            overflowed: false,
            last_data: std::time::Instant::now(),
            has_data: false,
        }
    }

    pub fn push<T: cpal::Sample>(&mut self, samples: &[T])
    where
        f32: cpal::FromSample<T>,
    {
        if !samples.is_empty() {
            self.last_data = std::time::Instant::now();
            self.has_data = true;
        }
        for &sample in samples {
            let value = sample.to_sample::<f32>();
            self.channel_sum += if value.is_finite() { value } else { 0.0 };
            self.channel_count += 1;
            if self.channel_count == self.channels {
                let mono = (self.channel_sum / self.channels as f32).clamp(-1.0, 1.0);
                let pcm = (mono * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
                self.pending.extend_from_slice(&pcm.to_le_bytes());
                self.channel_count = 0;
                self.channel_sum = 0.0;
                if self.pending.len() >= self.frame_bytes {
                    self.flush();
                }
            }
        }
    }

    pub fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let frame = std::mem::replace(&mut self.pending, Vec::with_capacity(self.frame_bytes));
        if self.sender.try_send(frame).is_err() {
            self.overflowed = true;
        }
    }
}
