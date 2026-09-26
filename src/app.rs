use std::{
    fs::{self, OpenOptions},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use tokio::task::JoinHandle;

use crate::{
    audio::Capture,
    cleanup,
    config::{Config, RecordingMode},
    credentials::Credentials,
    platform::{self, Command, Cue, Desktop, Status, Target},
    speech::{self, SpeechFailure},
    storage::{Dictation, Paths},
};

struct Session {
    id: u64,
    capture: Option<Capture>,
    task: JoinHandle<()>,
    target: Target,
    started: Instant,
    stopped: Option<Instant>,
    incomplete: Option<String>,
    ready_announced: bool,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Completed {
    id: u64,
    result: Result<Dictation, SpeechFailure>,
}

pub fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("Start background workers")
}

pub fn run(config: Config, paths: Paths, credentials: Credentials) -> Result<()> {
    fs::create_dir_all(&paths.data)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.data.join("instance.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).context("Whisper Bro is already running")?;
    let runtime = runtime()?;
    let mut desktop = Desktop::new(&config.shortcut.parse()?)?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .build()?;
    let (tx, rx) = mpsc::channel::<Completed>();
    let (quit_tx, quit_rx) = mpsc::channel();
    runtime.spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = quit_tx.send(());
    });
    let mut session: Option<Session> = None;
    let mut next_id = 0;
    let mut talk_key = crate::hotkey::PressLatch::default();
    desktop.status(Status::Idle, &format!("Ready · {}", config.shortcut));
    tracing::info!(shortcut = %config.shortcut, mode = ?config.mode, "Dictation ready");

    'running: loop {
        let commands = desktop.poll(Duration::from_millis(15))?;
        if quit_rx.try_recv().is_ok() {
            break;
        }
        for command in commands {
            match command {
                Command::TalkPressed if !talk_key.press() => continue,
                Command::TalkReleased => talk_key.release(),
                _ => {}
            }
            match command {
                Command::Quit => break 'running,
                Command::Cancel => {
                    if session.take().is_some() {
                        desktop.cancel_shortcut(false)?;
                        desktop.status(Status::Idle, "Cancelled");
                        if config.sounds {
                            platform::cue(Cue::Stop);
                        }
                    }
                }
                Command::CopyLast => {
                    if session.is_some() {
                        continue;
                    }
                    match Dictation::load(&paths.recovery())
                        .and_then(|saved| platform::copy_text(&saved.text))
                    {
                        Ok(()) => desktop.status(Status::Idle, "Last dictation copied"),
                        Err(error) => report(&mut desktop, &config, &error.to_string()),
                    }
                }
                Command::TalkReleased if config.mode == RecordingMode::Hold => {
                    if let Some(current) = &mut session {
                        stop(current, &mut desktop, &config);
                    }
                }
                Command::TalkPressed | Command::Toggle => {
                    if let Some(current) = &mut session {
                        if command == Command::Toggle || config.mode == RecordingMode::Toggle {
                            stop(current, &mut desktop, &config);
                        }
                        continue;
                    }
                    next_id += 1;
                    let started = Instant::now();
                    let started_session = (|| -> Result<Session> {
                        let target = desktop.target()?;
                        let (capture, audio) = Capture::start(config.microphone.as_deref())?;
                        desktop.cancel_shortcut(true)?;
                        let sample_rate = capture.sample_rate;
                        let credentials = credentials.clone();
                        let config = config.clone();
                        let client = client.clone();
                        let tx = tx.clone();
                        let id = next_id;
                        let task = runtime.spawn(async move {
                            let result = match speech::transcribe(
                                &config,
                                &credentials.deepgram,
                                sample_rate,
                                audio,
                            )
                            .await
                            {
                                Ok(transcript) => {
                                    let cleanup_started = Instant::now();
                                    let (text, cleanup_status) = cleanup::clean(
                                        &client,
                                        &config.cleanup,
                                        credentials.groq.as_deref().map(|s| s.as_str()),
                                        &transcript.text,
                                    )
                                    .await;
                                    tracing::info!(
                                        connect_ms = transcript.connect_ms,
                                        finalize_ms = transcript.finalize_ms,
                                        cleanup_ms = cleanup_started.elapsed().as_millis(),
                                        cleanup = cleanup_status.label(),
                                        "Transcription finished"
                                    );
                                    Ok(Dictation {
                                        text,
                                        raw: transcript.text,
                                        complete: true,
                                        cleanup: cleanup_status.label().into(),
                                        recorded_at_unix: SystemTime::now()
                                            .duration_since(UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs(),
                                    })
                                }
                                Err(error) => Err(error),
                            };
                            let _ = tx.send(Completed { id, result });
                        });
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
                    })();
                    match started_session {
                        Ok(current) => {
                            desktop
                                .status(Status::Processing, "Opening microphone · Escape cancels");
                            session = Some(current);
                        }
                        Err(error) => report(&mut desktop, &config, &format!("{error:#}")),
                    }
                }
                Command::TalkReleased => {}
            }
        }

        if let Some(current) = &mut session {
            if let Some(capture) = &current.capture {
                if !current.ready_announced && capture.is_ready() {
                    current.ready_announced = true;
                    tracing::info!(
                        ready_ms = current.started.elapsed().as_millis(),
                        "Microphone ready"
                    );
                    desktop.status(Status::Recording, "Recording · Escape cancels");
                    if config.sounds {
                        platform::cue(Cue::Start);
                    }
                }
                if let Some(fault) = capture.fault() {
                    current.incomplete = Some(fault);
                    stop(current, &mut desktop, &config);
                } else if current.started.elapsed().as_secs() >= config.max_recording_seconds {
                    stop(current, &mut desktop, &config);
                }
            }
            if current.stopped.is_some_and(|t| {
                t.elapsed()
                    > Duration::from_millis(
                        config.finalize_timeout_ms + config.cleanup.timeout_ms + 8000,
                    )
            }) {
                session.take();
                desktop.cancel_shortcut(false)?;
                report(
                    &mut desktop,
                    &config,
                    "Dictation timed out. Please check your connection and try again",
                );
            }
        }

        while let Ok(completed) = rx.try_recv() {
            if !session.as_ref().is_some_and(|s| s.id == completed.id) {
                continue;
            }
            let Some(mut current) = session.take() else {
                continue;
            };
            current.capture.take();
            desktop.cancel_shortcut(false)?;
            match completed.result {
                Ok(mut dictation) => {
                    if dictation.text.is_empty() {
                        desktop.status(Status::Idle, "No speech detected");
                        continue;
                    }
                    dictation.complete = current.incomplete.is_none();
                    if let Err(error) = dictation.save(&paths.recovery()) {
                        report(
                            &mut desktop,
                            &config,
                            &format!("Could not save recovery text: {error}"),
                        );
                        continue;
                    }
                    if let Some(reason) = &current.incomplete {
                        report(
                            &mut desktop,
                            &config,
                            &format!("{reason}. Partial text is available in Copy last dictation"),
                        );
                        continue;
                    }
                    match desktop.paste(&current.target, &dictation.text) {
                        Ok(()) => {
                            tracing::info!(
                                stop_to_paste_ms = current.stopped.map(|t| t.elapsed().as_millis()),
                                "Text inserted"
                            );
                            let label = match dictation.cleanup.as_str() {
                                "timed_out" => "Inserted · cleanup timed out",
                                "unavailable" => "Inserted · cleanup unavailable",
                                _ => "Inserted",
                            };
                            desktop.status(Status::Idle, label);
                            if config.sounds {
                                platform::cue(Cue::Done);
                            }
                        }
                        Err(error) => report(
                            &mut desktop,
                            &config,
                            &format!("{error}. Use Copy last dictation to recover the text"),
                        ),
                    }
                }
                Err(error) => {
                    let mut message = error.message;
                    if !error.partial.is_empty() {
                        let partial = Dictation {
                            text: error.partial.clone(),
                            raw: error.partial,
                            complete: false,
                            cleanup: "disabled".into(),
                            recorded_at_unix: SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs(),
                        };
                        match partial.save(&paths.recovery()) {
                            Ok(()) => message
                                .push_str(". Partial text is available in Copy last dictation"),
                            Err(error) => {
                                tracing::error!(%error, "Could not save partial dictation")
                            }
                        }
                    }
                    report(&mut desktop, &config, &message);
                }
            }
        }
    }
    drop(session);
    tracing::info!("Dictation stopped");
    Ok(())
}

fn stop(session: &mut Session, desktop: &mut Desktop, config: &Config) {
    if let Some(capture) = session.capture.take() {
        session.stopped = Some(Instant::now());
        if let Err(error) = capture.stop() {
            session.incomplete = Some(error.to_string());
        }
        desktop.status(Status::Processing, "Finishing · Escape cancels");
        if config.sounds {
            platform::cue(Cue::Stop);
        }
    }
}

fn report(desktop: &mut Desktop, config: &Config, message: &str) {
    tracing::error!("{message}");
    desktop.status(Status::Error, message);
    desktop.notify(message);
    if config.sounds {
        platform::cue(Cue::Error);
    }
}
