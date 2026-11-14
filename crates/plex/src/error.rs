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
}
