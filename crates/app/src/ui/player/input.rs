//! A view that hears the keyboard and the mouse.

use std::cell::OnceCell;

use objc2::rc::{Allocated, Retained};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSTrackingArea, NSTrackingAreaOptions, NSView};
use objc2_foundation::{NSPoint, NSRect};

/// A key press, reduced to the keys the app acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Space,
    Return,
    Escape,
    Left,
    Right,
    /// Left with Shift held.
    ShiftLeft,
    ShiftRight,
    Up,
    Down,
    /// A key that types a character, in lower case.
    Character(char),
    /// Not a key: the viewer doing nothing for long enough that the
    /// controls go away. Only the key script of a snapshot can press it,
    /// because a snapshot cannot wait that long.
    Idle,
    /// Not a key either: a state of the player that a snapshot has no
    /// pointer or window of its own to bring about.
    Scene(Scene),
    /// And a jump to 15 seconds before the end, where a snapshot would
    /// otherwise have to play the whole file to get to.
    NearEnd,
    /// A second in which nothing is pressed, for what the key before it
    /// set off to happen before the next key or the snapshot.
    Wait,
}

/// A state of the player that only a snapshot's key script asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scene {
    /// The pointer resting halfway along the scrubber.
    Hover,
    /// The pointer taking the scrubber's knob three quarters of the way
    /// along and keeping hold of it.
    Drag,
    /// A white picture, the hardest to read the controls on.
    Bright,
    /// The window at its smallest.
    Small,
    /// The window in the shape of a 2.39 to 1 film.
    Wide,
    /// Playback inside the item's first intro or credits.
    Marker,
}

impl Key {
    /// Reads a key from its name, as written on a command line: `space`,
    /// `left`, or a single character such as `d`. `idle`, `end`, `wait` and
    /// the names of the scenes stand for what a script cannot do by itself.
    pub fn named(name: &str) -> Option<Self> {
        let mut characters = name.chars();
        match (name, characters.next(), characters.next()) {
            ("space", ..) => Some(Self::Space),
            ("return", ..) => Some(Self::Return),
            ("escape", ..) => Some(Self::Escape),
            ("left", ..) => Some(Self::Left),
            ("right", ..) => Some(Self::Right),
            ("shift-left", ..) => Some(Self::ShiftLeft),
            ("shift-right", ..) => Some(Self::ShiftRight),
            ("up", ..) => Some(Self::Up),
            ("down", ..) => Some(Self::Down),
            ("idle", ..) => Some(Self::Idle),
            ("hover", ..) => Some(Self::Scene(Scene::Hover)),
            ("drag", ..) => Some(Self::Scene(Scene::Drag)),
            ("bright", ..) => Some(Self::Scene(Scene::Bright)),
            ("small", ..) => Some(Self::Scene(Scene::Small)),
            ("wide", ..) => Some(Self::Scene(Scene::Wide)),
            ("marker", ..) => Some(Self::Scene(Scene::Marker)),
            ("end", ..) => Some(Self::NearEnd),
            ("wait", ..) => Some(Self::Wait),
            (_, Some(character), None) => Some(Self::Character(character)),
            _ => None,
        }
    }
}

/// Whether a key event is the key going down or the key being held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    First,
    /// One of the repeats the system sends while a key stays down.
    Held,
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

/// The modifiers that turn a letter into a command.
const COMMANDS: NSEventModifierFlags =
    NSEventModifierFlags::Command.union(NSEventModifierFlags::Control);

#[derive(Default)]
pub struct Ivars {
    on_key: OnceCell<Box<dyn Fn(Key, Press) -> bool>>,
    on_pointer: OnceCell<Box<dyn Fn(Option<NSPoint>)>>,
    on_click: OnceCell<Box<dyn Fn(Clicks)>>,
    on_display_changed: OnceCell<Box<dyn Fn()>>,
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `InputView` does
    // not implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustInputView"]
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
            let press = if event.isARepeat() { Press::Held } else { Press::First };
            let handled = key(event)
                .zip(self.ivars().on_key.get())
                .is_some_and(|(key, on_key)| on_key(key, press));
            if !handled {
                // Lets AppKit beep, as it does for any key nothing wants.
                // SAFETY: forwards the event to the superclass unchanged.
                unsafe { msg_send![super(self), keyDown: event] }
            }
        }

        /// Called when the window moves to a display with another pixel
        /// density, among other things.
        #[unsafe(method(viewDidChangeBackingProperties))]
        fn backing_properties_changed(&self) {
            if let Some(on_display_changed) = self.ivars().on_display_changed.get() {
                on_display_changed();
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            // A third click of a triple click is nothing new.
            let clicks = match event.clickCount() {
                1 => Clicks::Single,
                2 => Clicks::Double,
                _ => return,
            };
            if let Some(on_click) = self.ivars().on_click.get() {
                on_click(clicks);
            }
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            if let Some(on_pointer) = self.ivars().on_pointer.get() {
                on_pointer(Some(self.convertPoint_fromView(event.locationInWindow(), None)));
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            if let Some(on_pointer) = self.ivars().on_pointer.get() {
                on_pointer(None);
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
            | NSTrackingAreaOptions::MouseEnteredAndExited
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
    /// key. `on_pointer` hears where in the view the pointer has moved to,
    /// or `None` when it has left. `on_click` hears clicks on the view
    /// itself, not on the controls in front of it. `on_display_changed` hears the window moving to a
    /// display of another kind. Can be called once.
    pub fn listen(
        &self,
        on_key: impl Fn(Key, Press) -> bool + 'static,
        on_pointer: impl Fn(Option<NSPoint>) + 'static,
        on_click: impl Fn(Clicks) + 'static,
        on_display_changed: impl Fn() + 'static,
    ) {
        // Cannot fail the first time, which is the only time by contract.
        let _ = self.ivars().on_key.set(Box::new(on_key));
        let _ = self.ivars().on_pointer.set(Box::new(on_pointer));
        let _ = self.ivars().on_click.set(Box::new(on_click));
        let _ = self.ivars().on_display_changed.set(Box::new(on_display_changed));
    }
}

fn key(event: &NSEvent) -> Option<Key> {
    let shift = event.modifierFlags().contains(NSEventModifierFlags::Shift);
    match event.keyCode() {
        code::SPACE => Some(Key::Space),
        code::RETURN => Some(Key::Return),
        code::ESCAPE => Some(Key::Escape),
        code::LEFT if shift => Some(Key::ShiftLeft),
        code::RIGHT if shift => Some(Key::ShiftRight),
        code::LEFT => Some(Key::Left),
        code::RIGHT => Some(Key::Right),
        code::UP => Some(Key::Up),
        code::DOWN => Some(Key::Down),
        // AppKit sends a Command or Control combination that no menu item
        // took on to `keyDown:`. Command-C is not the C key.
        _ if event.modifierFlags().intersects(COMMANDS) => None,
        _ => {
            let characters = event.charactersIgnoringModifiers()?.to_string();
            let mut characters = characters.chars();
            let character = characters.next()?;
            // More than one character is not a plain key.
            characters.next().is_none().then(|| Key::Character(character.to_ascii_lowercase()))
        }
    }
}
