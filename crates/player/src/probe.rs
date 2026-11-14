//! Watching what mpv does and when, for measuring how long a seek stalls.

use std::ffi::CStr;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use libmpv2_sys as sys;

use crate::core::Player;
use crate::error::{Error, Result, check};

/// Which observed property a change is for.
const POSITION: u64 = 1;
const AV_OFFSET: u64 = 2;

// mpv has no event or property for the next moments, only log lines.

/// Logged when the first picture after a start or a seek has been drawn
/// (`write_video` in mpv's player/video.c).
const PICTURE_SHOWN: &CStr = c"first video frame after restart shown\n";

/// Logged when the first sound after a start or a seek is handed to the
/// audio device (`audio_start_ao` in mpv's player/audio.c).
const AUDIO_STARTED: &CStr = c"starting audio playback\n";

/// Logged by the CoreAudio output when it opens the device, one after the
/// other (`ca_get_hardware_latency` in mpv's audio/out/ao_coreaudio.c).
/// Their sum is how long sound takes from the output to the ear.
const UNIT_LATENCY: &str = "audiounit latency [ns]: ";
const DEVICE_LATENCY: &str = "device latency [ns]: ";

/// A second listener on a [`Player`]. It sees the steps of a seek, which
/// [`Player::wait_event`] leaves out, and takes no events away from it.
pub struct Probe {
    client: NonNull<sys::mpv_handle>,
    /// The audio unit's part of the output latency, until the device's part
    /// is logged and both are reported as one.
    unit_latency: Duration,
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
    /// How far the sound being heard is ahead of the picture, in seconds,
    /// as mpv works it out from what the audio device reports. mpv updates
    /// it with every frame while picture and sound both play.
    AvOffset(f64),
    /// The audio device was opened. Sound handed to it is heard this much
    /// later.
    AudioLatency(Duration),
    /// mpv is shutting down. No further moments will arrive.
    Shutdown,
}

impl Probe {
    pub fn new(player: &Player) -> Result<Self> {
        // SAFETY: the player's handle is valid and the name is
        // NUL-terminated.
        let client = unsafe { sys::mpv_create_client(player.raw(), c"probe".as_ptr()) };
        let client = NonNull::new(client).ok_or(Error::Create)?;
        let probe = Self { client, unit_latency: Duration::ZERO, _player: player.clone() };

        for (id, name) in [(POSITION, c"time-pos"), (AV_OFFSET, c"avsync")] {
            // SAFETY: the client is valid and the name is NUL-terminated.
            let status = unsafe {
                sys::mpv_observe_property(
                    probe.client.as_ptr(),
                    id,
                    name.as_ptr(),
                    sys::mpv_format_MPV_FORMAT_DOUBLE,
                )
            };
            check(status, || format!("observe {name:?}"))?;
        }
        // SAFETY: the client is valid and the level is NUL-terminated.
        let status = unsafe { sys::mpv_request_log_messages(probe.client.as_ptr(), c"v".as_ptr()) };
        check(status, || "request log messages".into())?;
        Ok(probe)
    }

    /// Blocks until the next moment, or returns `None` after `timeout`.
    pub fn wait(&mut self, timeout: Duration) -> Option<Moment> {
        // mpv also reports what is no moment, such as every other log
        // line, and none of that may start the wait over.
        let until = Instant::now() + timeout;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            // SAFETY: the client is valid, and the returned event is valid
            // until the next wait on this client, which only this loop makes.
            let event = unsafe { &*sys::mpv_wait_event(self.client.as_ptr(), left.as_secs_f64()) };
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
                    let text = text.to_string_lossy();
                    if let Some(latency) = nanoseconds(&text, UNIT_LATENCY) {
                        self.unit_latency = latency;
                    }
                    if let Some(latency) = nanoseconds(&text, DEVICE_LATENCY) {
                        return Some(Moment::AudioLatency(self.unit_latency + latency));
                    }
                }
                sys::mpv_event_id_MPV_EVENT_PROPERTY_CHANGE => {
                    // SAFETY: for PROPERTY_CHANGE, mpv sets `data` to a valid
                    // mpv_event_property.
                    let property = unsafe { &*event.data.cast::<sys::mpv_event_property>() };
                    // Both properties are observed as doubles. `data` is
                    // null while the property has no value.
                    if property.format == sys::mpv_format_MPV_FORMAT_DOUBLE
                        && !property.data.is_null()
                    {
                        // SAFETY: for MPV_FORMAT_DOUBLE, `data` points to a
                        // double.
                        let value = unsafe { *property.data.cast::<f64>() };
                        return Some(match event.reply_userdata {
                            AV_OFFSET => Moment::AvOffset(value),
                            _ => Moment::Position(value),
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

/// The number of nanoseconds that a log line of the form `<label><number>`
/// reports.
fn nanoseconds(line: &str, label: &str) -> Option<Duration> {
    line.strip_prefix(label)?.trim_end().parse().ok().map(Duration::from_nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_is_read_from_its_log_line() {
        let line = "device latency [ns]: 171000000\n";
        assert_eq!(nanoseconds(line, DEVICE_LATENCY), Some(Duration::from_millis(171)));
        assert_eq!(nanoseconds(line, UNIT_LATENCY), None);
        assert_eq!(nanoseconds("device latency [ns]: soon\n", DEVICE_LATENCY), None);
    }
}
