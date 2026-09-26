use anyhow::{Context, Result, bail};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct Credentials {
    pub openrouter: Zeroizing<String>,
}

const SERVICE: &str = "dev.whisper-bro";

pub fn get(name: &str) -> Result<Option<Zeroizing<String>>> {
    if let Ok(value) = std::env::var(name)
        && !value.trim().is_empty()
    {
        return Ok(Some(Zeroizing::new(value.trim().to_owned())));
    }
    match keyring::Entry::new(SERVICE, name)?.get_password() {
        Ok(value) => Ok(Some(Zeroizing::new(value))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("Read {name} from the OS credential store"))
        }
    }
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
            "Missing OpenRouter API key. Run `whisper-bro setup` first, or set OPENROUTER_API_KEY",
        )?;
        Ok(Self { openrouter })
    }
}
