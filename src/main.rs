#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{
    fs::{self, OpenOptions},
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use whisper_bro::{
    app, audio, cleanup,
    config::Config,
    credentials::{self, Credentials},
    platform, speech,
    storage::{Dictation, Paths},
};
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(
    version,
    about = "Voice typing anywhere. A native background utility for Windows and macOS."
)]
struct Cli {
    #[arg(long, global = true, help = "Use a specific TOML configuration file")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the tray/menu-bar utility (the default command).
    Run,
    /// Configure API keys in the OS credential store and create config.toml.
    Setup,
    /// List microphone names for the microphone config option.
    Devices,
    /// Print the config path, then the effective configuration.
    Config,
    /// Recover the last dictation. Incomplete results are labelled on stderr.
    Last {
        #[arg(long)]
        copy: bool,
        #[arg(long)]
        raw: bool,
    },
    /// Transcribe a WAV file without inserting text into another app.
    Transcribe {
        file: PathBuf,
        #[arg(
            long,
            help = "Send audio at recording speed to measure finalization latency"
        )]
        realtime: bool,
        #[arg(long)]
        no_cleanup: bool,
    },
    /// Enable or disable starting the utility at the next login.
    Autostart {
        #[arg(value_parser = ["enable", "disable"])]
        action: String,
    },
    /// Show configuration, credential availability, and OS permission status.
    Doctor,
}

fn main() {
    platform::attach_console();
    let cli = Cli::parse();
    let background = matches!(cli.command, None | Some(Command::Run));
    if let Err(error) = execute(cli) {
        eprintln!("whisper-bro: {error:#}");
        if background {
            platform::startup_error(&format!("{error:#}"));
        }
        std::process::exit(1);
    }
}

fn execute(cli: Cli) -> Result<()> {
    let paths = Paths::discover(cli.config)?;
    let mut config = Config::load(&paths.config)?;
    match cli.command.unwrap_or(Command::Run) {
        Command::Setup => setup(&mut config, &paths),
        Command::Devices => {
            for name in audio::devices()? {
                println!("{name}");
            }
            Ok(())
        }
        Command::Config => {
            println!(
                "# {}\n{}",
                paths.config.display(),
                toml::to_string_pretty(&config)?
            );
            Ok(())
        }
        Command::Last { copy, raw } => {
            let saved = Dictation::load(&paths.recovery())?;
            if !saved.complete {
                eprintln!("This dictation is incomplete; recording or transcription failed.");
            }
            let text = if raw { saved.raw } else { saved.text };
            if copy {
                platform::copy_text(&text)?;
                eprintln!("Copied last dictation.");
            } else {
                println!("{text}");
            }
            Ok(())
        }
        Command::Autostart { action } => {
            platform::autostart(action == "enable", &paths.config)?;
            println!(
                "Login startup {}d. Changes apply at the next login.",
                action
            );
            Ok(())
        }
        Command::Doctor => {
            println!("Configuration: {}", paths.config.display());
            println!("Recovery and logs: {}", paths.data.display());
            println!(
                "OPENROUTER_API_KEY: {}",
                if credentials::get("OPENROUTER_API_KEY")?.is_some() {
                    "available"
                } else {
                    "not configured"
                }
            );
            println!(
                "Transcription: {} (upload after stop)",
                config.transcription_model
            );
            println!("Shortcut: {} ({:?})", config.shortcut, config.mode);
            println!("Cleanup: {}", config.cleanup.enabled);
            println!("{}", platform::permission_status());
            Ok(())
        }
        Command::Transcribe {
            file,
            realtime,
            no_cleanup,
        } => {
            config.cleanup.enabled &= !no_cleanup;
            let credentials = Credentials::load()?;
            app::runtime()?.block_on(transcribe_file(config, credentials, file, realtime))
        }
        Command::Run => {
            let credentials = Credentials::load()?;
            init_log(&paths)?;
            app::run(config, paths, credentials)
        }
    }
}

fn setup(config: &mut Config, paths: &Paths) -> Result<()> {
    platform::setup_console();
    ensure!(
        io::stdin().is_terminal(),
        "Run setup in a terminal. For automation, set OPENROUTER_API_KEY and use `whisper-bro config` to inspect defaults"
    );
    eprintln!(
        "Transcription and optional cleanup use OpenRouter. Your key is stored in the OS credential store."
    );
    let key = Zeroizing::new(rpassword::prompt_password(
        "OpenRouter API key (Enter keeps existing): ",
    )?);
    if !key.trim().is_empty() {
        credentials::set("OPENROUTER_API_KEY", &key)?;
    }
    ensure!(
        credentials::get("OPENROUTER_API_KEY")?.is_some(),
        "An OpenRouter API key is required: https://openrouter.ai/settings/keys"
    );
    eprint!("Enable text cleanup? [y/N]: ");
    io::stderr().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    config.cleanup.enabled = matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes");
    config.save(&paths.config)?;
    println!(
        "Saved {}\nShortcut: {} ({:?})\nCleanup: {}\nRun `whisper-bro run` to start. Enable login startup with `whisper-bro autostart enable`.",
        paths.config.display(),
        config.shortcut,
        config.mode,
        config.cleanup.enabled
    );
    Ok(())
}

fn init_log(paths: &Paths) -> Result<()> {
    fs::create_dir_all(&paths.data)?;
    let path = paths.data.join("whisper-bro.log");
    if fs::metadata(&path).is_ok_and(|m| m.len() > 2_000_000) {
        let previous = paths.data.join("whisper-bro.previous.log");
        if previous.exists() {
            fs::remove_file(&previous)?;
        }
        fs::rename(&path, previous)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(Mutex::new(file))
        .init();
    Ok(())
}

async fn transcribe_file(
    config: Config,
    credentials: Credentials,
    path: PathBuf,
    realtime: bool,
) -> Result<()> {
    let mut reader = hound::WavReader::open(path).context("Open WAV file")?;
    let spec = reader.spec();
    ensure!(
        spec.channels > 0 && spec.sample_rate >= 8000,
        "WAV must have channels and a sample rate of at least 8000 Hz"
    );
    ensure!(
        reader.duration() as u64 <= spec.sample_rate as u64 * config.max_recording_seconds,
        "WAV exceeds max_recording_seconds"
    );
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => reader.samples::<f32>().collect::<Result<_, _>>()?,
        (hound::SampleFormat::Int, bits @ 8..=32) => reader
            .samples::<i32>()
            .map(|value| value.map(|v| v as f32 / (1u64 << (bits - 1)) as f32))
            .collect::<Result<_, _>>()?,
        _ => anyhow::bail!("Use a PCM integer or float32 WAV file"),
    };
    ensure!(!samples.is_empty(), "WAV contains no audio");
    let (tx, rx) = tokio::sync::mpsc::channel(audio::BUFFER_FRAMES);
    let producer = tokio::spawn(async move {
        let (frame_tx, mut frame_rx) = tokio::sync::mpsc::channel(2);
        let mut encoder = audio::Encoder::new(spec.channels as usize, spec.sample_rate, frame_tx);
        let chunk = (spec.sample_rate as usize / 50).max(1) * spec.channels as usize;
        let start = tokio::time::Instant::now();
        for (index, samples) in samples.chunks(chunk).enumerate() {
            if realtime {
                tokio::time::sleep_until(start + Duration::from_millis(index as u64 * 20)).await;
            }
            encoder.push(samples);
            encoder.flush();
            while let Ok(frame) = frame_rx.try_recv() {
                if tx.send(frame).await.is_err() {
                    return Instant::now();
                }
            }
        }
        Instant::now()
    });
    let result = speech::transcribe(&config, &credentials.openrouter, spec.sample_rate, rx).await;
    if result.is_err() {
        producer.abort();
    }
    let transcript = result?;
    let stopped = producer.await?;
    let client = reqwest::Client::new();
    let (text, status) = cleanup::clean(
        &client,
        &config.cleanup,
        Some(credentials.openrouter.as_str()),
        &transcript.text,
    )
    .await;
    writeln!(io::stdout(), "{text}")?;
    eprintln!(
        "connect={}ms finalize={}ms stop_to_result={}ms cleanup={}",
        transcript.connect_ms,
        transcript.finalize_ms,
        stopped.elapsed().as_millis(),
        status.label()
    );
    Ok(())
}
