use std::fmt;

const MESSAGE_LIMIT: usize = 200;

/// Why an OpenRouter request failed, in terms the user can act on.
#[derive(Debug)]
pub enum ApiError {
    Unauthorized,
    NoCredits,
    RateLimited,
    Rejected { status: u16, message: String },
    Server { status: u16, message: String },
    Connect(String),
    Timeout,
    Invalid(String),
}

impl ApiError {
    /// Whether trying the same request again could plausibly succeed.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Server { .. } | Self::Connect(_) | Self::Timeout)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized => formatter
                .write_str("OpenRouter rejected the API key (run `yap setup` or set a new key)"),
            Self::NoCredits => formatter.write_str("OpenRouter account has no credits"),
            Self::RateLimited => {
                formatter.write_str("OpenRouter rate limited the request; try again in a moment")
            }
            Self::Rejected { status, message } => {
                write!(
                    formatter,
                    "OpenRouter rejected the request ({status}): {message}"
                )
            }
            Self::Server { status, message } => {
                write!(formatter, "OpenRouter provider error ({status}): {message}")
            }
            Self::Connect(detail) => write!(formatter, "Could not reach OpenRouter: {detail}"),
            Self::Timeout => formatter.write_str("OpenRouter did not answer in time"),
            Self::Invalid(detail) => {
                write!(
                    formatter,
                    "OpenRouter sent an unreadable response: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for ApiError {}

impl From<reqwest::Error> for ApiError {
    fn from(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout
        } else if error.is_connect() || error.is_request() {
            Self::Connect(error.without_url().to_string())
        } else {
            Self::Invalid(error.without_url().to_string())
        }
    }
}

/// Maps an error response to an `ApiError`, preferring OpenRouter's own
/// `error.message` over the raw body.
pub fn classify(status: u16, body: &str) -> ApiError {
    let message = || {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|json| json["error"]["message"].as_str().map(str::to_owned))
            .unwrap_or_else(|| body.trim().chars().take(MESSAGE_LIMIT).collect())
    };
    match status {
        401 | 403 => ApiError::Unauthorized,
        402 => ApiError::NoCredits,
        429 => ApiError::RateLimited,
        500..=599 => ApiError::Server {
            status,
            message: message(),
        },
        _ => ApiError::Rejected {
            status,
            message: message(),
        },
    }
}

/// Passes a successful response through; otherwise reads the body and
/// returns the classified error.
pub async fn check(response: reqwest::Response) -> Result<reqwest::Response, ApiError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    tracing::debug!(status = status.as_u16(), %body, "OpenRouter error response");
    Err(classify(status.as_u16(), &body))
}

#[cfg(test)]
#[path = "openrouter_tests.rs"]
mod tests;
