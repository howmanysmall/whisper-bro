use std::{
    ffi::c_void,
    ptr,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use core_foundation::{
    base::{CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
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
    fn CGEventCreateKeyboardEvent(source: *const c_void, key: u16, down: bool) -> CFTypeRef;
    fn CGEventSetFlags(event: CFTypeRef, flags: u64);
    fn CGEventPost(tap: u32, event: CFTypeRef);
    fn CGEventSourceFlagsState(state: i32) -> u64;
}

pub fn trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub fn focused(pid: i32) -> Result<CFType> {
    if !trusted() {
        let options = CFDictionary::from_CFType_pairs(&[(
            CFString::new("AXTrustedCheckOptionPrompt"),
            CFBoolean::true_value(),
        )]);
        unsafe {
            AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef());
        }
    }
    ensure!(
        trusted(),
        "Enable Accessibility for Whisper Bro in System Settings → Privacy & Security → Accessibility, then restart it"
    );
    let pointer = unsafe { AXUIElementCreateApplication(pid) };
    ensure!(!pointer.is_null(), "Could not inspect the foreground app");
    let application = unsafe { CFType::wrap_under_create_rule(pointer) };
    let mut value = ptr::null();
    let attribute = CFString::new("AXFocusedUIElement");
    let status = unsafe {
        AXUIElementCopyAttributeValue(
            application.as_CFTypeRef(),
            attribute.as_concrete_TypeRef(),
            &mut value,
        )
    };
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

pub fn paste_key() -> Result<()> {
    ensure!(trusted(), "Accessibility permission was revoked");
    for down in [true, false] {
        let pointer = unsafe { CGEventCreateKeyboardEvent(ptr::null(), 9, down) };
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
