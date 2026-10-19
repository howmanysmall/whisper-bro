mod accessibility;
mod clipboard;
mod hotkeys;
mod hud;
mod permissions;

use std::{
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use core_foundation::base::CFType;
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::AnyObject,
    sel,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSApplicationActivationPolicy, NSColor,
    NSControlStateValue, NSControlStateValueOff, NSControlStateValueOn, NSEventMask, NSImage,
    NSImageSymbolConfiguration, NSMenu, NSMenuItem, NSSecureTextField, NSSound,
    NSSquareStatusItemLength, NSStatusBar, NSStatusItem, NSWorkspace,
};
use objc2_foundation::{
    NSDate, NSDefaultRunLoopMode, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    NSURL,
};
use zeroize::Zeroizing;

use crate::{
    credentials,
    hotkey::Hotkey,
    platform::{Command, Cue, Status, autostart_executable},
    preview::Panel,
    storage::write_private,
};

pub use clipboard::copy_text;

struct MenuState {
    sender: Sender<Command>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = MenuState]
    struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        #[unsafe(method(toggle:))]
        fn toggle(&self, _: Option<&AnyObject>) { let _ = self.ivars().sender.send(Command::Toggle); }
        #[unsafe(method(cancel:))]
        fn cancel(&self, _: Option<&AnyObject>) { let _ = self.ivars().sender.send(Command::Cancel); }
        #[unsafe(method(copyLast:))]
        fn copy_last(&self, _: Option<&AnyObject>) { let _ = self.ivars().sender.send(Command::CopyLast); }
        #[unsafe(method(retryLast:))]
        fn retry_last(&self, _: Option<&AnyObject>) { let _ = self.ivars().sender.send(Command::RetryLast); }
        #[unsafe(method(setApiKey:))]
        fn set_api_key(&self, _: Option<&AnyObject>) { let _ = self.ivars().sender.send(Command::SetApiKey); }
        #[unsafe(method(openConfig:))]
        fn open_config(&self, _: Option<&AnyObject>) { let _ = self.ivars().sender.send(Command::OpenConfig); }
        #[unsafe(method(openLogs:))]
        fn open_logs(&self, _: Option<&AnyObject>) { let _ = self.ivars().sender.send(Command::OpenLogs); }
        #[unsafe(method(toggleCleanup:))]
        fn toggle_cleanup(&self, _: Option<&AnyObject>) { let _ = self.ivars().sender.send(Command::ToggleCleanup); }
        #[unsafe(method(toggleSounds:))]
        fn toggle_sounds(&self, _: Option<&AnyObject>) { let _ = self.ivars().sender.send(Command::ToggleSounds); }
        #[unsafe(method(toggleMode:))]
        fn toggle_mode(&self, _: Option<&AnyObject>) { let _ = self.ivars().sender.send(Command::ToggleMode); }
        #[unsafe(method(toggleAutostart:))]
        fn toggle_autostart(&self, _: Option<&AnyObject>) { let _ = self.ivars().sender.send(Command::ToggleAutostart); }
        #[unsafe(method(openReleases:))]
        fn open_releases(&self, _: Option<&AnyObject>) { let _ = self.ivars().sender.send(Command::OpenReleases); }
        #[unsafe(method(quit:))]
        fn quit(&self, _: Option<&AnyObject>) { let _ = self.ivars().sender.send(Command::Quit); }
    }
);

pub struct Desktop {
    app: Retained<NSApplication>,
    item: Retained<NSStatusItem>,
    label: Retained<NSMenuItem>,
    toggle: Retained<NSMenuItem>,
    copy: Retained<NSMenuItem>,
    retry: Retained<NSMenuItem>,
    autostart: Retained<NSMenuItem>,
    cleanup: Retained<NSMenuItem>,
    sounds: Retained<NSMenuItem>,
    hold: Retained<NSMenuItem>,
    _target: Retained<MenuTarget>,
    keys: hotkeys::Hotkeys,
    hud: hud::Hud,
    /// The status line and live preview the HUD shows while dictating.
    panel: Panel,
    receiver: Receiver<Command>,
}

pub struct Target {
    pid: i32,
    element: CFType,
}

impl Desktop {
    pub fn new(shortcut: &Hotkey) -> Result<Self> {
        let mtm = MainThreadMarker::new()
            .context("The desktop event loop must run on the main thread")?;
        let app = application(mtm);
        app.finishLaunching();
        if let Some(message) = permissions::input_switch_conflict(shortcut) {
            bail!(message);
        }
        accessibility::prompt();
        permissions::prompt_microphone();
        let (sender, receiver) = mpsc::channel();
        let keys = hotkeys::Hotkeys::new(shortcut, sender.clone())?;
        let target: Retained<MenuTarget> = unsafe {
            msg_send![
                super(MenuTarget::alloc(mtm).set_ivars(MenuState { sender })),
                init
            ]
        };
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let label = NSMenuItem::new(mtm);
        label.setTitle(&NSString::from_str("Yap"));
        label.setEnabled(false);
        menu.addItem(&label);
        let version = NSMenuItem::new(mtm);
        version.setTitle(&NSString::from_str(concat!(
            "Yap ",
            env!("CARGO_PKG_VERSION")
        )));
        version.setEnabled(false);
        menu.addItem(&version);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let toggle = menu_item(mtm, &target, "Start recording", sel!(toggle:));
        menu.addItem(&toggle);
        menu.addItem(&menu_item(mtm, &target, "Cancel dictation", sel!(cancel:)));
        let copy = menu_item(mtm, &target, "Copy last dictation", sel!(copyLast:));
        menu.addItem(&copy);
        let retry = menu_item(mtm, &target, "Retry last recording", sel!(retryLast:));
        menu.addItem(&retry);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(mtm, &target, "Set API key…", sel!(setApiKey:)));
        menu.addItem(&menu_item(
            mtm,
            &target,
            "Open config file",
            sel!(openConfig:),
        ));
        menu.addItem(&menu_item(mtm, &target, "Open log folder", sel!(openLogs:)));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let cleanup = menu_item(mtm, &target, "Clean up transcript", sel!(toggleCleanup:));
        menu.addItem(&cleanup);
        let sounds = menu_item(mtm, &target, "Play sounds", sel!(toggleSounds:));
        menu.addItem(&sounds);
        let hold = menu_item(mtm, &target, "Hold to talk", sel!(toggleMode:));
        menu.addItem(&hold);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let autostart = menu_item(mtm, &target, "Start at login", sel!(toggleAutostart:));
        menu.addItem(&autostart);
        menu.addItem(&menu_item(
            mtm,
            &target,
            "Check for updates…",
            sel!(openReleases:),
        ));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(mtm, &target, "Quit Yap", sel!(quit:)));
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSSquareStatusItemLength);
        item.setMenu(Some(&menu));
        if let Some(button) = item.button(mtm) {
            match status_image(Status::Idle) {
                Some(image) => button.setImage(Some(&image)),
                None => button.setTitle(&NSString::from_str(status_title(Status::Idle))),
            }
        }
        let hud = hud::Hud::new(mtm, item.clone());
        Ok(Self {
            app,
            item,
            label,
            toggle,
            copy,
            retry,
            autostart,
            cleanup,
            sounds,
            hold,
            _target: target,
            keys,
            hud,
            panel: Panel::default(),
            receiver,
        })
    }

    pub fn poll(&mut self, wait: Duration) -> Result<Vec<Command>> {
        autoreleasepool(|_| {
            let deadline = NSDate::dateWithTimeIntervalSinceNow(wait.as_secs_f64());
            let event = unsafe {
                self.app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any,
                    Some(&deadline),
                    NSDefaultRunLoopMode,
                    true,
                )
            };
            if let Some(event) = event {
                self.app.sendEvent(&event);
            }
            self.hud.tick();
            Ok(self.receiver.try_iter().collect())
        })
    }

    pub fn cancel_shortcut(&mut self, enabled: bool) -> Result<()> {
        self.keys.cancel(enabled)
    }

    pub fn status(&mut self, status: Status, text: &str) {
        self.label.setTitle(&NSString::from_str(text));
        self.toggle
            .setTitle(&NSString::from_str(if status == Status::Recording {
                "Stop recording"
            } else {
                "Start recording"
            }));
        self.toggle.setEnabled(status != Status::Processing);
        let idle = matches!(status, Status::Idle | Status::Error);
        self.copy.setEnabled(idle);
        self.retry.setEnabled(idle);
        if let Some(mtm) = MainThreadMarker::new()
            && let Some(button) = self.item.button(mtm)
        {
            match status_image(status) {
                Some(image) => {
                    button.setTitle(&NSString::from_str(""));
                    button.setImage(Some(&image));
                }
                None => {
                    button.setImage(None);
                    button.setTitle(&NSString::from_str(status_title(status)));
                }
            }
            button.setToolTip(Some(&NSString::from_str(text)));
        }
        self.panel.status(status, text);
        match status {
            Status::Recording | Status::Processing => {
                self.hud.show(&self.panel.text().unwrap_or_default(), None);
            }
            Status::Idle => self.hud.show(text, Some(Duration::from_millis(1500))),
            // `notify` follows every error and shows it for longer.
            Status::Error => {}
        }
    }

    /// Shows the live preview below the status line, growing the HUD
    /// downward from under the status item.
    pub fn preview(&mut self, text: &str) {
        self.panel.preview(text);
        if let Some(text) = self.panel.text() {
            self.hud.show(&text, None);
        }
    }

    pub fn set_autostart(&mut self, enabled: bool) {
        self.autostart.setState(check_state(enabled));
    }

    pub fn set_options(&mut self, cleanup: bool, sounds: bool, hold: bool) {
        self.cleanup.setState(check_state(cleanup));
        self.sounds.setState(check_state(sounds));
        self.hold.setState(check_state(hold));
    }

    pub fn notify(&mut self, message: &str) {
        self.label.setTitle(&NSString::from_str(message));
        self.hud.show(message, Some(Duration::from_secs(5)));
    }

    pub fn target(&self) -> Result<Target> {
        permissions::microphone()?;
        let application = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .context("No foreground application")?;
        let pid = application.processIdentifier();
        ensure!(
            pid != std::process::id() as i32,
            "Click the destination text field and use the recording shortcut"
        );
        Ok(Target {
            pid,
            element: accessibility::focused(pid)?,
        })
    }

    pub fn paste(&mut self, target: &Target, text: &str, restore_after: Duration) -> Result<()> {
        accessibility::wait_for_modifiers()?;
        let current = self.target()?;
        ensure!(
            current.pid == target.pid && current.element == target.element,
            "The destination changed while dictating"
        );
        clipboard::paste(text, restore_after, || {
            let current = self.target()?;
            ensure!(
                current.pid == target.pid && current.element == target.element,
                "The destination changed while preparing to paste"
            );
            Ok(())
        })
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        if MainThreadMarker::new().is_some() {
            NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
        }
    }
}

fn status_image(status: Status) -> Option<Retained<NSImage>> {
    let (name, description) = match status {
        Status::Idle => ("mic", "Yap"),
        Status::Recording => ("mic.fill", "Yap is recording"),
        Status::Processing => ("ellipsis.circle", "Yap is transcribing"),
        Status::Error => ("exclamationmark.triangle.fill", "Yap error"),
    };
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )?;
    if matches!(status, Status::Recording | Status::Error) {
        let red = NSImageSymbolConfiguration::configurationWithHierarchicalColor(
            &NSColor::systemRedColor(),
        );
        let image = image.imageWithSymbolConfiguration(&red)?;
        image.setTemplate(false);
        Some(image)
    } else {
        image.setTemplate(true);
        Some(image)
    }
}

/// Text shown when SF Symbols are unavailable.
fn status_title(status: Status) -> &'static str {
    match status {
        Status::Idle => "◉",
        Status::Recording => "● REC",
        Status::Processing => "…",
        Status::Error => "!",
    }
}

fn check_state(on: bool) -> NSControlStateValue {
    if on {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    }
}

fn menu_item(
    mtm: MainThreadMarker,
    target: &MenuTarget,
    title: &str,
    action: objc2::runtime::Sel,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(""),
        )
    };
    unsafe {
        item.setTarget(Some(target));
    }
    item
}

pub fn cue(cue: Cue) {
    let name = match cue {
        Cue::Start => "Tink",
        Cue::Stop => "Pop",
        Cue::Done => "Glass",
        Cue::Error => "Basso",
        Cue::Warning => "Funk",
    };
    if let Some(sound) = NSSound::soundNamed(&NSString::from_str(name)) {
        sound.play();
    }
}

pub fn attach_console() {}
pub fn setup_console() {}

/// The shared application as a menu-bar accessory without a Dock icon.
fn application(mtm: MainThreadMarker) -> Retained<NSApplication> {
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app
}

pub fn startup_error(message: &str) {
    if let Some(mtm) = MainThreadMarker::new() {
        application(mtm);
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str("Yap couldn't start"));
        alert.setInformativeText(&NSString::from_str(message));
        alert.runModal();
    }
}

/// Asks for the OpenRouter key and saves it. Returns false when cancelled.
pub fn prompt_api_key() -> Result<bool> {
    let mtm = MainThreadMarker::new().context("The key prompt must run on the main thread")?;
    let app = application(mtm);
    #[allow(deprecated)] // `activate` needs macOS 14.
    app.activateIgnoringOtherApps(true);
    loop {
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str("Enter your OpenRouter API key"));
        alert.setInformativeText(&NSString::from_str(
            "Create a key at https://openrouter.ai/settings/keys. Yap stores it in your Keychain.",
        ));
        alert.addButtonWithTitle(&NSString::from_str("Save"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let field = NSSecureTextField::initWithFrame(
            NSSecureTextField::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(340.0, 24.0)),
        );
        field.setPlaceholderString(Some(&NSString::from_str("sk-or-…")));
        alert.setAccessoryView(Some(&field));
        alert.window().setInitialFirstResponder(Some(&field));
        if alert.runModal() != NSAlertFirstButtonReturn {
            return Ok(false);
        }
        let key = Zeroizing::new(field.stringValue().to_string());
        field.setStringValue(&NSString::from_str(""));
        if key.trim().is_empty() {
            continue;
        }
        credentials::set("OPENROUTER_API_KEY", &key)?;
        return Ok(true);
    }
}

/// Opens a folder in Finder, or a file in the default text editor since
/// `.toml` often has no registered handler.
pub fn open_path(path: &Path) -> Result<()> {
    if path.is_dir() {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        ensure!(
            NSWorkspace::sharedWorkspace().openURL(&url),
            "Could not open {}",
            path.display()
        );
        return Ok(());
    }
    let status = std::process::Command::new("open")
        .arg("-t")
        .arg(path)
        .status()
        .context("Run open")?;
    ensure!(status.success(), "Could not open {}", path.display());
    Ok(())
}

pub fn open_url(url: &str) -> Result<()> {
    let link = NSURL::URLWithString(&NSString::from_str(url)).context("Invalid URL")?;
    ensure!(
        NSWorkspace::sharedWorkspace().openURL(&link),
        "Could not open {url}"
    );
    Ok(())
}

pub fn permission_status() -> String {
    format!(
        "Accessibility: {}. Microphone: {}.",
        if accessibility::trusted() {
            "granted"
        } else {
            "not granted"
        },
        permissions::microphone_label()
    )
}

fn launch_agents() -> Result<std::path::PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(Path::new(&home).join("Library/LaunchAgents"))
}

pub fn autostart_enabled() -> bool {
    launch_agents().is_ok_and(|agents| agents.join("dev.yap.plist").is_file())
}

pub fn autostart(enabled: bool, config: &Path) -> Result<()> {
    let agents = launch_agents()?;
    match std::fs::remove_file(agents.join("dev.whisper-bro.plist")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let path = agents.join("dev.yap.plist");
    if !enabled {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    let mise_data = std::env::var_os("MISE_DATA_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(|data| Path::new(&data).join("mise")))
        .or_else(|| {
            std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/share/mise"))
        });
    let executable = autostart_executable(
        &std::env::current_exe()?,
        mise_data.as_deref(),
        Path::is_file,
    );
    let config = std::path::absolute(config)?;
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>dev.yap</string>
<key>ProgramArguments</key><array><string>{}</string><string>--config</string><string>{}</string><string>run</string></array>
<key>RunAtLoad</key><true/>
<key>ProcessType</key><string>Interactive</string>
</dict></plist>
"#,
        xml_text(&executable.to_string_lossy()),
        xml_text(&config.to_string_lossy())
    );
    write_private(&path, xml.as_bytes())
}

fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
