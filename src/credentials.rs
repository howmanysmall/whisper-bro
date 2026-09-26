use anyhow::{Context, Result, bail};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct Credentials {
    pub deepgram: Zeroizing<String>,
    pub groq: Option<Zeroizing<String>>,
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
    pub fn load(cleanup: bool) -> Result<Self> {
        let deepgram = get("DEEPGRAM_API_KEY")?.context(
            "Missing Deepgram API key. Run `whisper-bro setup` first, or set DEEPGRAM_API_KEY",
        )?;
        let groq = if cleanup {
            Some(get("GROQ_API_KEY")?.context("Cleanup is enabled but GROQ_API_KEY is missing. Run `whisper-bro setup` or disable cleanup in config.toml")?)
        } else {
            None
        };
        Ok(Self { deepgram, groq })
    }
}
