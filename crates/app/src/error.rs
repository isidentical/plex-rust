pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Usage(&'static str),

    #[error(transparent)]
    Player(#[from] player::Error),

    #[error(transparent)]
    Plex(#[from] plex::Error),

    #[error("the Keychain refused: {0}")]
    Keychain(#[from] security_framework::base::Error),

    #[error("{path}: {source}")]
    File {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[error("the async runtime did not start: {0}")]
    Runtime(std::io::Error),

    #[error("not signed in, run `reel login` first")]
    SignedOut,

    #[error("the home directory is unknown because HOME is not set")]
    NoHome,
}
