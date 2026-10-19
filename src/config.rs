use std::{fs, path::Path, time::Duration};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{hotkey::Hotkey, storage::write_private};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub shortcut: String,
    pub mode: RecordingMode,
    pub microphone: Option<String>,
    pub sounds: bool,
    pub language: String,
    pub vocabulary: Vec<String>,
    pub max_recording_seconds: u64,
    /// In toggle mode, stop after this many seconds without speech; 0 never stops.
    pub silence_timeout_seconds: u64,
    /// While recording, transcribe each phrase as it ends and show the text so far.
    pub live_preview: bool,
    /// Unused since the streaming path was removed; accepted so config files written by older `setup` runs still load.
    #[serde(skip_serializing)]
    pub finalize_timeout_ms: serde::de::IgnoredAny,
    pub cleanup: CleanupConfig,
    pub transcription_model: String,
    pub transcription_timeout_ms: u64,
    pub paste_restore_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    #[default]
    Toggle,
    Hold,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CleanupConfig {
    pub enabled: bool,
    pub model: String,
    pub timeout_ms: u64,
    /// Extra guidance for the cleanup model, appended to the built-in prompt.
    pub instructions: Option<String>,
    /// Use `instructions` instead of the built-in prompt.
    pub replace_instructions: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shortcut: "Ctrl+Alt+Space".into(),
            mode: RecordingMode::Toggle,
            microphone: None,
            sounds: true,
            language: "en".into(),
            vocabulary: Vec::new(),
            max_recording_seconds: 600,
            silence_timeout_seconds: 30,
            live_preview: true,
            finalize_timeout_ms: serde::de::IgnoredAny,
            cleanup: CleanupConfig::default(),
            transcription_model: "openai/whisper-large-v3-turbo".into(),
            transcription_timeout_ms: 60_000,
            paste_restore_ms: 200,
        }
    }
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: "openai/gpt-oss-20b".into(),
            timeout_ms: 8000,
            instructions: None,
            replace_instructions: false,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let config = match fs::read_to_string(path) {
            Ok(contents) => toml::from_str::<Self>(&contents)
                .with_context(|| format!("Read configuration at {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(error.into()),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        let hotkey: Hotkey = self.shortcut.parse()?;
        ensure!(
            hotkey.key != crate::hotkey::Key::Escape,
            "Escape is reserved for cancel"
        );
        ensure!(
            !self.transcription_model.trim().is_empty(),
            "transcription_model cannot be empty"
        );
        ensure!(
            (1000..=120_000).contains(&self.transcription_timeout_ms),
            "transcription_timeout_ms must be 1000–120000"
        );
        ensure!(
            (50..=5000).contains(&self.paste_restore_ms),
            "paste_restore_ms must be 50–5000"
        );
        ensure!(
            (1..=3600).contains(&self.max_recording_seconds),
            "max_recording_seconds must be 1–3600"
        );
        ensure!(
            self.silence_timeout_seconds <= 600,
            "silence_timeout_seconds must be 0–600"
        );
        ensure!(
            (100..=60_000).contains(&self.cleanup.timeout_ms),
            "cleanup.timeout_ms must be 100–60000"
        );
        ensure!(
            self.cleanup
                .instructions
                .as_ref()
                .is_none_or(|text| !text.trim().is_empty()),
            "cleanup.instructions cannot be blank; remove it to use the built-in prompt"
        );
        ensure!(
            !self.cleanup.model.trim().is_empty(),
            "cleanup.model cannot be empty"
        );
        ensure!(
            self.vocabulary.len() <= 100,
            "Keep vocabulary to at most 100 terms; the transcription provider truncates longer prompts"
        );
        ensure!(
            self.vocabulary.iter().all(|term| !term.trim().is_empty()),
            "Vocabulary terms cannot be blank"
        );
        Ok(())
    }

    /// How long after recording stops the app waits for a result before
    /// giving up: two transcription attempts, cleanup, and slack for the
    /// retry pause and the paste.
    pub fn watchdog(&self) -> Duration {
        Duration::from_millis(2 * self.transcription_timeout_ms + self.cleanup.timeout_ms + 8000)
    }

    /// The language hint for transcription, or `None` to let the model detect it.
    pub fn language_code(&self) -> Option<&str> {
        let language = self.language.trim();
        (!language.is_empty() && !language.eq_ignore_ascii_case("auto")).then_some(language)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        write_private(path, toml::to_string_pretty(self)?.as_bytes())
    }
}
