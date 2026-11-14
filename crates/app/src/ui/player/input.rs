//! A view that hears the keyboard and the mouse.

use std::cell::{Cell, OnceCell};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::ProtocolObject;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSEvent, NSEventModifierFlags, NSEventSubtype, NSEventType, NSGestureRecognizer,
    NSGestureRecognizerDelegate, NSGestureRecognizerState, NSMagnificationGestureRecognizer,
    NSPanGestureRecognizer, NSTouch, NSTouchType, NSTrackingArea, NSTrackingAreaOptions, NSView,
};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};

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
    /// Moves the keyboard to the next control.
    Tab,
    /// Tab with Shift held, which moves it to the one before.
    BackTab,
    /// Command with plus, with minus and with 0: the picture larger,
    /// smaller, and whole again.
    ZoomIn,
    ZoomOut,
    ZoomFit,
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
    /// A real press on the scrubber, dragged to the left until the pointer
    /// is far outside the window, and still held there.
    Pull,
    /// The mouse button let go where a pull ended.
    Release,
    /// A white picture, the hardest to read the controls on.
    Bright,
    /// The window at its smallest.
    Small,
    /// The window in the shape of a 2.39 to 1 film.
    Wide,
    /// Playback inside the item's first intro or credits.
    Marker,
    /// Two fingers pinching the picture to twice its size at a point up
    /// and to the right of the middle.
    Pinch,
    /// Playback held up waiting for the network.
    Stalled,
    /// Playback ended by a failure.
    Failed,
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
            ("tab", ..) => Some(Self::Tab),
            ("shift-tab", ..) => Some(Self::BackTab),
            ("zoom-in", ..) => Some(Self::ZoomIn),
            ("zoom-out", ..) => Some(Self::ZoomOut),
            ("zoom-fit", ..) => Some(Self::ZoomFit),
            ("pinch", ..) => Some(Self::Scene(Scene::Pinch)),
            ("idle", ..) => Some(Self::Idle),
            ("hover", ..) => Some(Self::Scene(Scene::Hover)),
            ("drag", ..) => Some(Self::Scene(Scene::Drag)),
            ("pull", ..) => Some(Self::Scene(Scene::Pull)),
            ("release", ..) => Some(Self::Scene(Scene::Release)),
            ("bright", ..) => Some(Self::Scene(Scene::Bright)),
            ("small", ..) => Some(Self::Scene(Scene::Small)),
            ("wide", ..) => Some(Self::Scene(Scene::Wide)),
            ("marker", ..) => Some(Self::Scene(Scene::Marker)),
            ("stall", ..) => Some(Self::Scene(Scene::Stalled)),
            ("fail", ..) => Some(Self::Scene(Scene::Failed)),
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

/// How many times the mouse button went down in quick succession, or a
/// finger on a touch screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clicks {
    Single,
    Double,
    Tap,
    DoubleTap,
}

/// What fingers ask of the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    /// A pinch: the picture is to be `factor` times the size it is, with
    /// what was under `from` now under `to`. Both are in the view.
    Pinch { factor: f64, from: NSPoint, to: NSPoint },
    /// A drag: the picture is to move by this many points.
    Pan(NSSize),
    /// Two fingers tapping twice on a trackpad: between the whole
    /// picture and one that fills the view.
    Toggle,
}

/// Hardware key codes of the keys that type no character.
pub mod code {
    pub const RETURN: u16 = 36;
    pub const TAB: u16 = 48;
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
    on_gesture: OnceCell<Box<dyn Fn(Gesture)>>,
    on_fit_changed: OnceCell<Box<dyn Fn()>>,
    /// How large the pinch in progress had made things, and where it
    /// was, when it was last heard from.
    pinch: Cell<(f64, NSPoint)>,
    /// Whether a drag moves the picture: only while it is larger than
    /// the view.
    pannable: Cell<bool>,
    /// The view the video is drawn in, which lies over this one.
    picture: OnceCell<Retained<NSView>>,
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `InputView` does
    // not implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustInputView"]
    #[ivars = Ivars]
    pub struct InputView;

    unsafe impl NSObjectProtocol for InputView {}

    unsafe impl NSGestureRecognizerDelegate for InputView {
        /// A drag is the picture's only while there is somewhere to move
        /// it to, and only when it starts on the picture: the scrubber and
        /// the buttons keep theirs.
        #[unsafe(method(gestureRecognizer:shouldAttemptToRecognizeWithEvent:))]
        fn should_attempt(&self, recognizer: &NSGestureRecognizer, event: &NSEvent) -> bool {
            let drags = recognizer.downcast_ref::<NSPanGestureRecognizer>().is_some();
            let from_mouse = event.r#type() == NSEventType::LeftMouseDown;
            let on_picture = !from_mouse || self.is_bare_at(event.locationInWindow());
            !drags || (self.ivars().pannable.get() && on_picture)
        }

        /// A finger on the screen that lands on a control is that
        /// control's. Only a touch on a screen says where in a view it is,
        /// and without a view it answers in "the root container of touch"
        /// (NSTouch.h), which is not promised to be the window.
        #[unsafe(method(gestureRecognizer:shouldReceiveTouch:))]
        fn should_receive(&self, _recognizer: &NSGestureRecognizer, touch: &NSTouch) -> bool {
            touch.r#type() != NSTouchType::Direct
                || self.is_bare_at(self.convertPoint_toView(touch.locationInView(Some(self)), None))
        }
    }

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
            if let Some(on_fit_changed) = self.ivars().on_fit_changed.get() {
                on_fit_changed();
            }
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: forwards the argument to the superclass unchanged.
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            if let Some(on_fit_changed) = self.ivars().on_fit_changed.get() {
                on_fit_changed();
            }
        }

        /// The magnification a recognizer reports is the whole pinch so
        /// far. What has changed since it was last heard from is what the
        /// picture is asked for, so that each step starts from where the
        /// picture now is, clamped or not.
        #[unsafe(method(pinched:))]
        fn pinched(&self, recognizer: &NSMagnificationGestureRecognizer) {
            let place = recognizer.locationInView(Some(self));
            let size = 1.0 + recognizer.magnification();
            let began = recognizer.state() == NSGestureRecognizerState::Began;
            let (last_size, last_place) =
                if began { (1.0, place) } else { self.ivars().pinch.get() };
            // Fingers brought all the way together report a size of
            // nothing, which nothing can be scaled by.
            if size <= 0.0 || last_size <= 0.0 {
                return;
            }
            self.ivars().pinch.set((size, place));
            self.ask(Gesture::Pinch { factor: size / last_size, from: last_place, to: place });
        }

        #[unsafe(method(panned:))]
        fn panned(&self, recognizer: &NSPanGestureRecognizer) {
            let by = recognizer.translationInView(Some(self));
            recognizer.setTranslation_inView(NSPoint::ZERO, Some(self));
            self.ask(Gesture::Pan(NSSize::new(by.x, by.y)));
        }

        /// Two fingers sliding on a trackpad, which arrive as scrolling.
        /// A wheel's clicks are not a drag and are left alone.
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            if self.ivars().pannable.get() && event.hasPreciseScrollingDeltas() {
                // Scrolling counts downwards, a view upwards.
                let by = NSSize::new(event.scrollingDeltaX(), -event.scrollingDeltaY());
                self.ask(Gesture::Pan(by));
            }
        }

        #[unsafe(method(smartMagnifyWithEvent:))]
        fn smart_magnify(&self, _event: &NSEvent) {
            self.ask(Gesture::Toggle);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            // A third click of a triple click is nothing new. A mouse
            // event that a finger on the screen made says so.
            let touched = event.subtype() == NSEventSubtype::Touch;
            let clicks = match (event.clickCount(), touched) {
                (1, false) => Clicks::Single,
                (1, true) => Clicks::Tap,
                (2, false) => Clicks::Double,
                (2, true) => Clicks::DoubleTap,
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
        view.hear_gestures(main_thread);
        view
    }

    /// Asks to hear pinches and drags.
    ///
    /// A recognizer hears a pinch on a trackpad, and since macOS 27 the
    /// fingers on a touch screen too: NSGestureRecognizer.h there says
    /// of `allowedTouchTypes` that it "defaults to NSTouchTypeDirect as
    /// of macOS 27". The responder's own `magnifyWithEvent:` is "sent
    /// only on hardware capable of generating" such events, and its
    /// `touchesBegan` family is "used for indirect touches" only
    /// (NSResponder.h), so neither would hear a screen.
    fn hear_gestures(&self, main_thread: MainThreadMarker) {
        let target: &objc2::runtime::AnyObject = self;
        // SAFETY: `pinched:` and `panned:` are defined above with the
        // recognizer as their argument. A recognizer does not retain its
        // target, which as its view outlives it.
        let (pinch, pan) = unsafe {
            (
                NSMagnificationGestureRecognizer::initWithTarget_action(
                    NSMagnificationGestureRecognizer::alloc(main_thread),
                    Some(target),
                    Some(sel!(pinched:)),
                ),
                NSPanGestureRecognizer::initWithTarget_action(
                    NSPanGestureRecognizer::alloc(main_thread),
                    Some(target),
                    Some(sel!(panned:)),
                ),
            )
        };
        let recognizers: [&NSGestureRecognizer; 2] = [&pinch, &pan];
        for recognizer in recognizers {
            recognizer.setDelegate(Some(ProtocolObject::from_ref(self)));
            self.addGestureRecognizer(recognizer);
        }
    }

    /// Says whether a drag moves the picture.
    pub fn set_pannable(&self, pannable: bool) {
        self.ivars().pannable.set(pannable);
    }

    fn ask(&self, gesture: Gesture) {
        if let Some(on_gesture) = self.ivars().on_gesture.get() {
            on_gesture(gesture);
        }
    }

    /// Names the view the video is drawn in. It covers this one, so it is
    /// what a finger or the pointer on the picture lands on. Can be
    /// called once.
    pub fn hold_picture(&self, picture: &NSView) {
        let _ = self.ivars().picture.set(picture.retain());
    }

    /// Whether `place`, in the window's coordinates, is on the picture
    /// and on none of the controls in front of it.
    fn is_bare_at(&self, place: NSPoint) -> bool {
        // The content view fills the window, so a point in the window is
        // the same point in the view above it, which a hit test is given.
        let hit = self.window().and_then(|window| window.contentView()?.hitTest(place));
        let this: &NSView = self;
        let picture = self.ivars().picture.get();
        hit.is_some_and(|hit| {
            std::ptr::eq(&*hit, this) || picture.is_some_and(|picture| hit == *picture)
        })
    }

    /// Sets what happens on input. `on_key` returns whether it acted on the
    /// key. `on_pointer` hears where in the view the pointer has moved to,
    /// or `None` when it has left. `on_click` hears clicks on the view
    /// itself, not on the controls in front of it. `on_gesture` hears
    /// what fingers ask of the picture. `on_fit_changed` hears that the
    /// view has another size, or the window a display of another kind.
    /// Can be called once.
    pub fn listen(
        &self,
        on_key: impl Fn(Key, Press) -> bool + 'static,
        on_pointer: impl Fn(Option<NSPoint>) + 'static,
        on_click: impl Fn(Clicks) + 'static,
        on_gesture: impl Fn(Gesture) + 'static,
        on_fit_changed: impl Fn() + 'static,
    ) {
        // Cannot fail the first time, which is the only time by contract.
        let _ = self.ivars().on_key.set(Box::new(on_key));
        let _ = self.ivars().on_pointer.set(Box::new(on_pointer));
        let _ = self.ivars().on_click.set(Box::new(on_click));
        let _ = self.ivars().on_gesture.set(Box::new(on_gesture));
        let _ = self.ivars().on_fit_changed.set(Box::new(on_fit_changed));
    }
}

fn key(event: &NSEvent) -> Option<Key> {
    let shift = event.modifierFlags().contains(NSEventModifierFlags::Shift);
    match event.keyCode() {
        code::SPACE => Some(Key::Space),
        code::RETURN => Some(Key::Return),
        code::ESCAPE => Some(Key::Escape),
        code::TAB if shift => Some(Key::BackTab),
        code::TAB => Some(Key::Tab),
        code::LEFT if shift => Some(Key::ShiftLeft),
        code::RIGHT if shift => Some(Key::ShiftRight),
        code::LEFT => Some(Key::Left),
        code::RIGHT => Some(Key::Right),
        code::UP => Some(Key::Up),
        code::DOWN => Some(Key::Down),
        // AppKit sends a Command or Control combination that no menu item
        // took on to `keyDown:`. Command-C is not the C key. Three of
        // them are the picture's, as they are a page's in a browser.
        _ if event.modifierFlags().intersects(COMMANDS) => {
            let command = event.modifierFlags().contains(NSEventModifierFlags::Command);
            match (command, event.charactersIgnoringModifiers()?.to_string().as_str()) {
                (true, "=" | "+") => Some(Key::ZoomIn),
                (true, "-") => Some(Key::ZoomOut),
                (true, "0") => Some(Key::ZoomFit),
                _ => None,
            }
        }
        _ => {
            let characters = event.charactersIgnoringModifiers()?.to_string();
            let mut characters = characters.chars();
            let character = characters.next()?;
            // More than one character is not a plain key.
            characters.next().is_none().then(|| Key::Character(character.to_ascii_lowercase()))
        }
    }
}
