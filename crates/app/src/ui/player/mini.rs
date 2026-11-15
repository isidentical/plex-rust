//! The mini player: a small window that floats above other apps and holds
//! the video while the player's own window is put away.
//!
//! The video view moves into it as it is, with its layer, its renderer and
//! its frame clock, so the film plays on without a pause and without being
//! loaded again. The system's Picture in Picture is not used: it shows what
//! is put into an `AVSampleBufferDisplayLayer`, and mpv draws through
//! OpenGL into a layer of the app's own.

use std::cell::OnceCell;

use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::rc::{Allocated, Retained, Weak};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSEventType, NSFloatingWindowLevel, NSPanel, NSScreen,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow, NSWindowCollectionBehavior,
    NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, ns_string};

use super::chip::{Chip, Style};
use super::input::{self, Key, Press};
use super::layout::{add, cover, pin, row};
use super::pane::Pane;
use super::surface::{Material, Surface};
use super::theme;
use crate::error::Result;
use crate::ui::snapshot;

/// The shape of a picture that is not known yet.
const WIDESCREEN: (f64, f64) = (16.0, 9.0);

pub struct Mini {
    window: Retained<NSWindow>,
    controls: Retained<Pane>,
    pub toggle: Retained<Chip>,
    /// Back to the player's own window.
    pub expand: Retained<Chip>,
    /// Stops playback, as leaving the player does.
    pub close: Retained<Chip>,
    root: Retained<MiniView>,
}

impl Mini {
    /// Opens the mini window in the corner of the display that `beside` is
    /// on, and moves `video` into it. `picture` is the width and height of
    /// the picture, which the window takes the shape of. A `staged` window
    /// is a snapshot's, where no display shows it.
    pub fn open(
        main_thread: MainThreadMarker,
        video: &NSView,
        picture: Option<(f64, f64)>,
        beside: &NSWindow,
        material: Material,
        staged: bool,
    ) -> Result<Self> {
        let shape = shape(picture);
        let screen = beside.screen().or_else(|| NSScreen::mainScreen(main_thread));
        let frame = corner(screen.map(|screen| screen.visibleFrame()), shape);
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::Resizable;
        let window = if staged {
            snapshot::window(frame, style, main_thread)
        } else {
            panel(main_thread, frame, style | NSWindowStyleMask::NonactivatingPanel)
        };
        // SAFETY: called before the window can be closed. `Mini` owns it.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setLevel(NSFloatingWindowLevel);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        // A panel goes away whenever its app is not the active one, which
        // is all the time for a window that floats above other apps.
        window.setHidesOnDeactivate(false);
        // The corners are the content's, rounded below. The window around
        // them is clear, and its shadow follows what is drawn.
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(true);
        window.setContentAspectRatio(shape);
        window.setContentMinSize(smallest(shape));

        let button = |symbol, name, key| Chip::button(main_thread, symbol, name, key);
        let mini = Self {
            controls: Pane::new(main_thread),
            toggle: button("pause.fill", "Pause", "Space"),
            expand: button("pip.exit", "Back to the window", "O"),
            close: Chip::new(main_thread, Style::Plain),
            root: MiniView::new(main_thread),
            window,
        };
        mini.close.set_symbol("xmark", theme::SYMBOL_SIZE);
        mini.close.set_name("Close");
        mini.close.setToolTip(Some(ns_string!("Close")));
        mini.root.setWantsLayer(true);
        if let Some(layer) = mini.root.layer() {
            layer.setCornerRadius(theme::MINI_RADIUS);
            layer.setMasksToBounds(true);
            layer.setBackgroundColor(Some(&NSColor::blackColor().CGColor()));
        }
        mini.window.setContentView(Some(&mini.root));
        mini.place_controls(main_thread, material);
        mini.hold(video);
        let controls = Weak::new(&*mini.controls);
        mini.root.on_hover(move |inside| {
            if let Some(controls) = controls.load() {
                controls.set_shown(inside);
            }
        });
        // Out of sight until the pointer comes over the picture.
        mini.controls.set_shown(false);

        if staged {
            snapshot::stage(&mini.window, main_thread)?;
        } else {
            mini.window.makeKeyAndOrderFront(None);
        }
        Ok(mini)
    }

    /// The buttons, side by side on one capsule above the bottom edge.
    fn place_controls(&self, main_thread: MainThreadMarker, material: Material) {
        let (root, pane) = (&self.root, &self.controls);
        let surface = Surface::new(main_thread, material, theme::BAR_RADIUS);
        let bar = surface.content();
        let buttons: [&NSView; 3] = [&self.expand, &self.toggle, &self.close];
        let buttons = row(&buttons, theme::CONTROL_SPACING, main_thread);
        add(root, pane);
        add(pane, surface.view());
        cover(pane, surface.view());
        add(bar, &buttons);
        pin(&[
            pane.centerXAnchor().constraintEqualToAnchor(&root.centerXAnchor()),
            pane.bottomAnchor()
                .constraintEqualToAnchor_constant(&root.bottomAnchor(), -theme::MINI_BAR_GAP),
            buttons
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&bar.leadingAnchor(), theme::BAR_INSET),
            buttons
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&bar.trailingAnchor(), -theme::BAR_INSET),
            buttons
                .topAnchor()
                .constraintEqualToAnchor_constant(&bar.topAnchor(), theme::BAR_INSET),
            buttons
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&bar.bottomAnchor(), -theme::BAR_INSET),
        ]);
    }

    /// Puts `video` under the buttons, filling the window.
    fn hold(&self, video: &NSView) {
        put_under(&self.root, video);
    }

    /// The window, for a snapshot to take its picture.
    pub fn window(&self) -> &NSWindow {
        &self.window
    }

    /// Sets what happens on a key while the window has the keyboard, and
    /// when the window has another size or display. `on_key` returns
    /// whether it acted on the key. Can be called once.
    pub fn listen(
        &self,
        on_key: impl Fn(Key, Press) -> bool + 'static,
        on_fit_changed: impl Fn() + 'static,
    ) {
        // Cannot fail the first time, which is the only time by contract.
        let _ = self.root.ivars().on_key.set(Box::new(on_key));
        let _ = self.root.ivars().on_fit_changed.set(Box::new(on_fit_changed));
    }

    /// Shows the button that resumes a paused film, or pauses a playing one.
    pub fn show_paused(&self, paused: bool) {
        let (symbol, name) = if paused { ("play.fill", "Play") } else { ("pause.fill", "Pause") };
        self.toggle.set_symbol(symbol, theme::SYMBOL_SIZE);
        self.toggle.set_name(name);
    }

    /// Brings the buttons up, or takes them away, as the pointer does.
    pub fn show_controls(&self, shown: bool) {
        self.controls.set_shown(shown);
    }

    /// Gives the window the shape of a picture `picture` wide and high,
    /// keeping its width.
    pub fn keep_shape(&self, picture: (f64, f64)) {
        let shape = shape(Some(picture));
        let window = &self.window;
        window.setContentAspectRatio(shape);
        window.setContentMinSize(smallest(shape));
        let mut frame = window.frame();
        let height = frame.size.width * shape.height / shape.width;
        // The top edge stays where it is, as when a window is resized by
        // its lower corner.
        frame.origin.y += frame.size.height - height;
        frame.size.height = height;
        window.setFrame_display(frame, true);
    }

    /// Closes the window. What it held has to have been moved out first.
    pub fn close(self) {
        self.window.orderOut(None);
        self.window.close();
        // This may be running inside a click on one of the window's own
        // buttons, which AppKit is still delivering. The window is let go
        // of once that is over.
        let main_thread = self.window.mtm();
        let mini = MainThreadBound::new(self, main_thread);
        DispatchQueue::main().exec_async(move || drop(mini));
    }
}

/// Adds `video` to `parent` behind everything else in it, covering all of
/// it. A view has one parent, so this takes it from the one it had.
pub fn put_under(parent: &NSView, video: &NSView) {
    parent.addSubview_positioned_relativeTo(video, NSWindowOrderingMode::Below, None);
    cover(parent, video);
}

/// The shape of a picture `picture` wide and high, or of a widescreen one
/// while that is not known.
fn shape(picture: Option<(f64, f64)>) -> NSSize {
    let (width, height) = picture.filter(|(w, h)| *w > 0.0 && *h > 0.0).unwrap_or(WIDESCREEN);
    NSSize::new(width, height)
}

/// The smallest the window may be made, in the picture's shape.
fn smallest(shape: NSSize) -> NSSize {
    let width = theme::MINI_MIN_WIDTH;
    NSSize::new(width, width * shape.height / shape.width)
}

/// Where the window first goes: in the lower right corner of the part of
/// the display that the Dock and the menu bar leave, `visible`.
fn corner(visible: Option<NSRect>, shape: NSSize) -> NSRect {
    let size = NSSize::new(theme::MINI_WIDTH, theme::MINI_WIDTH * shape.height / shape.width);
    let Some(visible) = visible else {
        return NSRect::new(NSPoint::ZERO, size);
    };
    let gap = theme::MINI_SCREEN_GAP;
    let origin = NSPoint::new(
        visible.origin.x + visible.size.width - size.width - gap,
        visible.origin.y + gap,
    );
    NSRect::new(origin, size)
}

define_class!(
    // SAFETY: NSPanel is designed to be subclassed, and `MiniWindow` does
    // not implement `Drop`.
    #[unsafe(super(NSPanel, NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustMiniWindow"]
    struct MiniWindow;

    impl MiniWindow {
        /// A window with no title bar does not take the keyboard by itself
        /// (`canBecomeKeyWindow` in NSWindow.h), and this one has keys.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }

        /// The player's own window stays the app's main window.
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            false
        }

        /// The first click on a window that does not have the keyboard
        /// only gives it the keyboard, unless the view clicked asks for
        /// that click. Here the click is always meant for a button or for
        /// moving the window, so the window takes the keyboard first.
        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            if event.r#type() == NSEventType::LeftMouseDown && !self.isKeyWindow() {
                self.makeKeyWindow();
            }
            // SAFETY: forwards the event to the superclass unchanged.
            unsafe { msg_send![super(self), sendEvent: event] }
        }
    }
);

/// A panel that floats above other apps without making its own app the
/// active one when it is clicked.
fn panel(
    main_thread: MainThreadMarker,
    frame: NSRect,
    style: NSWindowStyleMask,
) -> Retained<NSWindow> {
    // SAFETY: the arguments are valid, and the caller sets
    // `setReleasedWhenClosed(false)` as for any window it owns.
    let window: Retained<MiniWindow> = unsafe {
        msg_send![
            MiniWindow::alloc(main_thread),
            initWithContentRect: frame,
            styleMask: style,
            backing: NSBackingStoreType::Buffered,
            defer: false,
        ]
    };
    Retained::into_super(Retained::into_super(window))
}

#[derive(Default)]
pub struct Ivars {
    on_key: OnceCell<Box<dyn Fn(Key, Press) -> bool>>,
    on_hover: OnceCell<Box<dyn Fn(bool)>>,
    on_fit_changed: OnceCell<Box<dyn Fn()>>,
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `MiniView` does not
    // implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustMiniView"]
    #[ivars = Ivars]
    struct MiniView;

    unsafe impl NSObjectProtocol for MiniView {}

    impl MiniView {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(Ivars::default());
            // SAFETY: forwards the designated initializer unchanged.
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let press = if event.isARepeat() { Press::Held } else { Press::First };
            let handled = input::key(event)
                .zip(self.ivars().on_key.get())
                .is_some_and(|(key, on_key)| on_key(key, press));
            if !handled {
                // Lets AppKit beep, as it does for any key nothing wants.
                // SAFETY: forwards the event to the superclass unchanged.
                unsafe { msg_send![super(self), keyDown: event] }
            }
        }

        /// A press anywhere on the picture moves the window. The buttons
        /// take their own presses and never send them here.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if let Some(window) = self.window() {
                window.performWindowDragWithEvent(event);
            }
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.hover(true);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.hover(false);
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn backing_properties_changed(&self) {
            self.fit_changed();
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: forwards the argument to the superclass unchanged.
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            self.fit_changed();
        }
    }
);

impl MiniView {
    fn new(main_thread: MainThreadMarker) -> Retained<Self> {
        let view: Retained<Self> =
            // SAFETY: `initWithFrame:` is NSView's designated initializer.
            unsafe { msg_send![Self::alloc(main_thread), initWithFrame: NSRect::ZERO] };
        // Whatever app is in front: the window floats above all of them.
        let options = NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveAlways
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

    fn on_hover(&self, on_hover: impl Fn(bool) + 'static) {
        // Cannot fail: set once, as the window is built.
        let _ = self.ivars().on_hover.set(Box::new(on_hover));
    }

    fn hover(&self, inside: bool) {
        if let Some(on_hover) = self.ivars().on_hover.get() {
            on_hover(inside);
        }
    }

    fn fit_changed(&self) {
        if let Some(on_fit_changed) = self.ivars().on_fit_changed.get() {
            on_fit_changed();
        }
    }
}
