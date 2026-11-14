use std::ffi::CStr;

use libmpv2_sys as sys;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("mpv could not be created")]
    Create,

    /// mpv rejected a call. `action` says what was attempted.
    #[error("{action}: {message}")]
    Mpv { action: String, message: String },

    #[error("OpenGL is not usable: {0}")]
    OpenGl(&'static str),

    #[error("{0:?} contains a NUL byte and cannot be passed to mpv")]
    Nul(String),
}

/// Turns an mpv status code into a `Result`, naming the failed action.
pub(crate) fn check(status: libc::c_int, action: impl FnOnce() -> String) -> Result<()> {
    if status >= 0 {
        return Ok(());
    }
    Err(Error::Mpv {
        action: action(),
        message: message(status),
    })
}

pub(crate) fn message(status: libc::c_int) -> String {
    // SAFETY: mpv_error_string returns a static NUL-terminated string for
    // every input, including unknown codes.
    unsafe { CStr::from_ptr(sys::mpv_error_string(status)) }
        .to_string_lossy()
        .into_owned()
}
