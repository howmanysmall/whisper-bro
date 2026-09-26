mod clipboard;

use std::{
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use windows::{
    Win32::{
        Foundation::*,
        System::{
            Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
            Console::*,
            Diagnostics::Debug::MessageBeep,
            LibraryLoader::GetModuleHandleW,
            Registry::*,
        },
        UI::{
            Accessibility::{CUIAutomation8, IUIAutomation2, IUIAutomationElement},
            Input::KeyboardAndMouse::*,
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};

use crate::{
    hotkey::{Hotkey, Key},
    platform::{Command, Cue, Status},
};
pub use clipboard::copy_text;

const TRAY_MESSAGE: u32 = WM_APP + 1;
const TALK_ID: i32 = 1;
const CANCEL_ID: i32 = 2;

pub struct Desktop {
    window: HWND,
    icon: NOTIFYICONDATAW,
    receiver: Receiver<Command>,
    _sender: Box<Sender<Command>>,
    automation: IUIAutomation2,
    cancel: bool,
    held: bool,
    key: i32,
    taskbar_created: u32,
    _ole: clipboard::Ole,
}

pub struct Target {
    window: HWND,
    focus: HWND,
    element: IUIAutomationElement,
}

impl Desktop {
    pub fn new(shortcut: &Hotkey) -> Result<Self> {
        let ole = clipboard::Ole::new()?;
        let (sender, receiver) = mpsc::channel();
        let sender = Box::new(sender);
        unsafe {
            let automation: IUIAutomation2 =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
            automation.SetConnectionTimeout(500)?;
            automation.SetTransactionTimeout(500)?;
            let instance = HINSTANCE(GetModuleHandleW(None)?.0);
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: w!("WhisperBro"),
                ..Default::default()
            };
            ensure!(
                RegisterClassW(&class) != 0,
                "Could not register the background window"
            );
            let window = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("WhisperBro"),
                w!("Whisper Bro"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )?;
            SetWindowLongPtrW(
                window,
                GWLP_USERDATA,
                (&*sender as *const Sender<Command>) as isize,
            );
            let icon = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: window,
                uID: 1,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: TRAY_MESSAGE,
                hIcon: LoadIconW(None, IDI_INFORMATION)?,
                ..Default::default()
            };
            let mut desktop = Self {
                window,
                icon,
                receiver,
                _sender: sender,
                _ole: ole,
                automation,
                cancel: false,
                held: false,
                key: key_code(shortcut.key) as i32,
                taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            };
            let modifiers = MOD_NOREPEAT
                | if shortcut.control {
                    MOD_CONTROL
                } else {
                    HOT_KEY_MODIFIERS(0)
                }
                | if shortcut.alt {
                    MOD_ALT
                } else {
                    HOT_KEY_MODIFIERS(0)
                }
                | if shortcut.shift {
                    MOD_SHIFT
                } else {
                    HOT_KEY_MODIFIERS(0)
                }
                | if shortcut.command {
                    MOD_WIN
                } else {
                    HOT_KEY_MODIFIERS(0)
                };
            RegisterHotKey(Some(window), TALK_ID, modifiers, key_code(shortcut.key))
                .context("Shortcut is already in use. Change shortcut in config.toml")?;
            wide_into("Whisper Bro", &mut desktop.icon.szTip);
            ensure!(
                Shell_NotifyIconW(NIM_ADD, &desktop.icon).as_bool(),
                "Could not create the tray icon"
            );
            Ok(desktop)
        }
    }

    pub fn poll(&mut self, wait: Duration) -> Result<Vec<Command>> {
        let mut commands = Vec::new();
        unsafe {
            MsgWaitForMultipleObjectsEx(
                None,
                wait.as_millis() as u32,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            );
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if message.message == self.taskbar_created {
                    let _ = Shell_NotifyIconW(NIM_ADD, &self.icon);
                }
                if message.message == WM_QUIT {
                    commands.push(Command::Quit);
                }
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            for command in self.receiver.try_iter() {
                if command == Command::TalkPressed {
                    self.held = true;
                }
                commands.push(command);
            }
            if self.held && GetAsyncKeyState(self.key) >= 0 {
                self.held = false;
                commands.push(Command::TalkReleased);
            }
        }
        Ok(commands)
    }

    pub fn cancel_shortcut(&mut self, enabled: bool) -> Result<()> {
        if enabled && !self.cancel {
            unsafe {
                RegisterHotKey(
                    Some(self.window),
                    CANCEL_ID,
                    MOD_NOREPEAT,
                    VK_ESCAPE.0 as u32,
                )?;
            }
            self.cancel = true;
        } else if !enabled && self.cancel {
            unsafe {
                UnregisterHotKey(Some(self.window), CANCEL_ID)?;
            }
            self.cancel = false;
        }
        Ok(())
    }

    pub fn status(&mut self, status: Status, text: &str) {
        let label = match status {
            Status::Idle => "Ready",
            Status::Recording => "RECORDING",
            Status::Processing => "Finishing",
            Status::Error => "Error",
        };
        wide_into(
            &format!("Whisper Bro · {label}\n{text}"),
            &mut self.icon.szTip,
        );
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &self.icon);
        }
    }

    pub fn notify(&mut self, message: &str) {
        let mut notification = self.icon;
        notification.uFlags = NIF_INFO;
        notification.dwInfoFlags = NIIF_WARNING;
        wide_into("Whisper Bro", &mut notification.szInfoTitle);
        wide_into(message, &mut notification.szInfo);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &notification);
        }
    }

    pub fn target(&self) -> Result<Target> {
        target(&self.automation)
    }

    pub fn paste(&mut self, destination: &Target, text: &str) -> Result<()> {
        let started = Instant::now();
        while modifiers_down() {
            ensure!(
                started.elapsed() < Duration::from_secs(1),
                "Release modifier keys before inserting dictation"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        self.check_target(destination)?;
        clipboard::paste(self.window, text, || self.check_target(destination))
    }

    fn check_target(&self, destination: &Target) -> Result<()> {
        let current = self.target()?;
        ensure!(
            current.window == destination.window
                && current.focus == destination.focus
                && unsafe {
                    self.automation
                        .CompareElements(&current.element, &destination.element)?
                }
                .as_bool(),
            "The destination changed while dictating"
        );
        Ok(())
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.icon);
            let _ = UnregisterHotKey(Some(self.window), TALK_ID);
            if self.cancel {
                let _ = UnregisterHotKey(Some(self.window), CANCEL_ID);
            }
            SetWindowLongPtrW(self.window, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.window);
        }
    }
}

fn target(automation: &IUIAutomation2) -> Result<Target> {
    unsafe {
        let window = GetForegroundWindow();
        ensure!(!window.is_invalid(), "No foreground application");
        let mut pid = 0;
        let thread = GetWindowThreadProcessId(window, Some(&mut pid));
        ensure!(
            pid != std::process::id(),
            "Click the destination text field first"
        );
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        GetGUIThreadInfo(thread, &mut info)?;
        Ok(Target {
            window,
            focus: info.hwndFocus,
            element: automation
                .GetFocusedElement()
                .context("Could not identify the focused text field")?,
        })
    }
}

fn modifiers_down() -> bool {
    [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
        .iter()
        .any(|key| unsafe { GetAsyncKeyState(key.0 as i32) < 0 })
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Sender<Command>;
    if !pointer.is_null() {
        let command = match message {
            WM_HOTKEY if wparam.0 == TALK_ID as usize => Some(Command::TalkPressed),
            WM_HOTKEY if wparam.0 == CANCEL_ID as usize => Some(Command::Cancel),
            TRAY_MESSAGE if matches!(lparam.0 as u32, WM_RBUTTONUP | WM_LBUTTONUP) => {
                popup(window).ok().flatten()
            }
            WM_CLOSE => Some(Command::Quit),
            _ => None,
        };
        if let Some(command) = command {
            let _ = unsafe { &*pointer }.send(command);
            return LRESULT(0);
        }
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn popup(window: HWND) -> Result<Option<Command>> {
    unsafe {
        let previous = GetForegroundWindow();
        let menu = CreatePopupMenu()?;
        let result = (|| -> Result<Option<Command>> {
            AppendMenuW(menu, MF_STRING, 1, w!("Start / stop recording"))?;
            AppendMenuW(menu, MF_STRING, 2, w!("Cancel dictation"))?;
            AppendMenuW(menu, MF_STRING, 3, w!("Copy last dictation"))?;
            AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
            AppendMenuW(menu, MF_STRING, 4, w!("Quit Whisper Bro"))?;
            let mut position = POINT::default();
            GetCursorPos(&mut position)?;
            let _ = SetForegroundWindow(window);
            let choice = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY,
                position.x,
                position.y,
                None,
                window,
                None,
            )
            .0;
            let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
            if !previous.is_invalid() {
                let _ = SetForegroundWindow(previous);
            }
            Ok(match choice {
                1 => Some(Command::Toggle),
                2 => Some(Command::Cancel),
                3 => Some(Command::CopyLast),
                4 => Some(Command::Quit),
                _ => None,
            })
        })();
        let _ = DestroyMenu(menu);
        result
    }
}

fn key_code(key: Key) -> u32 {
    match key {
        Key::Letter(letter) => letter as u32,
        Key::Digit(number) => b'0' as u32 + number as u32,
        Key::Space => VK_SPACE.0 as u32,
        Key::Escape => VK_ESCAPE.0 as u32,
        Key::Backspace => VK_BACK.0 as u32,
        Key::Function(number) => VK_F1.0 as u32 + number as u32 - 1,
    }
}

fn wide_into<const N: usize>(text: &str, destination: &mut [u16; N]) {
    destination.fill(0);
    for (out, unit) in destination
        .iter_mut()
        .take(N.saturating_sub(1))
        .zip(text.encode_utf16())
    {
        *out = unit;
    }
}

pub fn cue(cue: Cue) {
    unsafe {
        let _ = MessageBeep(match cue {
            Cue::Error => MB_ICONWARNING,
            _ => MB_OK,
        });
    }
}

pub fn attach_console() {
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
pub fn setup_console() {
    unsafe {
        let _ = AllocConsole();
    }
}

pub fn startup_error(message: &str) {
    let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            w!("Whisper Bro couldn't start"),
            MB_OK | MB_ICONERROR,
        );
    }
}

pub fn permission_status() -> String {
    "Enable microphone access for desktop apps in Windows Settings. Insertion uses the current user's privileges.".into()
}

pub fn autostart(enabled: bool, config: &Path) -> Result<()> {
    unsafe {
        let mut key = HKEY::default();
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let result = if enabled {
            let executable = std::env::current_exe()?;
            let config = std::path::absolute(config)?;
            let command = format!(
                "\"{}\" --config \"{}\" run",
                executable.display(),
                config.display()
            );
            let bytes: Vec<u8> = command
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect();
            RegSetValueExW(key, w!("Whisper Bro"), None, REG_SZ, Some(&bytes)).ok()
        } else {
            let error = RegDeleteValueW(key, w!("Whisper Bro"));
            if error == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                error.ok()
            }
        };
        let _ = RegCloseKey(key);
        result?;
        Ok(())
    }
}
