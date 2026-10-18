use std::ffi::c_void;

use anyhow::{Result, bail};
use block2::RcBlock;
use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::{CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFDictionaryRef},
    number::CFNumber,
    string::{CFString, CFStringRef},
};
use objc2::runtime::Bool;
use objc2_app_kit::NSWorkspace;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_foundation::{NSString, NSURL};

use crate::hotkey::{Hotkey, Key};

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    static kTISPropertyInputSourceCategory: CFStringRef;
    static kTISCategoryKeyboardInputSource: CFStringRef;
    static kTISPropertyInputSourceIsEnabled: CFStringRef;
    static kTISPropertyInputSourceIsSelectCapable: CFStringRef;
    fn TISCreateInputSourceList(properties: CFDictionaryRef, include_all: bool) -> CFArrayRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFPreferencesCopyAppValue(key: CFStringRef, application: CFStringRef) -> CFTypeRef;
}

fn microphone_status() -> Option<AVAuthorizationStatus> {
    let audio = unsafe { AVMediaTypeAudio }?;
    Some(unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) })
}

fn request_microphone() {
    if let Some(audio) = unsafe { AVMediaTypeAudio } {
        let done = RcBlock::new(|_: Bool| {});
        unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &done) };
    }
}

/// Shows the macOS microphone prompt at launch when the user has never answered it.
pub fn prompt_microphone() {
    if microphone_status() == Some(AVAuthorizationStatus::NotDetermined) {
        request_microphone();
    }
}

/// A denied microphone delivers silence instead of failing, so check before recording.
pub fn microphone() -> Result<()> {
    match microphone_status() {
        Some(AVAuthorizationStatus::Authorized) | None => Ok(()),
        Some(AVAuthorizationStatus::NotDetermined) => {
            request_microphone();
            bail!("macOS is asking for microphone access. Allow it, then press the shortcut again")
        }
        Some(_) => {
            if let Some(url) = NSURL::URLWithString(&NSString::from_str(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
            )) {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
            bail!(
                "Microphone access is off for Yap. Turn it on in System Settings → Privacy & Security → Microphone"
            )
        }
    }
}

pub fn microphone_label() -> &'static str {
    match microphone_status() {
        Some(AVAuthorizationStatus::Authorized) => "granted",
        Some(AVAuthorizationStatus::NotDetermined) => "not requested yet",
        Some(AVAuthorizationStatus::Denied) => "denied",
        Some(AVAuthorizationStatus::Restricted) => "restricted",
        _ => "unknown",
    }
}

/// Ctrl+Space and Ctrl+Alt+Space are the system input-source shortcuts. Registering them
/// still succeeds, but macOS also switches the input source on every press.
pub fn input_switch_conflict(shortcut: &Hotkey) -> Option<&'static str> {
    if shortcut.key != Key::Space || shortcut.shift || shortcut.command || !shortcut.control {
        return None;
    }
    let (id, message) = if shortcut.alt {
        (
            "61",
            "Ctrl+Alt+Space is also macOS's \"Select next source in Input menu\" shortcut, so every press would switch your keyboard input source. Turn that shortcut off in System Settings → Keyboard → Keyboard Shortcuts → Input Sources, or change shortcut in config.toml",
        )
    } else {
        (
            "60",
            "Ctrl+Space is also macOS's \"Select the previous input source\" shortcut, so every press would switch your keyboard input source. Turn that shortcut off in System Settings → Keyboard → Keyboard Shortcuts → Input Sources, or change shortcut in config.toml",
        )
    };
    (keyboard_sources() >= 2 && symbolic_hotkey_enabled(id)).then_some(message)
}

fn keyboard_sources() -> isize {
    let (category, keyboard, enabled, selectable) = unsafe {
        (
            CFString::wrap_under_get_rule(kTISPropertyInputSourceCategory),
            CFString::wrap_under_get_rule(kTISCategoryKeyboardInputSource),
            CFString::wrap_under_get_rule(kTISPropertyInputSourceIsEnabled),
            CFString::wrap_under_get_rule(kTISPropertyInputSourceIsSelectCapable),
        )
    };
    let filter = CFDictionary::from_CFType_pairs(&[
        (category, keyboard.as_CFType()),
        (enabled, CFBoolean::true_value().as_CFType()),
        (selectable, CFBoolean::true_value().as_CFType()),
    ]);
    let list = unsafe { TISCreateInputSourceList(filter.as_concrete_TypeRef(), false) };
    if list.is_null() {
        return 0;
    }
    unsafe { CFArray::<*const c_void>::wrap_under_create_rule(list) }.len()
}

/// Missing entries mean the shortcut has its default state, which is enabled.
fn symbolic_hotkey_enabled(id: &str) -> bool {
    let key = CFString::new("AppleSymbolicHotKeys");
    let domain = CFString::new("com.apple.symbolichotkeys");
    let value = unsafe {
        CFPreferencesCopyAppValue(key.as_concrete_TypeRef(), domain.as_concrete_TypeRef())
    };
    if value.is_null() {
        return true;
    }
    let hotkeys = unsafe { CFType::wrap_under_create_rule(value) };
    let enabled = lookup(&hotkeys, id).and_then(|entry| lookup(&entry, "enabled"));
    let Some(enabled) = enabled else {
        return true;
    };
    if let Some(flag) = enabled.downcast::<CFBoolean>() {
        bool::from(flag)
    } else if let Some(number) = enabled.downcast::<CFNumber>() {
        number.to_i64() != Some(0)
    } else {
        true
    }
}

fn lookup(dictionary: &CFType, key: &str) -> Option<CFType> {
    let dictionary = dictionary.downcast::<CFDictionary>()?;
    let key = CFString::new(key);
    dictionary
        .find(key.as_CFTypeRef())
        .map(|value| unsafe { CFType::wrap_under_get_rule(*value) })
}
