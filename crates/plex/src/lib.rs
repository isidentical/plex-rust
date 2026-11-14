//! A client for the plex.tv and Plex Media Server HTTP APIs.
//!
//! Endpoints and parameters follow Plex's published API description at
//! <https://developer.plex.tv/pms/>.

pub mod auth;
mod client;
pub mod discovery;
mod download;
mod error;
mod image;
pub mod library;
pub mod model;
pub mod playback;
mod server;

pub use crate::client::{Client, Identity, Token};
pub use crate::download::Transfer;
pub use crate::error::{Error, Result};
pub use crate::server::{MediaServer, stable_hash};
pub use reqwest::Method;
