//! Saving what the window shows as a PNG, without showing the window.
//!
//! The window is put on screen in the window server's sense, at a place no
//! display reaches, and the picture is the one the window server composes
//! of it: glass, the sidebar and the toolbar included, none of which a
//! view can draw into an image of its own. A process may read back its
//! own windows without the screen recording permission. It exists so that
//! an unattended run can check what the interface looks like.

use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSBitmapImageFileType, NSBitmapImageRep, NSScreen, NSWindow,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
#[allow(deprecated)]
use objc2_core_graphics::CGWindowListCreateImage;
use objc2_core_graphics::{CGRectNull, CGWindowID, CGWindowImageOption, CGWindowListOption};
use objc2_foundation::{
    NSArgumentDomain, NSDictionary, NSNumber, NSPoint, NSRect, NSString, NSUserDefaults, ns_string,
};

use crate::error::{Error, Result};

/// How far past the last display the window is put.
const CLEARANCE: f64 = 1000.0;

/// What the viewer can ask of the display in the system's Accessibility
/// settings, for a snapshot to be taken as if they had.
#[derive(Debug, Clone, Copy, Default)]
pub struct Asked {
    pub more_contrast: bool,
    pub less_transparency: bool,
    pub less_motion: bool,
}

/// Has this process, and nothing else on the Mac, find the settings in
/// `asked` switched on. They are read from the user's defaults, where what
/// a process was launched with ranks above what the user has set. Must run
/// before anything reads them, which is before there is an application.
pub fn pretend(asked: Asked) {
    let settings = [
        (ns_string!("increaseContrast"), asked.more_contrast),
        (ns_string!("reduceTransparency"), asked.less_transparency),
        (ns_string!("reduceMotion"), asked.less_motion),
    ];
    let on: Vec<&NSString> =
        settings.iter().filter(|(_, wanted)| *wanted).map(|(key, _)| *key).collect();
    let yes = NSNumber::new_bool(true);
    let values: Vec<&AnyObject> = on.iter().map(|_| -> &AnyObject { &yes }).collect();
    let arguments = NSDictionary::from_slices(&on, &values);
    // SAFETY: a domain holds property list values, which a number is, and
    // the name is a constant that Foundation provides.
    unsafe {
        NSUserDefaults::standardUserDefaults()
            .setVolatileDomain_forName(&arguments, NSArgumentDomain);
    }
}

define_class!(
    // SAFETY: NSWindow is designed to be subclassed, and `Staged` does not
    // implement `Drop`.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustStagedWindow"]
    pub struct Staged;

    impl Staged {
        /// AppKit asks this to keep a window within reach on a display.
        #[unsafe(method(constrainFrameRect:toScreen:))]
        fn constrain(&self, frame: NSRect, _screen: Option<&NSScreen>) -> NSRect {
            frame
        }

        /// A window nobody can see must not take the keyboard from the
        /// window the viewer is working in.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            false
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            false
        }
    }
);

/// A window that stays where it is put, even off every display.
pub fn window(
    frame: NSRect,
    style: NSWindowStyleMask,
    main_thread: MainThreadMarker,
) -> Retained<NSWindow> {
    // SAFETY: the arguments are valid, and the caller sets
    // `setReleasedWhenClosed(false)` as for any window it owns.
    let window: Retained<Staged> = unsafe {
        msg_send![
            Staged::alloc(main_thread),
            initWithContentRect: frame,
            styleMask: style,
            backing: NSBackingStoreType::Buffered,
            defer: false,
        ]
    };
    Retained::into_super(window)
}

/// Has the window server draw `window` where no display shows it.
pub fn stage(window: &NSWindow, main_thread: MainThreadMarker) -> Result<()> {
    let screens = NSScreen::screens(main_thread);
    let frames = || screens.iter().map(|screen| screen.frame());
    let left = frames().map(|frame| frame.origin.x).fold(0.0, f64::min);
    let bottom = frames().map(|frame| frame.origin.y).fold(0.0, f64::min);
    // Not a window to list in Mission Control or to cycle to.
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::Transient
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::Stationary,
    );
    // A window is moved onto a display as it is ordered in, so it goes in
    // clear and is moved away after, and is shown only once it is known
    // to have stayed away.
    window.setAlphaValue(0.0);
    window.orderBack(None);
    let size = window.frame().size;
    window.setFrameOrigin(NSPoint {
        x: left - size.width - CLEARANCE,
        y: bottom - size.height - CLEARANCE,
    });
    let frame = window.frame();
    if frames().any(|screen| intersects(screen, frame)) {
        window.orderOut(None);
        return Err(Error::Snapshot);
    }
    window.setAlphaValue(1.0);
    Ok(())
}

/// Writes the window server's picture of `window`, which [`stage`] put on
/// screen, to `path`.
pub fn write_png(window: &NSWindow, path: &Path) -> Result<()> {
    let id = CGWindowID::try_from(window.windowNumber()).map_err(|_| Error::Snapshot)?;
    // Deprecated in favour of ScreenCaptureKit, which cannot read a window
    // without the screen recording permission, not even the app's own.
    // SAFETY: the null rectangle is a constant that Core Graphics provides.
    #[allow(deprecated)]
    let image = CGWindowListCreateImage(
        unsafe { CGRectNull },
        CGWindowListOption::OptionIncludingWindow,
        id,
        CGWindowImageOption::BoundsIgnoreFraming | CGWindowImageOption::BestResolution,
    );
    let image = image.ok_or(Error::Snapshot)?;
    let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &image);

    // SAFETY: PNG takes no properties, so the empty dictionary is valid.
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    };
    let png = png.ok_or(Error::Snapshot)?;
    std::fs::write(path, png.to_vec())
        .map_err(|source| Error::File { path: path.to_owned(), source })
}

fn intersects(a: NSRect, b: NSRect) -> bool {
    a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}
