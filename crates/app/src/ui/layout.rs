//! Placing one view inside another.

use objc2_app_kit::{NSLayoutConstraint, NSView};
use objc2_foundation::NSArray;

/// Makes `child` the only content of `parent`, filling all of it,
/// including the part under the title bar. A scroll view keeps its content
/// clear of the bar by itself and lets it pass underneath when scrolled.
pub fn fill(parent: &NSView, child: &NSView) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    parent.setSubviews(&NSArray::from_slice(&[child]));
    pin(parent, child);
}

/// Adds `child` on top of whatever `parent` holds, covering all of it,
/// including the part under the title bar.
pub fn cover(parent: &NSView, child: &NSView) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    parent.addSubview(child);
    pin(parent, child);
}

/// Makes `child` the only content of `parent`, at its own size in the
/// middle.
pub fn centre(parent: &NSView, child: &NSView) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    parent.setSubviews(&NSArray::from_slice(&[child]));
    place_in_middle(parent, child);
}

/// Adds `child` on top of whatever `parent` holds, at its own size in the
/// middle.
pub fn float(parent: &NSView, child: &NSView) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    parent.addSubview(child);
    place_in_middle(parent, child);
}

fn place_in_middle(parent: &NSView, child: &NSView) {
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        child.centerXAnchor().constraintEqualToAnchor(&parent.centerXAnchor()),
        child.centerYAnchor().constraintEqualToAnchor(&parent.centerYAnchor()),
    ]));
}

fn pin(parent: &NSView, child: &NSView) {
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        child.leadingAnchor().constraintEqualToAnchor(&parent.leadingAnchor()),
        child.trailingAnchor().constraintEqualToAnchor(&parent.trailingAnchor()),
        child.topAnchor().constraintEqualToAnchor(&parent.topAnchor()),
        child.bottomAnchor().constraintEqualToAnchor(&parent.bottomAnchor()),
    ]));
}
