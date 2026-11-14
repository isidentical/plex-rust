//! A screen with nothing on it but a message: signed out, server not
//! reachable, nothing found.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSButton, NSColor, NSControlSize, NSFontWeightBold, NSImageView, NSLayoutAttribute,
    NSLayoutConstraint, NSStackView, NSTextAlignment, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSString, ns_string};

use super::artwork;
use super::callback::Callback;
use super::text;
use super::theme;

pub struct Notice {
    view: Retained<NSView>,
    button: Option<Retained<NSButton>>,
    /// Kept because a button does not retain its target.
    _pressed: Option<Retained<Callback>>,
}

impl Notice {
    /// A symbol, a title and a sentence under it, centred. `button` is the
    /// title of a button under those and what pressing it does.
    pub fn new(
        main_thread: MainThreadMarker,
        symbol: &str,
        title: &str,
        detail: &str,
        button: Option<(&str, Retained<Callback>)>,
    ) -> Self {
        let secondary = NSColor::secondaryLabelColor();
        let glyph = NSImageView::new(main_thread);
        glyph.setImage(artwork::symbol(symbol, theme::NOTICE_SYMBOL_SIZE).as_deref());
        glyph.setContentTintColor(Some(&secondary));

        // SAFETY: the weight is a constant that AppKit provides.
        let bold = unsafe { NSFontWeightBold };
        let heading = text::label(theme::FONT_TITLE, bold, &NSColor::labelColor(), main_thread);
        text::set(&heading, title);
        let body = text::paragraph(theme::FONT_BODY, &secondary, 6, main_thread);
        text::set(&body, detail);
        body.setAlignment(NSTextAlignment::Center);
        body.setPreferredMaxLayoutWidth(theme::NOTICE_WIDTH);
        // An error names addresses the viewer may want to copy.
        body.setSelectable(true);

        let mut views: Vec<&NSView> = vec![&glyph, &heading, &body];
        let (button, pressed) = match button {
            Some((title, pressed)) => {
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
                button.setKeyEquivalent(ns_string!("\r"));
                (Some(button), Some(pressed))
            }
            None => (None, None),
        };
        views.extend(button.as_deref().map(|button| -> &NSView { button }));

        let stack = NSStackView::stackViewWithViews(&NSArray::from_slice(&views), main_thread);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::CenterX);
        stack.setSpacing(theme::SPACE_M);
        stack.setCustomSpacing_afterView(theme::SPACE_L, &glyph);
        stack.setCustomSpacing_afterView(theme::SPACE_XL, &body);
        stack.setTranslatesAutoresizingMaskIntoConstraints(false);

        let view = NSView::new(main_thread);
        view.addSubview(&stack);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            stack.centerXAnchor().constraintEqualToAnchor(&view.centerXAnchor()),
            stack.centerYAnchor().constraintEqualToAnchor(&view.centerYAnchor()),
            stack.widthAnchor().constraintLessThanOrEqualToConstant(theme::NOTICE_WIDTH),
        ]));
        Self { view, button, _pressed: pressed }
    }

    pub fn view(&self) -> &NSView {
        &self.view
    }

    /// Greys the button out under a new title, while what it started runs.
    pub fn set_busy(&self, title: &str) {
        if let Some(button) = &self.button {
            button.setTitle(&NSString::from_str(title));
            button.setEnabled(false);
        }
    }
}
