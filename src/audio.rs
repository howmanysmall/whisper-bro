use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;

pub const BUFFER_FRAMES: usize = 500;

/// Transcription models resample to 16 kHz internally, so anything faster only inflates uploads.
pub const TARGET_RATE: u32 = 16_000;

/// RMS of a 100 ms window above which the microphone is taken to hear speech: −45 dBFS.
pub const SPEECH_RMS: f32 = 0.005_623_413;

/// The rate the encoder emits for a given input rate. Slower inputs pass through; they're never upsampled.
pub fn output_rate(input: u32) -> u32 {
    if input > TARGET_RATE {
        TARGET_RATE
    } else {
        input
    }
}

pub struct Capture {
    stream: Option<cpal::Stream>,
    encoder: Arc<Mutex<Encoder>>,
    fault: Arc<Mutex<Option<String>>>,
    pub sample_rate: u32,
}

/// The outcome of briefly opening the microphone.
#[derive(Clone, Copy, Debug)]
pub struct Probe {
    pub sample_rate: u32,
    /// How long the first audio took, or `None` if none arrived within the wait.
    pub ready_after: Option<std::time::Duration>,
}

/// Opens the microphone for up to `wait` and reports whether audio arrives.
pub fn probe(name: Option<&str>, wait: std::time::Duration) -> Result<Probe> {
    let (capture, _audio) = Capture::start(name)?;
    let started = std::time::Instant::now();
    let mut ready_after = None;
    let mut fault = None;
    while started.elapsed() < wait {
        if capture.is_ready() {
            ready_after = Some(started.elapsed());
            break;
        }
        fault = capture.fault();
        if fault.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let sample_rate = capture.sample_rate;
    // The probe discards its audio, so a flush error here is irrelevant.
    let _ = capture.stop();
    if let Some(fault) = fault {
        bail!(fault);
    }
    Ok(Probe {
        sample_rate,
        ready_after,
    })
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
                    format!("Microphone '{name}' is unavailable. Run `yap devices`")
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
        let (tx, rx) = mpsc::channel(BUFFER_FRAMES);
        let encoder = Encoder::new(config.channels as usize, config.sample_rate.0, tx);
        let sample_rate = encoder.output_rate();
        let encoder = Arc::new(Mutex::new(encoder));
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

    /// Whether any 100 ms window so far was louder than −45 dBFS.
    pub fn heard_speech(&self) -> bool {
        self.encoder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .heard_speech()
    }

    /// How much audio has arrived since the last loud window, or since the start.
    pub fn silent_for(&self) -> std::time::Duration {
        self.encoder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .silent_for()
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
    output_rate: u32,
    resampler: Option<Resampler>,
    frame_bytes: usize,
    pending: Vec<u8>,
    channel_sum: f32,
    channel_count: usize,
    sender: mpsc::Sender<Vec<u8>>,
    pub overflowed: bool,
    last_data: std::time::Instant,
    has_data: bool,
    input_rate: u32,
    window_frames: usize,
    window_energy: f32,
    window_count: usize,
    heard_speech: bool,
    /// Mono input frames since the last loud window ended, or since the start.
    quiet_frames: u64,
}

impl Encoder {
    pub fn new(channels: usize, sample_rate: u32, sender: mpsc::Sender<Vec<u8>>) -> Self {
        let output_rate = output_rate(sample_rate);
        let frame_bytes = (output_rate as usize / 50).max(1) * 2;
        Self {
            channels: channels.max(1),
            output_rate,
            resampler: Resampler::new(sample_rate, output_rate),
            frame_bytes,
            pending: Vec::with_capacity(frame_bytes),
            channel_sum: 0.0,
            channel_count: 0,
            sender,
            overflowed: false,
            last_data: std::time::Instant::now(),
            has_data: false,
            input_rate: sample_rate.max(1),
            window_frames: (sample_rate as usize / 10).max(1),
            window_energy: 0.0,
            window_count: 0,
            heard_speech: false,
            quiet_frames: 0,
        }
    }

    pub fn output_rate(&self) -> u32 {
        self.output_rate
    }

    /// Whether any 100 ms window so far was louder than −45 dBFS.
    pub fn heard_speech(&self) -> bool {
        self.heard_speech
    }

    /// How much audio has been pushed since the last loud window, or since the start.
    pub fn silent_for(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(self.quiet_frames as f64 / f64::from(self.input_rate))
    }

    fn measure(&mut self, mono: f32) {
        self.quiet_frames += 1;
        self.window_energy += mono * mono;
        self.window_count += 1;
        if self.window_count == self.window_frames {
            if (self.window_energy / self.window_count as f32).sqrt() > SPEECH_RMS {
                self.heard_speech = true;
                self.quiet_frames = 0;
            }
            self.window_energy = 0.0;
            self.window_count = 0;
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
                self.channel_count = 0;
                self.channel_sum = 0.0;
                self.measure(mono);
                let pending = &mut self.pending;
                let mut quantize = |value: f32| {
                    let pcm = (value * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
                    pending.extend_from_slice(&pcm.to_le_bytes());
                };
                match &mut self.resampler {
                    Some(resampler) => resampler.push(mono, &mut quantize),
                    None => quantize(mono),
                }
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

/// Streaming rational downsampler: a Blackman-windowed sinc low-pass split into `up` polyphase
/// branches, evaluated only at the output instants.
struct Resampler {
    up: usize,
    down: usize,
    taps_per_phase: usize,
    /// `up` phases of `taps_per_phase` taps, each stored oldest-input-first.
    taps: Vec<f32>,
    /// The last `taps_per_phase` inputs, written twice so they're always one contiguous slice.
    history: Vec<f32>,
    write: usize,
    /// Distance, in upsampled samples, from the newest input to the next output.
    offset: usize,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Option<Self> {
        if from <= to || to == 0 {
            return None;
        }
        let divisor = gcd(from, to);
        let up = (to / divisor) as usize;
        let down = (from / divisor) as usize;
        // A Blackman window needs about 5.5 / N of transition band. Ending the band at the output
        // Nyquist with a cutoff of 0.45 * `to` leaves 0.1 * `to` for it, at the input rate.
        let taps_per_phase = (55.0 * from as f64 / to as f64).ceil() as usize;
        let length = taps_per_phase * up;
        let cutoff = 0.45 * to as f64 / (from as f64 * up as f64);
        let center = (length - 1) as f64 / 2.0;
        let prototype: Vec<f64> = (0..length)
            .map(|i| {
                let x = i as f64 - center;
                let sinc = if x == 0.0 {
                    2.0 * cutoff
                } else {
                    (2.0 * std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x)
                };
                let angle = 2.0 * std::f64::consts::PI * i as f64 / (length - 1) as f64;
                sinc * (0.42 - 0.5 * angle.cos() + 0.08 * (2.0 * angle).cos())
            })
            .collect();
        // Each phase sees one input in `up`, so the whole prototype sums to `up` for unity gain.
        let gain = up as f64 / prototype.iter().sum::<f64>();
        let mut taps = vec![0.0; length];
        for phase in 0..up {
            for k in 0..taps_per_phase {
                taps[phase * taps_per_phase + taps_per_phase - 1 - k] =
                    (prototype[k * up + phase] * gain) as f32;
            }
        }
        Some(Self {
            up,
            down,
            taps_per_phase,
            taps,
            history: vec![0.0; taps_per_phase * 2],
            write: 0,
            offset: 0,
        })
    }

    fn push(&mut self, sample: f32, out: &mut impl FnMut(f32)) {
        self.history[self.write] = sample;
        self.history[self.write + self.taps_per_phase] = sample;
        self.write = (self.write + 1) % self.taps_per_phase;
        let window = &self.history[self.write..self.write + self.taps_per_phase];
        while self.offset < self.up {
            let start = self.offset * self.taps_per_phase;
            let taps = &self.taps[start..start + self.taps_per_phase];
            out(taps.iter().zip(window).map(|(t, x)| t * x).sum());
            self.offset += self.down;
        }
        self.offset -= self.up;
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}
