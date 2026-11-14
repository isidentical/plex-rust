//! The strip above a library's grid: how many items it lists on the left,
//! which ones and in what order on the right.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAccessibility, NSColor, NSLayoutConstraint, NSPopUpButton, NSSegmentSwitchTracking,
    NSSegmentedControl, NSTextField, NSView,
};
use objc2_foundation::{NSArray, NSRect, NSString, ns_string};
use plex::library::{Listing, Sort};

use super::callback::Callback;
use super::label::{self, Weight};
use super::text;
use super::theme;

/// Where "Unwatched" is among the segments that say which items are listed.
const UNWATCHED_SEGMENT: isize = 1;

pub struct LibraryBar {
    view: Retained<NSView>,
    count: Retained<NSTextField>,
    /// What the library holds one of: "film" or "show".
    noun: &'static str,
    /// Kept because controls do not retain their targets.
    _changed: Retained<Callback>,
}

impl LibraryBar {
    /// A bar showing `listing`. `on_change` is called with the new listing
    /// when the viewer picks another order or flips the unwatched filter.
    pub fn new(
        main_thread: MainThreadMarker,
        listing: Listing,
        noun: &'static str,
        on_change: impl Fn(Listing) + 'static,
    ) -> Self {
        let order = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(main_thread),
            NSRect::ZERO,
            false,
        );
        let titles: Vec<Retained<NSString>> =
            Sort::ALL.iter().map(|sort| NSString::from_str(sort.title())).collect();
        order.addItemsWithTitles(&NSArray::from_retained_slice(&titles));
        let chosen = Sort::ALL.iter().position(|sort| *sort == listing.sort).unwrap_or(0);
        order.selectItemAtIndex(chosen.cast_signed());
        // The menu shows only the order in use, such as "Title", which
        // does not say what it is the title of.
        order.setAccessibilityLabel(Some(ns_string!("Sort by")));

        // Two segments, of which one is always lit: which items are
        // listed can be read at a glance, in a window that is in front or
        // not.
        let labels = [ns_string!("All"), ns_string!("Unwatched")];
        // SAFETY: the target and action are set below, once they exist.
        let shown = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &NSArray::from_slice(&labels),
                NSSegmentSwitchTracking::SelectOne,
                None,
                None,
                main_thread,
            )
        };
        shown.setSelectedSegment(if listing.unwatched_only { UNWATCHED_SEGMENT } else { 0 });
        shown.setAccessibilityLabel(Some(ns_string!("Show")));

        let secondary = NSColor::secondaryLabelColor();
        let sort_by = label::line(theme::FONT_BODY, Weight::Regular, &secondary, main_thread);
        label::set(&sort_by, "Sort by");
        // The menu already says it to VoiceOver.
        sort_by.setAccessibilityElement(false);

        let on_change = Rc::new(on_change);
        let (order_read, shown_read) = (order.clone(), shown.clone());
        let changed = Callback::new(main_thread, move |_| {
            let chosen = usize::try_from(order_read.indexOfSelectedItem()).unwrap_or(0);
            on_change(Listing {
                sort: Sort::ALL.get(chosen).copied().unwrap_or_default(),
                unwatched_only: shown_read.selectedSegment() == UNWATCHED_SEGMENT,
            });
        });
        // SAFETY: the bar keeps the target for as long as it keeps the
        // controls.
        unsafe {
            order.setTarget(Some(&changed));
            order.setAction(Some(Callback::action()));
            shown.setTarget(Some(&changed));
            shown.setAction(Some(Callback::action()));
        }

        let count = label::line(theme::FONT_BODY, Weight::Regular, &secondary, main_thread);
        let view = NSView::new(main_thread);
        for control in [&**order, &**sort_by, &**shown, &**count] {
            let control: &NSView = control;
            control.setTranslatesAutoresizingMaskIntoConstraints(false);
            view.addSubview(control);
            control.centerYAnchor().constraintEqualToAnchor(&view.centerYAnchor()).setActive(true);
        }
        let margin = theme::PAGE_MARGIN;
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            count.leadingAnchor().constraintEqualToAnchor_constant(&view.leadingAnchor(), margin),
            order
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&view.trailingAnchor(), -margin),
            sort_by
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&order.leadingAnchor(), -theme::SPACE_S),
            shown
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&sort_by.leadingAnchor(), -theme::SPACE_XL),
        ]));
        Self { view, count, noun, _changed: changed }
    }

    pub fn view(&self) -> &NSView {
        &self.view
    }

    /// Says how many items the listing has in all.
    pub fn set_total(&self, total: u32) {
        label::set(&self.count, &text::count(total, self.noun));
    }
}
