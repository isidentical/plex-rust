//! Placing the player's controls on top of the video.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSLayoutConstraint, NSStackView, NSView};
use objc2_foundation::NSArray;

/// Adds `child` on top of whatever `parent` holds. Where it goes is up to
/// the constraints given to [`pin`] once everything they name is added.
pub fn add(parent: &NSView, child: &NSView) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    parent.addSubview(child);
}

/// Makes `child`, already added, as large as `parent`.
pub fn cover(parent: &NSView, child: &NSView) {
    pin(&[
        child.topAnchor().constraintEqualToAnchor(&parent.topAnchor()),
        child.bottomAnchor().constraintEqualToAnchor(&parent.bottomAnchor()),
        child.leadingAnchor().constraintEqualToAnchor(&parent.leadingAnchor()),
        child.trailingAnchor().constraintEqualToAnchor(&parent.trailingAnchor()),
    ]);
}

/// Puts `constraints` into effect.
pub fn pin(constraints: &[Retained<NSLayoutConstraint>]) {
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(constraints));
}

/// Views side by side, `spacing` apart, centred on one line.
pub fn row(
    views: &[&NSView],
    spacing: f64,
    main_thread: MainThreadMarker,
) -> Retained<NSStackView> {
    let row = NSStackView::stackViewWithViews(&NSArray::from_slice(views), main_thread);
    row.setSpacing(spacing);
    row
}
