//! Watching what mpv does and when, for measuring how long a seek stalls.

use std::ffi::CStr;
use std::ptr::NonNull;
use std::time::Duration;

use libmpv2_sys as sys;

use crate::core::Player;
use crate::error::{Error, Result, check};

// mpv has no event or property for the next two moments, only log lines.

/// Logged when the first picture after a start or a seek has been drawn
/// (`write_video` in mpv's player/video.c).
const PICTURE_SHOWN: &CStr = c"first video frame after restart shown\n";

/// Logged when the first sound after a start or a seek is handed to the
/// audio device (`audio_start_ao` in mpv's player/audio.c).
const AUDIO_STARTED: &CStr = c"starting audio playback\n";

/// A second listener on a [`Player`]. It sees the steps of a seek, which
/// [`Player::wait_event`] leaves out, and takes no events away from it.
pub struct Probe {
    client: NonNull<sys::mpv_handle>,
    /// Keeps the core alive for as long as the client is attached to it.
    _player: Player,
}

// SAFETY: the mpv client API is thread-safe (mpv/client.h, "Threads"), and
// `wait` takes `&mut self`, so only one thread waits on the client at a time.
unsafe impl Send for Probe {}

impl Drop for Probe {
    fn drop(&mut self) {
        // SAFETY: the client is valid and is not used again.
        unsafe { sys::mpv_destroy(self.client.as_ptr()) }
    }
}

/// A step of a start or a seek.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Moment {
    /// mpv began a seek and dropped the picture and sound it had queued.
    Seeking,
    /// The first picture at the new position is on screen. A seek that is
    /// overtaken by the next one gets this far and no further.
    PictureShown,
    /// Picture and sound are ready at the new position and playback goes
    /// on. This is mpv's `PLAYBACK_RESTART`, which also follows a load.
    Restarted,
    /// Sound was handed to the audio device again.
    AudioStarted,
    /// The playback position changed to this many seconds.
    Position(f64),
    /// mpv is shutting down. No further moments will arrive.
    Shutdown,
}

impl Probe {
    pub fn new(player: &Player) -> Result<Self> {
        // SAFETY: the player's handle is valid and the name is
        // NUL-terminated.
        let client = unsafe { sys::mpv_create_client(player.raw(), c"probe".as_ptr()) };
        let client = NonNull::new(client).ok_or(Error::Create)?;
        let probe = Self { client, _player: player.clone() };

        // SAFETY: the client is valid and the name is NUL-terminated.
        let status = unsafe {
            sys::mpv_observe_property(
                probe.client.as_ptr(),
                0,
                c"time-pos".as_ptr(),
                sys::mpv_format_MPV_FORMAT_DOUBLE,
            )
        };
        check(status, || "observe time-pos".into())?;
        // SAFETY: the client is valid and the level is NUL-terminated.
        let status = unsafe { sys::mpv_request_log_messages(probe.client.as_ptr(), c"v".as_ptr()) };
        check(status, || "request log messages".into())?;
        Ok(probe)
    }

    /// Blocks until the next moment, or returns `None` after `timeout`.
    pub fn wait(&mut self, timeout: Duration) -> Option<Moment> {
        loop {
            // SAFETY: the client is valid, and the returned event is valid
            // until the next wait on this client, which only this loop makes.
            let event =
                unsafe { &*sys::mpv_wait_event(self.client.as_ptr(), timeout.as_secs_f64()) };
            match event.event_id {
                sys::mpv_event_id_MPV_EVENT_NONE => return None,
                sys::mpv_event_id_MPV_EVENT_SHUTDOWN => return Some(Moment::Shutdown),
                sys::mpv_event_id_MPV_EVENT_SEEK => return Some(Moment::Seeking),
                sys::mpv_event_id_MPV_EVENT_PLAYBACK_RESTART => return Some(Moment::Restarted),
                sys::mpv_event_id_MPV_EVENT_LOG_MESSAGE => {
                    // SAFETY: for LOG_MESSAGE, mpv sets `data` to a valid
                    // mpv_event_log_message.
                    let line = unsafe { &*event.data.cast::<sys::mpv_event_log_message>() };
                    // SAFETY: `text` is a valid NUL-terminated string.
                    let text = unsafe { CStr::from_ptr(line.text) };
                    if text == PICTURE_SHOWN {
                        return Some(Moment::PictureShown);
                    }
                    if text == AUDIO_STARTED {
                        return Some(Moment::AudioStarted);
                    }
                }
                sys::mpv_event_id_MPV_EVENT_PROPERTY_CHANGE => {
                    // SAFETY: for PROPERTY_CHANGE, mpv sets `data` to a valid
                    // mpv_event_property.
                    let property = unsafe { &*event.data.cast::<sys::mpv_event_property>() };
                    // Only the position is observed, and as a double. `data`
                    // is null while nothing is loaded.
                    if property.format == sys::mpv_format_MPV_FORMAT_DOUBLE
                        && !property.data.is_null()
                    {
                        // SAFETY: for MPV_FORMAT_DOUBLE, `data` points to a
                        // double.
                        return Some(Moment::Position(unsafe { *property.data.cast::<f64>() }));
                    }
                }
                _ => {}
            }
        }
    }
}
