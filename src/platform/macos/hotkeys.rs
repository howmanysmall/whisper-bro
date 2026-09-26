use std::{ffi::c_void, ptr, sync::mpsc::Sender};

use anyhow::{Result, bail};

use crate::{
    hotkey::{Hotkey, Key},
    platform::Command,
};

type Ref = *mut c_void;

#[repr(C)]
struct EventType {
    class: u32,
    kind: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct HotkeyId {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> Ref;
    fn InstallEventHandler(
        target: Ref,
        callback: unsafe extern "C" fn(Ref, Ref, Ref) -> i32,
        count: u32,
        events: *const EventType,
        data: Ref,
        handler: *mut Ref,
    ) -> i32;
    fn RemoveEventHandler(handler: Ref) -> i32;
    fn RegisterEventHotKey(
        key: u32,
        modifiers: u32,
        id: HotkeyId,
        target: Ref,
        options: u32,
        hotkey: *mut Ref,
    ) -> i32;
    fn UnregisterEventHotKey(hotkey: Ref) -> i32;
    fn GetEventKind(event: Ref) -> u32;
    fn GetEventParameter(
        event: Ref,
        name: u32,
        kind: u32,
        actual_type: *mut u32,
        size: u32,
        actual_size: *mut u32,
        data: Ref,
    ) -> i32;
}

pub struct Hotkeys {
    talk: Ref,
    cancel: Ref,
    handler: Ref,
    sender: Box<Sender<Command>>,
}

impl Hotkeys {
    pub fn new(shortcut: &Hotkey, sender: Sender<Command>) -> Result<Self> {
        let mut keys = Self {
            talk: ptr::null_mut(),
            cancel: ptr::null_mut(),
            handler: ptr::null_mut(),
            sender: Box::new(sender),
        };
        let events = [
            EventType {
                class: u32::from_be_bytes(*b"keyb"),
                kind: 5,
            },
            EventType {
                class: u32::from_be_bytes(*b"keyb"),
                kind: 6,
            },
        ];
        // The boxed sender remains at this address until the event handler is removed.
        unsafe {
            let status = InstallEventHandler(
                GetApplicationEventTarget(),
                on_hotkey,
                2,
                events.as_ptr(),
                (&mut *keys.sender as *mut Sender<Command>).cast(),
                &mut keys.handler,
            );
            if status != 0 {
                bail!("Could not install shortcut handler (macOS {status})");
            }
        }
        keys.talk = register(shortcut, 1)?;
        Ok(keys)
    }

    pub fn cancel(&mut self, enabled: bool) -> Result<()> {
        if enabled && self.cancel.is_null() {
            self.cancel = register(
                &Hotkey {
                    control: false,
                    alt: false,
                    shift: false,
                    command: false,
                    key: Key::Escape,
                },
                2,
            )?;
        } else if !enabled && !self.cancel.is_null() {
            unsafe {
                UnregisterEventHotKey(self.cancel);
            }
            self.cancel = ptr::null_mut();
        }
        Ok(())
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        unsafe {
            if !self.talk.is_null() {
                UnregisterEventHotKey(self.talk);
            }
            if !self.cancel.is_null() {
                UnregisterEventHotKey(self.cancel);
            }
            if !self.handler.is_null() {
                RemoveEventHandler(self.handler);
            }
        }
    }
}

fn register(shortcut: &Hotkey, id: u32) -> Result<Ref> {
    let modifiers = (u32::from(shortcut.command) << 8)
        | (u32::from(shortcut.shift) << 9)
        | (u32::from(shortcut.alt) << 11)
        | (u32::from(shortcut.control) << 12);
    let mut handle = ptr::null_mut();
    let status = unsafe {
        RegisterEventHotKey(
            code(shortcut.key),
            modifiers,
            HotkeyId {
                signature: u32::from_be_bytes(*b"WBro"),
                id,
            },
            GetApplicationEventTarget(),
            0,
            &mut handle,
        )
    };
    if status != 0 {
        bail!(
            "Shortcut is unavailable or already in use (macOS {status}). Change shortcut in config.toml"
        );
    }
    Ok(handle)
}

unsafe extern "C" fn on_hotkey(_: Ref, event: Ref, data: Ref) -> i32 {
    if data.is_null() {
        return -9874;
    }
    let mut id = HotkeyId::default();
    let status = unsafe {
        GetEventParameter(
            event,
            u32::from_be_bytes(*b"----"),
            u32::from_be_bytes(*b"hkid"),
            ptr::null_mut(),
            std::mem::size_of::<HotkeyId>() as u32,
            ptr::null_mut(),
            (&mut id as *mut HotkeyId).cast(),
        )
    };
    if status != 0 {
        return status;
    }
    let pressed = unsafe { GetEventKind(event) } == 5;
    let command = match (id.id, pressed) {
        (1, true) => Command::TalkPressed,
        (1, false) => Command::TalkReleased,
        (2, true) => Command::Cancel,
        _ => return 0,
    };
    let sender = unsafe { &*data.cast::<Sender<Command>>() };
    let _ = sender.send(command);
    0
}

fn code(key: Key) -> u32 {
    match key {
        Key::Space => 49,
        Key::Escape => 53,
        Key::Backspace => 51,
        Key::Letter(letter) => match letter {
            b'A' => 0,
            b'B' => 11,
            b'C' => 8,
            b'D' => 2,
            b'E' => 14,
            b'F' => 3,
            b'G' => 5,
            b'H' => 4,
            b'I' => 34,
            b'J' => 38,
            b'K' => 40,
            b'L' => 37,
            b'M' => 46,
            b'N' => 45,
            b'O' => 31,
            b'P' => 35,
            b'Q' => 12,
            b'R' => 15,
            b'S' => 1,
            b'T' => 17,
            b'U' => 32,
            b'V' => 9,
            b'W' => 13,
            b'X' => 7,
            b'Y' => 16,
            b'Z' => 6,
            _ => 49,
        },
        Key::Digit(number) => [29, 18, 19, 20, 21, 23, 22, 26, 28, 25]
            .get(number as usize)
            .copied()
            .unwrap_or(49),
        Key::Function(number) => [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111]
            .get(number.saturating_sub(1) as usize)
            .copied()
            .unwrap_or(49),
    }
}
