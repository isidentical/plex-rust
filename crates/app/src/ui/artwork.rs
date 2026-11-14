//! A piece of artwork as every screen shows it: rounded, filled edge to
//! edge, with its progress, its unwatched mark and its reaction to the
//! pointer.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAccessibility, NSAnimationContext, NSColor, NSImage, NSImageSymbolConfiguration, NSImageView,
    NSLayoutConstraint, NSTextField, NSView,
};
use objc2_core_foundation::{CGAffineTransform, CGSize};
use objc2_foundation::{NSArray, NSPoint, NSString, ns_string};
use objc2_quartz_core::{
    CALayer, CAMediaTiming, CATransition, kCACornerCurveContinuous, kCAGravityResizeAspectFill,
    kCATransitionFade,
};

use super::label::{self, Weight};
use super::layout;
use super::theme;
use crate::images::{Artwork, Images};

/// What sits in the corner of the artwork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    None,
    /// Not watched yet.
    Unwatched,
    /// This many episodes inside are not watched yet.
    Left(u32),
}

pub struct ArtView {
    /// Carries the lift: the scale and the shadow.
    root: Retained<NSView>,
    /// Drawn around the picture, a little outside it, while the keyboard
    /// is on it.
    ring: Retained<NSView>,
    /// Clipped to rounded corners. Its layer shows the picture.
    picture: Retained<NSView>,
    /// Darkens the picture under the play glyph.
    scrim: Retained<NSView>,
    play: Retained<NSImageView>,
    /// Stands in for a picture there is none of.
    placeholder: Retained<NSImageView>,
    track: Retained<NSView>,
    fill: Retained<NSView>,
    fill_width: RefCell<Option<Retained<NSLayoutConstraint>>>,
    dot: Retained<NSView>,
    pill: Retained<NSView>,
    count: Retained<NSTextField>,
    /// The artwork path this view is waiting for. A cell is reused for
    /// another item while its picture is still on the way, and the late
    /// picture must not land on the wrong item.
    wanted: Rc<RefCell<Option<String>>>,
    playable: Cell<bool>,
    hovered: Cell<bool>,
    pressed: Cell<bool>,
    focused: Cell<bool>,
    current: Cell<bool>,
}

impl ArtView {
    pub fn new(main_thread: MainThreadMarker, radius: f64) -> Self {
        let root = layered(main_thread);
        let picture = layered(main_thread);
        if let Some(layer) = picture.layer() {
            // SAFETY: both are constants that Core Animation provides.
            unsafe {
                layer.setCornerCurve(kCACornerCurveContinuous);
                layer.setContentsGravity(kCAGravityResizeAspectFill);
            }
            layer.setMasksToBounds(true);
            // What shows while the picture is on its way.
            let placeholder = theme::white(theme::ARTWORK_PLACEHOLDER_OPACITY);
            layer.setBackgroundColor(Some(&placeholder.CGColor()));
        }
        let ring = layered(main_thread);
        ring.setHidden(true);
        if let Some(layer) = ring.layer() {
            layer.setBorderWidth(theme::FOCUS_RING_WIDTH);
            layer.setBorderColor(Some(&NSColor::controlAccentColor().CGColor()));
            // SAFETY: the curve is a constant that Core Animation provides.
            unsafe { layer.setCornerCurve(kCACornerCurveContinuous) };
        }
        root.addSubview(&ring);
        let outset = theme::FOCUS_RING_OUTSET;
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            ring.leadingAnchor().constraintEqualToAnchor_constant(&root.leadingAnchor(), -outset),
            ring.trailingAnchor().constraintEqualToAnchor_constant(&root.trailingAnchor(), outset),
            ring.topAnchor().constraintEqualToAnchor_constant(&root.topAnchor(), -outset),
            ring.bottomAnchor().constraintEqualToAnchor_constant(&root.bottomAnchor(), outset),
        ]));
        layout::cover(&root, &picture);

        let placeholder = NSImageView::new(main_thread);
        placeholder.setContentTintColor(Some(&theme::white(theme::PLACEHOLDER_GLYPH_OPACITY)));
        placeholder.setTranslatesAutoresizingMaskIntoConstraints(false);
        placeholder.setHidden(true);
        placeholder.setAccessibilityElement(false);
        picture.addSubview(&placeholder);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            placeholder.centerXAnchor().constraintEqualToAnchor(&picture.centerXAnchor()),
            placeholder.centerYAnchor().constraintEqualToAnchor(&picture.centerYAnchor()),
        ]));

        let scrim = layered(main_thread);
        fill_with(&scrim, &theme::black(theme::HOVER_SCRIM_OPACITY));
        scrim.setAlphaValue(0.0);
        layout::cover(&picture, &scrim);
        let play = play_glyph(main_thread);
        scrim.addSubview(&play);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            play.centerXAnchor().constraintEqualToAnchor(&scrim.centerXAnchor()),
            play.centerYAnchor().constraintEqualToAnchor(&scrim.centerYAnchor()),
        ]));

        let track = layered(main_thread);
        fill_with(&track, &theme::black(theme::PROGRESS_TRACK_OPACITY));
        let fill = layered(main_thread);
        fill_with(&fill, &NSColor::controlAccentColor());
        for bar in [&track, &fill] {
            if let Some(layer) = bar.layer() {
                layer.setCornerRadius(theme::PROGRESS_BAR_HEIGHT / 2.0);
            }
        }
        picture.addSubview(&track);
        track.addSubview(&fill);
        let inset = theme::PROGRESS_BAR_INSET;
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            track.leadingAnchor().constraintEqualToAnchor_constant(&picture.leadingAnchor(), inset),
            track
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&picture.trailingAnchor(), -inset),
            track.bottomAnchor().constraintEqualToAnchor_constant(&picture.bottomAnchor(), -inset),
            track.heightAnchor().constraintEqualToConstant(theme::PROGRESS_BAR_HEIGHT),
            fill.leadingAnchor().constraintEqualToAnchor(&track.leadingAnchor()),
            fill.topAnchor().constraintEqualToAnchor(&track.topAnchor()),
            fill.bottomAnchor().constraintEqualToAnchor(&track.bottomAnchor()),
        ]));

        let dot = layered(main_thread);
        fill_with(&dot, &NSColor::controlAccentColor());
        if let Some(layer) = dot.layer() {
            layer.setCornerRadius(theme::BADGE_DOT_SIZE / 2.0);
            // A white rim keeps the dot apart from dark artwork, and its
            // own colour keeps it apart from light artwork.
            layer.setBorderWidth(theme::BADGE_DOT_RIM_WIDTH);
            layer.setBorderColor(Some(&theme::white(theme::BADGE_DOT_RIM_OPACITY).CGColor()));
            layer.setShadowOpacity(theme::BADGE_SHADOW_OPACITY);
            layer.setShadowRadius(theme::BADGE_SHADOW_RADIUS);
            layer.setShadowOffset(CGSize::ZERO);
        }
        let (pill, count) = count_pill(main_thread);
        let inset = theme::BADGE_INSET;
        for badge in [&dot, &pill] {
            picture.addSubview(badge);
            NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
                badge
                    .trailingAnchor()
                    .constraintEqualToAnchor_constant(&picture.trailingAnchor(), -inset),
                badge.topAnchor().constraintEqualToAnchor_constant(&picture.topAnchor(), inset),
            ]));
        }
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            dot.widthAnchor().constraintEqualToConstant(theme::BADGE_DOT_SIZE),
            dot.heightAnchor().constraintEqualToConstant(theme::BADGE_DOT_SIZE),
        ]));

        let art = Self {
            root,
            ring,
            picture,
            scrim,
            play,
            placeholder,
            track,
            fill,
            fill_width: RefCell::new(None),
            dot,
            pill,
            count,
            wanted: Rc::new(RefCell::new(None)),
            playable: Cell::new(false),
            hovered: Cell::new(false),
            pressed: Cell::new(false),
            focused: Cell::new(false),
            current: Cell::new(false),
        };
        // A cell that is drawn before it is given an item shows nothing
        // but its empty frame.
        art.set_radius(radius);
        art.set_progress(0.0);
        art.set_badge(Badge::None);
        art.restyle();
        art
    }

    pub fn view(&self) -> &NSView {
        &self.root
    }

    /// Shows the artwork at server path `path`, requested for a box of
    /// `size` points. A picture already in memory appears at once. One that
    /// has to be fetched fades in when it arrives. Without a path, the
    /// SF Symbol `placeholder` stands in, if one is given.
    pub fn show(
        &self,
        path: Option<&str>,
        size: (u32, u32),
        placeholder: Option<&str>,
        images: &Rc<Images>,
    ) {
        let glyph = placeholder.filter(|_| path.is_none());
        let glyph = glyph.and_then(|name| symbol(name, theme::PORTRAIT_GLYPH_SIZE));
        self.placeholder.setImage(glyph.as_deref());
        self.placeholder.setHidden(glyph.is_none());
        // The same picture is already here or on its way. Asking again
        // would fetch and decode it a second time.
        if path.is_some() && self.wanted.borrow().as_deref() == path {
            return;
        }
        let (width, height) = (size.0 * theme::RETINA_SCALE, size.1 * theme::RETINA_SCALE);
        *self.wanted.borrow_mut() = path.map(str::to_owned);
        let ready = path.and_then(|path| images.cached(path, width, height));
        set_contents(&self.picture, ready.as_ref());
        let (Some(path), None) = (path, ready) else {
            return;
        };

        let (picture, wanted, asked) =
            (self.picture.clone(), Rc::clone(&self.wanted), path.to_owned());
        images.load(path, width, height, move |artwork| {
            if wanted.borrow().as_deref() != Some(asked.as_str()) {
                return;
            }
            if let Some(layer) = picture.layer() {
                let fade = CATransition::new();
                // SAFETY: the type is a constant that Core Animation provides.
                fade.setType(unsafe { kCATransitionFade });
                fade.setDuration(theme::ARTWORK_FADE);
                layer.addAnimation_forKey(&fade, Some(ns_string!("contents")));
            }
            set_contents(&picture, Some(artwork));
        });
    }

    /// Rounds the corners of the picture by `radius` points.
    pub fn set_radius(&self, radius: f64) {
        if let Some(layer) = self.picture.layer() {
            layer.setCornerRadius(radius);
        }
        if let Some(layer) = self.ring.layer() {
            layer.setCornerRadius(radius + theme::FOCUS_RING_OUTSET);
        }
    }

    /// Marks the artwork as standing for what the page is about.
    pub fn set_current(&self, current: bool) {
        self.current.set(current);
        self.restyle();
    }

    /// Draws how much was watched, from 0 to 1. Zero hides the bar.
    pub fn set_progress(&self, fraction: f64) {
        self.track.setHidden(fraction <= 0.0);
        if let Some(old) = self.fill_width.borrow_mut().take() {
            old.setActive(false);
        }
        let width = self.fill.widthAnchor().constraintEqualToAnchor_multiplier(
            &self.track.widthAnchor(),
            fraction.clamp(0.0, 1.0),
        );
        width.setActive(true);
        *self.fill_width.borrow_mut() = Some(width);
    }

    pub fn set_badge(&self, badge: Badge) {
        self.dot.setHidden(badge != Badge::Unwatched);
        self.pill.setHidden(!matches!(badge, Badge::Left(_)));
        if let Badge::Left(episodes) = badge {
            label::set(&self.count, &episodes.to_string());
        }
    }

    /// Whether the pointer brings up a play glyph.
    pub fn set_playable(&self, playable: bool) {
        self.playable.set(playable);
    }

    pub fn set_hovered(&self, hovered: bool) {
        self.hovered.set(hovered);
        self.restyle_animated();
    }

    pub fn set_pressed(&self, pressed: bool) {
        self.pressed.set(pressed);
        self.restyle_animated();
    }

    /// Whether the keyboard is on this artwork.
    pub fn set_focused(&self, focused: bool) {
        self.focused.set(focused);
        self.restyle();
    }

    /// Takes the pointer's marks off the artwork, for a cell about to show
    /// another item. Where the keyboard is stays as the collection view
    /// last said, which tells every cell it hands out.
    pub fn reset(&self) {
        self.hovered.set(false);
        self.pressed.set(false);
        self.restyle();
    }

    /// Whether `point`, in the window's coordinates, is on the play glyph
    /// while it is showing.
    pub fn is_on_play(&self, point: NSPoint) -> bool {
        let shown = self.playable.get() && self.hovered.get();
        let local = self.play.convertPoint_fromView(point, None);
        shown && self.play.mouse_inRect(local, self.play.bounds())
    }

    fn restyle_animated(&self) {
        NSAnimationContext::beginGrouping();
        let context = NSAnimationContext::currentContext();
        context.setDuration(theme::HOVER_DURATION);
        context.setAllowsImplicitAnimation(true);
        self.restyle();
        NSAnimationContext::endGrouping();
    }

    /// Makes the views match the pointer and keyboard state.
    fn restyle(&self) {
        let (hovered, pressed, focused) =
            (self.hovered.get(), self.pressed.get(), self.focused.get());
        let scale = match (pressed, hovered) {
            _ if theme::reduces_motion() => 1.0,
            (true, _) => theme::PRESS_SCALE,
            (false, true) => theme::HOVER_SCALE,
            (false, false) => 1.0,
        };
        if let Some(layer) = self.root.layer() {
            layer.setAffineTransform(scaled_about_center(&layer, scale));
            layer.setShadowOpacity(if hovered { theme::HOVER_SHADOW_OPACITY } else { 0.0 });
            layer.setShadowRadius(theme::HOVER_SHADOW_RADIUS);
            layer.setShadowOffset(CGSize { width: 0.0, height: -theme::HOVER_SHADOW_DROP });
        }
        if let Some(layer) = self.picture.layer() {
            let hairline = theme::ARTWORK_OUTLINE_WIDTH;
            let (color, width) = match (self.current.get(), hovered) {
                (true, _) => {
                    (theme::white(theme::CURRENT_OUTLINE_OPACITY), theme::CURRENT_OUTLINE_WIDTH)
                }
                (false, true) => (theme::white(theme::ARTWORK_HOVER_OUTLINE_OPACITY), hairline),
                (false, false) => (theme::white(theme::artwork_outline_opacity()), hairline),
            };
            layer.setBorderColor(Some(&color.CGColor()));
            layer.setBorderWidth(width);
        }
        self.ring.setHidden(!focused);
        let glyph_shown = hovered && self.playable.get();
        self.scrim.setAlphaValue(if glyph_shown { 1.0 } else { 0.0 });
    }
}

/// A scale that keeps the middle of `layer` where it is, wherever the
/// layer's anchor point happens to be.
fn scaled_about_center(layer: &CALayer, scale: f64) -> CGAffineTransform {
    let (bounds, anchor) = (layer.bounds(), layer.anchorPoint());
    let shift = |length: f64, anchor: f64| (1.0 - scale) * (0.5 - anchor) * length;
    CGAffineTransform {
        a: scale,
        b: 0.0,
        c: 0.0,
        d: scale,
        tx: shift(bounds.size.width, anchor.x),
        ty: shift(bounds.size.height, anchor.y),
    }
}

fn set_contents(picture: &NSView, artwork: Option<&Artwork>) {
    if let Some(layer) = picture.layer() {
        // SAFETY: the contents are a decoded CGImage or nothing, both of
        // which a layer accepts.
        unsafe { layer.setContents(artwork.map(Artwork::contents)) };
    }
}

/// A view with a layer, placed by constraints.
pub fn layered(main_thread: MainThreadMarker) -> Retained<NSView> {
    let view = NSView::new(main_thread);
    view.setWantsLayer(true);
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    view
}

pub fn fill_with(view: &NSView, color: &NSColor) {
    if let Some(layer) = view.layer() {
        layer.setBackgroundColor(Some(&color.CGColor()));
    }
}

/// An SF Symbol at `size` points.
pub fn symbol(name: &str, size: f64) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    let configuration = NSImageSymbolConfiguration::configurationWithPointSize_weight(
        size,
        Weight::Semibold.value(),
    );
    image.imageWithSymbolConfiguration(&configuration)
}

fn play_glyph(main_thread: MainThreadMarker) -> Retained<NSImageView> {
    let glyph = NSImageView::new(main_thread);
    glyph.setImage(symbol("play.circle.fill", theme::PLAY_GLYPH_SIZE).as_deref());
    glyph.setContentTintColor(Some(&NSColor::whiteColor()));
    glyph.setTranslatesAutoresizingMaskIntoConstraints(false);
    // The card it is on speaks for it.
    glyph.setAccessibilityElement(false);
    glyph
}

/// A dark capsule with a number in it.
fn count_pill(main_thread: MainThreadMarker) -> (Retained<NSView>, Retained<NSTextField>) {
    let pill = layered(main_thread);
    fill_with(&pill, &theme::black(theme::BADGE_BACKGROUND_OPACITY));
    if let Some(layer) = pill.layer() {
        layer.setCornerRadius(theme::BADGE_HEIGHT / 2.0);
    }
    let count =
        label::line(theme::FONT_CAPTION, Weight::Semibold, &NSColor::whiteColor(), main_thread);
    pill.addSubview(&count);
    let padding = theme::BADGE_PADDING;
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        pill.heightAnchor().constraintEqualToConstant(theme::BADGE_HEIGHT),
        pill.widthAnchor().constraintGreaterThanOrEqualToConstant(theme::BADGE_HEIGHT),
        count.centerXAnchor().constraintEqualToAnchor(&pill.centerXAnchor()),
        count.centerYAnchor().constraintEqualToAnchor(&pill.centerYAnchor()),
        count
            .leadingAnchor()
            .constraintGreaterThanOrEqualToAnchor_constant(&pill.leadingAnchor(), padding),
    ]));
    (pill, count)
}
