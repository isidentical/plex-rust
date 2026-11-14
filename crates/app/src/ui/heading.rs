//! The title above a shelf. When the shelf is the start of a longer list,
//! the title is a link to all of it and says so with a chevron.

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityButtonRole, NSColor, NSImageView, NSLayoutConstraint, NSView,
};
use objc2_foundation::{NSArray, NSString};

use super::artwork;
use super::hover::HoverView;
use super::label::{self, Weight};
use super::theme;

/// Puts the heading `title` in `strip`, the view the collection keeps for
/// it, in place of whatever heading was there. `on_more` is what a click
/// does when there is more to see.
pub fn fill(
    strip: &NSView,
    title: &str,
    on_more: Option<Box<dyn Fn()>>,
    main_thread: MainThreadMarker,
) {
    let words = label::line(theme::FONT_TITLE, Weight::Bold, &NSColor::labelColor(), main_thread);
    label::set(&words, title);
    let chevron = NSImageView::new(main_thread);
    chevron.setImage(artwork::symbol("chevron.right", theme::FONT_BODY).as_deref());
    chevron.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    chevron.setTranslatesAutoresizingMaskIntoConstraints(false);
    chevron.setAccessibilityElement(false);

    let has_more = on_more.is_some();
    chevron.setHidden(!has_more);
    let lit = chevron.clone();
    let link = HoverView::link(
        main_thread,
        move |over| {
            let color = if over { NSColor::labelColor() } else { NSColor::secondaryLabelColor() };
            lit.setContentTintColor(Some(&color));
        },
        move || {
            if let Some(on_more) = &on_more {
                on_more();
            }
        },
    );
    link.set_live(has_more);
    if has_more {
        // VoiceOver reads the link as one button that says where it leads.
        link.setAccessibilityElement(true);
        // SAFETY: the role is a constant that AppKit provides.
        link.setAccessibilityRole(Some(unsafe { NSAccessibilityButtonRole }));
        link.setAccessibilityLabel(Some(&NSString::from_str(&format!("See all of {title}"))));
    }
    link.setTranslatesAutoresizingMaskIntoConstraints(false);
    link.addSubview(&words);
    link.addSubview(&chevron);
    strip.setSubviews(&NSArray::from_slice(&[&**link]));
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        // At the bottom of its strip, close to the shelf it names.
        link.leadingAnchor().constraintEqualToAnchor(&strip.leadingAnchor()),
        link.bottomAnchor()
            .constraintEqualToAnchor_constant(&strip.bottomAnchor(), -theme::SPACE_XS),
        link.trailingAnchor().constraintLessThanOrEqualToAnchor(&strip.trailingAnchor()),
        words.leadingAnchor().constraintEqualToAnchor(&link.leadingAnchor()),
        words.topAnchor().constraintEqualToAnchor(&link.topAnchor()),
        words.bottomAnchor().constraintEqualToAnchor(&link.bottomAnchor()),
        chevron
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&words.trailingAnchor(), theme::SPACE_XS),
        chevron.centerYAnchor().constraintEqualToAnchor(&words.centerYAnchor()),
        chevron.trailingAnchor().constraintEqualToAnchor(&link.trailingAnchor()),
    ]));
}
