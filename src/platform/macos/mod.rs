mod accessibility;
mod clipboard;
mod hotkeys;

use std::{
    path::Path,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use core_foundation::base::CFType;
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::AnyObject,
    sel,
};
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSEventMask, NSMenu, NSMenuItem,
    NSSound, NSStatusBar, NSStatusItem, NSVariableStatusItemLength, NSWorkspace,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSObject, NSObjectProtocol, NSString};

use crate::{
    hotkey::Hotkey,
    platform::{Command, Cue, Status},
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
    _target: Retained<MenuTarget>,
    keys: hotkeys::Hotkeys,
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
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        app.finishLaunching();
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
        label.setTitle(&NSString::from_str("Whisper Bro"));
        label.setEnabled(false);
        menu.addItem(&label);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let toggle = menu_item(mtm, &target, "Start recording", sel!(toggle:));
        menu.addItem(&toggle);
        menu.addItem(&menu_item(mtm, &target, "Cancel dictation", sel!(cancel:)));
        let copy = menu_item(mtm, &target, "Copy last dictation", sel!(copyLast:));
        menu.addItem(&copy);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(mtm, &target, "Quit Whisper Bro", sel!(quit:)));
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        item.setMenu(Some(&menu));
        if let Some(button) = item.button(mtm) {
            button.setTitle(&NSString::from_str("◉"));
        }
        Ok(Self {
            app,
            item,
            label,
            toggle,
            copy,
            _target: target,
            keys,
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
        self.copy
            .setEnabled(matches!(status, Status::Idle | Status::Error));
        if let Some(mtm) = MainThreadMarker::new()
            && let Some(button) = self.item.button(mtm)
        {
            button.setTitle(&NSString::from_str(match status {
                Status::Idle => "◉",
                Status::Recording => "● REC",
                Status::Processing => "…",
                Status::Error => "!",
            }));
            button.setToolTip(Some(&NSString::from_str(text)));
        }
    }

    pub fn notify(&mut self, message: &str) {
        self.label.setTitle(&NSString::from_str(message));
    }

    pub fn target(&self) -> Result<Target> {
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

    pub fn paste(&mut self, target: &Target, text: &str) -> Result<()> {
        accessibility::wait_for_modifiers()?;
        let current = self.target()?;
        ensure!(
            current.pid == target.pid && current.element == target.element,
            "The destination changed while dictating"
        );
        clipboard::paste(text, || {
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
    };
    if let Some(sound) = NSSound::soundNamed(&NSString::from_str(name)) {
        sound.play();
    }
}

pub fn attach_console() {}
pub fn setup_console() {}

pub fn startup_error(message: &str) {
    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str("Whisper Bro couldn't start"));
        alert.setInformativeText(&NSString::from_str(message));
        alert.runModal();
    }
}

pub fn permission_status() -> String {
    format!(
        "Accessibility: {}. Microphone permission is checked by macOS when recording starts.",
        if accessibility::trusted() {
            "granted"
        } else {
            "not granted"
        }
    )
}

pub fn autostart(enabled: bool, config: &Path) -> Result<()> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let path = Path::new(&home).join("Library/LaunchAgents/dev.whisper-bro.plist");
    if !enabled {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    let executable = std::env::current_exe()?;
    let config = std::path::absolute(config)?;
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>dev.whisper-bro</string>
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
