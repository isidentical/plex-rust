//! The strip above a library's grid: how many items it lists on the left,
//! which ones and in what order on the right.

use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAccessibility, NSButton, NSButtonType, NSColor, NSControlStateValueOn, NSLayoutConstraint,
    NSPopUpButton, NSTextField, NSView,
};
use objc2_foundation::{NSArray, NSRect, NSString, ns_string};
use plex::library::{Listing, Sort};

use super::callback::Callback;
use super::label::{self, Weight};
use super::text;
use super::theme;

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

        // SAFETY: the target and action are set below, once they exist.
        let unwatched = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Unwatched"),
                None,
                None,
                main_thread,
            )
        };
        unwatched.setButtonType(NSButtonType::PushOnPushOff);
        if listing.unwatched_only {
            unwatched.setState(NSControlStateValueOn);
        }

        let on_change = Rc::new(on_change);
        let (order_read, unwatched_read) = (order.clone(), unwatched.clone());
        let changed = Callback::new(main_thread, move |_| {
            let chosen = usize::try_from(order_read.indexOfSelectedItem()).unwrap_or(0);
            on_change(Listing {
                sort: Sort::ALL.get(chosen).copied().unwrap_or_default(),
                unwatched_only: unwatched_read.state() == NSControlStateValueOn,
            });
        });
        // SAFETY: the bar keeps the target for as long as it keeps the
        // controls.
        unsafe {
            order.setTarget(Some(&changed));
            order.setAction(Some(Callback::action()));
            unwatched.setTarget(Some(&changed));
            unwatched.setAction(Some(Callback::action()));
        }

        let count = label::line(
            theme::FONT_BODY,
            Weight::Regular,
            &NSColor::secondaryLabelColor(),
            main_thread,
        );
        let view = NSView::new(main_thread);
        for control in [&**order, &**unwatched, &**count] {
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
            unwatched
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&order.leadingAnchor(), -theme::SPACE_M),
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
