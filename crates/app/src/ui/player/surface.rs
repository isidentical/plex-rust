//! A panel that floats over the picture and holds controls.
//!
//! It is Liquid Glass, as the system's own player and its toolbars are: the
//! picture behind shows through, bent and dimmed, so the controls read as
//! lying on the film and not as a band cut out of it. Where glass will not
//! do, the panel is plain and dark, in the same shape.

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBezierPath, NSEvent, NSGlassEffectView, NSView, NSWorkspace};
use objc2_foundation::{NSInsetRect, NSRect};

use super::layout::cover;
use super::theme;

pub struct Ivars {
    radius: f64,
    /// Whether the panel is all there is. One that lies in glass only
    /// dims it.
    opaque: bool,
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `Plate` does not
    // implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustPlayerPlate"]
    #[ivars = Ivars]
    pub struct Plate;

    impl Plate {
        #[unsafe(method(drawRect:))]
        fn draw(&self, _dirty: NSRect) {
            let (radius, opaque) = (self.ivars().radius, self.ivars().opaque);
            // Glass over a white picture is a light grey, whatever its
            // tint, and white text on it has a contrast of under 3 to 1.
            // A shade laid into the glass brings that to over 5.
            let fill = if opaque { theme::plate() } else { theme::glass_shade() };
            fill.setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), radius, radius)
                .fill();
            if !opaque {
                return;
            }
            // A dark panel on a dark scene has no edge without one.
            let inset = theme::RIM_WIDTH / 2.0;
            let rim = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                NSInsetRect(self.bounds(), inset, inset),
                radius - inset,
                radius - inset,
            );
            rim.setLineWidth(theme::RIM_WIDTH);
            theme::plate_rim().setStroke();
            rim.stroke();
        }

        /// A click between the controls ends here, instead of reaching the
        /// video and pausing it.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {}
    }
);

/// What a panel is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    Glass,
    Plain,
}

impl Material {
    /// Glass, unless the viewer has asked the system for less
    /// transparency.
    pub fn choose() -> Self {
        let reduced = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency();
        if reduced { Self::Plain } else { Self::Glass }
    }
}

/// A floating panel. `view` is what is placed; controls go into `content`
/// and are laid out against it.
pub struct Surface {
    view: Retained<NSView>,
    content: Retained<Plate>,
}

impl Surface {
    /// A panel of `material` whose corners are rounded by `radius`.
    pub fn new(main_thread: MainThreadMarker, material: Material, radius: f64) -> Self {
        let plain = material == Material::Plain;
        let ivars = Ivars { radius, opaque: plain };
        let this = Plate::alloc(main_thread).set_ivars(ivars);
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let content: Retained<Plate> =
            unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        content.setTranslatesAutoresizingMaskIntoConstraints(false);
        if plain {
            let view = Retained::into_super(content.clone());
            return Self { view, content };
        }

        let glass = NSGlassEffectView::new(main_thread);
        glass.setCornerRadius(radius);
        // The controls are white whatever plays behind them. The tint keeps
        // the glass dark enough for that over a bright scene.
        glass.setTintColor(Some(&theme::glass_tint()));
        glass.setContentView(Some(&content));
        glass.setTranslatesAutoresizingMaskIntoConstraints(false);
        // The glass is as large as what it holds asks to be.
        cover(&glass, &content);
        Self { view: Retained::into_super(glass), content }
    }

    /// What to place over the picture.
    pub fn view(&self) -> &NSView {
        &self.view
    }

    /// What holds the controls.
    pub fn content(&self) -> &NSView {
        &self.content
    }
}
