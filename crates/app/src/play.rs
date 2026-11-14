//! `plex-rust play`: one file, URL or library item in a bare window.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationOcclusionState, NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    NSActivityOptions, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSProcessInfo, NSRect,
    NSSize, ns_string,
};
use player::{EndReason, Event, Player, Probe, Source};
use plex::playback::PlaybackState;

use crate::error::with_causes;
use crate::progress::Progress;
use crate::seeks::{self, Step};
use crate::video_layer::{self, VideoLayer};

/// How often the server hears from a running playback. Plex asks for 10 s.
const REPORT_INTERVAL: Duration = Duration::from_secs(10);

/// A playback and, for library items, the means to report its progress.
struct Playback {
    player: Player,
    progress: Option<Progress>,
}

impl Playback {
    fn report(&self, state: PlaybackState) {
        let Some(progress) = &self.progress else {
            return;
        };
        if let Err(error) = progress.report(&self.player, state) {
            eprintln!("plex-rust: progress was not saved: {}", with_causes(&error));
        }
    }

    fn report_current_state(&self) {
        match self.player.is_paused() {
            Ok(true) => self.report(PlaybackState::Paused),
            Ok(false) => self.report(PlaybackState::Playing),
            Err(error) => eprintln!("plex-rust: {error}"),
        }
    }
}

/// What a run that is a check, and not a viewing, looks at.
pub enum Check {
    /// The playback state after this long.
    State(Duration),
    /// How long these jumps stall playback, and then the playback state.
    Seeks(Vec<Step>),
}

/// Plays `source` in a window until it ends or the window is closed.
///
/// With `progress`, the server is kept up to date on the position.
///
/// With `check`, the run is muted, leaves the keyboard focus where it is,
/// prints what it measured as one line of JSON, and exits.
pub fn run(
    source: &Source,
    progress: Option<Progress>,
    check: Option<Check>,
) -> player::Result<()> {
    let main_thread =
        MainThreadMarker::new().unwrap_or_else(|| panic!("main() runs on the main thread"));

    let playback = Arc::new(Playback { player: Player::new()?, progress });
    let player = &playback.player;

    let app = NSApplication::sharedApplication(main_thread);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate = AppDelegate::new(main_thread, Arc::clone(&playback));
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    // Held until the process exits: the display must not sleep mid-film.
    let _watching = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
        NSActivityOptions::UserInitiated | NSActivityOptions::IdleDisplaySleepDisabled,
        ns_string!("Playing video"),
    );

    let window = video_window(main_thread, player)?;
    player.set_muted(check.is_some())?;
    // Made before loading, so that it sees the playback start.
    let probe = check.is_some().then(|| Probe::new(player)).transpose()?;
    let loading = Instant::now();
    player.load(source)?;

    spawn(&playback, exit_when_playback_ends);
    spawn(&playback, report_periodically);
    match check.zip(probe) {
        Some((check, probe)) => {
            window.orderFrontRegardless();
            spawn(&playback, move |playback| {
                print_check_and_exit(&playback.player, probe, loading, &check);
            });
        }
        None => {
            window.makeKeyAndOrderFront(None);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
    }

    app.run();
    Ok(())
}

fn spawn(playback: &Arc<Playback>, work: impl FnOnce(&Playback) + Send + 'static) {
    let playback = Arc::clone(playback);
    thread::spawn(move || work(&playback));
}

fn video_window(
    main_thread: MainThreadMarker,
    player: &Player,
) -> player::Result<Retained<NSWindow>> {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 720.0));
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    // SAFETY: the arguments are valid, and `setReleasedWhenClosed(false)`
    // below gives the window the ownership rules `Retained` expects.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(main_thread),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: called before the window can be closed.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(ns_string!("plex-rust"));
    window.center();

    let layer = VideoLayer::new(player, main_thread)?;
    let view = NSView::initWithFrame(NSView::alloc(main_thread), frame);
    view.setLayer(Some(&layer));
    view.setWantsLayer(true);
    window.setContentView(Some(&view));
    video_layer::fit_to_display(&view, player)?;
    Ok(window)
}

fn exit_when_playback_ends(playback: &Playback) {
    loop {
        match playback.player.wait_event() {
            Event::Loaded => playback.report(PlaybackState::Playing),
            Event::VideoChanged => match_video(&playback.player),
            Event::Buffering(_) => {}
            Event::Ended(EndReason::Failed(error)) => {
                eprintln!("plex-rust: playback failed: {error}");
                std::process::exit(1);
            }
            Event::Ended(EndReason::Finished | EndReason::Stopped) | Event::Shutdown => {
                playback.report(PlaybackState::Stopped);
                std::process::exit(0);
            }
        }
    }
}

/// Matches the window's video layer to the colours of the video.
fn match_video(player: &Player) {
    let matched = dispatch2::run_on_main(|main_thread| {
        let windows = NSApplication::sharedApplication(main_thread).windows();
        let view = windows.firstObject().and_then(|window| window.contentView());
        view.map_or(Ok(()), |view| video_layer::fit_to_display(&view, player))
    });
    if let Err(error) = matched {
        eprintln!("plex-rust: {error}");
    }
}

fn report_periodically(playback: &Playback) {
    loop {
        thread::sleep(REPORT_INTERVAL);
        playback.report_current_state();
    }
}

/// Prints the result of `check` as one line of JSON, then exits.
fn print_check_and_exit(player: &Player, probe: Probe, loading: Instant, check: &Check) {
    match checked(player, probe, loading, check) {
        Ok(report) => {
            println!("{report}");
            std::process::exit(0);
        }
        Err(error) => {
            eprintln!("plex-rust: {error}");
            std::process::exit(1);
        }
    }
}

fn checked(
    player: &Player,
    mut probe: Probe,
    loading: Instant,
    check: &Check,
) -> player::Result<serde_json::Value> {
    let started = seeks::started(&mut probe, loading);
    let seeks = match check {
        Check::State(delay) => {
            seeks::idle(&mut probe, delay.saturating_sub(loading.elapsed()));
            None
        }
        Check::Seeks(steps) => Some(seeks::run(player, &mut probe, steps)?),
    };
    let mut report = state(player)?;
    // Seconds from asking for the file to its first picture being ready.
    report["started_in"] = started.map(|time| time.as_secs_f64()).into();
    report["seeks"] = seeks.into();
    Ok(report)
}

fn state(player: &Player) -> player::Result<serde_json::Value> {
    let seconds = |time: Option<Duration>| time.map(|time| time.as_secs_f64());
    Ok(serde_json::json!({
        "position": seconds(player.position()?),
        "duration": seconds(player.duration()?),
        "buffered": seconds(player.buffered()?),
        "hardware_decoder": player.hardware_decoder()?,
        "dropped_frames": player.dropped_frames()?,
        "colors": player.video_colors()?.map(|colors| format!("{colors:?}")),
        // Whether the layer asked the display for more than SDR brightness.
        "hdr_output": shows_hdr(),
        "drawn_size": drawn_size(),
        "volume": player.volume()?,
        "tracks": player.tracks()?.iter().map(track).collect::<Vec<_>>(),
        // macOS does not draw windows that are covered or on another Space,
        // and mpv counts every frame that was not drawn as dropped. A drop
        // count only says something about playback when this is true.
        "window_visible": window_is_visible(),
    }))
}

fn track(track: &player::Track) -> serde_json::Value {
    serde_json::json!({
        "id": track.id,
        "kind": format!("{:?}", track.kind),
        "language": track.language,
        "selected": track.selected,
    })
}

/// The window's video layer, if the window is up.
fn video_layer(main_thread: MainThreadMarker) -> Option<Retained<VideoLayer>> {
    let windows = NSApplication::sharedApplication(main_thread).windows();
    let view = windows.firstObject().and_then(|window| window.contentView())?;
    view.layer()?.downcast::<VideoLayer>().ok()
}

fn shows_hdr() -> bool {
    dispatch2::run_on_main(|main_thread| {
        video_layer(main_thread).is_some_and(|layer| layer.wantsExtendedDynamicRangeContent())
    })
}

/// The resolution frames are rendered at, as `[width, height]` in pixels.
fn drawn_size() -> Option<(i32, i32)> {
    dispatch2::run_on_main(|main_thread| video_layer(main_thread).map(|layer| layer.drawn_size()))
}

fn window_is_visible() -> bool {
    dispatch2::run_on_main(|main_thread| {
        let state = NSApplication::sharedApplication(main_thread).occlusionState();
        state.contains(NSApplicationOcclusionState::Visible)
    })
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `AppDelegate`
    // does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustAppDelegate"]
    #[ivars = Arc<Playback>]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(&self, _app: &NSApplication) -> bool {
            true
        }

        /// Closing the window ends the app without the player ever
        /// reporting an end, so the last position is sent from here.
        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            self.ivars().report(PlaybackState::Stopped);
        }
    }
);

impl AppDelegate {
    fn new(main_thread: MainThreadMarker, playback: Arc<Playback>) -> Retained<Self> {
        let this = Self::alloc(main_thread).set_ivars(playback);
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}
