//! Video playback through libmpv.
//!
//! [`Player`] owns the mpv core: it loads media and reports and changes
//! playback state. [`Renderer`] draws the current video frame with OpenGL.
//! [`Probe`] times what the core does and [`Offscreen`] draws where nobody
//! sees it, both for measuring.

mod cadence;
mod clock;
mod core;
mod error;
mod offscreen;
mod probe;
mod render;

pub use crate::cadence::{Cadence, Refresh};
pub use crate::clock::{
    HostTime, cpu_time, cpu_time_by_thread, run_thread_at_interactive_priority,
};
pub use crate::core::{
    Chapter, Colors, DolbyVision, EndReason, Event, Player, Source, Track, TrackKind,
};
pub use crate::error::{Error, Result};
pub use crate::offscreen::Offscreen;
pub use crate::probe::{Moment, Probe};
pub use crate::render::{FrameTiming, Pending, Renderer};
