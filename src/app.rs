use std::{
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use tokio::{runtime::Handle, sync::mpsc::Receiver, task::JoinHandle};

use crate::{
    audio::Capture,
    cleanup,
    config::{Config, RecordingMode},
    credentials::Credentials,
    hotkey::PressLatch,
    openrouter::ApiError,
    platform::{self, Command, Cue, Desktop, Status, Target},
    speech,
    storage::{Dictation, Paths},
};

/// Transcribes a finished WAV, cleans the transcript and stamps the result.
/// Also returns how long transcription took, in milliseconds.
pub async fn dictate(
    client: &reqwest::Client,
    config: &Config,
    key: &str,
    wav: &[u8],
) -> Result<(Dictation, u128), ApiError> {
    let transcript = speech::transcribe(client, config, key, wav).await?;
    let cleanup_started = Instant::now();
    let (text, cleanup_status) =
        cleanup::clean(client, &config.cleanup, Some(key), &transcript.text).await;
    tracing::info!(
        finalize_ms = transcript.finalize_ms,
        cleanup_ms = cleanup_started.elapsed().as_millis(),
        cleanup = cleanup_status.label(),
        "Transcription finished"
    );
    let dictation = Dictation {
        text,
        raw: transcript.text,
        complete: true,
        cleanup: cleanup_status.label().into(),
        recorded_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    Ok((dictation, transcript.finalize_ms))
}

/// The desktop surface the controller drives: tray status, sounds, the
/// Escape shortcut and pasting into the focused application.
pub trait Ui {
    type Target;
    fn target(&self) -> Result<Self::Target>;
    fn status(&mut self, status: Status, text: &str);
    fn notify(&mut self, message: &str);
    fn cue(&mut self, cue: Cue);
    fn cancel_shortcut(&mut self, enabled: bool) -> Result<()>;
    fn paste(&mut self, target: &Self::Target, text: &str, restore_after: Duration) -> Result<()>;
}

impl Ui for Desktop {
    type Target = Target;

    fn target(&self) -> Result<Target> {
        Desktop::target(self)
    }

    fn status(&mut self, status: Status, text: &str) {
        Desktop::status(self, status, text);
    }

    fn notify(&mut self, message: &str) {
        Desktop::notify(self, message);
    }

    fn cue(&mut self, cue: Cue) {
        platform::cue(cue);
    }

    fn cancel_shortcut(&mut self, enabled: bool) -> Result<()> {
        Desktop::cancel_shortcut(self, enabled)
    }

    fn paste(&mut self, target: &Target, text: &str, restore_after: Duration) -> Result<()> {
        Desktop::paste(self, target, text, restore_after)
    }
}

struct Session<T> {
    id: u64,
    capture: Option<Capture>,
    task: JoinHandle<()>,
    target: T,
    started: Instant,
    stopped: Option<Instant>,
    incomplete: Option<String>,
    ready_announced: bool,
}

impl<T> Drop for Session<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Completed {
    id: u64,
    result: Result<Dictation, ApiError>,
}

/// The dictation state machine: turns commands, microphone state and
/// finished transcriptions into status updates and pastes.
pub struct Controller<U: Ui> {
    ui: U,
    config: Config,
    paths: Paths,
    credentials: Credentials,
    client: reqwest::Client,
    runtime: Handle,
    tx: mpsc::Sender<Completed>,
    rx: mpsc::Receiver<Completed>,
    session: Option<Session<U::Target>>,
    next_id: u64,
    talk_key: PressLatch,
}

impl<U: Ui> Controller<U> {
    pub fn new(
        ui: U,
        config: Config,
        paths: Paths,
        credentials: Credentials,
        runtime: Handle,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        let (tx, rx) = mpsc::channel();
        Ok(Self {
            ui,
            config,
            paths,
            credentials,
            client,
            runtime,
            tx,
            rx,
            session: None,
            next_id: 0,
            talk_key: PressLatch::default(),
        })
    }

    pub fn ui_mut(&mut self) -> &mut U {
        &mut self.ui
    }

    /// Applies one command. Returns false when the app should quit.
    pub fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::TalkPressed if !self.talk_key.press() => return true,
            Command::TalkReleased => self.talk_key.release(),
            _ => {}
        }
        match command {
            Command::Quit => return false,
            Command::Cancel => {
                if self.session.take().is_some() {
                    self.release_cancel();
                    self.ui.status(Status::Idle, "Cancelled");
                }
            }
            Command::CopyLast => {
                if self.session.is_none() {
                    match Dictation::load(&self.paths.recovery())
                        .and_then(|saved| platform::copy_text(&saved.text))
                    {
                        Ok(()) => self.ui.status(Status::Idle, "Last dictation copied"),
                        Err(error) => self.report(&error.to_string()),
                    }
                }
            }
            Command::TalkReleased if self.config.mode == RecordingMode::Hold => {
                self.stop_recording();
            }
            Command::TalkPressed | Command::Toggle => {
                if self.session.is_some() {
                    if command == Command::Toggle || self.config.mode == RecordingMode::Toggle {
                        self.stop_recording();
                    }
                    return true;
                }
                match self.start_session() {
                    Ok(current) => {
                        self.ui
                            .status(Status::Processing, "Opening microphone · Escape cancels");
                        self.session = Some(current);
                    }
                    Err(error) => self.report(&format!("{error:#}")),
                }
            }
            Command::TalkReleased => {}
        }
        true
    }

    fn start_session(&mut self) -> Result<Session<U::Target>> {
        self.next_id += 1;
        let id = self.next_id;
        let started = Instant::now();
        let target = self.ui.target()?;
        let (capture, audio) = Capture::start(self.config.microphone.as_deref())?;
        self.ui.cancel_shortcut(true)?;
        let task = self.spawn_transcription(id, audio, capture.sample_rate);
        Ok(Session {
            id,
            capture: Some(capture),
            task,
            target,
            started,
            stopped: None,
            incomplete: None,
            ready_announced: false,
        })
    }

    fn spawn_transcription(
        &mut self,
        id: u64,
        audio: Receiver<Vec<u8>>,
        sample_rate: u32,
    ) -> JoinHandle<()> {
        let credentials = self.credentials.clone();
        let config = self.config.clone();
        let client = self.client.clone();
        let tx = self.tx.clone();
        self.runtime.spawn(async move {
            let pcm = speech::collect(audio, sample_rate, config.max_recording_seconds).await;
            let wav = speech::wav(pcm, sample_rate);
            let result = dictate(&client, &config, &credentials.openrouter, &wav)
                .await
                .map(|(dictation, _)| dictation);
            let _ = tx.send(Completed { id, result });
        })
    }

    /// Reacts to the microphone and the clock: announces readiness, stops on a
    /// fault or the length limit, and gives up on an upload that never ends.
    pub fn tick(&mut self, now: Instant) {
        let deadline = Duration::from_millis(
            self.config.transcription_timeout_ms + self.config.cleanup.timeout_ms + 8000,
        );
        let max_recording_seconds = self.config.max_recording_seconds;
        let Some(current) = &mut self.session else {
            return;
        };
        let mut announce = false;
        let mut stop = false;
        if let Some(capture) = &current.capture {
            if !current.ready_announced && capture.is_ready() {
                current.ready_announced = true;
                announce = true;
                tracing::info!(
                    ready_ms = now.saturating_duration_since(current.started).as_millis(),
                    "Microphone ready"
                );
            }
            if let Some(fault) = capture.fault() {
                current.incomplete = Some(fault);
                stop = true;
            } else if now.saturating_duration_since(current.started).as_secs()
                >= max_recording_seconds
            {
                stop = true;
            }
        }
        let timed_out = current
            .stopped
            .is_some_and(|t| now.saturating_duration_since(t) > deadline);
        if announce {
            self.ui
                .status(Status::Recording, "Recording · Escape cancels");
            self.cue(Cue::Start);
        }
        if stop {
            self.stop_recording();
        }
        if timed_out {
            self.session = None;
            self.report("Dictation timed out. Please check your connection and try again");
        }
    }

    pub fn drain(&mut self) {
        while let Ok(completed) = self.rx.try_recv() {
            self.complete(completed);
        }
    }

    fn complete(&mut self, completed: Completed) {
        if !self.session.as_ref().is_some_and(|s| s.id == completed.id) {
            return;
        }
        let Some(mut current) = self.session.take() else {
            return;
        };
        if current.capture.take().is_some() {
            self.release_cancel();
        }
        let mut dictation = match completed.result {
            Ok(dictation) => dictation,
            Err(error) => {
                self.report(&error.to_string());
                return;
            }
        };
        if let Some(reason) = &current.incomplete {
            if dictation.text.is_empty() {
                self.report(&format!("{reason}. Nothing was transcribed"));
                return;
            }
            dictation.complete = false;
            if let Err(error) = dictation.save(&self.paths.recovery()) {
                self.report(&format!("Could not save recovery text: {error}"));
                return;
            }
            self.report(&format!(
                "{reason}. Partial text is available in Copy last dictation"
            ));
            return;
        }
        if dictation.text.is_empty() {
            self.ui.status(Status::Idle, "No speech detected");
            return;
        }
        dictation.complete = true;
        if let Err(error) = dictation.save(&self.paths.recovery()) {
            self.report(&format!("Could not save recovery text: {error}"));
            return;
        }
        let restore_after = Duration::from_millis(self.config.paste_restore_ms);
        match self
            .ui
            .paste(&current.target, &dictation.text, restore_after)
        {
            Ok(()) => {
                tracing::info!(
                    stop_to_paste_ms = current.stopped.map(|t| t.elapsed().as_millis()),
                    "Text inserted"
                );
                let (label, warning) = match dictation.cleanup.as_str() {
                    "timed_out" => (
                        "Inserted · cleanup timed out",
                        Some("Inserted without cleanup: cleanup timed out"),
                    ),
                    "unavailable" => (
                        "Inserted · cleanup unavailable",
                        Some("Inserted without cleanup: cleanup was unavailable"),
                    ),
                    _ => ("Inserted", None),
                };
                self.ui.status(Status::Idle, label);
                if let Some(warning) = warning {
                    self.ui.notify(warning);
                    self.cue(Cue::Warning);
                } else {
                    self.cue(Cue::Done);
                }
            }
            Err(error) => self.report(&format!(
                "{error}. Use Copy last dictation to recover the text"
            )),
        }
    }

    fn stop_recording(&mut self) {
        let Some(current) = &mut self.session else {
            return;
        };
        let Some(capture) = current.capture.take() else {
            return;
        };
        current.stopped = Some(Instant::now());
        if let Err(error) = capture.stop() {
            current.incomplete = Some(error.to_string());
        }
        self.release_cancel();
        self.ui.status(Status::Processing, "Finishing");
        self.cue(Cue::Stop);
    }

    fn release_cancel(&mut self) {
        if let Err(error) = self.ui.cancel_shortcut(false) {
            tracing::warn!(%error, "Could not release the Escape shortcut");
        }
    }

    fn report(&mut self, message: &str) {
        tracing::error!("{message}");
        self.ui.status(Status::Error, message);
        self.ui.notify(message);
        self.cue(Cue::Error);
    }

    fn cue(&mut self, cue: Cue) {
        if self.config.sounds {
            self.ui.cue(cue);
        }
    }
}

pub fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("Start background workers")
}

pub fn run(config: Config, paths: Paths, credentials: Credentials) -> Result<()> {
    let Some(_lock) = paths.instance_lock()? else {
        bail!("Yap is already running");
    };
    let runtime = runtime()?;
    let mut desktop = Desktop::new(&config.shortcut.parse()?)?;
    let (quit_tx, quit_rx) = mpsc::channel();
    runtime.spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = quit_tx.send(());
    });
    desktop.status(Status::Idle, &format!("Ready · {}", config.shortcut));
    tracing::info!(shortcut = %config.shortcut, mode = ?config.mode, "Dictation ready");
    let mut controller = Controller::new(
        desktop,
        config,
        paths,
        credentials,
        runtime.handle().clone(),
    )?;

    'running: loop {
        let commands = controller.ui_mut().poll(Duration::from_millis(15))?;
        if quit_rx.try_recv().is_ok() {
            break;
        }
        for command in commands {
            if !controller.handle(command) {
                break 'running;
            }
        }
        controller.tick(Instant::now());
        controller.drain();
    }
    drop(controller);
    tracing::info!("Dictation stopped");
    Ok(())
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
