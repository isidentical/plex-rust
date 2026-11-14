//! A line near the bottom of the window that says something the viewer
//! asked for did not happen, with a button to ask for it again. The screen
//! under it stays as it was.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAccessibility, NSButton, NSColor, NSControlSize, NSImageView, NSLayoutAttribute,
    NSLayoutConstraint, NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSEdgeInsets, ns_string};

use super::artwork;
use super::callback::Callback;
use super::label::{self, Weight};
use super::theme;

/// What the button does, which changes with what failed.
type Retry = Rc<RefCell<Option<Box<dyn Fn()>>>>;

pub struct Toast {
    bar: Retained<NSView>,
    words: Retained<NSTextField>,
    retry: Retry,
    /// Kept because a button does not retain its target.
    _pressed: Retained<Callback>,
}

impl Toast {
    pub fn new(main_thread: MainThreadMarker) -> Self {
        let glyph = NSImageView::new(main_thread);
        let warning = "exclamationmark.triangle.fill";
        glyph.setImage(artwork::symbol(warning, theme::FONT_BODY, Weight::Medium).as_deref());
        glyph.setContentTintColor(Some(&NSColor::systemYellowColor()));
        // Decoration: the words next to it say what happened.
        glyph.setAccessibilityElement(false);
        let words =
            label::line(theme::FONT_BODY, Weight::Medium, &NSColor::labelColor(), main_thread);

        let retry: Retry = Rc::default();
        let asked = Rc::clone(&retry);
        let pressed = Callback::new(main_thread, move |_| {
            // Taken out first: trying again may fail again and put
            // another in its place.
            let retry = asked.borrow_mut().take();
            if let Some(retry) = retry {
                retry();
            }
        });
        // SAFETY: the toast keeps the target for as long as it keeps the
        // button.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Try Again"),
                Some(&pressed),
                Some(Callback::action()),
                main_thread,
            )
        };
        button.setControlSize(NSControlSize::Large);

        let views: [&NSView; 3] = [&glyph, &words, &button];
        let bar = NSStackView::stackViewWithViews(&NSArray::from_slice(&views), main_thread);
        bar.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        bar.setAlignment(NSLayoutAttribute::CenterY);
        bar.setSpacing(theme::SPACE_S);
        bar.setCustomSpacing_afterView(theme::SPACE_L, &words);
        bar.setEdgeInsets(NSEdgeInsets {
            top: 0.0,
            left: theme::PILL_PADDING,
            bottom: 0.0,
            right: theme::SPACE_S,
        });
        bar.setTranslatesAutoresizingMaskIntoConstraints(false);
        bar.heightAnchor().constraintEqualToConstant(theme::TOUCH_TARGET).setActive(true);
        bar.setWantsLayer(true);
        if let Some(layer) = bar.layer() {
            layer.setCornerRadius(theme::TOUCH_TARGET / 2.0);
            layer.setBackgroundColor(Some(&theme::page_background().CGColor()));
            layer.setBorderWidth(theme::ARTWORK_OUTLINE_WIDTH);
            layer.setBorderColor(Some(
                &theme::white(theme::ARTWORK_HOVER_OUTLINE_OPACITY).CGColor(),
            ));
        }
        bar.setHidden(true);
        Self { bar: Retained::into_super(bar), words, retry, _pressed: pressed }
    }

    /// Puts the line near the bottom of `parent`, over whatever else
    /// `parent` holds and above the line that says the screen is from an
    /// earlier time, in the middle of the part of it that nothing covers.
    pub fn float_in(&self, parent: &NSView) {
        parent.addSubview(&self.bar);
        let clear = parent.safeAreaLayoutGuide();
        let above = -(2.0 * theme::SPACE_L + theme::PILL_HEIGHT);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            self.bar.centerXAnchor().constraintEqualToAnchor(&clear.centerXAnchor()),
            self.bar.bottomAnchor().constraintEqualToAnchor_constant(&parent.bottomAnchor(), above),
            self.bar
                .widthAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&clear.widthAnchor(), -theme::SPACE_XL),
        ]));
    }

    /// Shows the line saying `words`, with a button that calls `retry`.
    pub fn show(&self, words: &str, retry: impl Fn() + 'static) {
        label::set(&self.words, words);
        *self.retry.borrow_mut() = Some(Box::new(retry));
        self.bar.setHidden(false);
    }

    pub fn hide(&self) {
        self.bar.setHidden(true);
        self.retry.borrow_mut().take();
    }
}
