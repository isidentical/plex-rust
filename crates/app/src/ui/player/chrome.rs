//! The window's title bar while the player has the window.
//!
//! A title bar that draws itself is a band across the top that the picture
//! cannot use. The player makes it see-through and nameless, so the video
//! fills the whole window, and puts it back as it was on the way out.

use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSWindow, NSWindowButton,
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
pub fn show_buttons(window: &NSWindow, shown: bool) {
    NSAnimationContext::beginGrouping();
    let duration = if shown { theme::FADE_IN } else { theme::FADE_OUT };
    NSAnimationContext::currentContext().setDuration(duration);
    for button in BUTTONS {
        if let Some(button) = window.standardWindowButton(button) {
            button.animator().setAlphaValue(if shown { 1.0 } else { 0.0 });
        }
    }
    NSAnimationContext::endGrouping();
}
