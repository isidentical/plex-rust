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
}

impl Error {
    /// Whether Plex turned the token down. Only signing in again helps.
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Http(error) if error.status() == Some(reqwest::StatusCode::UNAUTHORIZED))
    }
}
