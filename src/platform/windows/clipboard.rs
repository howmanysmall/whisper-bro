use std::time::Duration;

use anyhow::{Context, Result, ensure};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{DataExchange::*, Memory::*, Ole::*},
        UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
    core::w,
};

pub struct Ole;

impl Ole {
    pub fn new() -> Result<Self> {
        unsafe {
            OleInitialize(None)?;
        }
        Ok(Self)
    }
}

impl Drop for Ole {
    fn drop(&mut self) {
        unsafe {
            OleUninitialize();
        }
    }
}

struct OpenClipboardGuard;
impl Drop for OpenClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

pub fn copy_text(text: &str) -> Result<()> {
    let _ole = Ole::new()?;
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!(""),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )?
    };
    let result = write_text(Some(window), text, None);
    unsafe {
        let _ = DestroyWindow(window);
    }
    result
}

fn write_text(owner: Option<HWND>, text: &str, expected_sequence: Option<u32>) -> Result<()> {
    let units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        OpenClipboard(owner).context("Another app is holding the clipboard")?;
        let _clipboard = OpenClipboardGuard;
        if let Some(expected) = expected_sequence {
            ensure!(
                GetClipboardSequenceNumber() == expected,
                "The clipboard changed while preparing to paste"
            );
        }
        let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2)?;
        let pointer = GlobalLock(memory).cast::<u16>();
        if pointer.is_null() {
            let _ = GlobalFree(Some(memory));
            anyhow::bail!("Could not allocate clipboard data");
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), pointer, units.len());
        let _ = GlobalUnlock(memory);
        if let Err(error) = EmptyClipboard().and_then(|()| {
            SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(memory.0))).map(|_| ())
        }) {
            let _ = GlobalFree(Some(memory));
            return Err(error.into());
        }
    }
    Ok(())
}

pub fn paste(owner: HWND, text: &str, check_target: impl FnOnce() -> Result<()>) -> Result<()> {
    let (initial_sequence, mut previous) = snapshot(owner)?;
    write_text(Some(owner), text, Some(initial_sequence))?;
    let sequence = unsafe { GetClipboardSequenceNumber() };
    let inserted = check_target().and_then(|()| paste_key());
    std::thread::sleep(Duration::from_millis(200));
    unsafe {
        if GetClipboardSequenceNumber() == sequence
            && let Err(error) = restore(owner, sequence, &mut previous)
        {
            tracing::warn!(%error, "Text was inserted, but the previous clipboard could not be restored");
        }
    }
    inserted
}

struct Format {
    kind: u32,
    handle: HANDLE,
}

impl Drop for Format {
    fn drop(&mut self) {
        if self.handle.is_invalid() {
            return;
        }
        unsafe {
            match CLIPBOARD_FORMAT(self.kind as u16) {
                CF_BITMAP | CF_PALETTE => {
                    let _ = DeleteObject(HGDIOBJ(self.handle.0));
                }
                CF_ENHMETAFILE => {
                    let _ = DeleteEnhMetaFile(Some(HENHMETAFILE(self.handle.0)));
                }
                CF_METAFILEPICT => {
                    let memory = HGLOBAL(self.handle.0);
                    let pointer = GlobalLock(memory).cast::<METAFILEPICT>();
                    if !pointer.is_null() {
                        let _ = DeleteMetaFile((*pointer).hMF);
                        let _ = GlobalUnlock(memory);
                    }
                    let _ = GlobalFree(Some(memory));
                }
                _ => {
                    let _ = GlobalFree(Some(HGLOBAL(self.handle.0)));
                }
            }
        }
    }
}

fn snapshot(owner: HWND) -> Result<(u32, Vec<Format>)> {
    unsafe {
        OpenClipboard(Some(owner)).context("Another app is holding the clipboard")?;
        let _clipboard = OpenClipboardGuard;
        let mut formats = Vec::new();
        let mut kind = 0;
        loop {
            SetLastError(ERROR_SUCCESS);
            kind = EnumClipboardFormats(kind);
            if kind == 0 {
                ensure!(
                    GetLastError() == ERROR_SUCCESS,
                    "Could not enumerate clipboard formats"
                );
                break;
            }
            let source = GetClipboardData(kind)
                .context("Could not read a clipboard format to preserve it")?;
            let copy = if kind == CF_ENHMETAFILE.0 as u32 {
                HANDLE(CopyEnhMetaFileW(HENHMETAFILE(source.0), None).0)
            } else {
                OleDuplicateData(source, CLIPBOARD_FORMAT(kind as u16), GMEM_MOVEABLE)
            };
            ensure!(!copy.is_invalid(), "Could not preserve a clipboard format");
            formats.push(Format { kind, handle: copy });
        }
        Ok((GetClipboardSequenceNumber(), formats))
    }
}

fn restore(owner: HWND, sequence: u32, formats: &mut [Format]) -> Result<()> {
    unsafe {
        OpenClipboard(Some(owner))?;
        let _clipboard = OpenClipboardGuard;
        if GetClipboardSequenceNumber() != sequence {
            return Ok(());
        }
        EmptyClipboard()?;
        for format in formats {
            SetClipboardData(format.kind, Some(format.handle))?;
            format.handle = HANDLE::default();
        }
    }
    Ok(())
}

fn key(code: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: code,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                ..Default::default()
            },
        },
    }
}

fn paste_key() -> Result<()> {
    let keys = [
        key(VK_CONTROL, false),
        key(VK_V, false),
        key(VK_V, true),
        key(VK_CONTROL, true),
    ];
    let sent = unsafe { SendInput(&keys, std::mem::size_of::<INPUT>() as i32) };
    if sent != keys.len() as u32 {
        let releases = [key(VK_V, true), key(VK_CONTROL, true)];
        unsafe {
            SendInput(&releases, std::mem::size_of::<INPUT>() as i32);
        }
    }
    ensure!(
        sent == keys.len() as u32,
        "Windows blocked text insertion (the target may be running as administrator)"
    );
    Ok(())
}
