#![allow(clippy::unwrap_used)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::anyhow;
use zeroize::Zeroizing;

use super::*;
use crate::{
    preview::Panel,
    test_support::{REPLY, Service, frames, silence, tone},
};

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
    CopyText(String),
    /// Cleanup, sounds and hold-to-talk checkmarks.
    SetOptions(bool, bool, bool),
    Preview(String),
}

struct FakeUi {
    events: Vec<Event>,
    paste_result: Result<(), String>,
    copy_result: Result<(), String>,
    /// What the key prompt answers: true as if the user saved a key.
    prompt_result: bool,
    recovery: PathBuf,
    /// What the HUD or overlay would show, driven the way the platforms drive it.
    panel: Panel,
}

impl Ui for FakeUi {
    type Target = ();

    fn target(&self) -> Result<()> {
        Ok(())
    }

    fn status(&mut self, status: Status, text: &str) {
        self.panel.status(status, text);
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

    fn copy_text(&mut self, text: &str) -> Result<()> {
        self.events.push(Event::CopyText(text.into()));
        self.copy_result.clone().map_err(|message| anyhow!(message))
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

    fn set_options(&mut self, cleanup: bool, sounds: bool, hold: bool) {
        self.events.push(Event::SetOptions(cleanup, sounds, hold));
    }

    fn preview(&mut self, text: &str) {
        self.panel.preview(text);
        self.events.push(Event::Preview(text.into()));
    }
}

/// A microphone that reports fixed readings; dropping it closes `audio`,
/// as stopping a real capture does.
struct FakeMic {
    heard_speech: bool,
    silent_for: Duration,
    _audio: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
}

impl FakeMic {
    fn new(heard_speech: bool, silent_for: Duration) -> Self {
        Self {
            heard_speech,
            silent_for,
            _audio: None,
        }
    }
}

impl Microphone for FakeMic {
    fn is_ready(&self) -> bool {
        true
    }

    fn fault(&self) -> Option<String> {
        None
    }

    fn heard_speech(&self) -> bool {
        self.heard_speech
    }

    fn silent_for(&self) -> Duration {
        self.silent_for
    }

    fn stop(self: Box<Self>) -> Result<()> {
        Ok(())
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
            copy_result: Ok(()),
            prompt_result: false,
            recovery: paths.recovery(),
            panel: Panel::default(),
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
            shown_second: None,
            preview: BTreeMap::new(),
        });
    }

    /// Starts a fake OpenRouter on the harness runtime and points the
    /// controller at it.
    fn serve(&mut self, status: u16, reply: &'static str) -> Service {
        self.serve_all(vec![(status, reply)])
    }

    /// Like [`Harness::serve`], answering successive requests in order.
    fn serve_all(&mut self, replies: Vec<(u16, &'static str)>) -> Service {
        let service = self
            .runtime
            .block_on(Service::start(replies, Duration::ZERO));
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
            shown_second: None,
            preview: BTreeMap::new(),
        });
    }

    /// Installs a session that is still recording from `mic`, started at
    /// `started`. With `upload`, the mic feeds a real upload of `recording()`.
    fn recording(&mut self, mut mic: FakeMic, started: Instant, upload: bool) {
        self.controller.next_id += 1;
        let id = self.controller.next_id;
        let task = if upload {
            let (audio_tx, audio) = tokio::sync::mpsc::channel(4);
            audio_tx.try_send(vec![0, 0, 255, 127]).unwrap();
            mic._audio = Some(audio_tx);
            self.controller.spawn_transcription(
                id,
                Source::Live {
                    audio,
                    sample_rate: 16000,
                },
            )
        } else {
            self.runtime.spawn(async {})
        };
        self.controller.session = Some(Session {
            id,
            capture: Some(Box::new(mic)),
            task,
            target: (),
            started,
            stopped: None,
            incomplete: None,
            ready_announced: false,
            shown_second: None,
            preview: BTreeMap::new(),
        });
    }

    fn statuses(&self) -> Vec<(Status, &str)> {
        self.events()
            .iter()
            .filter_map(|event| match event {
                Event::Status(status, text) => Some((*status, text.as_str())),
                _ => None,
            })
            .collect()
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
    harness.controller.ui.copy_result = Err("The clipboard is busy".into());
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

    harness.controller.stop_recording("Finishing");
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

#[test]
fn a_paste_failure_puts_the_text_on_the_clipboard() {
    let mut harness = Harness::new();
    harness.controller.ui.paste_result = Err("The destination changed while dictating".into());
    harness.uploading(1, Some(Instant::now()), None);

    harness.controller.complete(finished(1, "important words"));

    assert!(
        harness
            .events()
            .contains(&Event::CopyText("important words".into()))
    );
    let (status, text) = harness.last_status().unwrap();
    assert_eq!(status, Status::Error);
    assert!(text.contains("on your clipboard"), "{text}");
}

#[test]
fn copy_last_dictation_goes_through_the_clipboard() {
    let mut harness = Harness::new();
    finished(1, "saved words")
        .result
        .unwrap()
        .save(&harness.recovery())
        .unwrap();

    harness.controller.handle(Command::CopyLast);

    assert!(
        harness
            .events()
            .contains(&Event::CopyText("saved words".into()))
    );
    assert_eq!(
        harness.last_status(),
        Some((Status::Idle, "Last dictation copied"))
    );
}

#[test]
fn recording_labels_count_minutes_and_warn_before_the_limit() {
    let limit = Duration::from_secs(600);
    assert_eq!(
        recording_label(Duration::from_millis(42_900), limit),
        "Recording 0:42 · Escape cancels"
    );
    assert_eq!(
        recording_label(Duration::from_secs(575), limit),
        "Recording 9:35 · stops at 10:00"
    );
    assert_eq!(
        recording_label(Duration::from_secs(569), limit),
        "Recording 9:29 · Escape cancels"
    );
}

#[test]
fn the_recording_status_updates_once_per_elapsed_second() {
    let mut harness = Harness::new();
    let started = Instant::now();
    harness.recording(FakeMic::new(true, Duration::ZERO), started, false);

    for millis in [0, 300, 999, 1000, 1500, 2100, 2900] {
        harness
            .controller
            .tick(started + Duration::from_millis(millis));
    }

    assert_eq!(
        harness.statuses(),
        [
            (Status::Recording, "Recording 0:00 · Escape cancels"),
            (Status::Recording, "Recording 0:01 · Escape cancels"),
            (Status::Recording, "Recording 0:02 · Escape cancels"),
        ]
    );
    assert_eq!(
        harness
            .events()
            .iter()
            .filter(|e| **e == Event::Cue("start"))
            .count(),
        1
    );
}

#[test]
fn a_long_silence_stops_a_toggle_recording() {
    let mut harness = Harness::new();
    let started = Instant::now();
    harness.recording(FakeMic::new(true, Duration::from_secs(30)), started, false);

    harness.controller.tick(started + Duration::from_secs(29));
    assert!(
        harness
            .controller
            .session
            .as_ref()
            .unwrap()
            .capture
            .is_some()
    );

    harness.controller.tick(started + Duration::from_secs(30));

    let session = harness.controller.session.as_ref().unwrap();
    assert!(session.capture.is_none());
    assert_eq!(
        harness.last_status(),
        Some((Status::Processing, "Stopped after 30 s of silence"))
    );
}

#[test]
fn silence_never_stops_hold_to_talk_or_a_disabled_timeout() {
    for (mode, timeout) in [(RecordingMode::Hold, 30), (RecordingMode::Toggle, 0)] {
        let mut harness = Harness::new();
        harness.controller.config.mode = mode;
        harness.controller.config.silence_timeout_seconds = timeout;
        let started = Instant::now();
        harness.recording(FakeMic::new(true, Duration::from_secs(120)), started, false);

        harness.controller.tick(started + Duration::from_secs(120));

        assert!(
            harness
                .controller
                .session
                .as_ref()
                .unwrap()
                .capture
                .is_some(),
            "{mode:?} {timeout}"
        );
    }
}

#[test]
fn a_recording_without_speech_is_never_uploaded() {
    let mut harness = Harness::new();
    let service = harness.serve(200, REPLY);
    harness.recording(FakeMic::new(false, Duration::ZERO), Instant::now(), true);

    harness.controller.handle(Command::Toggle);
    harness
        .runtime
        .block_on(async { tokio::time::sleep(Duration::from_millis(200)).await });
    harness.controller.drain();

    assert!(service.requests().is_empty());
    assert!(harness.controller.session.is_none());
    assert!(
        !harness
            .events()
            .iter()
            .any(|e| matches!(e, Event::Paste(..)))
    );
    assert!(!harness.recovery().exists());
    assert!(harness.events().contains(&Event::CancelShortcut(false)));
    assert_eq!(
        harness.last_status(),
        Some((Status::Idle, "No speech detected"))
    );
}

#[test]
fn a_silent_toggle_recording_that_times_out_is_dropped_too() {
    let mut harness = Harness::new();
    let service = harness.serve(200, REPLY);
    let started = Instant::now();
    harness.recording(FakeMic::new(false, Duration::from_secs(30)), started, true);

    harness.controller.tick(started + Duration::from_secs(30));
    harness
        .runtime
        .block_on(async { tokio::time::sleep(Duration::from_millis(200)).await });
    harness.controller.drain();

    assert!(service.requests().is_empty());
    assert!(harness.controller.session.is_none());
    assert_eq!(
        harness.last_status(),
        Some((Status::Idle, "No speech detected"))
    );
}

#[test]
fn a_recording_with_speech_is_uploaded_and_pasted() {
    let mut harness = Harness::new();
    let service = harness.serve(200, REPLY);
    harness.recording(FakeMic::new(true, Duration::ZERO), Instant::now(), true);

    harness.controller.handle(Command::Toggle);
    assert_eq!(
        harness.last_status(),
        Some((Status::Processing, "Finishing"))
    );
    harness.finish();

    assert_eq!(service.requests()[0].samples, vec![0, 32767]);
    assert!(
        harness
            .events()
            .contains(&Event::Paste("Keep the final word: café.".into(), true))
    );
}

fn credits(remaining: Option<f64>) -> Result<KeyInfo, ApiError> {
    Ok(KeyInfo {
        limit_remaining: remaining,
        ..KeyInfo::default()
    })
}

#[test]
fn a_low_balance_warns_at_startup() {
    let mut harness = Harness::new();

    harness.controller.announce_credits(credits(Some(0.42)));

    assert_eq!(
        harness.last_status(),
        Some((Status::Idle, "Ready · $0.42 credits · Ctrl+Alt+Space"))
    );
    assert!(harness.events().contains(&Event::Notify(
        "OpenRouter credits are low: $0.42 left".into()
    )));
    assert!(harness.events().contains(&Event::Cue("warning")));
}

#[test]
fn a_healthy_balance_shows_the_amount_without_a_warning() {
    let mut harness = Harness::new();

    harness.controller.announce_credits(credits(Some(12.5)));

    assert_eq!(
        harness.events(),
        [Event::Status(
            Status::Idle,
            "Ready · $12.50 credits · Ctrl+Alt+Space".into()
        )]
    );
}

#[test]
fn a_key_without_a_limit_or_an_unreachable_server_keeps_the_plain_ready_line() {
    for result in [credits(None), Err(ApiError::Timeout)] {
        let mut harness = Harness::new();

        harness.controller.announce_credits(result);

        assert_eq!(
            harness.events(),
            [Event::Status(Status::Idle, "Ready · Ctrl+Alt+Space".into())]
        );
    }
}

#[test]
fn a_rejected_key_is_announced_with_the_error_cue() {
    let mut harness = Harness::new();

    harness
        .controller
        .announce_credits(Err(ApiError::Unauthorized));

    assert!(harness.events().contains(&Event::Notify(
        "OpenRouter rejected the API key. Choose Set API key… in the tray".into()
    )));
    assert!(harness.events().contains(&Event::Cue("error")));
}

#[test]
fn credits_arriving_mid_dictation_leave_its_status_alone() {
    let mut harness = Harness::new();
    harness.uploading(1, Some(Instant::now()), None);

    harness.controller.announce_credits(credits(Some(5.0)));

    assert!(harness.events().is_empty(), "{:?}", harness.events());
}

#[test]
fn tray_toggles_rewrite_the_config_file() {
    let mut harness = Harness::new();
    let path = harness.controller.paths.config.clone();

    assert!(harness.controller.handle(Command::ToggleCleanup));

    let written: Config = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert!(written.cleanup.enabled);
    assert!(
        harness
            .events()
            .contains(&Event::SetOptions(true, true, false))
    );
    assert_eq!(harness.last_status(), Some((Status::Idle, "Cleanup on")));

    harness.controller.handle(Command::ToggleSounds);
    harness.controller.handle(Command::ToggleMode);

    let written: Config = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert!(!written.sounds);
    assert_eq!(written.mode, RecordingMode::Hold);
    assert!(
        harness
            .events()
            .contains(&Event::SetOptions(true, false, true))
    );
    assert_eq!(harness.last_status(), Some((Status::Idle, "Hold to talk")));
}

#[test]
fn toggles_are_ignored_while_dictating() {
    let mut harness = Harness::new();
    harness.uploading(1, Some(Instant::now()), None);

    for command in [
        Command::ToggleCleanup,
        Command::ToggleSounds,
        Command::ToggleMode,
    ] {
        assert!(harness.controller.handle(command));
    }

    assert!(harness.events().is_empty(), "{:?}", harness.events());
    assert!(!harness.controller.paths.config.exists());
    assert!(!harness.controller.config.cleanup.enabled);
    assert!(harness.controller.config.sounds);
    assert_eq!(harness.controller.config.mode, RecordingMode::Toggle);
}

fn preview_notice(harness: &Harness, id: u64, sequence: u32, text: &str) {
    harness
        .controller
        .notices_tx
        .send(Notice::Preview {
            id,
            sequence,
            text: text.into(),
        })
        .unwrap();
}

fn previews(harness: &Harness) -> Vec<&str> {
    harness
        .events()
        .iter()
        .filter_map(|event| match event {
            Event::Preview(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn preview_segments_render_in_sequence_order_even_when_they_arrive_out_of_order() {
    let mut harness = Harness::new();
    harness.recording(FakeMic::new(true, Duration::ZERO), Instant::now(), false);
    let id = harness.controller.next_id;

    preview_notice(&harness, id, 2, "the lazy dog.");
    preview_notice(&harness, id, 0, "The quick brown fox");
    preview_notice(&harness, id, 1, " jumps over ");
    harness.controller.drain();

    assert_eq!(
        previews(&harness).last(),
        Some(&"The quick brown fox jumps over the lazy dog.")
    );
}

#[test]
fn preview_for_a_stale_session_is_ignored() {
    let mut harness = Harness::new();
    harness.uploading(2, Some(Instant::now()), None);

    preview_notice(&harness, 1, 0, "words from before");
    harness.controller.drain();

    assert!(harness.events().is_empty(), "{:?}", harness.events());
    assert!(
        harness
            .controller
            .session
            .as_ref()
            .unwrap()
            .preview
            .is_empty()
    );
}

#[test]
fn the_preview_is_cleared_when_the_dictation_ends() {
    let mut harness = Harness::new();
    harness.uploading(1, Some(Instant::now()), None);
    harness
        .controller
        .ui
        .status(Status::Processing, "Finishing");
    preview_notice(&harness, 1, 0, "hello world");
    harness.controller.drain();
    assert_eq!(
        harness.controller.ui.panel.text().as_deref(),
        Some("Finishing\nhello world")
    );

    harness.controller.complete(finished(1, "Hello, world."));

    assert_eq!(harness.controller.ui.panel.text(), None);
    // A late segment of the finished dictation does not bring it back.
    preview_notice(&harness, 1, 1, "late words");
    harness.controller.drain();
    harness
        .controller
        .ui
        .status(Status::Recording, "Recording 0:00 · Escape cancels");
    assert_eq!(
        harness.controller.ui.panel.text().as_deref(),
        Some("Recording 0:00 · Escape cancels")
    );
}

const PREVIEW_REPLY: &str = r#"{"text":"keep the final word"}"#;

/// Records 2.5 s of tone and 1 s of silence from a live microphone, waits
/// for a preview when `wait_for_preview`, then stops and lets the final
/// upload finish.
fn dictate_live(harness: &mut Harness, wait_for_preview: bool) {
    let pcm = [tone(2500), silence(1000)].concat();
    let (audio_tx, audio) = tokio::sync::mpsc::channel(512);
    for frame in frames(&pcm) {
        audio_tx.try_send(frame.to_vec()).unwrap();
    }
    let mut mic = FakeMic::new(true, Duration::ZERO);
    mic._audio = Some(audio_tx);
    harness.controller.next_id += 1;
    let id = harness.controller.next_id;
    let task = harness.controller.spawn_transcription(
        id,
        Source::Live {
            audio,
            sample_rate: 16000,
        },
    );
    harness.controller.session = Some(Session {
        id,
        capture: Some(Box::new(mic)),
        task,
        target: (),
        started: Instant::now(),
        stopped: None,
        incomplete: None,
        ready_announced: true,
        shown_second: None,
        preview: BTreeMap::new(),
    });
    let waited = Instant::now();
    loop {
        harness
            .runtime
            .block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
        harness.controller.drain();
        let done = if wait_for_preview {
            !previews(harness).is_empty()
        } else {
            waited.elapsed() >= Duration::from_millis(300)
        };
        if done {
            break;
        }
        assert!(
            waited.elapsed() < Duration::from_secs(10),
            "no preview arrived"
        );
    }
    harness.controller.handle(Command::Toggle);
    harness.finish();
}

#[test]
fn a_live_recording_previews_its_first_phrase_before_the_full_upload_is_pasted() {
    let mut harness = Harness::new();
    let service = harness.serve_all(vec![(200, PREVIEW_REPLY), (200, REPLY)]);

    dictate_live(&mut harness, true);

    let preview = harness
        .events()
        .iter()
        .position(|e| matches!(e, Event::Preview(text) if text == "keep the final word"));
    let paste = harness
        .events()
        .iter()
        .position(|e| *e == Event::Paste("Keep the final word: café.".into(), true));
    assert!(preview.unwrap() < paste.unwrap(), "{:?}", harness.events());
    let requests = service.requests();
    assert_eq!(requests.len(), 2);
    // The preview uploads only the phrase; the paste comes from everything.
    assert!(requests[0].samples.len() < requests[1].samples.len());
    assert_eq!(requests[1].samples.len(), 3500 * 16);
}

#[test]
fn with_live_preview_off_only_the_full_recording_is_uploaded() {
    let mut harness = Harness::new();
    harness.controller.config.live_preview = false;
    let service = harness.serve_all(vec![(200, REPLY), (200, PREVIEW_REPLY)]);

    dictate_live(&mut harness, false);

    assert_eq!(service.requests().len(), 1);
    assert!(previews(&harness).is_empty());
    assert!(
        harness
            .events()
            .contains(&Event::Paste("Keep the final word: café.".into(), true))
    );
}
