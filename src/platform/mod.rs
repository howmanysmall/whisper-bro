use std::path::{Component, Path, PathBuf};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
compile_error!("yap currently supports Windows and macOS");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    TalkPressed,
    TalkReleased,
    Toggle,
    Cancel,
    CopyLast,
    RetryLast,
    SetApiKey,
    OpenConfig,
    OpenLogs,
    ToggleCleanup,
    ToggleSounds,
    ToggleMode,
    ToggleAutostart,
    OpenReleases,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Recording,
    Processing,
    Error,
}

#[derive(Clone, Copy)]
pub enum Cue {
    Start,
    Stop,
    Done,
    Error,
    Warning,
}

/// The program a login item should launch. A mise install lives in a
/// versioned directory that the next `mise up` deletes, so a mise-managed
/// binary is replaced by mise's shim when that shim exists.
pub fn autostart_executable(
    current: &Path,
    mise_data_dir: Option<&Path>,
    shim_exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let mise_install = current
        .components()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|pair| {
            pair[0] == Component::Normal("mise".as_ref())
                && pair[1] == Component::Normal("installs".as_ref())
        });
    if mise_install && let Some(data) = mise_data_dir {
        let shim = data
            .join("shims")
            .join(format!("yap{}", std::env::consts::EXE_SUFFIX));
        if shim_exists(&shim) {
            return shim;
        }
    }
    current.to_path_buf()
}
