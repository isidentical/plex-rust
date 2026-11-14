//! A line at the bottom of the window that says what is on screen is from
//! an earlier time, with a button to ask the server again.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAccessibility, NSButton, NSColor, NSControlSize, NSImageView, NSLayoutAttribute,
    NSLayoutConstraint, NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSEdgeInsets, NSString};

use super::artwork;
use super::callback::Callback;
use super::label::{self, Weight};
use super::theme;

const RETRY: &str = "Try Again";

/// Why what is on screen is not what the server would say now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Nothing answered: the server is off, asleep or on another network.
    Offline,
    /// The server answered with something that could not be used.
    Refused,
    /// The server is being asked again.
    Asking,
}

impl Reason {
    fn words(self) -> &'static str {
        match self {
            Self::Offline => "Offline, showing what was saved",
            Self::Refused => "Not up to date, the server could not be asked",
            Self::Asking => "Asking your server again",
        }
    }
}

/// The line itself. It floats over the screen without taking clicks from
/// anything but the strip it covers, and is hidden until there is
/// something to say.
pub struct Stale {
    bar: Retained<NSView>,
    words: Retained<NSTextField>,
    retry: Retained<NSButton>,
    /// Kept because a button does not retain its target.
    _pressed: Retained<Callback>,
}

impl Stale {
    pub fn new(main_thread: MainThreadMarker, on_retry: impl Fn() + 'static) -> Self {
        let glyph = NSImageView::new(main_thread);
        glyph.setImage(artwork::symbol("wifi.slash", theme::FONT_BODY, Weight::Medium).as_deref());
        glyph.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        // Decoration: the words next to it say what it stands for.
        glyph.setAccessibilityElement(false);

        let words =
            label::line(theme::FONT_BODY, Weight::Medium, &NSColor::labelColor(), main_thread);
        let pressed = Callback::new(main_thread, move |_| on_retry());
        // SAFETY: `Stale` keeps the target for as long as it keeps the
        // button.
        let retry = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(RETRY),
                Some(&pressed),
                Some(Callback::action()),
                main_thread,
            )
        };
        retry.setControlSize(NSControlSize::Small);

        let views: [&NSView; 3] = [&glyph, &words, &retry];
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
        bar.heightAnchor().constraintEqualToConstant(theme::PILL_HEIGHT).setActive(true);
        bar.setWantsLayer(true);
        if let Some(layer) = bar.layer() {
            layer.setCornerRadius(theme::PILL_HEIGHT / 2.0);
            layer.setBackgroundColor(Some(&theme::page_background().CGColor()));
            layer.setBorderWidth(theme::ARTWORK_OUTLINE_WIDTH);
            layer.setBorderColor(Some(
                &theme::white(theme::ARTWORK_HOVER_OUTLINE_OPACITY).CGColor(),
            ));
        }
        bar.setHidden(true);
        Self { bar: Retained::into_super(bar), words, retry, _pressed: pressed }
    }

    /// Puts the line at the bottom of `parent`, over whatever else
    /// `parent` holds, in the middle of the part of it that nothing
    /// covers: pages run on under the sidebar's glass.
    pub fn float_in(&self, parent: &NSView) {
        parent.addSubview(&self.bar);
        let clear = parent.safeAreaLayoutGuide();
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            self.bar.centerXAnchor().constraintEqualToAnchor(&clear.centerXAnchor()),
            self.bar
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&parent.bottomAnchor(), -theme::SPACE_L),
        ]));
    }

    /// Shows the line saying `reason`, or hides it for `None`.
    pub fn show(&self, reason: Option<Reason>) {
        self.bar.setHidden(reason.is_none());
        let Some(reason) = reason else {
            return;
        };
        label::set(&self.words, reason.words());
        // While the server is being asked there is nothing to press.
        self.retry.setEnabled(reason != Reason::Asking);
    }

    pub fn is_shown(&self) -> bool {
        !self.bar.isHidden()
    }
}
