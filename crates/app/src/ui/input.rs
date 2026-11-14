//! A view that hears the keyboard and the mouse.

use std::cell::OnceCell;

use objc2::rc::{Allocated, Retained};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSTrackingArea, NSTrackingAreaOptions, NSView};
use objc2_foundation::NSRect;

/// A key press, reduced to the keys the app acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Space,
    Return,
    Escape,
    Left,
    Right,
    Up,
    Down,
    /// A key that types a character, in lower case.
    Character(char),
}

/// How many times the mouse button went down in quick succession.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clicks {
    Single,
    Double,
}

/// Hardware key codes of the keys that type no character.
mod code {
    pub const RETURN: u16 = 36;
    pub const SPACE: u16 = 49;
    pub const ESCAPE: u16 = 53;
    pub const LEFT: u16 = 123;
    pub const RIGHT: u16 = 124;
    pub const DOWN: u16 = 125;
    pub const UP: u16 = 126;
}

#[derive(Default)]
pub struct Ivars {
    on_key: OnceCell<Box<dyn Fn(Key) -> bool>>,
    on_mouse_moved: OnceCell<Box<dyn Fn()>>,
    on_click: OnceCell<Box<dyn Fn(Clicks)>>,
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `InputView` does
    // not implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ReelInputView"]
    #[ivars = Ivars]
    pub struct InputView;

    impl InputView {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(Ivars::default());
            // SAFETY: forwards the designated initializer unchanged.
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        /// Without this, key presses never reach the view.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let handled = key(event)
                .zip(self.ivars().on_key.get())
                .is_some_and(|(key, on_key)| on_key(key));
            if !handled {
                // Lets AppKit beep, as it does for any key nothing wants.
                // SAFETY: forwards the event to the superclass unchanged.
                unsafe { msg_send![super(self), keyDown: event] }
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let clicks = if event.clickCount() >= 2 { Clicks::Double } else { Clicks::Single };
            if let Some(on_click) = self.ivars().on_click.get() {
                on_click(clicks);
            }
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, _event: &NSEvent) {
            if let Some(on_mouse_moved) = self.ivars().on_mouse_moved.get() {
                on_mouse_moved();
            }
        }
    }
);

impl InputView {
    pub fn new(main_thread: MainThreadMarker) -> Retained<Self> {
        let view: Retained<Self> =
            // SAFETY: `initWithFrame:` is NSView's designated initializer.
            unsafe { msg_send![Self::alloc(main_thread), initWithFrame: NSRect::ZERO] };

        // Mouse movement is only delivered inside a tracking area. This one
        // follows the view's visible bounds by itself.
        let options = NSTrackingAreaOptions::MouseMoved
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

    /// Sets what happens on input. `on_key` returns whether it acted on the
    /// key. `on_click` hears clicks on the view itself, not on the controls
    /// in front of it. Can be called once.
    pub fn listen(
        &self,
        on_key: impl Fn(Key) -> bool + 'static,
        on_mouse_moved: impl Fn() + 'static,
        on_click: impl Fn(Clicks) + 'static,
    ) {
        // Cannot fail the first time, which is the only time by contract.
        let _ = self.ivars().on_key.set(Box::new(on_key));
        let _ = self.ivars().on_mouse_moved.set(Box::new(on_mouse_moved));
        let _ = self.ivars().on_click.set(Box::new(on_click));
    }
}

fn key(event: &NSEvent) -> Option<Key> {
    match event.keyCode() {
        code::SPACE => Some(Key::Space),
        code::RETURN => Some(Key::Return),
        code::ESCAPE => Some(Key::Escape),
        code::LEFT => Some(Key::Left),
        code::RIGHT => Some(Key::Right),
        code::UP => Some(Key::Up),
        code::DOWN => Some(Key::Down),
        _ => {
            let characters = event.charactersIgnoringModifiers()?.to_string();
            let mut characters = characters.chars();
            let character = characters.next()?;
            // More than one character is not a plain key.
            characters.next().is_none().then(|| Key::Character(character.to_ascii_lowercase()))
        }
    }
}
