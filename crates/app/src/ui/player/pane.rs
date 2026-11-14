//! A strip of controls along the top or the bottom of the video.
//!
//! A pane shades the picture behind it, darkest at the window's edge, so
//! that white controls stay readable over a bright scene. Shades and not
//! glass: a blur of the video would have to be redone for every frame.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSEvent, NSGradient, NSView,
};
use objc2_foundation::{NSPoint, NSRect};

use super::theme;

/// The edge of the window a pane lies along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

pub struct Ivars {
    edge: Edge,
    shown: Cell<bool>,
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `Pane` does not
    // implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustPlayerPane"]
    #[ivars = Ivars]
    pub struct Pane;

    impl Pane {
        /// A pane that has faded away is not there: a click where one of its
        /// buttons was must not press that button.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            // SAFETY: forwards the argument to the superclass unchanged.
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            hit.filter(|_| self.ivars().shown.get())
        }

        /// A click that misses a control by a little ends here and does
        /// nothing, instead of reaching the video and pausing it.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {}

        #[unsafe(method(drawRect:))]
        fn draw(&self, _dirty: NSRect) {
            let (clear, dark) = (theme::shade(0.0), theme::shade(theme::SHADE_ALPHA));
            // A gradient is drawn from the bottom up at 90 degrees.
            let (bottom, top) = match self.ivars().edge {
                Edge::Top => (clear, dark),
                Edge::Bottom => (dark, clear),
            };
            let gradient = NSGradient::initWithStartingColor_endingColor(
                NSGradient::alloc(),
                &bottom,
                &top,
            );
            if let Some(gradient) = gradient {
                gradient.drawInRect_angle(self.bounds(), 90.0);
            }
        }
    }
);

impl Pane {
    pub fn new(main_thread: MainThreadMarker, edge: Edge) -> Retained<Self> {
        let this = Self::alloc(main_thread).set_ivars(Ivars { edge, shown: Cell::new(true) });
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    /// Fades the pane in or out.
    pub fn set_shown(&self, shown: bool) {
        if self.ivars().shown.replace(shown) == shown {
            return;
        }
        NSAnimationContext::beginGrouping();
        let duration = if shown { theme::FADE_IN } else { theme::FADE_OUT };
        NSAnimationContext::currentContext().setDuration(duration);
        self.animator().setAlphaValue(if shown { 1.0 } else { 0.0 });
        NSAnimationContext::endGrouping();
    }

    /// Whether `point`, in the coordinates of the pane's parent, is on it.
    pub fn holds(&self, point: NSPoint) -> bool {
        let frame = self.frame();
        let (x, y) = (point.x - frame.origin.x, point.y - frame.origin.y);
        (0.0..frame.size.width).contains(&x) && (0.0..frame.size.height).contains(&y)
    }
}
