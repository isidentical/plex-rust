//! Placing one view inside another.

use objc2_app_kit::{NSLayoutConstraint, NSView};
use objc2_foundation::NSArray;

/// Makes `child` the only content of `parent`, filling it up to the title
/// bar.
pub fn fill(parent: &NSView, child: &NSView) {
    replace_content(parent, child);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        child
            .leadingAnchor()
            .constraintEqualToAnchor(&parent.leadingAnchor()),
        child
            .trailingAnchor()
            .constraintEqualToAnchor(&parent.trailingAnchor()),
        child
            .topAnchor()
            .constraintEqualToAnchor(&parent.safeAreaLayoutGuide().topAnchor()),
        child
            .bottomAnchor()
            .constraintEqualToAnchor(&parent.bottomAnchor()),
    ]));
}

/// Makes `child` the only content of `parent`, centred at its own size.
pub fn center(parent: &NSView, child: &NSView) {
    replace_content(parent, child);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        child
            .centerXAnchor()
            .constraintEqualToAnchor(&parent.centerXAnchor()),
        child
            .centerYAnchor()
            .constraintEqualToAnchor(&parent.centerYAnchor()),
    ]));
}

fn replace_content(parent: &NSView, child: &NSView) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    parent.setSubviews(&NSArray::from_slice(&[child]));
}
