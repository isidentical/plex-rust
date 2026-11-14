//! A screen with nothing on it but a message: signed out, server not
//! reachable, nothing found.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityAnnouncementKey,
    NSAccessibilityAnnouncementRequestedNotification, NSAccessibilityPostNotificationWithUserInfo,
    NSAccessibilityPriorityKey, NSAccessibilityPriorityLevel, NSButton, NSColor, NSControlSize,
    NSImageView, NSLayoutAttribute, NSProgressIndicator, NSProgressIndicatorStyle, NSStackView,
    NSTextAlignment, NSTextField, NSUserInterfaceLayoutOrientation, NSView, NSWindow,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, ns_string};

use super::artwork;
use super::callback::Callback;
use super::label::{self, Weight};
use super::theme;

const TITLE_LINES: isize = 2;
/// How many lines the sentence under the title, and the small print, may
/// take before they are cut off.
const LINES: isize = 5;

/// The title of a notice's button and what pressing it does.
pub type Button<'a> = (&'a str, Box<dyn Fn()>);

/// What stands above a notice's title.
#[derive(Clone, Copy)]
pub enum Mark<'a> {
    /// The SF Symbol of this name: something happened, or is asked.
    Symbol(&'a str),
    /// A turning spinner: something is under way and the screen will
    /// move on by itself.
    Spinner,
}

pub struct Notice {
    stack: Retained<NSStackView>,
    /// The title and the sentence under it, as VoiceOver says them.
    spoken: Retained<NSString>,
    button: Option<Retained<NSButton>>,
    /// Kept because a button does not retain its target.
    _pressed: Option<Retained<Callback>>,
}

impl Notice {
    /// A mark, a title and a sentence under it, each centred on the one
    /// above. `button` is the title of a button under those and what
    /// pressing it does.
    pub fn new(
        main_thread: MainThreadMarker,
        mark: Mark,
        title: &str,
        detail: &str,
        button: Option<Button>,
    ) -> Self {
        let secondary = NSColor::secondaryLabelColor();
        let mark = match mark {
            Mark::Symbol(name) => symbol(name, &secondary, main_thread),
            Mark::Spinner => spinner(main_thread),
        };

        let primary = NSColor::labelColor();
        let heading = centred(theme::FONT_TITLE, Weight::Bold, &primary, TITLE_LINES, main_thread);
        label::set(&heading, title);
        let body = centred(theme::FONT_BODY, Weight::Regular, &secondary, LINES, main_thread);
        label::set(&body, detail);

        let mut views: Vec<&NSView> = vec![&mark, &heading, &body];
        let (button, pressed) = match button {
            Some((title, on_press)) => {
                let pressed = Callback::new(main_thread, move |_| on_press());
                // SAFETY: the notice keeps the target for as long as it
                // keeps the button.
                let button = unsafe {
                    NSButton::buttonWithTitle_target_action(
                        &NSString::from_str(title),
                        Some(&pressed),
                        Some(Callback::action()),
                        main_thread,
                    )
                };
                button.setControlSize(NSControlSize::Large);
                // The one thing to do here, so Return does it.
                button.setKeyEquivalent(ns_string!("\r"));
                (Some(button), Some(pressed))
            }
            None => (None, None),
        };
        views.extend(button.as_deref().map(|button| -> &NSView { button }));

        let stack = NSStackView::stackViewWithViews(&NSArray::from_slice(&views), main_thread);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::CenterX);
        stack.setSpacing(theme::SPACE_S);
        stack.setCustomSpacing_afterView(theme::SPACE_L, &mark);
        stack.setCustomSpacing_afterView(theme::SPACE_XL, &body);
        stack.setTranslatesAutoresizingMaskIntoConstraints(false);
        stack
            .widthAnchor()
            .constraintLessThanOrEqualToConstant(theme::NOTICE_WIDTH)
            .setActive(true);
        let spoken = NSString::from_str(&format!("{title}. {detail}"));
        Self { stack, spoken, button, _pressed: pressed }
    }

    /// The message, as large as its words. It goes in the middle of
    /// whatever it stands in for.
    pub fn view(&self) -> &NSView {
        &self.stack
    }

    /// Has VoiceOver say the notice. It took the place of a whole screen
    /// without the viewer asking, and VoiceOver's cursor may have been on
    /// something that is no longer there.
    pub fn announce(&self, window: &NSWindow) {
        let priority = NSNumber::new_isize(NSAccessibilityPriorityLevel::High.0);
        // SAFETY: the keys and the notification are constants that AppKit
        // provides. A window is an accessibility element, and the values
        // are of the types the two keys are documented to take.
        unsafe {
            let info = NSDictionary::<NSString, AnyObject>::from_slices(
                &[NSAccessibilityAnnouncementKey, NSAccessibilityPriorityKey],
                &[&**self.spoken, &**priority],
            );
            NSAccessibilityPostNotificationWithUserInfo(
                window,
                NSAccessibilityAnnouncementRequestedNotification,
                Some(&info),
            );
        }
    }

    /// Adds the error itself in small print at the bottom, for whoever
    /// wants to know exactly what the server said.
    pub fn add_small_print(&self, text: &str, main_thread: MainThreadMarker) {
        let faint = NSColor::tertiaryLabelColor();
        let print = centred(theme::FONT_CAPTION, Weight::Regular, &faint, LINES, main_thread);
        label::set(&print, text);
        // An error names addresses the viewer may want to copy.
        print.setSelectable(true);
        if let Some(last) = self.stack.arrangedSubviews().lastObject() {
            self.stack.setCustomSpacing_afterView(theme::SPACE_XL, &last);
        }
        self.stack.addArrangedSubview(&print);
    }

    /// Greys the button out under a new title, while what it started runs.
    pub fn set_busy(&self, title: &str) {
        if let Some(button) = &self.button {
            button.setTitle(&NSString::from_str(title));
            button.setEnabled(false);
        }
    }
}

fn symbol(name: &str, tint: &NSColor, main_thread: MainThreadMarker) -> Retained<NSView> {
    let glyph = NSImageView::new(main_thread);
    glyph.setImage(artwork::symbol(name, theme::NOTICE_SYMBOL_SIZE).as_deref());
    glyph.setContentTintColor(Some(tint));
    // Decoration: the title under it says what happened.
    glyph.setAccessibilityElement(false);
    Retained::into_super(Retained::into_super(glyph))
}

fn spinner(main_thread: MainThreadMarker) -> Retained<NSView> {
    let spinner = NSProgressIndicator::new(main_thread);
    spinner.setStyle(NSProgressIndicatorStyle::Spinning);
    spinner.setControlSize(NSControlSize::Regular);
    // SAFETY: the sender is not used.
    unsafe { spinner.startAnimation(None) };
    Retained::into_super(spinner)
}

/// Text that wraps onto at most `lines` lines, each centred, no wider
/// than a notice.
fn centred(
    size: f64,
    weight: Weight,
    color: &NSColor,
    lines: isize,
    main_thread: MainThreadMarker,
) -> Retained<NSTextField> {
    let text = label::paragraph(size, weight, color, lines, main_thread);
    text.setAlignment(NSTextAlignment::Center);
    text.setPreferredMaxLayoutWidth(theme::NOTICE_WIDTH);
    text
}
