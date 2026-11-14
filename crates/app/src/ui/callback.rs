//! A closure as the target of a button, menu item or other control.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_foundation::{NSObject, NSObjectProtocol};

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Callback` does
    // not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustCallback"]
    #[ivars = Box<dyn Fn(Option<&AnyObject>)>]
    pub struct Callback;

    unsafe impl NSObjectProtocol for Callback {}

    impl Callback {
        #[unsafe(method(call:))]
        fn call(&self, sender: Option<&AnyObject>) {
            // What the closure does may release the control this hangs
            // from, and with it this object, while it is still running.
            let _alive = self.retain();
            (self.ivars())(sender);
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
        let this = Self::alloc(main_thread).set_ivars(Box::new(then));
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}
