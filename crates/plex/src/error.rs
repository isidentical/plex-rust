pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The request failed, the server answered with an error status, or the
    /// response did not have the expected shape.
    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("the sign-in link expired before it was approved")]
    PinExpired,

    #[error("{server} did not answer at any of: {}", tried.join(", "))]
    Unreachable { server: String, tried: Vec<String> },

    /// The server answered, but not with what its API description promises.
    #[error("unexpected response from {path}")]
    Shape { path: String, source: serde_json::Error },

    /// Asked of [`MediaServer::remembered`](crate::MediaServer::remembered)
    /// for something it never kept.
    #[error("no answer to this was kept from an earlier launch")]
    NotRemembered,

    #[error("the server has no item {0}")]
    Missing(crate::model::RatingKey),

    #[error("{path}: {source}")]
    File { path: std::path::PathBuf, source: std::io::Error },

    /// A download, or the file the server offers for it, is not as long as
    /// the server said the file is. Trying again would not change that.
    #[error("the file should be {expected} bytes long, not {found}")]
    SizeMismatch { expected: u64, found: u64 },

    #[error("asked for the file from byte {asked} on, and the server answered {answer}")]
    Resume { asked: u64, answer: String },

    #[error("the server stopped sending the file")]
    Stalled,

    #[error("the download was cancelled")]
    Cancelled,

    #[error("the server holds no file for {0}")]
    NoFile(String),

    /// HTTP has no way to send a control character in a header.
    #[error("{0:?} cannot be sent to Plex: it has a control character in it")]
    Header(String),
}

impl Error {
    /// Whether nothing answered: no connection, or no reply in time.
    pub fn is_unanswered(&self) -> bool {
        match self {
            Self::Unreachable { .. } => true,
            Self::Http(error) => error.is_connect() || error.is_timeout(),
            _ => false,
        }
    }

    /// Whether the answer was that the token is not accepted.
    pub fn is_unauthorized(&self) -> bool {
        self.status() == Some(reqwest::StatusCode::UNAUTHORIZED)
    }

    /// Whether the answer was that what was asked for is not there.
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing(_)) || self.status() == Some(reqwest::StatusCode::NOT_FOUND)
    }

    fn status(&self) -> Option<reqwest::StatusCode> {
        match self {
            Self::Http(error) => error.status(),
            _ => None,
        }
    }
}
