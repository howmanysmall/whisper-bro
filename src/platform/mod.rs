#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::*;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
compile_error!("whisper-bro currently supports Windows and macOS");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    TalkPressed,
    TalkReleased,
    Toggle,
    Cancel,
    CopyLast,
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
}
