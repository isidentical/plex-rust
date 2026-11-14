//! Video playback through libmpv.
//!
//! [`Player`] owns the mpv core: it loads media and reports and changes
//! playback state. [`Renderer`] draws the current video frame with OpenGL.

mod core;
mod error;
mod render;

pub use crate::core::{EndReason, Event, Player, Source, Track, TrackKind};
pub use crate::error::{Error, Result};
pub use crate::render::Renderer;
