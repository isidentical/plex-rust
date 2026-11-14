//! A view that knows when the pointer is over it.

use std::cell::OnceCell;

use objc2::rc::{Allocated, Retained};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSCursor, NSEvent, NSTrackingArea, NSTrackingAreaOptions, NSView};
use objc2_foundation::NSRect;

#[derive(Default)]
pub struct Ivars {
    on_hover: OnceCell<Box<dyn Fn(bool)>>,
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `HoverView` does
    // not implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustHoverView"]
    #[ivars = Ivars]
    pub struct HoverView;

    impl HoverView {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(Ivars::default());
            // SAFETY: forwards the designated initializer unchanged.
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.hovered(true);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.hovered(false);
        }

        /// Shows the pointing hand over the view: it can be clicked.
        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            self.addCursorRect_cursor(self.bounds(), &NSCursor::pointingHandCursor());
        }
    }
);

impl HoverView {
    /// `on_hover` is called with `true` when the pointer comes over the
    /// view and `false` when it leaves.
    pub fn new(main_thread: MainThreadMarker, on_hover: impl Fn(bool) + 'static) -> Retained<Self> {
        let view: Retained<Self> =
            // SAFETY: `initWithFrame:` is NSView's designated initializer.
            unsafe { msg_send![Self::alloc(main_thread), initWithFrame: NSRect::ZERO] };
        // Cannot fail: the cell was created empty by the initializer.
        let _ = view.ivars().on_hover.set(Box::new(on_hover));

        // Entering and leaving are only reported inside a tracking area.
        // This one follows the view's visible bounds by itself.
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveInKeyWindow
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: the view owns the area, so the owner outlives it.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                options,
                Some(&view),
                None,
            )
        };
        view.addTrackingArea(&area);
        view
    }

    fn hovered(&self, hovered: bool) {
        if let Some(on_hover) = self.ivars().on_hover.get() {
            on_hover(hovered);
        }
    }
}
