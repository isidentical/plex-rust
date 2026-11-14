//! A group of controls that comes and goes as one.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSAnimatablePropertyContainer, NSAnimationContext, NSView};
use objc2_foundation::{NSPoint, NSRect};

use super::theme;

pub struct Ivars {
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
    }
);

impl Pane {
    pub fn new(main_thread: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(main_thread).set_ivars(Ivars { shown: Cell::new(true) });
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
