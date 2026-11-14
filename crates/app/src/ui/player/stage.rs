//! Setting the scene for a snapshot: the states of the player that only a
//! pointer, a window or a particular picture would otherwise bring about.

use std::time::Duration;

use objc2::MainThreadOnly;
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBox, NSBoxType, NSColor, NSView, NSWindowOrderingMode,
};
use objc2_foundation::NSSize;
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
