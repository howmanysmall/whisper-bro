#![allow(clippy::unwrap_used)]

use super::*;
use crate::test_support::{frames, silence, tone};

/// Feeds `pcm` frame by frame, then flushes, and returns each segment's
/// length in milliseconds in the order they closed.
fn segment(pcm: &[u8]) -> Vec<u32> {
    let mut segmenter = Segmenter::new(16_000);
    let mut lengths: Vec<u32> = frames(pcm)
        .filter_map(|frame| segmenter.push(frame))
        .map(|segment| segment.len() as u32 / 32)
        .collect();
    lengths.extend(segmenter.flush().map(|segment| segment.len() as u32 / 32));
    lengths
}

#[test]
fn a_phrase_followed_by_a_pause_closes_with_a_short_tail_of_silence() {
    let mut segmenter = Segmenter::new(16_000);
    let pcm = [tone(2000), silence(600)].concat();

    let closed: Vec<Vec<u8>> = frames(&pcm)
        .filter_map(|frame| segmenter.push(frame))
        .collect();

    assert_eq!(closed.len(), 1);
    let ms = closed[0].len() / 32;
    assert!((2250..=2350).contains(&ms), "{ms} ms");
    assert_eq!(&closed[0][..640], &pcm[..640]);
    assert_eq!(segmenter.flush(), None);
}

#[test]
fn continuous_speech_is_cut_at_twelve_seconds() {
    let mut segmenter = Segmenter::new(16_000);
    let pcm = tone(15_000);

    let mut first = None;
    for (index, frame) in frames(&pcm).enumerate() {
        if let Some(closed) = segmenter.push(frame) {
            first = Some((index + 1, closed));
            break;
        }
    }

    let (frames_in, closed) = first.unwrap();
    assert_eq!(frames_in * 20, 12_000);
    assert_eq!(closed.len() / 32, 12_000);
    assert_eq!(segment(&pcm), [12_000, 3000]);
}

#[test]
fn silence_alone_never_produces_a_segment() {
    assert!(segment(&silence(5000)).is_empty());
}

#[test]
fn two_phrases_separated_by_a_pause_become_two_segments() {
    let pcm = [tone(1500), silence(700), tone(1200), silence(100)].concat();

    let lengths = segment(&pcm);

    assert_eq!(lengths.len(), 2, "{lengths:?}");
    assert!((1750..=1850).contains(&lengths[0]), "{lengths:?}");
    // The second phrase starts with at most 300 ms of the pause before it.
    assert!((1500..=1650).contains(&lengths[1]), "{lengths:?}");
}

#[test]
fn leading_silence_is_trimmed_to_three_hundred_milliseconds() {
    let pcm = [silence(2000), tone(1500), silence(600)].concat();

    let lengths = segment(&pcm);

    assert_eq!(lengths.len(), 1, "{lengths:?}");
    assert!((2050..=2150).contains(&lengths[0]), "{lengths:?}");
}

#[test]
fn the_panel_shows_the_status_line_above_the_preview() {
    let mut panel = Panel::default();
    panel.status(Status::Recording, "Recording 0:03 · Escape cancels");
    assert_eq!(
        panel.text().as_deref(),
        Some("Recording 0:03 · Escape cancels")
    );

    panel.preview("hello there");

    assert_eq!(
        panel.text().as_deref(),
        Some("Recording 0:03 · Escape cancels\nhello there")
    );
}

#[test]
fn a_long_preview_keeps_only_its_end() {
    let mut panel = Panel::default();
    panel.status(Status::Processing, "Finishing");
    let words = (0..200)
        .map(|n| format!("w{n}"))
        .collect::<Vec<_>>()
        .join(" ");

    panel.preview(&words);

    let text = panel.text().unwrap();
    let (label, preview) = text.split_once('\n').unwrap();
    assert_eq!(label, "Finishing");
    assert!(preview.starts_with('…'), "{preview}");
    assert!(preview.ends_with("w199"), "{preview}");
    assert!(preview.chars().count() <= 301, "{preview}");
}

#[test]
fn idle_and_error_hide_the_panel_and_forget_the_preview() {
    for status in [Status::Idle, Status::Error] {
        let mut panel = Panel::default();
        panel.status(Status::Recording, "Recording 0:01 · Escape cancels");
        panel.preview("old words");

        panel.status(status, "Inserted");
        assert_eq!(panel.text(), None);

        panel.status(Status::Processing, "Opening microphone · Escape cancels");
        assert_eq!(
            panel.text().as_deref(),
            Some("Opening microphone · Escape cancels")
        );
    }
}
