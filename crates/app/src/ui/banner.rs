//! The backdrop of an item as the top of its page: the picture from the
//! window's edge to its edge, under the toolbar and the sidebar, giving way
//! downwards to the page. The artwork's own colour is laid over the part
//! of it that text is set on.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSColor, NSLayoutConstraint, NSView};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::{NSArray, NSNumber, ns_string};
use objc2_quartz_core::{
    CAGradientLayer, CALayer, CAMediaTiming, CATransaction, CATransition,
    kCAGravityResizeAspectFill, kCATransitionFade,
};
use plex::model::Rgb;

use super::artwork;
use super::backdrop;
use super::theme;
use crate::images::Images;

pub struct Banner {
    view: Retained<NSView>,
    /// How far the banner reaches past the header it is in, to the left
    /// and upwards, and with that how much of it the sidebar covers.
    reach: (Retained<NSLayoutConstraint>, Retained<NSLayoutConstraint>),
    covered: Rc<Cell<f64>>,
    /// How many points of the banner, from its bottom, text is set on.
    text: Cell<f64>,
    /// Everything drawn, which fades out towards the bottom.
    drawn: Retained<CALayer>,
    fade: Retained<CAGradientLayer>,
    art: Retained<CALayer>,
    /// The artwork's colour over the bottom of the picture, where the
    /// text is, and over its near side, where the text starts.
    rise: Retained<CAGradientLayer>,
    near: Retained<CAGradientLayer>,
    colour: RefCell<Retained<NSColor>>,
    /// The picture that is shown or on its way.
    wanted: Rc<RefCell<Option<String>>>,
}

impl Banner {
    /// An empty banner at the top of `header`. Whoever places the rest of
    /// the header says how far down it goes.
    pub fn new(header: &NSView, main_thread: MainThreadMarker) -> Self {
        let view = artwork::layered(main_thread);
        header.addSubview(&view);
        let reach = (
            view.leadingAnchor().constraintEqualToAnchor(&header.leadingAnchor()),
            view.topAnchor().constraintEqualToAnchor(&header.topAnchor()),
        );
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            reach.0.clone(),
            reach.1.clone(),
            view.trailingAnchor().constraintEqualToAnchor(&header.trailingAnchor()),
        ]));

        let (drawn, art) = (CALayer::new(), CALayer::new());
        // SAFETY: the gravity is a constant that Core Animation provides.
        art.setContentsGravity(unsafe { kCAGravityResizeAspectFill });
        art.setMasksToBounds(true);
        // Opaque at the top, clear at the bottom. A layer's own coordinates
        // have their origin at the bottom left.
        let upright = || {
            let gradient = CAGradientLayer::new();
            gradient.setStartPoint(CGPoint { x: 0.5, y: 1.0 });
            gradient.setEndPoint(CGPoint { x: 0.5, y: 0.0 });
            gradient
        };
        let (fade, rise, near) = (upright(), upright(), CAGradientLayer::new());
        near.setStartPoint(CGPoint { x: 0.0, y: 0.5 });
        near.setEndPoint(CGPoint { x: 1.0, y: 0.5 });
        set_stops(&fade, &NSColor::blackColor(), &theme::HERO_FADE_STOPS);
        drawn.addSublayer(&art);
        drawn.addSublayer(&rise);
        drawn.addSublayer(&near);
        // SAFETY: a mask is a layer with no parent, which this one is.
        unsafe { drawn.setMask(Some(&fade)) };
        if let Some(layer) = view.layer() {
            layer.addSublayer(&drawn);
        }

        let banner = Self {
            view,
            reach,
            covered: Rc::new(Cell::new(0.0)),
            text: Cell::new(0.0),
            drawn,
            fade,
            art,
            rise,
            near,
            colour: RefCell::new(NSColor::blackColor()),
            wanted: Rc::default(),
        };
        banner.paint(None);
        banner
    }

    pub fn view(&self) -> &NSView {
        &self.view
    }

    /// Hears how much of the page lies under the sidebar, to the left of
    /// the header, and under the toolbar, above it. The banner reaches
    /// that far past the header, to the edges of the window.
    pub fn reach(&self) -> impl Fn(f64, f64) + 'static {
        let (left, up) = self.reach.clone();
        let (covered, view) = (Rc::clone(&self.covered), self.view.clone());
        move |by_sidebar, by_toolbar| {
            left.setConstant(-by_sidebar);
            up.setConstant(-by_toolbar);
            covered.set(by_sidebar);
            view.setNeedsLayout(true);
        }
    }

    /// Sizes what is drawn for the size the view now has, with text on
    /// its lowest `text` points. Layers that are not a view's own do not
    /// follow it.
    pub fn fit(&self, text: f64) {
        self.text.set(text);
        let bounds = CGRect { origin: CGPoint::ZERO, size: self.view.bounds().size };
        // Without this each layer would glide to its new size, a quarter
        // of a second behind the window being resized.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let layers: [&CALayer; 5] = [&self.drawn, &self.art, &self.fade, &self.rise, &self.near];
        for layer in layers {
            layer.setFrame(bounds);
        }
        CATransaction::commit();
        self.shade();
    }

    /// Lays the colour of the artwork over the picture where the text is,
    /// or black for an item with no colours of its own.
    pub fn paint(&self, colour: Option<Rgb>) {
        *self.colour.borrow_mut() = colour.map_or_else(NSColor::blackColor, backdrop::tint);
        self.shade();
    }

    fn shade(&self) {
        let colour = self.colour.borrow();
        let strength = theme::hero_shade_opacity();
        let size = self.view.bounds().size;
        // Nothing of it above the text but a lead-in, most of it where
        // the text begins and all of it at the bottom.
        let begins = (1.0 - self.text.get() / size.height.max(1.0)).clamp(0.0, 1.0);
        let lead = (begins - theme::HERO_SHADE_LEAD).max(0.0);
        let at_text = theme::HERO_SHADE_AT_TEXT * strength;
        let rise = [(0.0, 0.0), (lead, 0.0), (begins, at_text), (1.0, strength)];
        // The near side starts where the sidebar ends: under the sidebar
        // the colour is whole, and from there it thins out over the part
        // of the banner that is clear.
        let width = size.width.max(1.0);
        let from = (self.covered.get() / width).clamp(0.0, 1.0);
        let to = from + (1.0 - from) * theme::HERO_SHADE_REACH;
        let near = [(0.0, strength), (from, strength), (to, 0.0)];
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        set_stops(&self.rise, &colour, &rise);
        set_stops(&self.near, &colour, &near);
        CATransaction::commit();
    }

    /// Shows the backdrop at server path `path`, fading it in when it had
    /// to be fetched, or none.
    pub fn show(&self, path: Option<&str>, images: &Rc<Images>) {
        if self.wanted.borrow().as_deref() == path {
            return;
        }
        *self.wanted.borrow_mut() = path.map(str::to_owned);
        let (width, height) = theme::BACKDROP_REQUEST;
        let ready = path.and_then(|path| images.cached(path, width, height));
        // SAFETY: the contents are a decoded CGImage or nothing.
        unsafe { self.art.setContents(ready.as_ref().map(|artwork| artwork.contents())) };
        let (Some(path), None) = (path, ready) else {
            return;
        };
        let (art, wanted, asked) = (self.art.clone(), Rc::clone(&self.wanted), path.to_owned());
        images.load(path, width, height, move |artwork| {
            if wanted.borrow().as_deref() != Some(asked.as_str()) {
                return;
            }
            let fade = CATransition::new();
            // SAFETY: the type is a constant that Core Animation provides.
            fade.setType(unsafe { kCATransitionFade });
            fade.setDuration(theme::MOTION_ARRIVE);
            art.addAnimation_forKey(&fade, Some(ns_string!("contents")));
            // SAFETY: the contents are a decoded CGImage.
            unsafe { art.setContents(Some(artwork.contents())) };
        });
    }
}

/// Makes `layer` run through `colour` at the strength each of `stops`
/// gives for its place along the way, from 0 at the start to 1 at the end.
fn set_stops(layer: &CAGradientLayer, colour: &NSColor, stops: &[(f64, f64)]) {
    let colours: Vec<_> =
        stops.iter().map(|(_, share)| colour.colorWithAlphaComponent(*share).CGColor()).collect();
    let colours: Vec<&AnyObject> = colours.iter().map(|colour| (**colour).as_ref()).collect();
    let places: Vec<Retained<NSNumber>> =
        stops.iter().map(|(place, _)| NSNumber::new_f64(*place)).collect();
    // SAFETY: a gradient layer takes an array of CGColors.
    unsafe { layer.setColors(Some(&NSArray::from_slice(&colours))) };
    layer.setLocations(Some(&NSArray::from_retained_slice(&places)));
}
