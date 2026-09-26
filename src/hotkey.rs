use std::str::FromStr;

use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub command: bool,
    pub key: Key,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Letter(u8),
    Digit(u8),
    Space,
    Escape,
    Backspace,
    Function(u8),
}

#[derive(Default)]
pub struct PressLatch {
    down: bool,
}

impl PressLatch {
    pub fn press(&mut self) -> bool {
        let fresh = !self.down;
        self.down = true;
        fresh
    }

    pub fn release(&mut self) {
        self.down = false;
    }
}

impl FromStr for Hotkey {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let mut result = Self {
            control: false,
            alt: false,
            shift: false,
            command: false,
            key: Key::Space,
        };
        let mut key = None;
        for part in value.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => result.control = true,
                "alt" | "option" => result.alt = true,
                "shift" => result.shift = true,
                "cmd" | "command" | "super" | "win" => result.command = true,
                name => {
                    if key.is_some() {
                        bail!("A shortcut needs exactly one key: {value}");
                    }
                    key = Some(match name {
                        "space" => Key::Space,
                        "escape" | "esc" => Key::Escape,
                        "backspace" => Key::Backspace,
                        name if name.len() == 1 && name.as_bytes()[0].is_ascii_alphabetic() => {
                            Key::Letter(name.as_bytes()[0].to_ascii_uppercase())
                        }
                        name if name.len() == 1 && name.as_bytes()[0].is_ascii_digit() => {
                            Key::Digit(name.as_bytes()[0] - b'0')
                        }
                        name if name.starts_with('f') => {
                            let number = name[1..].parse::<u8>()?;
                            if !(1..=12).contains(&number) {
                                bail!("Function keys must be F1–F12");
                            }
                            Key::Function(number)
                        }
                        _ => bail!("Unsupported shortcut key: {part}"),
                    });
                }
            }
        }
        result.key = key.ok_or_else(|| anyhow::anyhow!("Shortcut has no key: {value}"))?;
        if !(result.control || result.alt || result.shift || result.command)
            && !matches!(result.key, Key::Escape | Key::Function(_))
        {
            bail!("Use a modifier (Ctrl, Alt, Shift, Cmd) with {value}");
        }
        Ok(result)
    }
}
