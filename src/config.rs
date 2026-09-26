use std::{fs, path::Path};

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
    pub finalize_timeout_ms: u64,
    pub cleanup: CleanupConfig,
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
            finalize_timeout_ms: 4000,
            cleanup: CleanupConfig::default(),
        }
    }
}

impl Default for CleanupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model: "openai/gpt-oss-20b".into(),
            timeout_ms: 1200,
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
        ensure!(!self.language.trim().is_empty(), "language cannot be empty");
        ensure!(
            (1..=3600).contains(&self.max_recording_seconds),
            "max_recording_seconds must be 1–3600"
        );
        ensure!(
            (100..=30_000).contains(&self.finalize_timeout_ms),
            "finalize_timeout_ms must be 100–30000"
        );
        ensure!(
            (100..=10_000).contains(&self.cleanup.timeout_ms),
            "cleanup.timeout_ms must be 100–10000"
        );
        ensure!(
            !self.cleanup.model.trim().is_empty(),
            "cleanup.model cannot be empty"
        );
        ensure!(
            self.vocabulary.len() <= 100,
            "Keep vocabulary to at most 100 terms (the service also limits it to 500 tokens)"
        );
        ensure!(
            self.vocabulary.iter().all(|term| !term.trim().is_empty()),
            "Vocabulary terms cannot be blank"
        );
        Ok(())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        write_private(path, toml::to_string_pretty(self)?.as_bytes())
    }
}
