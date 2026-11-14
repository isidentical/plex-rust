//! `reel play`: one file, URL or library item in a bare window.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, ns_string,
};
use player::{EndReason, Event, Player, Source};
use plex::playback::PlaybackState;

use crate::error::with_causes;
use crate::progress::Progress;
use crate::video_layer::VideoLayer;

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
            eprintln!("reel: progress was not saved: {}", with_causes(&error));
        }
    }

    fn report_current_state(&self) {
        match self.player.is_paused() {
            Ok(true) => self.report(PlaybackState::Paused),
            Ok(false) => self.report(PlaybackState::Playing),
            Err(error) => eprintln!("reel: {error}"),
        }
    }
}

/// Plays `source` in a window until it ends or the window is closed.
///
/// With `progress`, the server is kept up to date on the position.
///
/// With `unattended`, the run is a check and not a viewing: it is muted,
/// leaves the keyboard focus where it is, prints the playback state after
/// that long, and exits.
pub fn run(
    source: &Source,
    progress: Option<Progress>,
    unattended: Option<Duration>,
) -> player::Result<()> {
    let main_thread =
        MainThreadMarker::new().unwrap_or_else(|| panic!("main() runs on the main thread"));

    let playback = Arc::new(Playback {
        player: Player::new()?,
        progress,
    });
    let player = &playback.player;

    let app = NSApplication::sharedApplication(main_thread);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate = AppDelegate::new(main_thread, Arc::clone(&playback));
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    let window = video_window(main_thread, player)?;
    player.set_muted(unattended.is_some())?;
    player.load(source)?;

    spawn(&playback, exit_when_playback_ends);
    spawn(&playback, report_periodically);
    match unattended {
        Some(delay) => {
            window.orderFrontRegardless();
            spawn(&playback, move |playback| {
                print_state_and_exit(playback, delay)
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
    window.setTitle(ns_string!("Reel"));
    window.center();

    let layer = VideoLayer::new(player, main_thread)?;
    layer.setContentsScale(window.backingScaleFactor());
    let view = NSView::initWithFrame(NSView::alloc(main_thread), frame);
    view.setLayer(Some(&layer));
    view.setWantsLayer(true);
    window.setContentView(Some(&view));
    Ok(window)
}

fn exit_when_playback_ends(playback: &Playback) {
    loop {
        match playback.player.wait_event() {
            Event::Loaded => playback.report(PlaybackState::Playing),
            Event::Ended(EndReason::Failed(error)) => {
                eprintln!("reel: playback failed: {error}");
                std::process::exit(1);
            }
            Event::Ended(EndReason::Finished | EndReason::Stopped) | Event::Shutdown => {
                playback.report(PlaybackState::Stopped);
                std::process::exit(0);
            }
        }
    }
}

fn report_periodically(playback: &Playback) {
    loop {
        thread::sleep(REPORT_INTERVAL);
        playback.report_current_state();
    }
}

/// Prints the playback state as one line of JSON, then exits.
fn print_state_and_exit(playback: &Playback, delay: Duration) {
    thread::sleep(delay);
    let state = state(&playback.player);
    playback.report(PlaybackState::Stopped);
    match state {
        Ok(state) => {
            println!("{state}");
            std::process::exit(0);
        }
        Err(error) => {
            eprintln!("reel: {error}");
            std::process::exit(1);
        }
    }
}

fn state(player: &Player) -> player::Result<serde_json::Value> {
    let seconds = |time: Option<Duration>| time.map(|time| time.as_secs_f64());
    Ok(serde_json::json!({
        "position": seconds(player.position()?),
        "duration": seconds(player.duration()?),
        "buffered": seconds(player.buffered()?),
        "hardware_decoder": player.hardware_decoder()?,
        "dropped_frames": player.dropped_frames()?,
    }))
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `AppDelegate`
    // does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ReelAppDelegate"]
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
