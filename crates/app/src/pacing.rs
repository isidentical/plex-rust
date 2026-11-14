//! `plex-rust pacing`: measures how evenly video frames reach the screen.
//!
//! Plays muted in a window that takes no focus, tells the server nothing,
//! and prints one JSON report. Every figure in it is a distribution over
//! the frames of the measured stretch, in milliseconds unless named
//! otherwise.

#![expect(
    deprecated,
    reason = "CVDisplayLink is the one display clock that calls back off the main thread, \
              so a busy main thread cannot distort the refresh times it records"
)]

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationOcclusionState, NSBackingStoreType,
    NSScreen, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_core_foundation::CFRetained;
use objc2_core_video::{CVDisplayLink, CVOptionFlags, CVReturn, CVTimeStamp};
use objc2_foundation::{NSPoint, NSRect, NSSize, ns_string};
use player::{EndReason, Event, FrameTiming, HostTime, Player, Source, cpu_time};
use plex::model::RatingKey;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::video_layer::{self, VideoLayer};
use crate::{USAGE, account, source};

/// Playback settles for this long before frames are counted, after the
/// file opens and after each change of speed.
const SETTLE: Duration = Duration::from_secs(2);

/// The mpv counters whose growth over the measured stretch is reported.
const COUNTERS: [&str; 4] = [
    "frame-drop-count",
    "decoder-frame-drop-count",
    "vo-delayed-frame-count",
    "mistimed-frame-count",
];

/// The mpv properties reported as they stand at the end.
const READINGS: [&str; 7] = [
    "container-fps",
    "estimated-vf-fps",
    "display-fps",
    "vsync-jitter",
    "avsync",
    "video-sync",
    "hwdec-current",
];

/// The times the display refreshed, as the display link reported them.
type Refreshes = Mutex<Vec<HostTime>>;

/// Plays `what` at each of `speeds` in turn, in one window, and reports
/// on `seconds` of each.
pub fn run(what: &str, seconds: &str, speeds: &[&str]) -> Result<()> {
    let seconds: u64 = seconds.parse().map_err(|_| Error::Usage(USAGE))?;
    let speeds: Option<Vec<f64>> = speeds.iter().map(|speed| speed.parse().ok()).collect();
    let speeds = speeds.filter(|speeds| !speeds.is_empty()).ok_or(Error::Usage(USAGE))?;
    let source = resolve(what)?;
    let main_thread =
        MainThreadMarker::new().unwrap_or_else(|| panic!("main() runs on the main thread"));

    let player = Player::new()?;
    let app = NSApplication::sharedApplication(main_thread);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let window = video_window(main_thread, &player)?;
    player.set_muted(true)?;
    player.load(&source)?;
    window.orderFrontRegardless();

    // Lives until the process exits, like the display link that fills it.
    let refreshes: &'static Refreshes = Box::leak(Box::default());
    let _link = watch_refreshes(refreshes);

    let events = player.clone();
    thread::spawn(move || follow_events(&events));
    thread::spawn(move || {
        let length = Duration::from_secs(seconds);
        let reports: player::Result<Vec<Value>> =
            speeds.iter().map(|speed| measure(&player, refreshes, *speed, length)).collect();
        match reports {
            Ok(reports) => {
                println!("{}", Value::Array(reports));
                std::process::exit(0);
            }
            Err(error) => {
                eprintln!("plex-rust: {error}");
                std::process::exit(1);
            }
        }
    });

    app.run();
    Ok(())
}

fn resolve(what: &str) -> Result<Source> {
    let Some(key) = what.strip_prefix("item:") else {
        return Ok(Source::new(what));
    };
    let (client, token) = account::signed_in()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::Runtime)?;
    runtime.block_on(async {
        let servers = plex::discovery::servers(&client, &token).await?;
        let server = servers.first().ok_or(Error::NoServer)?;
        let server = plex::MediaServer::connect(&client, server, None).await?;
        let item = server.item(&RatingKey::new(key)).await?;
        source::of(&server, &item)
    })
}

fn video_window(
    main_thread: MainThreadMarker,
    player: &Player,
) -> player::Result<Retained<NSWindow>> {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 720.0));
    // SAFETY: the arguments are valid, and `setReleasedWhenClosed(false)`
    // below gives the window the ownership rules `Retained` expects.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(main_thread),
            frame,
            NSWindowStyleMask::Titled | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: called before the window can be closed.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(ns_string!("plex-rust pacing"));
    // Bottom right, out of the way of whoever is using the Mac.
    if let Some(screen) = NSScreen::mainScreen(main_thread) {
        let visible = screen.visibleFrame();
        let x = visible.origin.x + visible.size.width - frame.size.width - 16.0;
        window.setFrameOrigin(NSPoint::new(x, visible.origin.y + 16.0));
    }

    let layer = VideoLayer::new(player, main_thread)?;
    let view = NSView::initWithFrame(NSView::alloc(main_thread), frame);
    view.setLayer(Some(&layer));
    view.setWantsLayer(true);
    window.setContentView(Some(&view));
    video_layer::fit_to_display(&view, player)?;
    Ok(window)
}

/// Starts a display link that appends the time of every refresh of the
/// main display to `refreshes`. `None` if the system gives no link, in
/// which case the report has no refresh figures.
fn watch_refreshes(refreshes: &'static Refreshes) -> Option<CFRetained<CVDisplayLink>> {
    let mut link = std::ptr::null_mut();
    // SAFETY: the out pointer is valid for the call.
    unsafe { CVDisplayLink::create_with_active_cg_displays(NonNull::from(&mut link)) };
    // SAFETY: on success the function stored a link that the caller owns.
    let link = unsafe { CFRetained::from_raw(NonNull::new(link)?) };
    // SAFETY: the callback matches the type Core Video calls, and its data
    // is a `Refreshes` that lives as long as the process.
    unsafe {
        link.set_output_callback(Some(refreshed), std::ptr::from_ref(refreshes).cast_mut().cast());
    }
    link.start();
    Some(link)
}

unsafe extern "C-unwind" fn refreshed(
    _link: NonNull<CVDisplayLink>,
    _now: NonNull<CVTimeStamp>,
    output: NonNull<CVTimeStamp>,
    _flags_in: CVOptionFlags,
    _flags_out: NonNull<CVOptionFlags>,
    data: *mut c_void,
) -> CVReturn {
    // SAFETY: `data` is the `Refreshes` passed to `set_output_callback`,
    // and Core Video passes a valid time stamp.
    let (refreshes, output) = unsafe { (&*data.cast::<Refreshes>(), output.as_ref()) };
    let refresh = HostTime::from_ticks(output.hostTime);
    refreshes.lock().unwrap_or_else(PoisonError::into_inner).push(refresh);
    0
}

fn follow_events(player: &Player) {
    loop {
        match player.wait_event() {
            Event::VideoChanged => {
                let fitted = on_layer(|_, view| video_layer::fit_to_display(view, player));
                if let Some(Err(error)) = fitted {
                    eprintln!("plex-rust: {error}");
                }
            }
            Event::Ended(EndReason::Failed(error)) => {
                eprintln!("plex-rust: playback failed: {error}");
                std::process::exit(1);
            }
            Event::Ended(_) | Event::Shutdown => {
                eprintln!("plex-rust: playback ended before the measurement did");
                std::process::exit(1);
            }
            Event::Loaded | Event::Buffering(_) => {}
        }
    }
}

/// Runs `work` on the main thread with the window's video layer and the
/// view that hosts it.
fn on_layer<T: Send>(work: impl FnOnce(&VideoLayer, &NSView) -> T + Send) -> Option<T> {
    dispatch2::run_on_main(|main_thread| {
        let windows = NSApplication::sharedApplication(main_thread).windows();
        let view = windows.firstObject().and_then(|window| window.contentView())?;
        let layer = view.layer()?.downcast::<VideoLayer>().ok()?;
        Some(work(&layer, &view))
    })
}

fn measure(
    player: &Player,
    refreshes: &Refreshes,
    speed: f64,
    length: Duration,
) -> player::Result<Value> {
    player.set_speed(speed)?;
    let start = player.position()?;
    while player.position()? == start {
        thread::sleep(Duration::from_millis(50));
    }
    thread::sleep(SETTLE);

    let before = counters(player)?;
    let (clock, cpu) = (Instant::now(), cpu_time());
    refreshes.lock().unwrap_or_else(PoisonError::into_inner).clear();
    on_layer(|layer, _| layer.record_timings());
    // macOS does not draw a window that is covered or on another Space, so
    // the figures only mean something if it was in view all along.
    let mut visible = true;
    let end = Instant::now() + length;
    while Instant::now() < end {
        visible &= window_is_visible();
        thread::sleep(Duration::from_millis(100));
    }
    let cpu_share = (cpu_time() - cpu).as_secs_f64() / clock.elapsed().as_secs_f64();
    let timings = on_layer(|layer, _| layer.take_timings()).unwrap_or_default();
    let refreshes = refreshes.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let after = counters(player)?;

    let grown: BTreeMap<&str, i64> = COUNTERS
        .iter()
        .zip(before.iter().zip(&after))
        .map(|(name, (a, b))| (*name, b - a))
        .collect();
    let mut readings = BTreeMap::new();
    for name in READINGS {
        readings.insert(name, player.diagnostic(name)?);
    }
    Ok(json!({
        "speed": speed,
        "seconds": length.as_secs(),
        "window_visible": visible,
        // Of one core, by the whole process: decoding, drawing, this report.
        "cpu_percent": (cpu_share * 1000.0).round() / 10.0,
        "display": display(&refreshes),
        "frames": frames(&timings, &refreshes),
        "mpv_counters_grown": grown,
        "mpv": readings,
    }))
}

fn counters(player: &Player) -> player::Result<Vec<i64>> {
    COUNTERS
        .iter()
        .map(|name| Ok(player.diagnostic(name)?.and_then(|text| text.parse().ok()).unwrap_or(0)))
        .collect()
}

fn window_is_visible() -> bool {
    dispatch2::run_on_main(|main_thread| {
        let state = NSApplication::sharedApplication(main_thread).occlusionState();
        state.contains(NSApplicationOcclusionState::Visible)
    })
}

fn display(refreshes: &[HostTime]) -> Value {
    let screen = dispatch2::run_on_main(|main_thread| {
        let windows = NSApplication::sharedApplication(main_thread).windows();
        let screen = windows.firstObject().and_then(|window| window.screen())?;
        Some(json!({
            "maximum_fps": screen.maximumFramesPerSecond(),
            "shortest_refresh": screen.minimumRefreshInterval() * 1e3,
            "longest_refresh": screen.maximumRefreshInterval() * 1e3,
            "update_granularity": screen.displayUpdateGranularity() * 1e3,
        }))
    });
    json!({
        "screen": screen,
        "refreshes_seen": refreshes.len(),
        "refresh_interval": spread(&gaps(refreshes.iter().copied())),
    })
}

/// The report on the frames themselves.
///
/// `late` is how long after the moment mpv wanted a frame on screen its
/// draw finished: the spread of it, not its size, is what judders.
/// `refreshes_per_frame` counts, for each frame, how many display
/// refreshes passed before the next one, taking a frame to appear at the
/// first refresh after its draw. Even pacing is one count throughout.
///
/// The `aimed` figures cover the frames drawn for a refresh of the
/// display: how long after its due time each one's refresh was, how long
/// before the refresh its draw was finished, and the count of refreshes
/// from one frame's to the next.
fn frames(timings: &[FrameTiming], refreshes: &[HostTime]) -> Value {
    let new: Vec<&FrameTiming> = timings.iter().filter(|timing| timing.due.is_some()).collect();
    let since = |later: HostTime, earlier: HostTime| millis(later.nanos(), earlier.nanos());
    let late: Vec<f64> = new.iter().filter_map(|t| t.due.map(|due| since(t.ended, due))).collect();
    let waited: Vec<f64> = new.iter().map(|t| since(t.began, t.signalled)).collect();
    let drew: Vec<f64> = timings.iter().map(|t| since(t.ended, t.began)).collect();
    let content = gaps(new.iter().filter_map(|t| t.due));
    let drawn = gaps(new.iter().map(|t| t.ended));
    let period = median(&gaps(refreshes.iter().copied()));
    let appeared = new.iter().filter_map(|t| first_at_or_after(refreshes, t.ended));
    let steps = gaps(appeared).into_iter().map(|gap| gap / period);
    let aimed: Vec<(&FrameTiming, HostTime)> =
        new.iter().filter_map(|t| t.refresh.map(|refresh| (*t, refresh))).collect();
    let aimed_late: Vec<f64> =
        aimed.iter().filter_map(|(t, refresh)| t.due.map(|due| since(*refresh, due))).collect();
    let spare: Vec<f64> = aimed.iter().map(|(t, refresh)| since(*refresh, t.ended)).collect();
    let aimed_steps = gaps(aimed.iter().map(|(_, refresh)| *refresh));

    json!({
        "new": new.len(),
        "redrawn": timings.len() - new.len(),
        // How many frames mpv meant to show: the span of their due times
        // over the usual gap between two.
        "expected": (content.iter().sum::<f64>() / median(&content)).round() + 1.0,
        "content_interval": spread(&content),
        "draw_interval": spread(&drawn),
        "draw_interval_histogram": histogram(drawn.into_iter()),
        "late": spread(&late),
        "signal_to_draw": spread(&waited),
        "draw_duration": spread(&drew),
        "refreshes_per_frame": histogram(steps),
        "aimed": aimed.len(),
        "aimed_late": spread(&aimed_late),
        "aimed_spare": spread(&spare),
        "aimed_refreshes_per_frame": histogram(aimed_steps.into_iter().map(|gap| gap / period)),
    })
}

fn millis(later: u64, earlier: u64) -> f64 {
    (later.cast_signed() - earlier.cast_signed()) as f64 / 1e6
}

/// The time from each moment to the next, in milliseconds.
fn gaps(moments: impl Iterator<Item = HostTime>) -> Vec<f64> {
    let moments: Vec<HostTime> = moments.collect();
    moments.windows(2).map(|pair| millis(pair[1].nanos(), pair[0].nanos())).collect()
}

fn first_at_or_after(refreshes: &[HostTime], moment: HostTime) -> Option<HostTime> {
    refreshes.get(refreshes.partition_point(|refresh| *refresh < moment)).copied()
}

/// How often each value occurs, with values rounded to whole numbers.
fn histogram(values: impl Iterator<Item = f64>) -> BTreeMap<i64, usize> {
    let mut counts = BTreeMap::new();
    for value in values.filter(|value| value.is_finite()) {
        *counts.entry(value.round() as i64).or_default() += 1;
    }
    counts
}

fn median(values: &[f64]) -> f64 {
    percentile(&sorted(values), 0.5)
}

fn sorted(values: &[f64]) -> Vec<f64> {
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    values
}

/// The value below which `share` of the sorted `values` lie.
fn percentile(values: &[f64], share: f64) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    let index = (share * (values.len() - 1) as f64).round() as usize;
    values[index.min(values.len() - 1)]
}

fn spread(values: &[f64]) -> Value {
    let values = sorted(values);
    let round = |value: f64| (value * 1e3).round() / 1e3;
    json!({
        "min": round(percentile(&values, 0.0)),
        "p1": round(percentile(&values, 0.01)),
        "p50": round(percentile(&values, 0.5)),
        "p99": round(percentile(&values, 0.99)),
        "max": round(percentile(&values, 1.0)),
    })
}
