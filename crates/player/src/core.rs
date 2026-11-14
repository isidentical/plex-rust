use std::ffi::{CStr, CString};
use std::ptr::{self, NonNull};
use std::sync::Arc;
use std::time::Duration;

use libmpv2_sys as sys;

use crate::error::{Error, Result, check, message};

/// Options applied before mpv starts. See SPEC.md section 7 for the reasons.
const OPTIONS: &[(&str, &str)] = &[
    // Frames are drawn by `Renderer`, mpv opens no window of its own.
    ("vo", "libmpv"),
    ("hwdec", "videotoolbox"),
    ("cache", "yes"),
    ("demuxer-max-bytes", "2GiB"),
    ("demuxer-max-back-bytes", "512MiB"),
    // Refill the buffer in bursts so the network can idle in between.
    ("demuxer-hysteresis-secs", "10"),
];

/// The mpv property that is set while playback waits for the network.
const STALLED: &str = "paused-for-cache";

/// mpv's status for a property that has no value right now, such as the
/// position while nothing is loaded.
const UNAVAILABLE: libc::c_int = sys::mpv_error_MPV_ERROR_PROPERTY_UNAVAILABLE;

/// The mpv core. Cloning is cheap and every clone drives the same playback.
///
/// mpv shuts down when the last clone is dropped.
#[derive(Clone)]
pub struct Player {
    handle: Arc<Handle>,
}

struct Handle(NonNull<sys::mpv_handle>);

// SAFETY: the mpv client API is thread-safe: any function may be called on a
// handle from any thread (mpv/client.h, "Threads").
unsafe impl Send for Handle {}
// SAFETY: as above.
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is valid and, this being the last reference, no
        // other call on it can be in progress.
        unsafe { sys::mpv_terminate_destroy(self.0.as_ptr()) }
    }
}

/// Something to play.
#[derive(Debug, Clone, Default)]
pub struct Source {
    /// A file path or URL.
    pub location: String,
    /// Extra HTTP request headers as name and value, for URLs that need a
    /// token.
    pub headers: Vec<(String, String)>,
    /// Where to start playing.
    pub start: Duration,
    /// The audio track to play, by its id. `None` leaves the choice to the
    /// file's own default.
    pub audio: Option<i64>,
    /// The subtitle track to show, by its id. `None` shows no subtitles,
    /// even when the file marks a track as its default.
    pub subtitles: Option<i64>,
}

impl Source {
    /// A source that plays `location` from the beginning.
    pub fn new(location: impl Into<String>) -> Self {
        Self { location: location.into(), ..Self::default() }
    }
}

/// The range of colour and brightness a picture is encoded in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colors {
    /// Standard dynamic range.
    Sdr,
    /// High dynamic range in the BT.2020 gamut, as on a UHD Blu-ray.
    HdrBt2020,
    /// High dynamic range in the Display P3 gamut.
    HdrDisplayP3,
}

/// One stream inside the loaded file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// mpv's id for the track, unique among tracks of the same kind.
    pub id: i64,
    pub kind: TrackKind,
    /// The name the file gives the track, such as "Director's commentary".
    pub title: Option<String>,
    /// A language code, such as `eng`.
    pub language: Option<String>,
    /// Whether this is the track being played or shown.
    pub selected: bool,
}

/// A named point in the loaded file, such as the start of a scene.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    pub start: Duration,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
    Subtitles,
}

/// What happened in the player, as returned by [`Player::wait_event`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The file is open and playback is starting.
    Loaded,
    /// Playback has stalled waiting for data (`true`) or picked up again
    /// (`false`).
    Buffering(bool),
    /// The picture's format is known or has changed. This is the moment
    /// [`Player::video_colors`] has an answer.
    VideoChanged,
    Ended(EndReason),
    /// mpv is shutting down. No further events will arrive.
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// Played to the end.
    Finished,
    /// Stopped or replaced on request.
    Stopped,
    /// Playback failed. Holds mpv's description of the error.
    Failed(String),
}

impl Player {
    pub fn new() -> Result<Self> {
        // SAFETY: mpv_create has no preconditions.
        let handle = NonNull::new(unsafe { sys::mpv_create() }).ok_or(Error::Create)?;
        let player = Self { handle: Arc::new(Handle(handle)) };

        for (name, value) in OPTIONS {
            let (c_name, c_value) = (c_string(name)?, c_string(value)?);
            // SAFETY: the handle is valid and both strings are NUL-terminated.
            let status = unsafe {
                sys::mpv_set_option_string(player.raw(), c_name.as_ptr(), c_value.as_ptr())
            };
            check(status, || format!("set option {name}={value}"))?;
        }

        // SAFETY: the handle is valid and has not been initialized yet.
        check(unsafe { sys::mpv_initialize(player.raw()) }, || "initialize mpv".into())?;

        // Ask to be told when playback stalls for lack of data and when it
        // picks up again. `wait_event` turns this into `Event::Buffering`.
        let stalled = c_string(STALLED)?;
        // SAFETY: the handle is valid and the name is NUL-terminated.
        let status = unsafe {
            sys::mpv_observe_property(
                player.raw(),
                0,
                stalled.as_ptr(),
                sys::mpv_format_MPV_FORMAT_FLAG,
            )
        };
        check(status, || format!("observe {STALLED}"))?;
        Ok(player)
    }

    /// Starts playing `source`, replacing what is playing.
    pub fn load(&self, source: &Source) -> Result<()> {
        let headers: Vec<String> =
            source.headers.iter().map(|(name, value)| format!("{name}: {value}")).collect();
        self.set_strings("http-header-fields", &headers)?;

        let mut options = vec![format!("start={}", source.start.as_secs_f64())];
        options.extend(source.audio.map(|id| format!("aid={id}")));
        options
            .push(source.subtitles.map_or_else(|| "sid=no".to_owned(), |id| format!("sid={id}")));
        // "-1" is the playlist index argument, which "replace" ignores.
        self.command(&["loadfile", &source.location, "replace", "-1", &options.join(",")])
    }

    /// Asks mpv to shut down. [`Event::Shutdown`] follows.
    pub fn quit(&self) -> Result<()> {
        self.command(&["quit"])
    }

    pub fn seek(&self, position: Duration) -> Result<()> {
        self.command(&["seek", &position.as_secs_f64().to_string(), "absolute"])
    }

    /// Moves the position by `seconds`, backwards if negative. Unlike a
    /// [`seek`](Self::seek) to a position worked out from the current one,
    /// several of these in quick succession add up, even when an earlier
    /// one has not finished.
    pub fn seek_by(&self, seconds: f64) -> Result<()> {
        self.command(&["seek", &seconds.to_string(), "relative+exact"])
    }

    /// Moves the position by at least `seconds`, backwards if negative, to
    /// the nearest picture that can be shown without decoding the ones
    /// before it. This is for a key that is held down, where a new picture
    /// with every repeat matters and where exactly it lands does not.
    ///
    /// [`seek_by`](Self::seek_by) decodes from the keyframe before its
    /// target, which for a 4K film with a keyframe every 10 seconds takes
    /// 270 ms on average. mpv gives up on a seek that takes over 300 ms
    /// when the next one arrives (`execute_queued_seek` in mpv's
    /// player/playloop.c), so a held key showed 1 to 3 pictures a second
    /// there. Going from keyframe to keyframe takes about 20 ms and shows
    /// one for nearly every repeat.
    pub fn scan_by(&self, seconds: f64) -> Result<()> {
        self.command(&["seek", &seconds.to_string(), "relative+keyframes"])
    }

    pub fn set_paused(&self, paused: bool) -> Result<()> {
        self.set_string("pause", if paused { "yes" } else { "no" })
    }

    pub fn set_muted(&self, muted: bool) -> Result<()> {
        self.set_string("mute", if muted { "yes" } else { "no" })
    }

    pub fn is_paused(&self) -> Result<bool> {
        Ok(self.string("pause")?.as_deref() == Some("yes"))
    }

    /// Sets the playback rate, where 1.0 is normal speed.
    pub fn set_speed(&self, speed: f64) -> Result<()> {
        self.set_string("speed", &speed.to_string())
    }

    /// The playback position, or `None` while nothing is loaded.
    pub fn position(&self) -> Result<Option<Duration>> {
        self.seconds("time-pos")
    }

    /// The length of the loaded file, or `None` if it is not known.
    pub fn duration(&self) -> Result<Option<Duration>> {
        self.seconds("duration")
    }

    /// How much playback time is buffered ahead of the position.
    pub fn buffered(&self) -> Result<Option<Duration>> {
        self.seconds("demuxer-cache-duration")
    }

    /// The position up to which the file is buffered and can play without
    /// waiting, or `None` while nothing is.
    pub fn buffered_until(&self) -> Result<Option<Duration>> {
        self.seconds("demuxer-cache-time")
    }

    /// The chapters of the loaded file, in order. Most files have none.
    pub fn chapters(&self) -> Result<Vec<Chapter>> {
        let count: usize = self.number("chapter-list/count")?.unwrap_or(0);
        (0..count).map(|index| self.chapter(index)).collect()
    }

    fn chapter(&self, index: usize) -> Result<Chapter> {
        let start = self.seconds(&format!("chapter-list/{index}/time"))?;
        Ok(Chapter {
            start: start.unwrap_or_default(),
            title: self.string(&format!("chapter-list/{index}/title"))?,
        })
    }

    /// The hardware decoder in use, or `None` when decoding in software.
    pub fn hardware_decoder(&self) -> Result<Option<String>> {
        let decoder = self.string("hwdec-current")?;
        Ok(decoder.filter(|name| name != "no"))
    }

    /// Frames dropped since the file was loaded because they came too late.
    pub fn dropped_frames(&self) -> Result<u64> {
        Ok(self.number("frame-drop-count")?.unwrap_or(0))
    }

    /// How the picture now playing is encoded, or `None` before the first
    /// frame has been decoded.
    pub fn video_colors(&self) -> Result<Option<Colors>> {
        let (Some(transfer), Some(primaries)) =
            (self.string("video-params/gamma")?, self.string("video-params/primaries")?)
        else {
            return Ok(None);
        };
        let is_hdr = matches!(transfer.as_str(), "pq" | "hlg");
        Ok(Some(match (is_hdr, primaries.as_str()) {
            (true, "bt.2020") => Colors::HdrBt2020,
            (true, "display-p3") => Colors::HdrDisplayP3,
            _ => Colors::Sdr,
        }))
    }

    /// Sets what the rendered frames are encoded as. It must agree with
    /// the colour space of the surface they are drawn into.
    ///
    /// SDR output is sRGB. HDR output keeps the picture's own gamut and
    /// uses the PQ curve, into which HLG pictures are converted.
    pub fn set_output_colors(&self, colors: Colors) -> Result<()> {
        let (primaries, transfer) = match colors {
            Colors::Sdr => ("bt.709", "srgb"),
            Colors::HdrBt2020 => ("bt.2020", "pq"),
            Colors::HdrDisplayP3 => ("display-p3", "pq"),
        };
        self.set_string("target-prim", primaries)?;
        self.set_string("target-trc", transfer)
    }

    /// The volume, where 100 is the file's own level.
    pub fn volume(&self) -> Result<f64> {
        Ok(self.number("volume")?.unwrap_or(100.0))
    }

    pub fn set_volume(&self, volume: f64) -> Result<()> {
        self.set_string("volume", &volume.to_string())
    }

    pub fn is_muted(&self) -> Result<bool> {
        Ok(self.string("mute")?.as_deref() == Some("yes"))
    }

    /// The video, audio and subtitle tracks of the loaded file.
    pub fn tracks(&self) -> Result<Vec<Track>> {
        let count: usize = self.number("track-list/count")?.unwrap_or(0);
        (0..count).map(|index| self.track(index)).collect()
    }

    /// Plays the audio track with this id.
    pub fn select_audio(&self, id: i64) -> Result<()> {
        self.set_string("aid", &id.to_string())
    }

    /// Shows the subtitle track with this id, or none.
    pub fn select_subtitles(&self, id: Option<i64>) -> Result<()> {
        self.set_string("sid", &id.map_or_else(|| "no".to_owned(), |id| id.to_string()))
    }

    fn track(&self, index: usize) -> Result<Track> {
        let field = |name: &str| self.string(&format!("track-list/{index}/{name}"));
        let kind = match field("type")?.as_deref() {
            Some("video") => TrackKind::Video,
            Some("audio") => TrackKind::Audio,
            Some("sub") => TrackKind::Subtitles,
            other => {
                return Err(Error::Mpv {
                    action: format!("read track {index}"),
                    message: format!("unknown track type {other:?}"),
                });
            }
        };
        Ok(Track {
            id: self.number(&format!("track-list/{index}/id"))?.unwrap_or(0),
            kind,
            title: field("title")?,
            language: field("lang")?,
            selected: field("selected")?.as_deref() == Some("yes"),
        })
    }

    /// Blocks until something happens that the caller needs to know about.
    pub fn wait_event(&self) -> Event {
        loop {
            // SAFETY: the handle is valid. A negative timeout waits forever,
            // and the returned event is valid until the next wait_event call
            // on this handle, which only this loop makes.
            let event = unsafe { &*sys::mpv_wait_event(self.raw(), -1.0) };
            match event.event_id {
                sys::mpv_event_id_MPV_EVENT_SHUTDOWN => return Event::Shutdown,
                sys::mpv_event_id_MPV_EVENT_FILE_LOADED => return Event::Loaded,
                sys::mpv_event_id_MPV_EVENT_VIDEO_RECONFIG => return Event::VideoChanged,
                sys::mpv_event_id_MPV_EVENT_PROPERTY_CHANGE => {
                    // SAFETY: for PROPERTY_CHANGE, mpv sets `data` to a valid
                    // mpv_event_property.
                    let property = unsafe { &*event.data.cast::<sys::mpv_event_property>() };
                    // Only one property is observed, and as a flag. `data`
                    // is null while the property has no value.
                    if property.format == sys::mpv_format_MPV_FORMAT_FLAG
                        && !property.data.is_null()
                    {
                        // SAFETY: for MPV_FORMAT_FLAG, `data` points to an int.
                        let stalled = unsafe { *property.data.cast::<libc::c_int>() } != 0;
                        return Event::Buffering(stalled);
                    }
                }
                sys::mpv_event_id_MPV_EVENT_END_FILE => {
                    // SAFETY: for END_FILE, mpv sets `data` to a valid
                    // mpv_event_end_file.
                    let end = unsafe { &*event.data.cast::<sys::mpv_event_end_file>() };
                    return Event::Ended(end_reason(end));
                }
                _ => {}
            }
        }
    }

    pub(crate) fn raw(&self) -> *mut sys::mpv_handle {
        self.handle.0.as_ptr()
    }

    fn command(&self, args: &[&str]) -> Result<()> {
        let owned = args.iter().map(|arg| c_string(arg)).collect::<Result<Vec<_>>>()?;
        let mut pointers: Vec<_> = owned.iter().map(|arg| arg.as_ptr()).collect();
        pointers.push(ptr::null());
        // SAFETY: the handle is valid and `pointers` is a NULL-terminated
        // array of NUL-terminated strings that outlive the call.
        let status = unsafe { sys::mpv_command(self.raw(), pointers.as_mut_ptr()) };
        check(status, || format!("run command {args:?}"))
    }

    fn set_string(&self, name: &str, value: &str) -> Result<()> {
        let (c_name, c_value) = (c_string(name)?, c_string(value)?);
        // SAFETY: the handle is valid and both strings are NUL-terminated.
        let status =
            unsafe { sys::mpv_set_property_string(self.raw(), c_name.as_ptr(), c_value.as_ptr()) };
        check(status, || format!("set {name}={value}"))
    }

    /// Sets a list property. Passing the items as a list, and not as one
    /// comma-separated string, keeps commas inside an item intact.
    fn set_strings(&self, name: &str, items: &[String]) -> Result<()> {
        let c_name = c_string(name)?;
        let owned = items.iter().map(|item| c_string(item)).collect::<Result<Vec<_>>>()?;
        let mut nodes: Vec<sys::mpv_node> = owned
            .iter()
            .map(|item| sys::mpv_node {
                u: sys::mpv_node__bindgen_ty_1 { string: item.as_ptr().cast_mut() },
                format: sys::mpv_format_MPV_FORMAT_STRING,
            })
            .collect();
        let mut list = sys::mpv_node_list {
            num: libc::c_int::try_from(nodes.len()).unwrap_or(libc::c_int::MAX),
            values: nodes.as_mut_ptr(),
            keys: ptr::null_mut(),
        };
        let mut node = sys::mpv_node {
            u: sys::mpv_node__bindgen_ty_1 { list: &mut list },
            format: sys::mpv_format_MPV_FORMAT_NODE_ARRAY,
        };
        // SAFETY: the handle is valid, the name is NUL-terminated, and the
        // node is a well-formed array of `num` strings. mpv copies what it
        // needs and does not keep or modify any of these pointers.
        let status = unsafe {
            sys::mpv_set_property(
                self.raw(),
                c_name.as_ptr(),
                sys::mpv_format_MPV_FORMAT_NODE,
                ptr::from_mut(&mut node).cast(),
            )
        };
        check(status, || format!("set {name}"))
    }

    /// Reads a property as text. `None` means it has no value right now.
    fn string(&self, name: &str) -> Result<Option<String>> {
        let c_name = c_string(name)?;
        let mut value: *mut libc::c_char = ptr::null_mut();
        // SAFETY: the handle is valid, the name is NUL-terminated, and for
        // MPV_FORMAT_STRING mpv writes a `char *` through the data pointer.
        let status = unsafe {
            sys::mpv_get_property(
                self.raw(),
                c_name.as_ptr(),
                sys::mpv_format_MPV_FORMAT_STRING,
                ptr::from_mut(&mut value).cast(),
            )
        };
        if status == UNAVAILABLE {
            return Ok(None);
        }
        check(status, || format!("read {name}"))?;

        // SAFETY: on success mpv stored a NUL-terminated string that we own.
        let text = unsafe { CStr::from_ptr(value) }.to_string_lossy().into_owned();
        // SAFETY: `value` was allocated by mpv and is not used again.
        unsafe { sys::mpv_free(value.cast()) };
        Ok(Some(text))
    }

    /// Reads a property that mpv prints as a number.
    fn number<T: std::str::FromStr>(&self, name: &str) -> Result<Option<T>> {
        let Some(text) = self.string(name)? else {
            return Ok(None);
        };
        text.parse().map(Some).map_err(|_| Error::Mpv {
            action: format!("read {name}"),
            message: format!("not a number: {text}"),
        })
    }

    fn seconds(&self, name: &str) -> Result<Option<Duration>> {
        let c_name = c_string(name)?;
        let mut seconds = 0.0_f64;
        // SAFETY: the handle is valid, the name is NUL-terminated, and for
        // MPV_FORMAT_DOUBLE mpv writes a `double` through the data pointer.
        let status = unsafe {
            sys::mpv_get_property(
                self.raw(),
                c_name.as_ptr(),
                sys::mpv_format_MPV_FORMAT_DOUBLE,
                ptr::from_mut(&mut seconds).cast(),
            )
        };
        if status == UNAVAILABLE {
            return Ok(None);
        }
        check(status, || format!("read {name}"))?;
        // Streams can report a slightly negative position right after a seek.
        Ok(Some(Duration::from_secs_f64(seconds.max(0.0))))
    }
}

fn end_reason(end: &sys::mpv_event_end_file) -> EndReason {
    match end.reason {
        sys::mpv_end_file_reason_MPV_END_FILE_REASON_EOF => EndReason::Finished,
        sys::mpv_end_file_reason_MPV_END_FILE_REASON_ERROR => EndReason::Failed(message(end.error)),
        _ => EndReason::Stopped,
    }
}

fn c_string(text: &str) -> Result<CString> {
    CString::new(text).map_err(|_| Error::Nul(text.to_owned()))
}
