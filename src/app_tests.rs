#![allow(clippy::unwrap_used)]

use std::{fs, path::PathBuf};

use anyhow::anyhow;
use zeroize::Zeroizing;

use super::*;

#[derive(Debug, PartialEq)]
enum Event {
    Status(Status, String),
    Notify(String),
    Cue(&'static str),
    CancelShortcut(bool),
    /// The pasted text and whether the recovery file existed at that moment.
    Paste(String, bool),
}

struct FakeUi {
    events: Vec<Event>,
    paste_result: Result<(), String>,
    recovery: PathBuf,
}

impl Ui for FakeUi {
    type Target = ();

    fn target(&self) -> Result<()> {
        Ok(())
    }

    fn status(&mut self, status: Status, text: &str) {
        self.events.push(Event::Status(status, text.into()));
    }

    fn notify(&mut self, message: &str) {
        self.events.push(Event::Notify(message.into()));
    }

    fn cue(&mut self, cue: Cue) {
        self.events.push(Event::Cue(match cue {
            Cue::Start => "start",
            Cue::Stop => "stop",
            Cue::Done => "done",
            Cue::Error => "error",
            Cue::Warning => "warning",
        }));
    }

    fn cancel_shortcut(&mut self, enabled: bool) -> Result<()> {
        self.events.push(Event::CancelShortcut(enabled));
        Ok(())
    }

    fn paste(&mut self, _target: &(), text: &str, _restore_after: Duration) -> Result<()> {
        self.events
            .push(Event::Paste(text.into(), self.recovery.exists()));
        self.paste_result
            .clone()
            .map_err(|message| anyhow!(message))
    }
}

struct Harness {
    controller: Controller<FakeUi>,
    runtime: tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config: dir.path().join("config.toml"),
            data: dir.path().join("data"),
        };
        let runtime = runtime().unwrap();
        let ui = FakeUi {
            events: Vec::new(),
            paste_result: Ok(()),
            recovery: paths.recovery(),
        };
        let credentials = Credentials {
            openrouter: Zeroizing::new("test-key".into()),
        };
        let controller = Controller::new(
            ui,
            Config::default(),
            paths,
            credentials,
            runtime.handle().clone(),
        )
        .unwrap();
        Self {
            controller,
            runtime,
            _dir: dir,
        }
    }

    /// Installs a session that is already uploading (no microphone open).
    fn uploading(&mut self, id: u64, stopped: Option<Instant>, incomplete: Option<&str>) {
        let started = stopped.unwrap_or_else(Instant::now);
        self.controller.session = Some(Session {
            id,
            capture: None,
            task: self.runtime.spawn(async {}),
            target: (),
            started,
            stopped,
            incomplete: incomplete.map(Into::into),
            ready_announced: true,
        });
    }

    fn events(&self) -> &[Event] {
        &self.controller.ui.events
    }

    fn last_status(&self) -> Option<(Status, &str)> {
        self.events().iter().rev().find_map(|event| match event {
            Event::Status(status, text) => Some((*status, text.as_str())),
            _ => None,
        })
    }

    fn recovery(&self) -> PathBuf {
        self.controller.paths.recovery()
    }
}

fn finished(id: u64, text: &str) -> Completed {
    Completed {
        id,
        result: Ok(Dictation {
            text: text.into(),
            raw: text.into(),
            complete: true,
            cleanup: "disabled".into(),
            recorded_at_unix: 0,
        }),
    }
}

#[test]
fn a_microphone_fault_is_reported_before_no_speech_detected() {
    let mut harness = Harness::new();
    harness.uploading(1, Some(Instant::now()), Some("Microphone disconnected"));

    harness.controller.complete(finished(1, ""));

    let (status, text) = harness.last_status().unwrap();
    assert_eq!(status, Status::Error);
    assert!(text.contains("Microphone disconnected"), "{text}");
    assert!(
        !harness
            .events()
            .iter()
            .any(|e| matches!(e, Event::Status(_, t) if t == "No speech detected"))
    );
    assert!(!harness.recovery().exists());
    assert!(harness.controller.session.is_none());
}

#[test]
fn a_microphone_fault_with_partial_text_saves_but_never_pastes() {
    let mut harness = Harness::new();
    harness.uploading(1, Some(Instant::now()), Some("Microphone disconnected"));

    harness.controller.complete(finished(1, "half a sentence"));

    let saved = Dictation::load(&harness.recovery()).unwrap();
    assert_eq!(saved.text, "half a sentence");
    assert!(!saved.complete);
    assert!(
        !harness
            .events()
            .iter()
            .any(|e| matches!(e, Event::Paste(..)))
    );
    assert!(harness.events().contains(&Event::Cue("error")));
    let (status, text) = harness.last_status().unwrap();
    assert_eq!(status, Status::Error);
    assert!(text.contains("Copy last dictation"), "{text}");
}

#[test]
fn an_empty_transcript_leaves_the_previous_recovery_file_alone() {
    let mut harness = Harness::new();
    let previous = Dictation {
        text: "yesterday's words".into(),
        raw: "yesterday's words".into(),
        complete: true,
        cleanup: "disabled".into(),
        recorded_at_unix: 1,
    };
    previous.save(&harness.recovery()).unwrap();
    let before = fs::read(harness.recovery()).unwrap();
    harness.uploading(1, Some(Instant::now()), None);

    harness.controller.complete(finished(1, ""));

    assert_eq!(fs::read(harness.recovery()).unwrap(), before);
    assert_eq!(
        harness.last_status(),
        Some((Status::Idle, "No speech detected"))
    );
}

#[test]
fn the_watchdog_releases_a_stuck_upload_only_after_the_deadline() {
    let mut harness = Harness::new();
    let config = &harness.controller.config;
    let deadline =
        Duration::from_millis(config.transcription_timeout_ms + config.cleanup.timeout_ms + 8000);
    let stopped = Instant::now();
    harness.uploading(1, Some(stopped), None);

    harness
        .controller
        .tick(stopped + deadline - Duration::from_secs(1));
    assert!(harness.controller.session.is_some());
    assert!(harness.events().is_empty());

    harness
        .controller
        .tick(stopped + deadline + Duration::from_secs(1));
    assert!(harness.controller.session.is_none());
    let (status, text) = harness.last_status().unwrap();
    assert_eq!(status, Status::Error);
    assert!(text.contains("timed out"), "{text}");
}

#[test]
fn cancel_during_upload_discards_the_late_result() {
    let mut harness = Harness::new();
    harness.uploading(1, Some(Instant::now()), None);

    assert!(harness.controller.handle(Command::Cancel));

    assert_eq!(harness.last_status(), Some((Status::Idle, "Cancelled")));
    assert!(!harness.events().iter().any(|e| matches!(e, Event::Cue(_))));
    assert!(harness.events().contains(&Event::CancelShortcut(false)));

    let before = harness.events().len();
    harness.controller.complete(finished(1, "late words"));

    assert_eq!(harness.events().len(), before);
    assert!(!harness.recovery().exists());
}

#[test]
fn a_result_from_an_earlier_session_is_ignored() {
    let mut harness = Harness::new();
    harness.uploading(2, Some(Instant::now()), None);

    harness.controller.complete(finished(1, "stale words"));

    assert_eq!(harness.controller.session.as_ref().map(|s| s.id), Some(2));
    assert!(harness.events().is_empty());
    assert!(!harness.recovery().exists());
}

#[test]
fn a_paste_failure_keeps_the_recovery_file_and_says_so() {
    let mut harness = Harness::new();
    harness.controller.ui.paste_result = Err("The destination changed while dictating".into());
    harness.uploading(1, Some(Instant::now()), None);

    harness.controller.complete(finished(1, "important words"));

    let (status, text) = harness.last_status().unwrap();
    assert_eq!(status, Status::Error);
    assert!(text.contains("Copy last dictation"), "{text}");
    assert_eq!(
        Dictation::load(&harness.recovery()).unwrap().text,
        "important words"
    );
}

#[test]
fn successful_text_is_saved_before_it_is_pasted() {
    let mut harness = Harness::new();
    harness.uploading(1, Some(Instant::now()), None);

    harness.controller.complete(finished(1, "hello world"));

    assert!(
        harness
            .events()
            .contains(&Event::Paste("hello world".into(), true))
    );
    assert_eq!(harness.last_status(), Some((Status::Idle, "Inserted")));
    assert!(Dictation::load(&harness.recovery()).unwrap().complete);
}

#[test]
fn escape_is_released_when_recording_stops_not_when_the_upload_ends() {
    // A Session with an open Capture needs a real microphone, so this checks
    // the two halves separately: an upload that is already past recording
    // never touches the Escape shortcut, and cancelling always releases it.
    let mut harness = Harness::new();
    harness.uploading(1, Some(Instant::now()), None);

    harness.controller.stop_recording();
    assert!(harness.events().is_empty());
    assert!(harness.controller.session.is_some());

    harness.controller.complete(finished(1, "done"));
    assert!(
        !harness
            .events()
            .iter()
            .any(|e| matches!(e, Event::CancelShortcut(_)))
    );

    harness.uploading(2, Some(Instant::now()), None);
    harness.controller.handle(Command::Cancel);
    assert_eq!(
        harness.events().last(),
        Some(&Event::Status(Status::Idle, "Cancelled".into()))
    );
    assert!(harness.events().contains(&Event::CancelShortcut(false)));
}
