use anyhow::{Context, Result, bail};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct Credentials {
    pub openrouter: Zeroizing<String>,
}

const SERVICE: &str = "dev.yap";
const LEGACY_SERVICE: &str = "dev.whisper-bro";

/// Where a resolved key came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Stored,
    Environment,
}

/// A stored key always wins. The environment variable is only a fallback,
/// so an unrelated key exported for other tools is never picked up over the
/// key the user stored for Yap. A blank variable counts as absent.
pub fn resolve(
    stored: Option<Zeroizing<String>>,
    environment: Option<Zeroizing<String>>,
) -> Option<(Zeroizing<String>, Source)> {
    if let Some(value) = stored {
        return Some((value, Source::Stored));
    }
    let value = environment?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some((Zeroizing::new(trimmed.to_owned()), Source::Environment))
}

pub fn get(name: &str) -> Result<Option<Zeroizing<String>>> {
    Ok(get_with_source(name)?.map(|(value, _)| value))
}

pub fn get_with_source(name: &str) -> Result<Option<(Zeroizing<String>, Source)>> {
    let stored = match keyring::Entry::new(SERVICE, name)?.get_password() {
        Ok(value) => Some(Zeroizing::new(value)),
        Err(keyring::Error::NoEntry) => migrate_legacy(name)?,
        Err(error) => {
            return Err(error).with_context(|| format!("Read {name} from the OS credential store"));
        }
    };
    let environment = if stored.is_none() {
        std::env::var(name).ok().map(Zeroizing::new)
    } else {
        None
    };
    Ok(resolve(stored, environment))
}

/// Copies a value stored under the pre-2.0 service name to the current one.
/// The old entry is left in place so a downgrade keeps working.
fn migrate_legacy(name: &str) -> Result<Option<Zeroizing<String>>> {
    let value = match keyring::Entry::new(LEGACY_SERVICE, name)?.get_password() {
        Ok(value) => Zeroizing::new(value),
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Read legacy {name} from the OS credential store"));
        }
    };
    if let Err(error) = keyring::Entry::new(SERVICE, name).and_then(|e| e.set_password(&value)) {
        tracing::warn!(%error, "Could not copy the stored {name} to the yap credential entry");
    }
    Ok(Some(value))
}

pub fn set(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("API key cannot be empty");
    }
    keyring::Entry::new(SERVICE, name)?
        .set_password(value.trim())
        .with_context(|| format!("Save {name} in the OS credential store"))
}

impl Credentials {
    pub fn load() -> Result<Self> {
        let openrouter = get("OPENROUTER_API_KEY")?.context(
            "Missing OpenRouter API key. Run `yap setup` first, or set OPENROUTER_API_KEY",
        )?;
        Ok(Self { openrouter })
    }
}

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;
