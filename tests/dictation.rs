#![allow(clippy::unwrap_used)]

use tokio::sync::mpsc;
use yap::{audio::Encoder, config::Config, hotkey::Hotkey, storage::Dictation};

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

fn encode(rate: u32, input: &[f32]) -> Vec<f32> {
    let (tx, mut rx) = mpsc::channel(input.len() + 1);
    let mut encoder = Encoder::new(1, rate, tx);
    encoder.push(input);
    encoder.flush();
    assert!(!encoder.overflowed);
    let mut bytes = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        bytes.extend(frame);
    }
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32768.0)
        .collect()
}

fn sine(rate: u32, frequency: f32, amplitude: f32) -> Vec<f32> {
    (0..rate)
        .map(|i| amplitude * (std::f32::consts::TAU * frequency * i as f32 / rate as f32).sin())
        .collect()
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

#[test]
fn a_48k_tone_survives_resampling_to_16k() {
    let output = encode(48_000, &sine(48_000, 1_000.0, 0.5));
    assert!(
        (15_900..=16_000).contains(&output.len()),
        "{}",
        output.len()
    );
    let expected = 0.5 / 2f32.sqrt();
    let measured = rms(&output);
    assert!((measured - expected).abs() < expected * 0.05, "{measured}");
    assert!(output.iter().all(|s| s.abs() <= 0.6));
}

#[test]
fn audio_above_8k_is_removed_instead_of_aliasing() {
    let input = sine(48_000, 10_000.0, 0.5);
    let measured = rms(&encode(48_000, &input));
    assert!(measured < rms(&input) * 0.01, "{measured}");
}

#[test]
fn a_44_1k_recording_resamples_to_16k_with_the_right_length() {
    let output = encode(44_100, &sine(44_100, 440.0, 0.5));
    assert!(
        (15_900..=16_000).contains(&output.len()),
        "{}",
        output.len()
    );
}

#[test]
fn rates_at_or_below_16k_pass_through_untouched() {
    let (tx, _rx) = mpsc::channel(1);
    assert_eq!(Encoder::new(1, 8_000, tx).output_rate(), 8_000);
    let input: Vec<f32> = (0..800).map(|i| (i as f32 / 400.0) - 1.0).collect();
    let output = encode(8_000, &input);
    let direct: Vec<f32> = input
        .iter()
        .map(|s| ((s * 32768.0).round().clamp(-32768.0, 32767.0)) / 32768.0)
        .collect();
    assert_eq!(output, direct);
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
    for paste_restore_ms in [10, 10_000] {
        let config = Config {
            paste_restore_ms,
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }
    assert!(toml::from_str::<Config>("finalize_timeout_ms = 4000").is_ok());
    assert!(
        !toml::to_string(&Config::default())
            .unwrap()
            .contains("finalize_timeout_ms")
    );
}

#[test]
fn shortcuts_print_in_the_spelling_users_write_in_config() {
    for (written, shown) in [
        ("control+option+space", "Ctrl+Alt+Space"),
        ("cmd+shift+k", "Shift+Cmd+K"),
        ("F5", "F5"),
        ("Ctrl+7", "Ctrl+7"),
    ] {
        let hotkey: Hotkey = written.parse().unwrap();
        assert_eq!(hotkey.to_string(), shown);
        assert_eq!(shown.parse::<Hotkey>().unwrap(), hotkey);
    }
}

#[test]
fn holding_the_shortcut_cannot_toggle_recording_on_and_off_due_to_key_repeat() {
    let mut key = yap::hotkey::PressLatch::default();
    assert!(key.press());
    assert!(!key.press());
    assert!(!key.press());
    key.release();
    assert!(key.press());
}

#[test]
fn cleanup_and_language_settings_respect_their_ranges() {
    use yap::config::CleanupConfig;
    for (timeout_ms, valid) in [(50, false), (70_000, false), (60_000, true)] {
        let config = Config {
            cleanup: CleanupConfig {
                timeout_ms,
                ..CleanupConfig::default()
            },
            ..Config::default()
        };
        assert_eq!(config.validate().is_ok(), valid, "{timeout_ms}");
    }
    let blank = Config {
        cleanup: CleanupConfig {
            instructions: Some("  ".into()),
            ..CleanupConfig::default()
        },
        ..Config::default()
    };
    assert!(blank.validate().is_err());
    let auto = Config {
        language: "AUTO".into(),
        ..Config::default()
    };
    assert!(auto.validate().is_ok());
    assert_eq!(auto.language_code(), None);
    assert_eq!(Config::default().language_code(), Some("en"));
}

#[test]
fn login_items_launch_the_mise_shim_instead_of_a_versioned_install() {
    use std::path::{Path, PathBuf};

    let data = Path::new("/Users/me/.local/share/mise");
    let shim = data
        .join("shims")
        .join(format!("yap{}", std::env::consts::EXE_SUFFIX));
    let installed = data.join("installs/github-howmanysmall-yap/2.0.1/bin/yap");
    let app = PathBuf::from("/Applications/Yap.app/Contents/MacOS/yap");
    for (current, shim_exists, expected) in [
        (&installed, true, &shim),
        (&installed, false, &installed),
        (&app, true, &app),
    ] {
        assert_eq!(
            yap::platform::autostart_executable(current, Some(data), |path| shim_exists
                && path == shim),
            *expected,
            "{}",
            current.display()
        );
    }
}

fn tone_then_silence(tone_seconds: f32, silence_seconds: f32) -> Encoder {
    let (tx, _rx) = mpsc::channel(1000);
    let mut encoder = Encoder::new(1, 16_000, tx);
    // A sine with peak 0.1·√2 has an RMS of 0.1, i.e. −20 dBFS.
    let amplitude = 0.1 * 2f32.sqrt();
    let tone: Vec<f32> = (0..(16_000.0 * tone_seconds) as usize)
        .map(|i| amplitude * (std::f32::consts::TAU * 440.0 * i as f32 / 16_000.0).sin())
        .collect();
    encoder.push(&tone);
    encoder.push(&vec![0.0f32; (16_000.0 * silence_seconds) as usize]);
    encoder
}

#[test]
fn a_second_of_silence_is_not_speech() {
    let encoder = tone_then_silence(0.0, 1.0);
    assert!(!encoder.heard_speech());
    assert_eq!(encoder.silent_for(), std::time::Duration::from_secs(1));
}

#[test]
fn speech_is_heard_and_the_silence_after_it_is_timed() {
    let encoder = tone_then_silence(0.5, 0.0);
    assert!(encoder.heard_speech());
    assert_eq!(encoder.silent_for(), std::time::Duration::ZERO);

    let encoder = tone_then_silence(0.5, 2.0);
    assert!(encoder.heard_speech());
    let silent = encoder.silent_for().as_secs_f32();
    assert!((1.9..=2.1).contains(&silent), "{silent}");
}

#[test]
fn background_hiss_below_minus_45_dbfs_is_not_speech() {
    let (tx, _rx) = mpsc::channel(1000);
    let mut encoder = Encoder::new(2, 48_000, tx);
    // RMS 0.003 is about −50 dBFS.
    let hiss: Vec<f32> = (0..96_000)
        .map(|i| if i % 4 < 2 { 0.003 } else { -0.003 })
        .collect();
    encoder.push(&hiss);
    assert!(!encoder.heard_speech());
}

#[test]
fn silence_timeouts_are_limited_to_ten_minutes() {
    for (silence_timeout_seconds, valid) in [(0, true), (30, true), (600, true), (601, false)] {
        let config = Config {
            silence_timeout_seconds,
            ..Config::default()
        };
        assert_eq!(
            config.validate().is_ok(),
            valid,
            "{silence_timeout_seconds}"
        );
    }
    assert_eq!(Config::default().silence_timeout_seconds, 30);
}
