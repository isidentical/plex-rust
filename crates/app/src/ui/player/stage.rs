//! Setting the scene for a snapshot: the states of the player that only a
//! pointer, a window or a particular picture would otherwise bring about.

use std::time::Duration;

use objc2::rc::Retained;
use objc2::{MainThreadOnly, Message};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBox, NSBoxType, NSColor, NSEvent, NSEventModifierFlags,
    NSEventType, NSView, NSWindowOrderingMode,
};
use objc2_foundation::{NSPoint, NSProcessInfo, NSSize};
use player::Player;
use plex::model::Item;

use super::controls::Controls;
use super::input::Scene;

/// The smallest the app's window gets.
const SMALL: NSSize = NSSize { width: 880.0, height: 560.0 };
/// A window the shape of a 2.39 to 1 film.
const WIDE: NSSize = NSSize { width: 1280.0, height: 536.0 };
/// How far into a marker the scene is set, so that the player has
/// certainly arrived in it by the time the snapshot is taken.
const INTO_MARKER: Duration = Duration::from_secs(2);
/// How far a pull moves the pointer between one event and the next.
const PULL_STEP: f64 = 24.0;
/// A pull ends this far left of the window's edge and this far above the
/// scrubber, where a hand that overshoots ends up.
const PULL_PAST_EDGE: f64 = 300.0;
const PULL_RISE: f64 = 90.0;

pub fn set(
    scene: Scene,
    root: &NSView,
    controls: &Controls,
    player: &Player,
    item: &Item,
) -> player::Result<()> {
    match scene {
        Scene::Hover => {
            let scrubber = &controls.scrubber;
            scrubber.point_at(scrubber.bounds().size.width / 2.0);
        }
        Scene::Drag => {
            let scrubber = &controls.scrubber;
            scrubber.hold_at(scrubber.bounds().size.width * 0.75);
        }
        Scene::Bright => {
            // The hardest picture to read white controls on. It goes
            // between the video and the controls.
            let white = NSBox::new(root.mtm());
            white.setBoxType(NSBoxType::Custom);
            white.setBorderWidth(0.0);
            white.setFillColor(&NSColor::whiteColor());
            white.setFrame(root.bounds());
            white.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            root.addSubview_positioned_relativeTo(
                &white,
                NSWindowOrderingMode::Below,
                Some(&controls.top),
            );
        }
        Scene::Pull => {
            let (from, to) = pull(controls);
            let held = send(root, NSEventType::LeftMouseDown, from, None);
            let steps = ((from.x - to.x) / PULL_STEP).ceil();
            for step in 1..=steps as u32 {
                let along = f64::from(step) / steps;
                let at = NSPoint::new(
                    from.x + (to.x - from.x) * along,
                    from.y + (to.y - from.y) * along,
                );
                send(root, NSEventType::LeftMouseDragged, at, held.as_deref());
            }
        }
        Scene::Release => {
            let scrubber: &NSView = &controls.scrubber;
            send(root, NSEventType::LeftMouseUp, pull(controls).1, Some(scrubber));
        }
        Scene::Small => resize(root, SMALL),
        Scene::Wide => resize(root, WIDE),
        Scene::Marker => {
            if let Some(marker) = item.markers.first() {
                let start: Duration = marker.start_time_offset.into();
                player.seek(start + INTO_MARKER)?;
            }
        }
    }
    Ok(())
}

fn resize(root: &NSView, size: NSSize) {
    if let Some(window) = root.window() {
        window.setContentSize(size);
    }
}

/// Where a pull takes hold and where it ends, in the window's coordinates:
/// from three quarters of the way along the scrubber to well outside the
/// window on the left.
fn pull(controls: &Controls) -> (NSPoint, NSPoint) {
    let scrubber = &controls.scrubber;
    let size = scrubber.bounds().size;
    let from = NSPoint::new(size.width * 0.75, size.height / 2.0);
    let from = scrubber.convertPoint_toView(from, None);
    (from, NSPoint::new(-PULL_PAST_EDGE, from.y + PULL_RISE))
}

/// Sends a mouse event at `at`, in the window's coordinates, to the view
/// it lands on, and returns that view. The window is then laid out, as it
/// is between any two events that arrive from a hand.
///
/// `held` is the view that took the press, which hears of the drag and
/// the release wherever the pointer has gone, as AppKit has it. The event
/// goes to the view directly: a window that is not on screen, as a
/// snapshot's is not, passes no mouse events on.
fn send(
    root: &NSView,
    kind: NSEventType,
    at: NSPoint,
    held: Option<&NSView>,
) -> Option<Retained<NSView>> {
    let window = root.window()?;
    let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        kind,
        at,
        NSEventModifierFlags::empty(),
        NSProcessInfo::processInfo().systemUptime(),
        window.windowNumber(),
        None,
        0,
        1,
        1.0,
    )?;
    let target = match held {
        Some(held) => held.retain(),
        None => {
            // The content view fills the window, so a point in the window
            // is the same point in the view above it, which is what a hit
            // test is given.
            let content = window.contentView()?;
            content.hitTest(at)?
        }
    };
    match kind {
        NSEventType::LeftMouseDown => target.mouseDown(&event),
        NSEventType::LeftMouseDragged => target.mouseDragged(&event),
        _ => target.mouseUp(&event),
    }
    window.layoutIfNeeded();
    Some(target)
}
