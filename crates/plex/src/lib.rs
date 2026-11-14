//! A client for the plex.tv and Plex Media Server HTTP APIs.
//!
//! Endpoints and parameters follow Plex's published API description at
//! <https://developer.plex.tv/pms/>.

pub mod auth;
mod client;
pub mod discovery;
mod error;

pub use crate::client::{Client, Identity, Token};
pub use crate::error::{Error, Result};
