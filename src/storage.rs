use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
}

impl Paths {
    pub fn discover(config: Option<PathBuf>) -> Result<Self> {
        let dirs = ProjectDirs::from("dev", "yap", "yap")
            .context("Cannot locate the user configuration directory")?;
        let migrate = config.is_none();
        let paths = Self {
            config: config.unwrap_or_else(|| dirs.config_dir().join("config.toml")),
            data: dirs.data_local_dir().to_path_buf(),
        };
        if migrate
            && let Some(old) = ProjectDirs::from("dev", "whisper-bro", "whisper-bro")
            && let Err(error) = paths.migrate_from(&Self {
                config: old.config_dir().join("config.toml"),
                data: old.data_local_dir().to_path_buf(),
            })
        {
            tracing::warn!(%error, "Could not carry over settings from Whisper Bro");
        }
        Ok(paths)
    }

    /// Copies the config and last dictation from `old` when the matching file
    /// here does not exist yet. Existing files are never overwritten.
    pub fn migrate_from(&self, old: &Self) -> Result<()> {
        for (from, to) in [
            (&old.config, &self.config),
            (&old.recovery(), &self.recovery()),
        ] {
            if to.exists() || !from.is_file() {
                continue;
            }
            let contents = fs::read(from).with_context(|| format!("Read {}", from.display()))?;
            write_private(to, &contents)?;
        }
        Ok(())
    }

    /// Takes the single-instance lock. `Ok(None)` means another instance holds it;
    /// the lock is released when the returned file is dropped.
    pub fn instance_lock(&self) -> Result<Option<fs::File>> {
        fs::create_dir_all(&self.data)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.data.join("instance.lock"))?;
        match lock.try_lock() {
            Ok(()) => Ok(Some(lock)),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }

    pub fn recovery(&self) -> PathBuf {
        self.data.join("last-dictation.json")
    }

    /// The WAV of the last recording whose upload failed, kept for a retry.
    pub fn last_recording(&self) -> PathBuf {
        self.data.join("last-recording.wav")
    }

    /// Deletes the kept recording, if there is one.
    pub fn discard_last_recording(&self) -> std::io::Result<()> {
        match fs::remove_file(self.last_recording()) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Dictation {
    pub text: String,
    pub raw: String,
    pub complete: bool,
    pub cleanup: String,
    pub recorded_at_unix: u64,
}

impl Dictation {
    pub fn save(&self, path: &Path) -> Result<()> {
        write_private(path, &serde_json::to_vec_pretty(self)?)
    }

    pub fn load(path: &Path) -> Result<Self> {
        serde_json::from_slice(&fs::read(path).context("No saved dictation yet")?)
            .context("Read saved dictation")
    }
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;

pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(contents)?;
    file.as_file().sync_all()?;
    file.persist(path)
        .with_context(|| format!("Save {}", path.display()))?;
    Ok(())
}
