//! The title above a shelf. When the shelf is the start of a longer list,
//! the title is a link to all of it and says so with a chevron. A shelf
//! with more cards than fit across has two arrows at the far end that
//! bring the next lot into view, for a pointer with no way to scroll
//! sideways.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityButtonRole, NSColor, NSImageView, NSLayoutConstraint,
    NSStackView, NSView,
};
use objc2_foundation::{NSArray, NSString};

use super::artwork;
use super::hover::HoverView;
use super::label::{self, Weight};
use super::theme;

/// What the two arrows of a shelf do.
pub struct Paging {
    pub back: Box<dyn Fn()>,
    pub on: Box<dyn Fn()>,
}

/// Puts the heading `title` in `strip`, the view the collection keeps for
/// it, in place of whatever heading was there. `on_more` is what a click
/// does when there is more to see. With `paging` the heading has arrows,
/// hidden until [`set_paging`] says which way there is to go.
pub fn fill(
    strip: &NSView,
    title: &str,
    on_more: Option<Box<dyn Fn()>>,
    paging: Option<Paging>,
    main_thread: MainThreadMarker,
) {
    let words = label::line(theme::FONT_TITLE, Weight::Bold, &NSColor::labelColor(), main_thread);
    label::set(&words, title);
    let chevron = NSImageView::new(main_thread);
    chevron.setImage(artwork::symbol("chevron.right", theme::FONT_BODY, Weight::Bold).as_deref());
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
    if let Some(paging) = paging {
        let back = arrow("chevron.left", &format!("Earlier in {title}"), paging.back, main_thread);
        let on = arrow("chevron.right", &format!("Further in {title}"), paging.on, main_thread);
        let arrows =
            NSStackView::stackViewWithViews(&NSArray::from_slice(&[&**back, &**on]), main_thread);
        arrows.setSpacing(theme::SPACE_S);
        arrows.setTranslatesAutoresizingMaskIntoConstraints(false);
        arrows.setHidden(true);
        strip.addSubview(&arrows);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            arrows.trailingAnchor().constraintEqualToAnchor(&strip.trailingAnchor()),
            arrows.centerYAnchor().constraintEqualToAnchor(&link.centerYAnchor()),
            arrows.leadingAnchor().constraintGreaterThanOrEqualToAnchor_constant(
                &link.trailingAnchor(),
                theme::SPACE_M,
            ),
        ]));
    }
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

/// Says which way the shelf under the heading in `strip` can still be
/// paged. An arrow with nowhere to go is dimmed, and with nowhere to go
/// either way, as when every card fits, both are hidden.
pub fn set_paging(strip: &NSView, back: bool, on: bool) {
    let Some(arrows) = strip.subviews().iter().find_map(|view| view.downcast::<NSStackView>().ok())
    else {
        return;
    };
    arrows.setHidden(!(back || on));
    let arrows = arrows.arrangedSubviews();
    for (arrow, live) in arrows.iter().zip([back, on]) {
        if let Some(arrow) = arrow.downcast_ref::<HoverView>() {
            arrow.set_live(live);
            arrow.setAlphaValue(if live { 1.0 } else { theme::SHELF_ARROW_SPENT_OPACITY });
        }
    }
}

/// A round button with a chevron in it, which VoiceOver calls `spoken`.
fn arrow(
    symbol: &str,
    spoken: &str,
    on_click: Box<dyn Fn()>,
    main_thread: MainThreadMarker,
) -> Retained<HoverView> {
    let glyph = NSImageView::new(main_thread);
    glyph.setImage(artwork::symbol(symbol, theme::FONT_CAPTION, Weight::Bold).as_deref());
    glyph.setContentTintColor(Some(&NSColor::labelColor()));
    glyph.setTranslatesAutoresizingMaskIntoConstraints(false);
    glyph.setAccessibilityElement(false);

    let arrow = HoverView::pressable(main_thread, |_| {}, on_click);
    arrow.setWantsLayer(true);
    artwork::fill_with(&arrow, &theme::white(theme::SHELF_ARROW_WASH_OPACITY));
    if let Some(layer) = arrow.layer() {
        layer.setCornerRadius(theme::SHELF_ARROW_SIZE / 2.0);
    }
    arrow.setAccessibilityElement(true);
    // SAFETY: the role is a constant that AppKit provides.
    arrow.setAccessibilityRole(Some(unsafe { NSAccessibilityButtonRole }));
    arrow.setAccessibilityLabel(Some(&NSString::from_str(spoken)));
    arrow.addSubview(&glyph);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        arrow.widthAnchor().constraintEqualToConstant(theme::SHELF_ARROW_SIZE),
        arrow.heightAnchor().constraintEqualToConstant(theme::SHELF_ARROW_SIZE),
        glyph.centerXAnchor().constraintEqualToAnchor(&arrow.centerXAnchor()),
        glyph.centerYAnchor().constraintEqualToAnchor(&arrow.centerYAnchor()),
    ]));
    arrow
}
