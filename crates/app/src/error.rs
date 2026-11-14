pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Usage(&'static str),

    #[error(transparent)]
    Player(#[from] player::Error),

    #[error(transparent)]
    Plex(#[from] plex::Error),

    #[error("{path}: {source}")]
    File {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[error("the async runtime did not start: {0}")]
    Runtime(std::io::Error),

    #[error("this account has no Plex Media Server")]
    NoServer,

    #[error("{0} has no file to play")]
    NotPlayable(String),

    #[error("the window could not be drawn into an image")]
    Snapshot,

    #[error("not signed in, run `reel login` first")]
    SignedOut,

    #[error("the home directory is unknown because HOME is not set")]
    NoHome,
}

/// The error followed by everything that caused it. Network errors in
/// particular only name the real problem several causes down.
pub fn with_causes(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(error) = cause {
        text.push_str(": ");
        text.push_str(&error.to_string());
        cause = error.source();
    }
    text
}
