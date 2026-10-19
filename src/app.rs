use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{Arc, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use tokio::{
    runtime::Handle,
    sync::{Semaphore, mpsc::Receiver},
    task::{JoinHandle, JoinSet},
};
use zeroize::Zeroizing;

use crate::{
    audio::Capture,
    cleanup,
    config::{Config, RecordingMode},
    credentials::Credentials,
    hotkey::{Hotkey, PressLatch},
    openrouter::{self, ApiError, KeyInfo},
    platform::{self, Command, Cue, Desktop, Status, Target},
    preview::Segmenter,
    speech,
    storage::{Dictation, Paths, write_private},
};

/// Transcribes a finished WAV, cleans the transcript and stamps the result.
/// Also returns how long transcription took, in milliseconds.
pub async fn dictate(
    client: &reqwest::Client,
    config: &Config,
    key: &str,
    wav: &[u8],
) -> Result<(Dictation, u128), ApiError> {
    dictate_at(client, config, key, wav, speech::ENDPOINT).await
}

async fn dictate_at(
    client: &reqwest::Client,
    config: &Config,
    key: &str,
    wav: &[u8],
    endpoint: &str,
) -> Result<(Dictation, u128), ApiError> {
    let transcript = speech::transcribe(client, config, key, wav, endpoint).await?;
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
/// Escape shortcut, pasting into the focused application, the clipboard,
/// and the tray menu's key prompt, file shortcuts and checkmarks.
pub trait Ui {
    type Target;
    fn target(&self) -> Result<Self::Target>;
    fn status(&mut self, status: Status, text: &str);
    fn notify(&mut self, message: &str);
    fn cue(&mut self, cue: Cue);
    fn cancel_shortcut(&mut self, enabled: bool) -> Result<()>;
    fn paste(&mut self, target: &Self::Target, text: &str, restore_after: Duration) -> Result<()>;
    fn copy_text(&mut self, text: &str) -> Result<()>;
    /// Asks for and stores the API key. Returns false when cancelled.
    fn prompt_api_key(&mut self) -> Result<bool>;
    fn open_path(&mut self, path: &Path) -> Result<()>;
    fn open_url(&mut self, url: &str) -> Result<()>;
    fn set_autostart(&mut self, enabled: bool);
    fn set_options(&mut self, cleanup: bool, sounds: bool, hold: bool);
    /// Shows the live preview so far under the recording status. The next
    /// idle or error status clears it.
    fn preview(&mut self, text: &str);
    /// Replaces the dictation shortcut. On failure the old one stays bound.
    fn rebind(&mut self, shortcut: &Hotkey) -> Result<()>;
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

    fn copy_text(&mut self, text: &str) -> Result<()> {
        platform::copy_text(text)
    }

    fn prompt_api_key(&mut self) -> Result<bool> {
        platform::prompt_api_key()
    }

    fn open_path(&mut self, path: &Path) -> Result<()> {
        platform::open_path(path)
    }

    fn open_url(&mut self, url: &str) -> Result<()> {
        platform::open_url(url)
    }

    fn set_autostart(&mut self, enabled: bool) {
        Desktop::set_autostart(self, enabled);
    }

    fn set_options(&mut self, cleanup: bool, sounds: bool, hold: bool) {
        Desktop::set_options(self, cleanup, sounds, hold);
    }

    fn preview(&mut self, text: &str) {
        Desktop::preview(self, text);
    }

    fn rebind(&mut self, shortcut: &Hotkey) -> Result<()> {
        Desktop::rebind(self, shortcut)
    }
}

/// The open microphone as the controller sees it, so tests can stand in
/// for a device.
trait Microphone {
    fn is_ready(&self) -> bool;
    fn fault(&self) -> Option<String>;
    fn heard_speech(&self) -> bool;
    fn silent_for(&self) -> Duration;
    /// Stops capturing; the audio channel closes once the microphone drops.
    fn stop(self: Box<Self>) -> Result<()>;
}

impl Microphone for Capture {
    fn is_ready(&self) -> bool {
        Capture::is_ready(self)
    }

    fn fault(&self) -> Option<String> {
        Capture::fault(self)
    }

    fn heard_speech(&self) -> bool {
        Capture::heard_speech(self)
    }

    fn silent_for(&self) -> Duration {
        Capture::silent_for(self)
    }

    fn stop(self: Box<Self>) -> Result<()> {
        Capture::stop(*self)
    }
}

/// The recording status for `elapsed` out of `limit`, as `m:ss`.
fn recording_label(elapsed: Duration, limit: Duration) -> String {
    let clock = |time: Duration| format!("{}:{:02}", time.as_secs() / 60, time.as_secs() % 60);
    if elapsed + Duration::from_secs(30) >= limit {
        format!("Recording {} · stops at {}", clock(elapsed), clock(limit))
    } else {
        format!("Recording {} · Escape cancels", clock(elapsed))
    }
}

struct Session<T> {
    id: u64,
    capture: Option<Box<dyn Microphone>>,
    task: JoinHandle<()>,
    target: T,
    started: Instant,
    stopped: Option<Instant>,
    incomplete: Option<String>,
    ready_announced: bool,
    /// The elapsed second the recording status last showed.
    shown_second: Option<u64>,
    /// Live preview text by segment sequence number.
    preview: BTreeMap<u32, String>,
}

impl<T> Drop for Session<T> {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Completed {
    id: u64,
    result: Result<Dictation, ApiError>,
    /// Whether a failed upload's WAV was saved for a retry.
    kept: bool,
}

/// What background tasks report besides a finished dictation.
enum Notice {
    Credits(Result<KeyInfo, ApiError>),
    /// The transcript of one live preview segment of session `id`.
    Preview {
        id: u64,
        sequence: u32,
        text: String,
    },
}

/// At most this many preview uploads run at once; later segments wait
/// for a free slot, in order.
const PREVIEW_UPLOADS: usize = 2;

/// Uploads a live recording's phrases as they close, for the preview only.
/// Dropping it aborts the uploads still running.
struct Previews {
    id: u64,
    segmenter: Segmenter,
    sample_rate: u32,
    sequence: u32,
    slots: Arc<Semaphore>,
    uploads: JoinSet<()>,
    client: reqwest::Client,
    config: Arc<Config>,
    key: Arc<Zeroizing<String>>,
    endpoint: Arc<str>,
    notices: mpsc::Sender<Notice>,
}

impl Previews {
    fn push(&mut self, frame: &[u8]) {
        while self.uploads.try_join_next().is_some() {}
        let Some(pcm) = self.segmenter.push(frame) else {
            return;
        };
        let wav = speech::wav([&[0; speech::HEADER][..], &pcm].concat(), self.sample_rate);
        let (id, sequence) = (self.id, self.sequence);
        self.sequence += 1;
        let slots = Arc::clone(&self.slots);
        let client = self.client.clone();
        let config = Arc::clone(&self.config);
        let key = Arc::clone(&self.key);
        let endpoint = Arc::clone(&self.endpoint);
        let notices = self.notices.clone();
        self.uploads.spawn(async move {
            let Ok(_slot) = slots.acquire_owned().await else {
                return;
            };
            match speech::transcribe(&client, &config, &key, &wav, &endpoint).await {
                Ok(transcript) => {
                    let _ = notices.send(Notice::Preview {
                        id,
                        sequence,
                        text: transcript.text,
                    });
                }
                Err(error) => tracing::debug!(%error, sequence, "Preview upload failed"),
            }
        });
    }
}

/// Where a session's audio comes from: the open microphone, or a recording
/// that is already a complete WAV.
enum Source {
    Live {
        audio: Receiver<Vec<u8>>,
        sample_rate: u32,
    },
    Wav(Vec<u8>),
}

/// The dictation state machine: turns commands, microphone state and
/// finished transcriptions into status updates and pastes.
pub struct Controller<U: Ui> {
    ui: U,
    config: Config,
    paths: Paths,
    credentials: Credentials,
    client: reqwest::Client,
    endpoint: String,
    runtime: Handle,
    tx: mpsc::Sender<Completed>,
    rx: mpsc::Receiver<Completed>,
    notices_tx: mpsc::Sender<Notice>,
    notices: mpsc::Receiver<Notice>,
    session: Option<Session<U::Target>>,
    next_id: u64,
    talk_key: PressLatch,
    /// The config file's modification time and size when last read, so a
    /// duplicate change event does not reload it twice.
    config_stamp: Option<(SystemTime, u64)>,
    config_changes_tx: mpsc::Sender<()>,
    config_changes: mpsc::Receiver<()>,
    /// When the latest change event arrived; the reload waits for an
    /// editor's writes to settle.
    config_changed: Option<Instant>,
    /// A change saved during a dictation, applied once it ends.
    pending_config: Option<Config>,
    /// Kept alive for as long as the controller; dropping it stops the events.
    _config_watcher: Option<notify::RecommendedWatcher>,
}

/// How long the config file must stay quiet before it is read.
const CONFIG_SETTLE: Duration = Duration::from_millis(200);

/// The config file's modification time and size, or `None` when it is missing.
fn config_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
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
        let (notices_tx, notices) = mpsc::channel();
        let (config_changes_tx, config_changes) = mpsc::channel();
        let config_stamp = config_stamp(&paths.config);
        Ok(Self {
            ui,
            config,
            paths,
            credentials,
            client,
            endpoint: speech::ENDPOINT.into(),
            runtime,
            tx,
            rx,
            notices_tx,
            notices,
            session: None,
            next_id: 0,
            talk_key: PressLatch::default(),
            config_stamp,
            config_changes_tx,
            config_changes,
            config_changed: None,
            pending_config: None,
            _config_watcher: None,
        })
    }

    /// Watches the config file's folder and signals `tick` whenever the
    /// file is written, created, renamed into place or removed. Editors
    /// often save through a temporary file, so events are matched on the
    /// final file name.
    pub fn watch_config(&mut self) -> Result<()> {
        use notify::{EventKind, RecursiveMode, Watcher};

        let folder = self
            .paths
            .config
            .parent()
            .context("The config file has no folder")?;
        fs::create_dir_all(folder)?;
        let name = self.paths.config.file_name().map(ToOwned::to_owned);
        let changes = self.config_changes_tx.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| match event {
                Ok(event)
                    if !matches!(event.kind, EventKind::Access(_))
                        && event
                            .paths
                            .iter()
                            .any(|path| path.file_name() == name.as_deref()) =>
                {
                    let _ = changes.send(());
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "Could not watch the config file"),
            })
            .context("Watch the config file")?;
        watcher
            .watch(folder, RecursiveMode::NonRecursive)
            .with_context(|| format!("Watch {}", folder.display()))?;
        self._config_watcher = Some(watcher);
        Ok(())
    }

    /// Reloads the config file once its change events have settled, and
    /// applies a change that waited for a dictation to end.
    fn check_config(&mut self, now: Instant) {
        if self.config_changes.try_iter().count() > 0 {
            self.config_changed = Some(now);
        }
        if self.session.is_none()
            && let Some(config) = self.pending_config.take()
        {
            self.apply_config(config);
        }
        if self
            .config_changed
            .is_some_and(|at| now.saturating_duration_since(at) >= CONFIG_SETTLE)
        {
            self.config_changed = None;
            let stamp = config_stamp(&self.paths.config);
            if stamp != self.config_stamp {
                self.config_stamp = stamp;
                self.reload_config();
            }
        }
    }

    /// Reads the config file again. A broken file is reported and ignored;
    /// a change during a dictation waits until it ends.
    fn reload_config(&mut self) {
        let config = match Config::load(&self.paths.config) {
            Ok(config) => config,
            Err(error) => {
                self.report(&format!(
                    "config.toml has an error: {error:#}. Keeping the current settings"
                ));
                return;
            }
        };
        if config == self.config {
            // The tray toggles write the file too, and an edit may undo a pending one.
            self.pending_config = None;
        } else if self.session.is_some() {
            tracing::info!("Settings changed during a dictation; applying them when it ends");
            self.pending_config = Some(config);
        } else {
            self.apply_config(config);
        }
    }

    fn apply_config(&mut self, mut config: Config) {
        let mut rebind_error = None;
        if config.shortcut != self.config.shortcut {
            let current = self.config.shortcut.parse::<Hotkey>().ok();
            let result = config.shortcut.parse::<Hotkey>().and_then(|shortcut| {
                if current.as_ref() == Some(&shortcut) {
                    Ok(())
                } else {
                    self.ui.rebind(&shortcut)
                }
            });
            if let Err(error) = result {
                rebind_error = Some(format!(
                    "Could not switch the shortcut to {}: {error:#}. Keeping {}",
                    config.shortcut, self.config.shortcut
                ));
                config.shortcut = self.config.shortcut.clone();
            }
        }
        self.config = config;
        self.ui.set_options(
            self.config.cleanup.enabled,
            self.config.sounds,
            self.config.mode == RecordingMode::Hold,
        );
        tracing::info!(shortcut = %self.config.shortcut, mode = ?self.config.mode, "Settings reloaded");
        match rebind_error {
            Some(message) => self.report(&message),
            None => self.ui.status(
                Status::Idle,
                &format!("Settings reloaded · {}", self.config.shortcut),
            ),
        }
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
                        .and_then(|saved| self.ui.copy_text(&saved.text))
                    {
                        Ok(()) => self.ui.status(Status::Idle, "Last dictation copied"),
                        Err(error) => self.report(&error.to_string()),
                    }
                }
            }
            Command::RetryLast => {
                if self.session.is_none()
                    && let Err(error) = self.retry_last()
                {
                    self.report(&format!("{error:#}"));
                }
            }
            Command::TalkReleased if self.config.mode == RecordingMode::Hold => {
                self.stop_recording("Finishing");
            }
            Command::TalkPressed | Command::Toggle => {
                if self.session.is_some() {
                    if command == Command::Toggle || self.config.mode == RecordingMode::Toggle {
                        self.stop_recording("Finishing");
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
            Command::ToggleCleanup | Command::ToggleSounds | Command::ToggleMode => {
                if self.session.is_none() {
                    self.toggle_option(command);
                }
            }
            Command::SetApiKey
            | Command::OpenConfig
            | Command::OpenLogs
            | Command::ToggleAutostart
            | Command::OpenReleases => {
                if self.session.is_none()
                    && let Err(error) = self.menu_action(command)
                {
                    self.report(&format!("{error:#}"));
                }
            }
        }
        true
    }

    /// Runs a tray menu item that has nothing to do with dictation.
    fn menu_action(&mut self, command: Command) -> Result<()> {
        match command {
            Command::SetApiKey => {
                if self.ui.prompt_api_key()? {
                    self.credentials = Credentials::load()?;
                    self.ui.status(Status::Idle, "API key saved");
                    self.check_credits();
                }
            }
            Command::OpenConfig => {
                if !self.paths.config.exists() {
                    self.config.save(&self.paths.config)?;
                }
                self.ui.open_path(&self.paths.config)?;
            }
            Command::OpenLogs => {
                fs::create_dir_all(&self.paths.data)?;
                self.ui.open_path(&self.paths.data)?;
            }
            Command::ToggleAutostart => {
                let result =
                    platform::autostart(!platform::autostart_enabled(), &self.paths.config);
                self.ui.set_autostart(platform::autostart_enabled());
                result?;
            }
            Command::OpenReleases => self
                .ui
                .open_url("https://github.com/howmanysmall/yap/releases/latest")?,
            _ => {}
        }
        Ok(())
    }

    /// Flips a setting from the tray and writes it to the config file.
    fn toggle_option(&mut self, command: Command) {
        let label = match command {
            Command::ToggleCleanup => {
                self.config.cleanup.enabled = !self.config.cleanup.enabled;
                if self.config.cleanup.enabled {
                    "Cleanup on"
                } else {
                    "Cleanup off"
                }
            }
            Command::ToggleSounds => {
                self.config.sounds = !self.config.sounds;
                if self.config.sounds {
                    "Sounds on"
                } else {
                    "Sounds off"
                }
            }
            _ => {
                self.config.mode = match self.config.mode {
                    RecordingMode::Toggle => RecordingMode::Hold,
                    RecordingMode::Hold => RecordingMode::Toggle,
                };
                match self.config.mode {
                    RecordingMode::Hold => "Hold to talk",
                    RecordingMode::Toggle => "Toggle to talk",
                }
            }
        };
        self.ui.set_options(
            self.config.cleanup.enabled,
            self.config.sounds,
            self.config.mode == RecordingMode::Hold,
        );
        match self.config.save(&self.paths.config) {
            Ok(()) => self.ui.status(Status::Idle, label),
            Err(error) => self.report(&format!(
                "{label} until Yap restarts; the config file could not be saved: {error:#}"
            )),
        }
    }

    /// Asks OpenRouter about the key in the background; `drain` announces
    /// the answer.
    pub fn check_credits(&self) {
        let client = self.client.clone();
        let key = self.credentials.openrouter.clone();
        let notices = self.notices_tx.clone();
        self.runtime.spawn(async move {
            let result =
                tokio::time::timeout(Duration::from_secs(5), openrouter::key_info(&client, &key))
                    .await
                    .unwrap_or(Err(ApiError::Timeout));
            let _ = notices.send(Notice::Credits(result));
        });
    }

    /// Shows the remaining credits in the ready line and warns when the
    /// key is rejected or nearly out of money.
    pub fn announce_credits(&mut self, result: Result<KeyInfo, ApiError>) {
        let shortcut = &self.config.shortcut;
        let mut ready = format!("Ready · {shortcut}");
        let mut warning = None;
        match result {
            Ok(info) => {
                if let Some(left) = info.limit_remaining {
                    ready = format!("Ready · ${left:.2} credits · {shortcut}");
                    if left < 1.0 {
                        warning = Some((
                            format!("OpenRouter credits are low: ${left:.2} left"),
                            Cue::Warning,
                        ));
                    }
                }
            }
            Err(ApiError::Unauthorized) => {
                warning = Some((
                    "OpenRouter rejected the API key. Choose Set API key… in the tray".into(),
                    Cue::Error,
                ));
            }
            Err(error) => tracing::warn!(%error, "Could not check OpenRouter credits"),
        }
        // A dictation that started meanwhile keeps its own status.
        if self.session.is_none() {
            self.ui.status(Status::Idle, &ready);
        }
        if let Some((message, cue)) = warning {
            tracing::warn!("{message}");
            self.ui.notify(&message);
            self.cue(cue);
        }
    }

    fn start_session(&mut self) -> Result<Session<U::Target>> {
        self.next_id += 1;
        let id = self.next_id;
        let started = Instant::now();
        let target = self.ui.target()?;
        let (capture, audio) = Capture::start(self.config.microphone.as_deref())?;
        self.ui.cancel_shortcut(true)?;
        let task = self.spawn_transcription(
            id,
            Source::Live {
                audio,
                sample_rate: capture.sample_rate,
            },
        );
        Ok(Session {
            id,
            capture: Some(Box::new(capture)),
            task,
            target,
            started,
            stopped: None,
            incomplete: None,
            ready_announced: false,
            shown_second: None,
            preview: BTreeMap::new(),
        })
    }

    /// Uploads the recording kept by a failed upload, as a session that is
    /// already past recording.
    fn retry_last(&mut self) -> Result<()> {
        let wav = fs::read(self.paths.last_recording())
            .map_err(|_| anyhow!("No failed recording to retry"))?;
        let target = self.ui.target()?;
        self.next_id += 1;
        let id = self.next_id;
        let now = Instant::now();
        let task = self.spawn_transcription(id, Source::Wav(wav));
        self.session = Some(Session {
            id,
            capture: None,
            task,
            target,
            started: now,
            stopped: Some(now),
            incomplete: None,
            ready_announced: true,
            shown_second: None,
            preview: BTreeMap::new(),
        });
        self.ui.status(Status::Processing, "Retrying upload");
        Ok(())
    }

    fn spawn_transcription(&mut self, id: u64, source: Source) -> JoinHandle<()> {
        let credentials = self.credentials.clone();
        let config = self.config.clone();
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let paths = self.paths.clone();
        let tx = self.tx.clone();
        let notices = self.notices_tx.clone();
        self.runtime.spawn(async move {
            // Previews still running finish while the final upload runs and
            // are aborted when this task ends.
            let (wav, _previews) = match source {
                Source::Live { audio, sample_rate } => {
                    let mut previews = config.live_preview.then(|| Previews {
                        id,
                        segmenter: Segmenter::new(sample_rate),
                        sample_rate,
                        sequence: 0,
                        slots: Arc::new(Semaphore::new(PREVIEW_UPLOADS)),
                        uploads: JoinSet::new(),
                        client: client.clone(),
                        config: Arc::new(config.clone()),
                        key: Arc::new(credentials.openrouter.clone()),
                        endpoint: endpoint.as_str().into(),
                        notices,
                    });
                    let pcm = speech::collect_with(
                        audio,
                        sample_rate,
                        config.max_recording_seconds,
                        |frame| {
                            if let Some(previews) = &mut previews {
                                previews.push(frame);
                            }
                        },
                    )
                    .await;
                    // The final upload covers the unfinished last phrase, so
                    // it is never previewed.
                    (speech::wav(pcm, sample_rate), previews)
                }
                Source::Wav(wav) => (wav, None),
            };
            let result = dictate_at(&client, &config, &credentials.openrouter, &wav, &endpoint)
                .await
                .map(|(dictation, _)| dictation);
            // Only failed uploads touch the disk, so a successful dictation
            // never waits on a large synced write and its audio stays in memory.
            let kept = result.is_err()
                && write_private(&paths.last_recording(), &wav)
                    .inspect_err(|error| {
                        tracing::warn!(error = %format!("{error:#}"), "Could not keep the recording for a retry");
                    })
                    .is_ok();
            let _ = tx.send(Completed { id, result, kept });
        })
    }

    /// Reacts to the microphone and the clock: announces readiness, counts
    /// the recording time, stops on a fault, the length limit or a long
    /// silence, and gives up on an upload that never ends.
    pub fn tick(&mut self, now: Instant) {
        self.check_config(now);
        let deadline = self.config.watchdog();
        let limit = Duration::from_secs(self.config.max_recording_seconds);
        let silence_limit = (self.config.mode == RecordingMode::Toggle
            && self.config.silence_timeout_seconds > 0)
            .then(|| Duration::from_secs(self.config.silence_timeout_seconds));
        let Some(current) = &mut self.session else {
            return;
        };
        let mut announce = false;
        let mut label = None;
        let mut stop = None;
        if let Some(capture) = &current.capture {
            let elapsed = now.saturating_duration_since(current.started);
            if !current.ready_announced && capture.is_ready() {
                current.ready_announced = true;
                announce = true;
                tracing::info!(ready_ms = elapsed.as_millis(), "Microphone ready");
            }
            if current.ready_announced && current.shown_second != Some(elapsed.as_secs()) {
                current.shown_second = Some(elapsed.as_secs());
                label = Some(recording_label(elapsed, limit));
            }
            if let Some(fault) = capture.fault() {
                current.incomplete = Some(fault);
                stop = Some("Finishing".to_owned());
            } else if elapsed >= limit {
                stop = Some("Finishing".to_owned());
            } else if let Some(silence) = silence_limit
                && elapsed >= silence
                && capture.silent_for() >= silence
            {
                stop = Some(format!("Stopped after {} s of silence", silence.as_secs()));
            }
        }
        let timed_out = current
            .stopped
            .is_some_and(|t| now.saturating_duration_since(t) > deadline);
        if let Some(label) = label {
            self.ui.status(Status::Recording, &label);
        }
        if announce {
            self.cue(Cue::Start);
        }
        if let Some(text) = stop {
            self.stop_recording(&text);
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
        while let Ok(notice) = self.notices.try_recv() {
            match notice {
                Notice::Credits(credits) => self.announce_credits(credits),
                Notice::Preview { id, sequence, text } => self.show_preview(id, sequence, text),
            }
        }
    }

    /// Adds one preview segment to the current session and shows the
    /// segments so far in order. Segments of an ended session are ignored.
    fn show_preview(&mut self, id: u64, sequence: u32, text: String) {
        let Some(current) = self.session.as_mut().filter(|s| s.id == id) else {
            return;
        };
        current.preview.insert(sequence, text);
        let joined = current
            .preview
            .values()
            .map(|text| text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        self.ui.preview(&joined);
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
            Err(error) if completed.kept => {
                self.report(&format!(
                    "{error}. Retry last recording from the tray or run `yap last --retry`"
                ));
                return;
            }
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
        if let Err(error) = self.paths.discard_last_recording() {
            tracing::warn!(%error, "Could not delete the kept recording");
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
            Err(error) => match self.ui.copy_text(&dictation.text) {
                Ok(()) => self.report(&format!("{error}. The text is on your clipboard")),
                Err(copy_error) => {
                    tracing::warn!(error = %copy_error, "Could not copy the text after a failed paste");
                    self.report(&format!(
                        "{error}. Use Copy last dictation to recover the text"
                    ));
                }
            },
        }
    }

    /// Closes the microphone and lets the upload start, showing `text`. A
    /// recording that never heard speech is dropped instead of uploaded.
    fn stop_recording(&mut self, text: &str) {
        let Some(current) = &mut self.session else {
            return;
        };
        let Some(capture) = current.capture.take() else {
            return;
        };
        if !capture.heard_speech() {
            // Abort first: closing the audio channel is what starts the upload.
            current.task.abort();
            let fault = current.incomplete.take();
            let _ = capture.stop();
            self.session = None;
            self.release_cancel();
            match fault {
                Some(fault) => self.report(&fault),
                None => {
                    self.ui.status(Status::Idle, "No speech detected");
                    self.cue(Cue::Stop);
                }
            }
            return;
        }
        current.stopped = Some(Instant::now());
        if let Err(error) = capture.stop() {
            current.incomplete = Some(error.to_string());
        }
        self.release_cancel();
        self.ui.status(Status::Processing, text);
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
    desktop.set_autostart(platform::autostart_enabled());
    desktop.set_options(
        config.cleanup.enabled,
        config.sounds,
        config.mode == RecordingMode::Hold,
    );
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
    controller.check_credits();
    if let Err(error) = controller.watch_config() {
        tracing::warn!(error = %format!("{error:#}"), "Config changes need a restart");
    }

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
