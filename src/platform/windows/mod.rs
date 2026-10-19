mod clipboard;
mod dialog;
mod icons;
mod overlay;

use std::{
    ffi::c_void,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use windows::{
    Win32::{
        Foundation::*,
        Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_NODEFAULT},
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::{
            Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
            Console::*,
            LibraryLoader::GetModuleHandleW,
            Registry::*,
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
            },
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
    platform::{Command, Cue, Status, autostart_executable},
    preview::Panel,
};
pub use clipboard::copy_text;
pub use dialog::prompt_api_key;

const TRAY_MESSAGE: u32 = WM_APP + 1;
const TALK_ID: i32 = 1;
const CANCEL_ID: i32 = 2;
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

pub struct Desktop {
    window: HWND,
    icon: NOTIFYICONDATAW,
    icons: [HICON; 4],
    receiver: Receiver<Command>,
    /// Declared before `shared` so it is destroyed first: it paints from it.
    overlay: overlay::Overlay,
    /// The status line and live preview the overlay shows while dictating.
    panel: Panel,
    shared: Box<Shared>,
    automation: IUIAutomation2,
    cancel: bool,
    held: bool,
    key: i32,
    taskbar_created: u32,
    _ole: clipboard::Ole,
}

/// State the window procedure reads through `GWLP_USERDATA`.
struct Shared {
    sender: Sender<Command>,
    status: AtomicU8,
    cleanup: AtomicBool,
    sounds: AtomicBool,
    hold: AtomicBool,
    /// The text the preview overlay paints.
    overlay: Mutex<String>,
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
        let shared = Box::new(Shared {
            sender,
            status: AtomicU8::new(Status::Idle as u8),
            cleanup: AtomicBool::new(false),
            sounds: AtomicBool::new(false),
            hold: AtomicBool::new(false),
            overlay: Mutex::new(String::new()),
        });
        unsafe {
            let automation: IUIAutomation2 =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
            automation.SetConnectionTimeout(500)?;
            automation.SetTransactionTimeout(500)?;
            let instance = HINSTANCE(GetModuleHandleW(None)?.0);
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: w!("Yap"),
                ..Default::default()
            };
            ensure!(
                RegisterClassW(&class) != 0,
                "Could not register the background window"
            );
            let window = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("Yap"),
                w!("Yap"),
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
            let icons = match icons::status_icons() {
                Ok(icons) => icons,
                Err(error) => {
                    let _ = DestroyWindow(window);
                    return Err(error);
                }
            };
            let overlay = match overlay::Overlay::new(instance, &shared) {
                Ok(overlay) => overlay,
                Err(error) => {
                    let _ = DestroyWindow(window);
                    for icon in icons {
                        let _ = DestroyIcon(icon);
                    }
                    return Err(error);
                }
            };
            SetWindowLongPtrW(window, GWLP_USERDATA, (&*shared as *const Shared) as isize);
            let icon = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: window,
                uID: 1,
                uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
                uCallbackMessage: TRAY_MESSAGE,
                hIcon: icons[Status::Idle as usize],
                ..Default::default()
            };
            let mut desktop = Self {
                window,
                icon,
                icons,
                receiver,
                overlay,
                panel: Panel::default(),
                shared,
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
            wide_into("Yap", &mut desktop.icon.szTip);
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
        wide_into(&format!("Yap · {label}\n{text}"), &mut self.icon.szTip);
        self.icon.hIcon = self.icons[status as usize];
        self.shared.status.store(status as u8, Ordering::Relaxed);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &self.icon);
        }
        self.panel.status(status, text);
        self.render();
    }

    /// Shows the live preview in the overlay below the status line.
    pub fn preview(&mut self, text: &str) {
        self.panel.preview(text);
        self.render();
    }

    /// Shows the overlay while recording or transcribing and hides it otherwise.
    fn render(&self) {
        match self.panel.text() {
            Some(text) => self.overlay.show(&self.shared, &text),
            None => self.overlay.hide(),
        }
    }

    /// The tray menu reads `autostart_enabled` each time it opens, so there
    /// is no checkmark to keep in sync.
    pub fn set_autostart(&mut self, _enabled: bool) {}

    /// Stores the checkmarks the tray menu shows the next time it opens.
    pub fn set_options(&mut self, cleanup: bool, sounds: bool, hold: bool) {
        self.shared.cleanup.store(cleanup, Ordering::Relaxed);
        self.shared.sounds.store(sounds, Ordering::Relaxed);
        self.shared.hold.store(hold, Ordering::Relaxed);
    }

    pub fn notify(&mut self, message: &str) {
        let mut notification = self.icon;
        notification.uFlags = NIF_INFO;
        notification.dwInfoFlags = NIIF_WARNING;
        wide_into("Yap", &mut notification.szInfoTitle);
        wide_into(message, &mut notification.szInfo);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &notification);
        }
    }

    pub fn target(&self) -> Result<Target> {
        target(&self.automation)
    }

    pub fn paste(
        &mut self,
        destination: &Target,
        text: &str,
        restore_after: Duration,
    ) -> Result<()> {
        let started = Instant::now();
        while modifiers_down() {
            ensure!(
                started.elapsed() < Duration::from_secs(1),
                "Release modifier keys before inserting dictation"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        self.check_target(destination)?;
        clipboard::paste(self.window, text, restore_after, || {
            self.check_target(destination)
        })
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
            for icon in self.icons {
                let _ = DestroyIcon(icon);
            }
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
        // SendInput into an elevated window fails without an error, so refuse up front.
        if let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let blocked = elevated(process).unwrap_or(false);
            let _ = CloseHandle(process);
            if blocked && !elevated(GetCurrentProcess())? {
                bail!(
                    "Windows blocked text insertion (the target may be running as administrator). Use Copy last dictation, or run Yap as administrator."
                );
            }
        }
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

fn elevated(process: HANDLE) -> Result<bool> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token)?;
        let mut elevation = TOKEN_ELEVATION::default();
        let mut length = 0;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        );
        let _ = CloseHandle(token);
        result?;
        Ok(elevation.TokenIsElevated != 0)
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
    let pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Shared;
    if !pointer.is_null() {
        let shared = unsafe { &*pointer };
        let command = match message {
            WM_HOTKEY if wparam.0 == TALK_ID as usize => Some(Command::TalkPressed),
            WM_HOTKEY if wparam.0 == CANCEL_ID as usize => Some(Command::Cancel),
            TRAY_MESSAGE if matches!(lparam.0 as u32, WM_RBUTTONUP | WM_LBUTTONUP) => {
                popup(window, shared).ok().flatten()
            }
            WM_CLOSE => Some(Command::Quit),
            _ => None,
        };
        if let Some(command) = command {
            let _ = shared.sender.send(command);
            return LRESULT(0);
        }
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn status(value: u8) -> Status {
    match value {
        value if value == Status::Recording as u8 => Status::Recording,
        value if value == Status::Processing as u8 => Status::Processing,
        value if value == Status::Error as u8 => Status::Error,
        _ => Status::Idle,
    }
}

fn popup(window: HWND, shared: &Shared) -> Result<Option<Command>> {
    let status = status(shared.status.load(Ordering::Relaxed));
    unsafe {
        let previous = GetForegroundWindow();
        let menu = CreatePopupMenu()?;
        let result = (|| -> Result<Option<Command>> {
            let enabled = |on: bool| if on { MF_STRING } else { MF_STRING | MF_GRAYED };
            let checked = |on: bool| {
                if on {
                    MF_STRING | MF_CHECKED
                } else {
                    MF_STRING
                }
            };
            let header = wide(concat!("Yap ", env!("CARGO_PKG_VERSION")));
            AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(header.as_ptr()))?;
            AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
            AppendMenuW(
                menu,
                enabled(status != Status::Processing),
                1,
                if status == Status::Recording {
                    w!("Stop recording")
                } else {
                    w!("Start recording")
                },
            )?;
            AppendMenuW(menu, MF_STRING, 2, w!("Cancel dictation"))?;
            AppendMenuW(
                menu,
                enabled(matches!(status, Status::Idle | Status::Error)),
                3,
                w!("Copy last dictation"),
            )?;
            AppendMenuW(
                menu,
                enabled(matches!(status, Status::Idle | Status::Error)),
                10,
                w!("Retry last recording"),
            )?;
            AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
            AppendMenuW(menu, MF_STRING, 5, w!("Set API key…"))?;
            AppendMenuW(menu, MF_STRING, 6, w!("Open config file"))?;
            AppendMenuW(menu, MF_STRING, 7, w!("Open log folder"))?;
            AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
            AppendMenuW(
                menu,
                checked(shared.cleanup.load(Ordering::Relaxed)),
                11,
                w!("Clean up transcript"),
            )?;
            AppendMenuW(
                menu,
                checked(shared.sounds.load(Ordering::Relaxed)),
                12,
                w!("Play sounds"),
            )?;
            AppendMenuW(
                menu,
                checked(shared.hold.load(Ordering::Relaxed)),
                13,
                w!("Hold to talk"),
            )?;
            AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
            AppendMenuW(menu, checked(autostart_enabled()), 8, w!("Start at login"))?;
            AppendMenuW(menu, MF_STRING, 9, w!("Check for updates…"))?;
            AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
            AppendMenuW(menu, MF_STRING, 4, w!("Quit Yap"))?;
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
                5 => Some(Command::SetApiKey),
                6 => Some(Command::OpenConfig),
                7 => Some(Command::OpenLogs),
                8 => Some(Command::ToggleAutostart),
                9 => Some(Command::OpenReleases),
                10 => Some(Command::RetryLast),
                11 => Some(Command::ToggleCleanup),
                12 => Some(Command::ToggleSounds),
                13 => Some(Command::ToggleMode),
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

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
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
    let alias = match cue {
        Cue::Start => w!("DeviceConnect"),
        Cue::Stop => w!("DeviceDisconnect"),
        Cue::Done => w!("SystemAsterisk"),
        Cue::Error => w!("SystemHand"),
        Cue::Warning => w!("SystemExclamation"),
    };
    unsafe {
        let _ = PlaySoundW(alias, None, SND_ALIAS | SND_ASYNC | SND_NODEFAULT);
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
            w!("Yap couldn't start"),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// Opens a folder in Explorer, or a file in Notepad since `.toml` often has
/// no registered handler.
pub fn open_path(path: &Path) -> Result<()> {
    let target: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let target = PCWSTR(target.as_ptr());
    let result = unsafe {
        if path.is_dir() {
            ShellExecuteW(None, w!("open"), target, None, None, SW_SHOWNORMAL)
        } else {
            ShellExecuteW(
                None,
                w!("open"),
                w!("notepad.exe"),
                target,
                None,
                SW_SHOWNORMAL,
            )
        }
    };
    ensure!(result.0 as isize > 32, "Could not open {}", path.display());
    Ok(())
}

pub fn open_url(url: &str) -> Result<()> {
    let link = wide(url);
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(link.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    ensure!(result.0 as isize > 32, "Could not open {url}");
    Ok(())
}

pub fn permission_status() -> String {
    "Enable microphone access for desktop apps in Windows Settings. Insertion uses the current user's privileges.".into()
}

pub fn autostart_enabled() -> bool {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_QUERY_VALUE, &mut key)
            != ERROR_SUCCESS
        {
            return false;
        }
        let found = RegQueryValueExW(key, w!("Yap"), None, None, None, None) == ERROR_SUCCESS;
        let _ = RegCloseKey(key);
        found
    }
}

pub fn autostart(enabled: bool, config: &Path) -> Result<()> {
    unsafe {
        let mut key = HKEY::default();
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()?;
        let legacy = RegDeleteValueW(key, w!("Whisper Bro"));
        if legacy != ERROR_FILE_NOT_FOUND
            && let Err(error) = legacy.ok()
        {
            let _ = RegCloseKey(key);
            return Err(error.into());
        }
        let result = if enabled {
            let mise_data = std::env::var_os("MISE_DATA_DIR")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("LOCALAPPDATA").map(|data| Path::new(&data).join("mise"))
                });
            let executable = autostart_executable(
                &std::env::current_exe()?,
                mise_data.as_deref(),
                Path::is_file,
            );
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
            RegSetValueExW(key, w!("Yap"), None, REG_SZ, Some(&bytes)).ok()
        } else {
            let error = RegDeleteValueW(key, w!("Yap"));
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
