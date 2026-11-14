//! The window's title bar while the player has the window.
//!
//! A title bar that draws itself is a band across the top that the picture
//! cannot use. The player makes it see-through and nameless, so the video
//! fills the whole window, and puts it back as it was on the way out.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSButton, NSWindow, NSWindowButton,
    NSWindowTitleVisibility,
};

use super::theme;

const BUTTONS: [NSWindowButton; 3] =
    [NSWindowButton::CloseButton, NSWindowButton::MiniaturizeButton, NSWindowButton::ZoomButton];

/// How the title bar looked before the player changed it.
pub struct Chrome {
    transparent: bool,
    title: NSWindowTitleVisibility,
    /// `None` for a window that has no toolbar.
    toolbar_shown: Option<bool>,
}

impl Chrome {
    /// Clears the title bar of `window` and returns how to undo that.
    pub fn clear(window: &NSWindow) -> Self {
        let toolbar = window.toolbar();
        let before = Self {
            transparent: window.titlebarAppearsTransparent(),
            title: window.titleVisibility(),
            toolbar_shown: toolbar.as_ref().map(|toolbar| toolbar.isVisible()),
        };
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        if let Some(toolbar) = toolbar {
            toolbar.setVisible(false);
        }
        before
    }

    pub fn restore(&self, window: &NSWindow) {
        window.setTitlebarAppearsTransparent(self.transparent);
        window.setTitleVisibility(self.title);
        if let (Some(toolbar), Some(shown)) = (window.toolbar(), self.toolbar_shown) {
            toolbar.setVisible(shown);
        }
        show_buttons(window, true);
    }
}

/// Fades the close, minimise and zoom buttons of `window` in or out, along
/// with the player's own controls.
///
/// A button that has faded out is also hidden once the fade is over. One
/// that is only see-through still takes clicks, and a click on the spot
/// where the close button was would close the window.
pub fn show_buttons(window: &NSWindow, shown: bool) {
    let buttons: Vec<Retained<NSButton>> =
        BUTTONS.into_iter().filter_map(|button| window.standardWindowButton(button)).collect();
    NSAnimationContext::beginGrouping();
    let context = NSAnimationContext::currentContext();
    context.setDuration(if shown { theme::FADE_IN } else { theme::FADE_OUT });
    if !shown {
        let faded = buttons.clone();
        // A fade that is cut short by the controls coming back ends here
        // too, and must leave the buttons in view. Only the fade knows
        // which it was, and what it says is where the buttons now stand.
        context.setCompletionHandler(Some(&RcBlock::new(move || {
            for button in &faded {
                button.setHidden(button.alphaValue() == 0.0);
            }
        })));
    }
    for button in &buttons {
        if shown {
            button.setHidden(false);
        }
        button.animator().setAlphaValue(if shown { 1.0 } else { 0.0 });
    }
    NSAnimationContext::endGrouping();
}
