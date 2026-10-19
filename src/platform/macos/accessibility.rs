use std::{
    ffi::c_void,
    ptr,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use core_foundation::{
    base::{CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    data::CFData,
    dictionary::{CFDictionary, CFDictionaryRef},
    string::{CFString, CFStringRef},
};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> i32;
    fn CGEventCreateKeyboardEvent(source: *const c_void, key: u16, down: bool) -> CFTypeRef;
    fn CGEventSetFlags(event: CFTypeRef, flags: u64);
    fn CGEventPost(tap: u32, event: CFTypeRef);
    fn CGEventSourceFlagsState(state: i32) -> u64;
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
    fn TISGetInputSourceProperty(source: CFTypeRef, key: CFStringRef) -> CFTypeRef;
    fn UCKeyTranslate(
        layout: *const c_void,
        key: u16,
        action: u16,
        modifiers: u32,
        kbd_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_len: usize,
        actual_len: *mut usize,
        out: *mut u16,
    ) -> i32;
    fn LMGetKbdType() -> u8;
}

/// The US-layout position of V, used when the current layout cannot be read.
const ANSI_V: u16 = 9;

pub fn trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Shows the Accessibility prompt at launch instead of on the first shortcut press.
pub fn prompt() {
    if !trusted() {
        let options = CFDictionary::from_CFType_pairs(&[(
            CFString::new("AXTrustedCheckOptionPrompt"),
            CFBoolean::true_value(),
        )]);
        unsafe {
            AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef());
        }
    }
}

pub fn focused(pid: i32) -> Result<CFType> {
    ensure!(
        trusted(),
        "Enable Accessibility for Yap in System Settings → Privacy & Security → Accessibility, then restart it"
    );
    let pointer = unsafe { AXUIElementCreateApplication(pid) };
    ensure!(!pointer.is_null(), "Could not inspect the foreground app");
    let application = unsafe { CFType::wrap_under_create_rule(pointer) };
    // A hung app would otherwise block the main thread for the default ~6 s per call.
    unsafe {
        AXUIElementSetMessagingTimeout(application.as_CFTypeRef(), 0.5);
    }
    let mut value = ptr::null();
    let attribute = CFString::new("AXFocusedUIElement");
    let status = unsafe {
        AXUIElementCopyAttributeValue(
            application.as_CFTypeRef(),
            attribute.as_concrete_TypeRef(),
            &mut value,
        )
    };
    // kAXErrorCannotComplete: the app did not answer within the messaging timeout.
    ensure!(
        status != -25204,
        "The foreground app is not responding; try again once it recovers"
    );
    ensure!(
        status == 0 && !value.is_null(),
        "Could not identify the focused text field; click a text field first"
    );
    Ok(unsafe { CFType::wrap_under_create_rule(value) })
}

pub fn wait_for_modifiers() -> Result<()> {
    let started = Instant::now();
    let mask = (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20);
    while unsafe { CGEventSourceFlagsState(0) } & mask != 0 {
        ensure!(
            started.elapsed() < Duration::from_secs(1),
            "Release modifier keys before inserting dictation"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// Finds the key that types "v" in the current layout, so Cmd+V still pastes on Dvorak, Colemak or AZERTY.
fn paste_keycode() -> u16 {
    let source = unsafe { TISCopyCurrentKeyboardLayoutInputSource() };
    if source.is_null() {
        return ANSI_V;
    }
    let source = unsafe { CFType::wrap_under_create_rule(source) };
    let layout = unsafe {
        TISGetInputSourceProperty(source.as_CFTypeRef(), kTISPropertyUnicodeKeyLayoutData)
    };
    // CJK input methods publish no Unicode layout.
    if layout.is_null() {
        return ANSI_V;
    }
    let layout = unsafe { CFData::wrap_under_get_rule(layout.cast()) };
    let bytes = layout.bytes();
    let kbd_type = u32::from(unsafe { LMGetKbdType() });
    for code in 0..128u16 {
        let mut dead = 0u32;
        let mut len = 0usize;
        let mut out = [0u16; 4];
        let status = unsafe {
            UCKeyTranslate(
                bytes.as_ptr().cast(),
                code,
                0, // kUCKeyActionDown
                0,
                kbd_type,
                1, // kUCKeyTranslateNoDeadKeysMask
                &mut dead,
                out.len(),
                &mut len,
                out.as_mut_ptr(),
            )
        };
        if status == 0 && len == 1 && out[0] == u16::from(b'v') {
            return code;
        }
    }
    ANSI_V
}

pub fn paste_key() -> Result<()> {
    ensure!(trusted(), "Accessibility permission was revoked");
    let code = paste_keycode();
    for down in [true, false] {
        let pointer = unsafe { CGEventCreateKeyboardEvent(ptr::null(), code, down) };
        let pointer = std::ptr::NonNull::new(pointer.cast_mut())
            .context("Could not create paste keystroke")?;
        let event = unsafe { CFType::wrap_under_create_rule(pointer.as_ptr()) };
        unsafe {
            CGEventSetFlags(event.as_CFTypeRef(), 1 << 20);
            CGEventPost(0, event.as_CFTypeRef());
        }
    }
    Ok(())
}
