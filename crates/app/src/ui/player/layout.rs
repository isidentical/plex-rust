//! Placing the player's controls on top of the video.

use objc2_app_kit::{NSLayoutConstraint, NSView};
use objc2_foundation::NSArray;

/// Adds `child` on top of whatever `parent` holds, in its centre.
pub fn dock_center(parent: &NSView, child: &NSView) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    parent.addSubview(child);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        child.centerXAnchor().constraintEqualToAnchor(&parent.centerXAnchor()),
        child.centerYAnchor().constraintEqualToAnchor(&parent.centerYAnchor()),
    ]));
}

/// Adds `child` on top of whatever `parent` holds, along its bottom edge,
/// `margin` in from the bottom and both sides.
pub fn dock_bottom(parent: &NSView, child: &NSView, margin: f64) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    parent.addSubview(child);
    let (leading, trailing) = (parent.leadingAnchor(), parent.trailingAnchor());
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        child.leadingAnchor().constraintEqualToAnchor_constant(&leading, margin),
        child.trailingAnchor().constraintEqualToAnchor_constant(&trailing, -margin),
        child.bottomAnchor().constraintEqualToAnchor_constant(&parent.bottomAnchor(), -margin),
    ]));
}

/// Adds `child` on top of whatever `parent` holds, in its top left corner
/// below the title bar, `margin` in from both edges.
pub fn dock_top_leading(parent: &NSView, child: &NSView, margin: f64) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    parent.addSubview(child);
    let top = parent.safeAreaLayoutGuide().topAnchor();
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        child.leadingAnchor().constraintEqualToAnchor_constant(&parent.leadingAnchor(), margin),
        child.topAnchor().constraintEqualToAnchor_constant(&top, margin),
    ]));
}

/// Adds `child` on top of whatever `parent` holds, in its top right corner
/// below the title bar, `margin` in from both edges.
pub fn dock_top_trailing(parent: &NSView, child: &NSView, margin: f64) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    parent.addSubview(child);
    let top = parent.safeAreaLayoutGuide().topAnchor();
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        child.trailingAnchor().constraintEqualToAnchor_constant(&parent.trailingAnchor(), -margin),
        child.topAnchor().constraintEqualToAnchor_constant(&top, margin),
    ]));
}

/// Adds `child` on top of whatever `parent` holds, `gap` above `below` and
/// lined up with its right edge.
pub fn dock_above(parent: &NSView, child: &NSView, below: &NSView, gap: f64) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    parent.addSubview(child);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        child.trailingAnchor().constraintEqualToAnchor(&below.trailingAnchor()),
        child.bottomAnchor().constraintEqualToAnchor_constant(&below.topAnchor(), -gap),
    ]));
}
