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
        let dirs = ProjectDirs::from("dev", "whisper-bro", "whisper-bro")
            .context("Cannot locate the user configuration directory")?;
        Ok(Self {
            config: config.unwrap_or_else(|| dirs.config_dir().join("config.toml")),
            data: dirs.data_local_dir().to_path_buf(),
        })
    }

    pub fn recovery(&self) -> PathBuf {
        self.data.join("last-dictation.json")
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
