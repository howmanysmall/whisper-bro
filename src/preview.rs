//! The live preview: cutting a recording into phrases while it is still
//! running, and the text the HUD or overlay shows for them.

use crate::{audio::SPEECH_RMS, platform::Status};

/// A closed segment needs at least this much audio when a pause ends it.
const MIN_SEGMENT_MS: u64 = 1000;
/// A pause this long ends a segment that already holds speech.
const PAUSE_MS: u64 = 500;
/// A segment is cut here even mid-sentence, so long monologues still preview.
const MAX_SEGMENT_MS: u64 = 12_000;
/// Silence kept before and after the speech in a segment.
const PAD_MS: u64 = 300;
/// How much preview text the HUD or overlay shows, from the end.
const MAX_PREVIEW_CHARS: usize = 300;

/// Cuts a stream of 16-bit mono PCM frames into segments at pauses.
pub struct Segmenter {
    sample_rate: u32,
    /// The open segment's PCM, starting with at most `PAD_MS` of silence.
    pcm: Vec<u8>,
    speech: bool,
    /// Bytes of silence at the end of `pcm` since the last loud frame.
    trailing_silence: usize,
}

impl Segmenter {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            pcm: Vec::new(),
            speech: false,
            trailing_silence: 0,
        }
    }

    /// Adds one frame. Returns a segment's PCM when a pause or the length
    /// cap closes it.
    pub fn push(&mut self, frame: &[u8]) -> Option<Vec<u8>> {
        let frame = &frame[..frame.len() & !1];
        if frame.is_empty() {
            return None;
        }
        let loud = rms(frame) > SPEECH_RMS;
        self.pcm.extend_from_slice(frame);
        if loud {
            self.speech = true;
            self.trailing_silence = 0;
        } else {
            self.trailing_silence += frame.len();
        }
        if !self.speech {
            // Only the last `PAD_MS` of leading silence is worth uploading.
            let excess = self.pcm.len().saturating_sub(self.bytes(PAD_MS));
            self.pcm.drain(..excess);
            self.trailing_silence = self.pcm.len();
            return None;
        }
        if self.pcm.len() >= self.bytes(MAX_SEGMENT_MS) {
            self.trailing_silence = 0;
            self.speech = false;
            return Some(std::mem::take(&mut self.pcm));
        }
        if self.pcm.len() >= self.bytes(MIN_SEGMENT_MS)
            && self.trailing_silence >= self.bytes(PAUSE_MS)
        {
            // Keep `PAD_MS` of the pause; the rest leads into the next segment.
            let cut = self.pcm.len() - self.trailing_silence + self.bytes(PAD_MS);
            let rest = self.pcm.split_off(cut);
            let segment = std::mem::replace(&mut self.pcm, rest);
            self.speech = false;
            self.trailing_silence = self.pcm.len();
            return Some(segment);
        }
        None
    }

    /// Closes the open segment at the end of a recording. A segment without
    /// speech is dropped.
    pub fn flush(&mut self) -> Option<Vec<u8>> {
        let speech = std::mem::take(&mut self.speech);
        self.trailing_silence = 0;
        let pcm = std::mem::take(&mut self.pcm);
        speech.then_some(pcm)
    }

    /// The byte length of `ms` of audio, rounded down to whole samples.
    fn bytes(&self, ms: u64) -> usize {
        (u64::from(self.sample_rate) * ms / 1000) as usize * 2
    }
}

fn rms(frame: &[u8]) -> f32 {
    let samples = frame.len() / 2;
    let energy: f32 = frame
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| {
            let value = f32::from(i16::from_le_bytes(pair)) / 32768.0;
            value * value
        })
        .sum();
    (energy / samples as f32).sqrt()
}

/// What the HUD or overlay shows: the status line and, below it, the
/// preview so far. Ending a dictation clears the preview.
#[derive(Default)]
pub struct Panel {
    label: String,
    preview: String,
    active: bool,
}

impl Panel {
    pub fn status(&mut self, status: Status, label: &str) {
        self.active = matches!(status, Status::Recording | Status::Processing);
        if !self.active {
            self.preview.clear();
        }
        label.clone_into(&mut self.label);
    }

    pub fn preview(&mut self, text: &str) {
        text.clone_into(&mut self.preview);
    }

    /// The text to show while recording or transcribing: the status line,
    /// then the last part of the preview. `None` when idle or after an error.
    pub fn text(&self) -> Option<String> {
        if !self.active {
            return None;
        }
        let preview = self.preview.trim();
        if preview.is_empty() {
            return Some(self.label.clone());
        }
        let count = preview.chars().count();
        Some(if count > MAX_PREVIEW_CHARS {
            let tail: String = preview.chars().skip(count - MAX_PREVIEW_CHARS).collect();
            format!("{}\n…{}", self.label, tail.trim_start())
        } else {
            format!("{}\n{preview}", self.label)
        })
    }
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
