//! The API key prompt: a modal dialog built from an in-memory template, so
//! the GUI-subsystem executable needs no resource script.

use anyhow::Result;
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::SetFocus,
            WindowsAndMessaging::{
                BS_DEFPUSHBUTTON, BS_PUSHBUTTON, DLGTEMPLATE, DS_CENTER, DS_MODALFRAME, DS_SETFONT,
                DialogBoxIndirectParamW, ES_AUTOHSCROLL, ES_PASSWORD, EndDialog, GetDlgItem,
                GetDlgItemTextW, IDCANCEL, IDOK, MB_ICONERROR, MB_OK, MessageBoxW, WM_COMMAND,
                WM_INITDIALOG, WS_BORDER, WS_CAPTION, WS_CHILD, WS_POPUP, WS_SYSMENU, WS_TABSTOP,
                WS_VISIBLE,
            },
        },
    },
    core::{PCWSTR, w},
};
use zeroize::Zeroizing;

use crate::credentials;

const KEY_ID: u16 = 100;
/// Predefined control classes, written as `0xFFFF, atom` in a template.
const BUTTON: u16 = 0x0080;
const EDIT: u16 = 0x0081;
const STATIC: u16 = 0x0082;

/// Asks for the OpenRouter key and saves it. Returns false when cancelled.
pub fn prompt_api_key() -> Result<bool> {
    let words = template();
    // DialogBoxIndirectParamW needs a DWORD-aligned template.
    let mut aligned = vec![0u32; words.len().div_ceil(2)];
    // SAFETY: `aligned` holds at least `words.len()` u16 values and the two
    // buffers do not overlap.
    unsafe {
        std::ptr::copy_nonoverlapping(
            words.as_ptr(),
            aligned.as_mut_ptr().cast::<u16>(),
            words.len(),
        );
    }
    unsafe {
        let instance = HINSTANCE(GetModuleHandleW(None)?.0);
        let result = DialogBoxIndirectParamW(
            Some(instance),
            aligned.as_ptr().cast::<DLGTEMPLATE>(),
            None,
            Some(dialog_proc),
            LPARAM(0),
        );
        if result == -1 {
            return Err(windows::core::Error::from_thread().into());
        }
        Ok(result == 1)
    }
}

/// A `DLGTEMPLATE` followed by its four `DLGITEMTEMPLATE`s, as UTF-16 words.
fn template() -> Vec<u16> {
    let style =
        (DS_MODALFRAME | DS_CENTER | DS_SETFONT) as u32 | (WS_POPUP | WS_CAPTION | WS_SYSMENU).0;
    let mut words = Vec::new();
    push_u32(&mut words, style);
    push_u32(&mut words, 0);
    // Item count, then x, y, width and height in dialog units.
    words.extend([4, 0, 0, 230, 78]);
    // No menu and the default dialog class.
    words.extend([0, 0]);
    push_text(&mut words, "Yap");
    words.push(9);
    push_text(&mut words, "Segoe UI");
    item(
        &mut words,
        0,
        [7, 7, 216, 20],
        u16::MAX,
        STATIC,
        "Enter your OpenRouter API key. Create one at https://openrouter.ai/settings/keys.",
    );
    item(
        &mut words,
        (ES_PASSWORD | ES_AUTOHSCROLL) as u32 | (WS_BORDER | WS_TABSTOP).0,
        [7, 31, 216, 14],
        KEY_ID,
        EDIT,
        "",
    );
    item(
        &mut words,
        BS_DEFPUSHBUTTON as u32 | WS_TABSTOP.0,
        [119, 56, 50, 14],
        IDOK.0 as u16,
        BUTTON,
        "Save",
    );
    item(
        &mut words,
        BS_PUSHBUTTON as u32 | WS_TABSTOP.0,
        [173, 56, 50, 14],
        IDCANCEL.0 as u16,
        BUTTON,
        "Cancel",
    );
    words
}

fn item(words: &mut Vec<u16>, style: u32, bounds: [i16; 4], id: u16, class: u16, text: &str) {
    if words.len() % 2 == 1 {
        words.push(0);
    }
    push_u32(words, style | (WS_CHILD | WS_VISIBLE).0);
    push_u32(words, 0);
    words.extend(bounds.map(|value| value as u16));
    words.push(id);
    words.extend([0xFFFF, class]);
    push_text(words, text);
    // No creation data.
    words.push(0);
}

fn push_u32(words: &mut Vec<u16>, value: u32) {
    words.extend([value as u16, (value >> 16) as u16]);
}

fn push_text(words: &mut Vec<u16>, text: &str) {
    words.extend(text.encode_utf16().chain(Some(0)));
}

unsafe extern "system" fn dialog_proc(
    dialog: HWND,
    message: u32,
    wparam: WPARAM,
    _lparam: LPARAM,
) -> isize {
    match message {
        WM_INITDIALOG => {
            unsafe {
                if let Ok(field) = GetDlgItem(Some(dialog), KEY_ID as i32) {
                    let _ = SetFocus(Some(field));
                }
            }
            // Zero tells the dialog manager focus was set here.
            0
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            if id == IDOK.0 {
                if save(dialog) {
                    unsafe {
                        let _ = EndDialog(dialog, 1);
                    }
                }
                1
            } else if id == IDCANCEL.0 {
                unsafe {
                    let _ = EndDialog(dialog, 0);
                }
                1
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// Stores the typed key. On failure the dialog stays open with the reason.
fn save(dialog: HWND) -> bool {
    let mut buffer = Zeroizing::new([0u16; 512]);
    let length = unsafe { GetDlgItemTextW(dialog, KEY_ID as i32, &mut buffer[..]) } as usize;
    let key = Zeroizing::new(String::from_utf16_lossy(
        &buffer[..length.min(buffer.len())],
    ));
    match credentials::set("OPENROUTER_API_KEY", &key) {
        Ok(()) => true,
        Err(error) => {
            let message: Vec<u16> = format!("{error:#}").encode_utf16().chain(Some(0)).collect();
            unsafe {
                MessageBoxW(
                    Some(dialog),
                    PCWSTR(message.as_ptr()),
                    w!("Yap"),
                    MB_OK | MB_ICONERROR,
                );
            }
            false
        }
    }
}
