#![allow(clippy::unwrap_used)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::anyhow;
use zeroize::Zeroizing;

use super::*;
use crate::test_support::{REPLY, Service};

#[derive(Debug, PartialEq)]
enum Event {
    Status(Status, String),
    Notify(String),
    Cue(&'static str),
    CancelShortcut(bool),
    /// The pasted text and whether the recovery file existed at that moment.
    Paste(String, bool),
    PromptApiKey,
    OpenPath(PathBuf),
    OpenUrl(String),
    SetAutostart(bool),
}

struct FakeUi {
    events: Vec<Event>,
    paste_result: Result<(), String>,
    /// What the key prompt answers: true as if the user saved a key.
    prompt_result: bool,
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

    fn prompt_api_key(&mut self) -> Result<bool> {
        self.events.push(Event::PromptApiKey);
        Ok(self.prompt_result)
    }

    fn open_path(&mut self, path: &Path) -> Result<()> {
        self.events.push(Event::OpenPath(path.to_path_buf()));
        Ok(())
    }

    fn open_url(&mut self, url: &str) -> Result<()> {
        self.events.push(Event::OpenUrl(url.into()));
        Ok(())
    }

    fn set_autostart(&mut self, enabled: bool) {
        self.events.push(Event::SetAutostart(enabled));
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
            prompt_result: false,
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

    /// Starts a fake OpenRouter on the harness runtime and points the
    /// controller at it.
    fn serve(&mut self, status: u16, reply: &'static str) -> Service {
        let service = self
            .runtime
            .block_on(Service::start(vec![(status, reply)], Duration::ZERO));
        self.controller.endpoint = service.url.clone();
        service
    }

    /// Installs a session that uploads `wav` for real.
    fn transcribing(&mut self, wav: Vec<u8>) {
        self.controller.next_id += 1;
        let id = self.controller.next_id;
        let task = self.controller.spawn_transcription(id, Source::Wav(wav));
        self.controller.session = Some(Session {
            id,
            capture: None,
            task,
            target: (),
            started: Instant::now(),
            stopped: Some(Instant::now()),
            incomplete: None,
            ready_announced: true,
        });
    }

    /// Waits for the active session's upload to end and applies its result.
    fn finish(&mut self) {
        let task = &mut self.controller.session.as_mut().unwrap().task;
        self.runtime.block_on(task).unwrap();
        self.controller.drain();
    }

    fn kept(&self) -> PathBuf {
        self.controller.paths.last_recording()
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
        kept: false,
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
    let deadline = harness.controller.config.watchdog();
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

#[test]
fn a_cleanup_timeout_is_announced_not_just_labelled() {
    let mut harness = Harness::new();
    harness.uploading(1, Some(Instant::now()), None);
    let mut completed = finished(1, "hello world");
    if let Ok(dictation) = &mut completed.result {
        dictation.cleanup = "timed_out".into();
    }

    harness.controller.complete(completed);

    assert!(
        harness
            .events()
            .iter()
            .any(|e| matches!(e, Event::Notify(m) if m.contains("cleanup timed out")))
    );
    assert!(harness.events().contains(&Event::Cue("warning")));
    assert!(!harness.events().contains(&Event::Cue("done")));
}

#[test]
fn open_config_writes_defaults_first_when_the_file_is_missing() {
    let mut harness = Harness::new();
    let config = harness.controller.paths.config.clone();
    assert!(!config.exists());

    assert!(harness.controller.handle(Command::OpenConfig));

    let written: Config = toml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(written.shortcut, Config::default().shortcut);
    assert!(harness.events().contains(&Event::OpenPath(config)));
    assert!(harness.last_status().is_none());
}

#[test]
fn open_config_keeps_an_existing_file() {
    let mut harness = Harness::new();
    let config = harness.controller.paths.config.clone();
    fs::write(&config, "shortcut = \"F5\"\n").unwrap();

    harness.controller.handle(Command::OpenConfig);

    assert_eq!(fs::read_to_string(&config).unwrap(), "shortcut = \"F5\"\n");
    assert_eq!(harness.events(), [Event::OpenPath(config)]);
}

#[test]
fn open_logs_creates_the_log_folder_before_opening_it() {
    let mut harness = Harness::new();
    let data = harness.controller.paths.data.clone();

    harness.controller.handle(Command::OpenLogs);

    assert!(data.is_dir());
    assert_eq!(harness.events(), [Event::OpenPath(data)]);
}

#[test]
fn tray_commands_are_ignored_while_dictating() {
    let mut harness = Harness::new();
    harness.controller.ui.prompt_result = true;
    harness.uploading(1, Some(Instant::now()), None);

    for command in [
        Command::SetApiKey,
        Command::OpenConfig,
        Command::OpenLogs,
        Command::ToggleAutostart,
        Command::OpenReleases,
    ] {
        assert!(harness.controller.handle(command));
    }

    assert!(harness.events().is_empty(), "{:?}", harness.events());
    assert!(!harness.controller.paths.config.exists());
    assert!(harness.controller.session.is_some());
}

#[test]
fn a_cancelled_key_prompt_changes_nothing() {
    let mut harness = Harness::new();

    assert!(harness.controller.handle(Command::SetApiKey));

    assert_eq!(harness.events(), [Event::PromptApiKey]);
    assert_eq!(
        harness.controller.credentials.openrouter.as_str(),
        "test-key"
    );
}

/// A two-sample WAV, as the spawned task would build from the microphone.
fn recording() -> Vec<u8> {
    let mut pcm = vec![0; 44];
    pcm.extend_from_slice(&[0, 0, 255, 127]);
    speech::wav(pcm, 16000)
}

#[test]
fn a_failed_upload_keeps_the_recording_for_retry() {
    let mut harness = Harness::new();
    let _service = harness.serve(402, r#"{"error":{"message":"Insufficient credits"}}"#);
    harness.transcribing(recording());

    harness.finish();

    assert!(harness.controller.session.is_none());
    assert_eq!(fs::read(harness.kept()).unwrap(), recording());
    let (status, text) = harness.last_status().unwrap();
    assert_eq!(status, Status::Error);
    assert!(text.contains("no credits"), "{text}");
    assert!(text.contains("yap last --retry"), "{text}");
}

#[test]
fn retry_last_uploads_the_kept_recording_and_pastes_it() {
    let mut harness = Harness::new();
    let service = harness.serve(200, REPLY);
    write_private(&harness.kept(), &recording()).unwrap();

    assert!(harness.controller.handle(Command::RetryLast));
    assert_eq!(
        harness.last_status(),
        Some((Status::Processing, "Retrying upload"))
    );
    harness.finish();

    assert_eq!(service.requests()[0].samples, vec![0, 32767]);
    assert!(
        harness
            .events()
            .contains(&Event::Paste("Keep the final word: café.".into(), true))
    );
    assert_eq!(harness.last_status(), Some((Status::Idle, "Inserted")));
    assert!(!harness.kept().exists());
    assert_eq!(
        Dictation::load(&harness.recovery()).unwrap().text,
        "Keep the final word: café."
    );
}

#[test]
fn retry_last_with_nothing_kept_reports_instead_of_starting_a_session() {
    let mut harness = Harness::new();

    assert!(harness.controller.handle(Command::RetryLast));

    assert!(harness.controller.session.is_none());
    assert_eq!(
        harness.last_status(),
        Some((Status::Error, "No failed recording to retry"))
    );
}

#[test]
fn retry_last_is_ignored_while_a_dictation_is_in_progress() {
    let mut harness = Harness::new();
    write_private(&harness.kept(), &recording()).unwrap();
    harness.uploading(7, Some(Instant::now()), None);

    assert!(harness.controller.handle(Command::RetryLast));

    assert_eq!(harness.controller.session.as_ref().map(|s| s.id), Some(7));
    assert!(harness.events().is_empty());
    assert!(harness.kept().exists());
}

#[test]
fn a_successful_dictation_discards_the_kept_recording() {
    let mut harness = Harness::new();
    write_private(&harness.kept(), &recording()).unwrap();
    harness.uploading(1, Some(Instant::now()), None);

    harness.controller.complete(finished(1, "hello world"));

    assert_eq!(harness.last_status(), Some((Status::Idle, "Inserted")));
    assert!(!harness.kept().exists());
}
