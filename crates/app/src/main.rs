//! Reel, a native macOS client for Plex.

mod video_layer;

use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, ns_string};
use player::{EndReason, Event, Player};

use crate::video_layer::VideoLayer;

const USAGE: &str = "usage: reel play <file-or-url> [--report-after <seconds>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    let (source, report_after) = match args.as_slice() {
        ["play", source] => (*source, None),
        ["play", source, "--report-after", seconds] => match seconds.parse() {
            Ok(seconds) => (*source, Some(Duration::from_secs(seconds))),
            Err(_) => return fail(USAGE),
        },
        _ => return fail(USAGE),
    };

    match play(source, report_after) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&error.to_string()),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("reel: {message}");
    ExitCode::FAILURE
}

/// Plays `source` in a window until it ends or the window is closed.
fn play(source: &str, report_after: Option<Duration>) -> player::Result<()> {
    let main_thread =
        MainThreadMarker::new().unwrap_or_else(|| panic!("main() runs on the main thread"));

    let player = Player::new()?;

    let app = NSApplication::sharedApplication(main_thread);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate = AppDelegate::new(main_thread);
    app.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate)));

    let window = video_window(main_thread, &player)?;
    window.makeKeyAndOrderFront(None);
    player.load(source)?;
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);

    let events = player.clone();
    thread::spawn(move || exit_when_playback_ends(&events));
    if let Some(delay) = report_after {
        let reporter = player.clone();
        thread::spawn(move || report_and_exit(&reporter, delay));
    }

    app.run();
    Ok(())
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

fn exit_when_playback_ends(player: &Player) -> ! {
    loop {
        match player.wait_event() {
            Event::Loaded => {}
            Event::Ended(EndReason::Failed(error)) => {
                eprintln!("reel: playback failed: {error}");
                std::process::exit(1);
            }
            Event::Ended(EndReason::Finished | EndReason::Stopped) | Event::Shutdown => {
                std::process::exit(0);
            }
        }
    }
}

/// Prints the playback state as one line of JSON, then exits.
///
/// This is how an unattended run checks that video really plays.
fn report_and_exit(player: &Player, delay: Duration) -> ! {
    thread::sleep(delay);
    match report(player) {
        Ok(report) => {
            println!("{report}");
            std::process::exit(0);
        }
        Err(error) => {
            eprintln!("reel: {error}");
            std::process::exit(1);
        }
    }
}

fn report(player: &Player) -> player::Result<serde_json::Value> {
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
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(&self, _app: &NSApplication) -> bool {
            true
        }
    }
);

impl AppDelegate {
    fn new(main_thread: MainThreadMarker) -> Retained<Self> {
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![Self::alloc(main_thread), init] }
    }
}
