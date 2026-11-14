//! A view that knows when the pointer is over it, and that can be a link.

use std::cell::{Cell, OnceCell};

use objc2::rc::{Allocated, Retained};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
};
use objc2_app_kit::{NSCursor, NSEvent, NSTrackingArea, NSTrackingAreaOptions, NSView};
use objc2_foundation::{NSPoint, NSRect};

#[derive(Default)]
pub struct Ivars {
    on_hover: OnceCell<Box<dyn Fn(bool)>>,
    /// Set for a link: what a click on it does.
    on_click: OnceCell<Box<dyn Fn()>>,
    /// Whether the link leads anywhere at the moment.
    is_live: Cell<bool>,
    /// Whether the pointer turns into a hand over a live link.
    shows_hand: Cell<bool>,
    /// Whether the pointer was over the view when last heard of.
    is_over: Cell<bool>,
    /// For a view that is no link but stands for a button: what pressing
    /// it through VoiceOver does. A click on such a view is handled by
    /// whatever it sits in.
    on_press: OnceCell<Box<dyn Fn()>>,
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

        /// A live link shows the pointing hand, as links do everywhere.
        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            if self.ivars().is_live.get() && self.ivars().shows_hand.get() {
                self.addCursorRect_cursor(self.bounds(), &NSCursor::pointingHandCursor());
            }
        }

        /// A link takes the press itself, so that the release comes to it
        /// too. Any other view leaves it to what is behind it.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if !self.ivars().is_live.get() {
                // SAFETY: forwards the event unchanged.
                let () = unsafe { msg_send![super(self), mouseDown: event] };
            }
        }

        /// VoiceOver presses a view that says it is a button. Without
        /// this the press is offered and then does nothing.
        #[unsafe(method(accessibilityPerformPress))]
        fn accessibility_perform_press(&self) -> bool {
            let ivars = self.ivars();
            let on_click = ivars.on_click.get().filter(|_| ivars.is_live.get());
            let press = on_click.or(ivars.on_press.get());
            // What the press leads to may replace the page this view is on.
            let _alive = self.retain();
            press.inspect(|press| press()).is_some()
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let point = self.convertPoint_fromView(event.locationInWindow(), None);
            let (Some(on_click), true, true) = (
                self.ivars().on_click.get(),
                self.ivars().is_live.get(),
                self.mouse_inRect(point, self.bounds()),
            ) else {
                // SAFETY: forwards the event unchanged.
                return unsafe { msg_send![super(self), mouseUp: event] };
            };
            // Where the link leads may replace the page this view is on.
            let _alive = self.retain();
            on_click();
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
        // This one follows the view's visible bounds by itself, and works
        // in a window that is not the key one, as long as the app is in
        // front: the pointer should not have to click first to get a
        // reaction.
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveInActiveApp
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

    /// A view that is also a link: `on_click` is called when it is clicked
    /// while [live](Self::set_live). It starts out live.
    pub fn link(
        main_thread: MainThreadMarker,
        on_hover: impl Fn(bool) + 'static,
        on_click: impl Fn() + 'static,
    ) -> Retained<Self> {
        let view = Self::pressable(main_thread, on_hover, on_click);
        view.ivars().shows_hand.set(true);
        view
    }

    /// A view that takes clicks as a link does but keeps the arrow
    /// pointer, for a surface that is pressed like a button.
    pub fn pressable(
        main_thread: MainThreadMarker,
        on_hover: impl Fn(bool) + 'static,
        on_click: impl Fn() + 'static,
    ) -> Retained<Self> {
        let view = Self::new(main_thread, on_hover);
        // Cannot fail: the cell was created empty by the initializer.
        let _ = view.ivars().on_click.set(Box::new(on_click));
        view.ivars().is_live.set(true);
        view
    }

    /// Whether [`set_on_press`](Self::set_on_press) was called.
    pub fn has_on_press(&self) -> bool {
        self.ivars().on_press.get().is_some()
    }

    /// Says what pressing the view through VoiceOver does, once: a view
    /// that already knows keeps what it was told first.
    pub fn set_on_press(&self, on_press: impl Fn() + 'static) {
        // Cannot matter if it fails: the first answer stands.
        let _ = self.ivars().on_press.set(Box::new(on_press));
    }

    /// Turns a link on or off. Off, it is a plain view: no hand, no click.
    pub fn set_live(&self, live: bool) {
        self.ivars().is_live.set(live);
        if let Some(window) = self.window() {
            window.invalidateCursorRectsForView(self);
        }
    }

    /// Tells the view the pointer came over it or left. AppKit calls this
    /// through the tracking area, and a snapshot calls it to show the state.
    pub fn hovered(&self, hovered: bool) {
        self.ivars().is_over.set(hovered);
        if let Some(on_hover) = self.ivars().on_hover.get() {
            on_hover(hovered);
        }
    }

    /// Checks whether `pointer`, in the window's coordinates, is over the
    /// view, and reports it if that changed. AppKit reports entering and
    /// leaving only when the pointer moves, not when the view is scrolled
    /// out from under it.
    pub fn follow(&self, pointer: NSPoint) {
        let over = self.mouse_inRect(self.convertPoint_fromView(pointer, None), self.bounds());
        if over != self.ivars().is_over.get() {
            self.hovered(over);
        }
    }
}
