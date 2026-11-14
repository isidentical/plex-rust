//! A closure as the target of a button, menu item or other control.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::NSMenuItem;
use objc2_foundation::{NSObject, NSObjectProtocol};

/// What a callback runs, given the control that was used.
type Then = dyn Fn(Option<&AnyObject>);

pub struct Ivars {
    then: Box<Then>,
    /// Whether there is anything for the closure to do right now.
    enabled: Cell<bool>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Callback` does
    // not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustCallback"]
    #[ivars = Ivars]
    pub struct Callback;

    unsafe impl NSObjectProtocol for Callback {}

    impl Callback {
        #[unsafe(method(call:))]
        fn call(&self, sender: Option<&AnyObject>) {
            // What the closure does may release the control this hangs
            // from, and with it this object, while it is still running.
            let _alive = self.retain();
            (self.ivars().then)(sender);
        }

        /// A menu asks this of an item's target each time it opens, and
        /// before it lets the item's key through.
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, _item: &NSMenuItem) -> bool {
            self.ivars().enabled.get()
        }
    }
);

impl Callback {
    /// The selector to give a control as its action, next to this as its
    /// target. Controls do not retain their targets: whoever owns the
    /// control must keep the callback too.
    pub fn action() -> Sel {
        sel!(call:)
    }

    /// `then` is called with the control that was used.
    pub fn new(
        main_thread: MainThreadMarker,
        then: impl Fn(Option<&AnyObject>) + 'static,
    ) -> Retained<Self> {
        let ivars = Ivars { then: Box::new(then), enabled: Cell::new(true) };
        let this = Self::alloc(main_thread).set_ivars(ivars);
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }

    /// Greys out the menu items this is the target of, or brings them
    /// back. Other controls are enabled by themselves.
    pub fn set_enabled(&self, enabled: bool) {
        self.ivars().enabled.set(enabled);
    }
}
