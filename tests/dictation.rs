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
