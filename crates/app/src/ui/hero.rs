//! The top of the home screen: the first few things to watch, one at a
//! time and large, as a banner that runs under the toolbar and the
//! sidebar to the edges of the window. Each has its backdrop on the right,
//! running into the artwork's own colour on the left, which holds its
//! title and a button that plays it.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityButtonRole, NSColor, NSImageView, NSLayoutConstraint,
    NSLayoutPriorityDefaultHigh, NSStackView, NSTextField, NSView,
};
use objc2_core_foundation::CGPoint;
use objc2_foundation::{NSArray, NSNumber, NSString, ns_string};
use objc2_quartz_core::{
    CAAutoresizingMask, CAGradientLayer, CAMediaTiming, CATransaction, CATransition,
    kCACornerCurveContinuous, kCATransitionFade,
};
use plex::model::{Item, Kind, Rgb};

use super::artwork::{self, ArtView};
use super::backdrop;
use super::callback::Callback;
use super::detail::Pill;
use super::hover::HoverView;
use super::label::{self, Weight};
use super::layout;
use super::poster;
use super::posters::{Action, HeaderKeys, Press, Shelf};
use super::text;
use super::theme;
use crate::images::Images;

/// How many items the hero pages through at most.
const PAGES: usize = 3;

/// The first few items of the first shelf, to show large above it. They
/// stay on their shelf too: taken off it, a shelf of four or five would be
/// left with one or two cards in a row made for many.
pub fn features(shelves: &[Shelf]) -> Vec<Item> {
    let Some(first) = shelves.first() else {
        return Vec::new();
    };
    first.cards.iter().take(PAGES).map_while(|card| card.item().cloned()).collect()
}

pub struct Hero(Rc<Views>);

struct Views {
    view: Retained<NSView>,
    /// Drawn around the play button while the keyboard is on the hero.
    ring: Retained<NSView>,
    ring_place: RefCell<Option<Retained<NSArray<NSLayoutConstraint>>>>,
    /// Painted in the colour of the item, and larger than `view` by what
    /// of the page lies under the sidebar and the toolbar.
    banner: Retained<NSView>,
    /// How far the banner reaches past `view` to the left and upwards.
    reach: (Retained<NSLayoutConstraint>, Retained<NSLayoutConstraint>),
    art: ArtView,
    /// Runs the banner's colour over the near edge of the artwork.
    shade: Retained<NSView>,
    fade: Retained<CAGradientLayer>,
    /// Runs the page's colour over the bottom of the banner.
    floor: Retained<NSView>,
    /// As large as `view`, which is the part of the page clear of the
    /// sidebar and the toolbar. Holds everything that says something
    /// about the item, and takes the click that opens it.
    card: Retained<HoverView>,
    title: Retained<NSTextField>,
    facts: Retained<NSTextField>,
    play: Pill,
    /// How much is left, in words over a bar, beside the button.
    left: Retained<NSTextField>,
    track: Retained<NSView>,
    fill: Retained<NSView>,
    fill_width: RefCell<Option<Retained<NSLayoutConstraint>>>,
    /// The way from one item to the next, shown when there are several.
    pager: Retained<NSView>,
    dots: Vec<Retained<HoverView>>,
    items: RefCell<Vec<Item>>,
    /// Which of the items is shown.
    at: Cell<usize>,
    images: Rc<Images>,
    on_action: Box<dyn Fn(Action)>,
    /// Kept because a button does not retain its target.
    _pressed: Retained<Callback>,
}

impl Hero {
    /// An empty hero, drawn as a grey outline until [`show`](Self::show)
    /// is called. `on_action` hears a click on the play button as
    /// [`Action::Play`] and a click anywhere else as [`Action::Open`].
    pub fn new(
        main_thread: MainThreadMarker,
        images: Rc<Images>,
        on_action: impl Fn(Action) + 'static,
    ) -> Self {
        let views = Rc::new_cyclic(|this: &Weak<Views>| {
            // Each handler looks the hero up when it runs, and does
            // nothing once the hero is gone.
            let with = |act: fn(&Views)| {
                let this = Weak::clone(this);
                move || {
                    if let Some(views) = this.upgrade() {
                        act(&views);
                    }
                }
            };
            Views::new(main_thread, images, Box::new(on_action), this, with)
        });
        views.place();
        views.show_item(false);
        Self(views)
    }

    pub fn view(&self) -> &NSView {
        &self.0.view
    }

    /// What the hero does with the keyboard once the arrow keys reach it:
    /// Left and Right go from item to item, and Return plays the one
    /// shown, or opens it when it is not something to play.
    pub fn keys(&self) -> HeaderKeys {
        let (focused, pressed) = (Rc::downgrade(&self.0), Rc::downgrade(&self.0));
        HeaderKeys {
            focus: Box::new(move |focused_now| {
                let views = focused.upgrade();
                let takes = views.as_ref().is_some_and(|views| !views.items.borrow().is_empty());
                if let Some(views) = views {
                    views.ring.setHidden(!(focused_now && takes));
                }
                takes
            }),
            press: Box::new(move |press| {
                let Some(views) = pressed.upgrade() else {
                    return;
                };
                match press {
                    Press::Left => views.turn(-1),
                    Press::Right => views.turn(1),
                    Press::Return => views.act(|item| match poster::is_playable(&item) {
                        true => Action::Play(item),
                        false => Action::Open(item),
                    }),
                    Press::Up | Press::Down => {}
                }
            }),
        }
    }

    /// Hears how much of the page lies under the sidebar, to the left of
    /// the hero's own view, and under the toolbar, above it. The banner
    /// reaches that far past the view, to the edges of the window.
    pub fn reach(&self) -> impl Fn(f64, f64) + 'static {
        let views = Rc::downgrade(&self.0);
        move |left, up| {
            if let Some(views) = views.upgrade() {
                views.reach.0.setConstant(-left);
                views.reach.1.setConstant(-up);
            }
        }
    }

    /// Shows the first of `items`, or the one that was on screen if it is
    /// still among them, and offers the others. With none it leaves the
    /// space empty.
    pub fn show(&self, items: Vec<Item>) {
        let views = &self.0;
        let was = views.items.borrow().get(views.at.get()).map(|item| item.rating_key.clone());
        let at = was.and_then(|was| items.iter().position(|item| item.rating_key == was));
        views.at.set(at.unwrap_or(0));
        views.items.replace(items);
        views.show_item(false);
    }
}

impl Views {
    fn new<H: Fn() + 'static>(
        main_thread: MainThreadMarker,
        images: Rc<Images>,
        on_action: Box<dyn Fn(Action)>,
        this: &Weak<Self>,
        with: impl Fn(fn(&Self)) -> H,
    ) -> Self {
        let art = ArtView::new(main_thread, 0.0);
        art.set_flat();
        let card = HoverView::pressable(main_thread, |_| {}, with(|views| views.act(Action::Open)));
        // VoiceOver reads the hero as one button that opens the item, with
        // the play button inside it as another.
        card.setAccessibilityElement(true);
        // SAFETY: the role is a constant that AppKit provides.
        card.setAccessibilityRole(Some(unsafe { NSAccessibilityButtonRole }));
        let view = NSView::new(main_thread);
        let banner = artwork::layered(main_thread);
        if let Some(layer) = banner.layer() {
            layer.setMasksToBounds(true);
        }
        let reach = (
            banner.leadingAnchor().constraintEqualToAnchor(&view.leadingAnchor()),
            banner.topAnchor().constraintEqualToAnchor(&view.topAnchor()),
        );

        let white = NSColor::whiteColor();
        let faint = theme::white(theme::SECONDARY_ON_ART_OPACITY);
        let lines = theme::HOME_HERO_TITLE_LINES;
        let title =
            label::paragraph(theme::FONT_LARGE_TITLE, Weight::Bold, &white, lines, main_thread);
        let facts = label::line(theme::FONT_HEADLINE, Weight::Medium, &faint, main_thread);
        let left = label::line(theme::FONT_CAPTION, Weight::Medium, &faint, main_thread);

        let press = with(|views| views.act(Action::Play));
        let pressed = Callback::new(main_thread, move |_| press());
        let play = Pill::new("Play", "play.fill", true, main_thread);
        // SAFETY: the hero keeps the target for as long as it keeps the
        // button.
        unsafe {
            play.button.setTarget(Some(&pressed));
            play.button.setAction(Some(Callback::action()));
        }

        let track = artwork::layered(main_thread);
        artwork::fill_with(&track, &theme::white(theme::HOME_HERO_TRACK_OPACITY));
        let fill = artwork::layered(main_thread);
        artwork::fill_with(&fill, &NSColor::controlAccentColor());
        for bar in [&track, &fill] {
            if let Some(layer) = bar.layer() {
                layer.setCornerRadius(theme::PROGRESS_BAR_HEIGHT / 2.0);
            }
        }

        let dots: Vec<Retained<HoverView>> = (0..PAGES)
            .map(|page| {
                let this = Weak::clone(this);
                dot(main_thread, move || {
                    if let Some(views) = this.upgrade() {
                        views.turn_to(page);
                    }
                })
            })
            .collect();
        let back = chevron("chevron.left", "Previous", with(|views| views.turn(-1)), main_thread);
        let on = chevron("chevron.right", "Next", with(|views| views.turn(1)), main_thread);
        let mut stops: Vec<&NSView> = vec![&back];
        stops.extend(dots.iter().map(|dot| -> &NSView { dot }));
        stops.push(&on);
        let pager = pager(&stops, main_thread);

        let ring = artwork::layered(main_thread);
        ring.setHidden(true);
        if let Some(layer) = ring.layer() {
            layer.setBorderWidth(theme::FOCUS_RING_WIDTH);
            layer.setBorderColor(Some(&NSColor::controlAccentColor().CGColor()));
            layer.setCornerRadius(theme::PILL_HEIGHT / 2.0 + theme::FOCUS_RING_OUTSET);
            // SAFETY: the curve is a constant that Core Animation provides.
            unsafe { layer.setCornerCurve(kCACornerCurveContinuous) };
        }

        Self {
            view,
            ring,
            ring_place: RefCell::new(None),
            banner,
            reach,
            art,
            shade: artwork::layered(main_thread),
            fade: CAGradientLayer::new(),
            floor: artwork::layered(main_thread),
            card,
            title,
            facts,
            play,
            left,
            track,
            fill,
            fill_width: RefCell::new(None),
            pager,
            dots,
            items: RefCell::new(Vec::new()),
            at: Cell::new(0),
            images,
            on_action,
            _pressed: pressed,
        }
    }

    /// Puts the views where they belong, once.
    fn place(&self) {
        let (view, banner, card) = (&self.view, &self.banner, &self.card);
        let (art, shade, floor) = (self.art.view(), &self.shade, &self.floor);
        view.addSubview(banner);
        layout::cover(view, card);
        view.addSubview(&self.ring);
        let (reach_left, reach_up) = &self.reach;
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            reach_left.clone(),
            reach_up.clone(),
            banner.trailingAnchor().constraintEqualToAnchor(&view.trailingAnchor()),
            banner.bottomAnchor().constraintEqualToAnchor(&view.bottomAnchor()),
        ]));

        // The artwork keeps to the right at its own proportions, so that
        // no window cuts the faces out of it, and reaches past the banner
        // by its own hairline, which the banner then clips away.
        banner.addSubview(art);
        banner.addSubview(shade);
        layout::cover(banner, floor);
        let past = theme::ARTWORK_OUTLINE_WIDTH;
        let uncut = art
            .widthAnchor()
            .constraintEqualToAnchor_multiplier(&art.heightAnchor(), 1.0 / theme::LANDSCAPE_ASPECT);
        uncut.setPriority(NSLayoutPriorityDefaultHigh);
        // In a banner far wider than the artwork is, the artwork is cut
        // top and bottom to keep the banner from being mostly empty.
        let least = art.widthAnchor().constraintGreaterThanOrEqualToAnchor_multiplier(
            &banner.widthAnchor(),
            theme::HOME_HERO_ART_LEAST,
        );
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            art.trailingAnchor().constraintEqualToAnchor_constant(&banner.trailingAnchor(), past),
            art.topAnchor().constraintEqualToAnchor_constant(&banner.topAnchor(), -past),
            art.bottomAnchor().constraintEqualToAnchor_constant(&banner.bottomAnchor(), past),
            uncut,
            least,
            shade.leadingAnchor().constraintEqualToAnchor(&art.leadingAnchor()),
            shade.topAnchor().constraintEqualToAnchor(&banner.topAnchor()),
            shade.bottomAnchor().constraintEqualToAnchor(&banner.bottomAnchor()),
            shade
                .widthAnchor()
                .constraintEqualToAnchor_multiplier(&art.widthAnchor(), theme::HOME_HERO_FADE),
        ]));
        self.fade.setStartPoint(CGPoint { x: 0.0, y: 0.5 });
        self.fade.setEndPoint(CGPoint { x: 1.0, y: 0.5 });
        // From clear part of the way down to the page's own colour at the
        // bottom, so that the banner ends in the page and not at an edge.
        // A layer's own coordinates have their origin at the bottom left.
        let sink = CAGradientLayer::new();
        sink.setStartPoint(CGPoint { x: 0.5, y: theme::HOME_HERO_SINK });
        sink.setEndPoint(CGPoint { x: 0.5, y: 0.0 });
        let page = theme::page_background();
        backdrop::set_gradient(&sink, &page.colorWithAlphaComponent(0.0), &page);
        // Neither gradient is its view's own layer, so each has to be told
        // to follow the view's size.
        for (gradient, holder) in [(&*self.fade, shade), (&*sink, floor)] {
            gradient.setAutoresizingMask(
                CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
            );
            if let Some(layer) = holder.layer() {
                gradient.setFrame(layer.bounds());
                layer.addSublayer(gradient);
            }
        }

        let (title, facts, play, left) = (&self.title, &self.facts, &self.play.button, &self.left);
        let (track, fill, pager) = (&self.track, &self.fill, &self.pager);
        let parts: [&NSView; 6] = [title, facts, play, left, track, pager];
        for part in parts {
            card.addSubview(part);
        }
        track.addSubview(fill);
        let (margin, text_width) = (theme::PAGE_MARGIN, theme::HERO_TEXT_WIDTH);
        for words in [title, facts] {
            NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
                words
                    .leadingAnchor()
                    .constraintEqualToAnchor_constant(&card.leadingAnchor(), margin),
                words.widthAnchor().constraintLessThanOrEqualToConstant(text_width),
                // No further over the artwork than where it is half
                // faded, so the words always have the colour under them.
                words.trailingAnchor().constraintLessThanOrEqualToAnchor(&shade.centerXAnchor()),
            ]));
        }
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            // From the bottom up: the button, the facts and the title.
            play.leadingAnchor().constraintEqualToAnchor_constant(&card.leadingAnchor(), margin),
            play.bottomAnchor()
                .constraintEqualToAnchor_constant(&card.bottomAnchor(), -theme::SPACE_XL),
            facts
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&play.topAnchor(), -theme::SPACE_L),
            title
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&facts.topAnchor(), -theme::SPACE_XS),
            // Beside the button, the words sit on the bar.
            left.leadingAnchor()
                .constraintEqualToAnchor_constant(&play.trailingAnchor(), theme::SPACE_L),
            left.bottomAnchor()
                .constraintEqualToAnchor_constant(&play.centerYAnchor(), -theme::SPACE_XXS),
            track.leadingAnchor().constraintEqualToAnchor(&left.leadingAnchor()),
            track
                .topAnchor()
                .constraintEqualToAnchor_constant(&play.centerYAnchor(), theme::SPACE_XS),
            track.widthAnchor().constraintEqualToConstant(theme::HOME_HERO_PROGRESS_WIDTH),
            track.heightAnchor().constraintEqualToConstant(theme::PROGRESS_BAR_HEIGHT),
            fill.leadingAnchor().constraintEqualToAnchor(&track.leadingAnchor()),
            fill.topAnchor().constraintEqualToAnchor(&track.topAnchor()),
            fill.bottomAnchor().constraintEqualToAnchor(&track.bottomAnchor()),
            pager
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&card.trailingAnchor(), -margin),
            pager.centerYAnchor().constraintEqualToAnchor(&play.centerYAnchor()),
        ]));
    }

    /// Draws the ring of the keyboard around the play button, or around
    /// the title of an item that is not something to play.
    fn ring_around(&self, target: &NSView) {
        let (ring, outset) = (&self.ring, theme::FOCUS_RING_OUTSET);
        let around = [
            ring.leadingAnchor().constraintEqualToAnchor_constant(&target.leadingAnchor(), -outset),
            ring.trailingAnchor()
                .constraintEqualToAnchor_constant(&target.trailingAnchor(), outset),
            ring.topAnchor().constraintEqualToAnchor_constant(&target.topAnchor(), -outset),
            ring.bottomAnchor().constraintEqualToAnchor_constant(&target.bottomAnchor(), outset),
        ];
        let around = NSArray::from_retained_slice(&around);
        if let Some(old) = self.ring_place.replace(Some(around.clone())) {
            NSLayoutConstraint::deactivateConstraints(&old);
        }
        NSLayoutConstraint::activateConstraints(&around);
    }

    /// Tells the owner what the viewer asked of the item on screen.
    fn act(&self, make: fn(Item) -> Action) {
        // Cloned out of the cell first, so that the handler is free to
        // show something else.
        let item = self.items.borrow().get(self.at.get()).cloned();
        if let Some(item) = item {
            (self.on_action)(make(item));
        }
    }

    /// Goes `by` items on, around the ends.
    fn turn(&self, by: isize) {
        let count = self.items.borrow().len().cast_signed();
        if count > 0 {
            self.turn_to((self.at.get().cast_signed() + by).rem_euclid(count).cast_unsigned());
        }
    }

    fn turn_to(&self, page: usize) {
        if page != self.at.get() && page < self.items.borrow().len() {
            self.at.set(page);
            self.show_item(true);
        }
    }

    /// Makes the views say what the item on screen is. `turned` says the
    /// viewer went from one item to another, which fades.
    fn show_item(&self, turned: bool) {
        let items = self.items.borrow();
        let item = items.get(self.at.get());
        if let (true, false, Some(layer)) = (turned, theme::reduces_motion(), self.card.layer()) {
            let fade = CATransition::new();
            // SAFETY: the type is a constant that Core Animation provides.
            fade.setType(unsafe { kCATransitionFade });
            fade.setDuration(theme::MOTION_ARRIVE);
            layer.addAnimation_forKey(&fade, Some(ns_string!("page")));
        }
        self.card.set_live(item.is_some());
        self.card.setHidden(item.is_none());
        self.art.view().setHidden(item.is_none());
        self.paint(item.and_then(|item| item.colors).map(|colors| colors.bottom_left));
        let Some(item) = item else {
            return;
        };

        // An episode's own picture is a frame of it, which may give the
        // episode away. Its show's backdrop does not.
        let art = match item.kind {
            Kind::Episode => item.grandparent_art.as_deref().or(item.art.as_deref()),
            _ => item.art.as_deref(),
        };
        self.art.show(art.or(item.thumb.as_deref()), theme::HOME_HERO_REQUEST, &self.images);

        let title = match item.kind {
            Kind::Episode => item.grandparent_title.as_deref().unwrap_or(&item.title),
            _ => &item.title,
        };
        let (facts, left) = (facts(item), text::remaining(item));
        label::set(&self.title, title);
        label::set(&self.facts, &facts);
        label::set(&self.left, left.as_deref().unwrap_or_default());
        self.set_progress(item.progress());
        self.play.set_title(if item.view_offset.is_some() { "Resume" } else { "Play" });
        let plays = poster::is_playable(item);
        self.play.button.setHidden(!plays);
        self.ring_around(if plays { &self.play.button } else { &self.title });
        let spoken: Vec<&str> = [title, &facts]
            .into_iter()
            .chain(left.as_deref())
            .filter(|part| !part.is_empty())
            .collect();
        self.card.setAccessibilityLabel(Some(&NSString::from_str(&spoken.join(", "))));

        self.pager.setHidden(items.len() < 2);
        for (page, dot) in self.dots.iter().enumerate() {
            dot.setHidden(page >= items.len());
            let opacity = if page == self.at.get() { 1.0 } else { theme::HOME_HERO_DOT_OPACITY };
            for mark in dot.subviews() {
                artwork::fill_with(&mark, &theme::white(opacity));
            }
        }
    }

    /// Paints the banner in `colour`, darkened until white text reads on it,
    /// and runs the same colour over the near edge of the artwork. Without
    /// a colour, in the grey of artwork that has not arrived.
    fn paint(&self, colour: Option<Rgb>) {
        let colour = match colour {
            Some(colour) => backdrop::tint(colour),
            None => NSColor::colorWithWhite_alpha(theme::HOME_HERO_PLAIN_WHITE, 1.0),
        };
        let stops = theme::HOME_HERO_FADE_STOPS;
        let colours: Vec<_> = stops
            .iter()
            .map(|(_, opacity)| colour.colorWithAlphaComponent(*opacity).CGColor())
            .collect();
        let colours: Vec<&AnyObject> = colours.iter().map(|colour| (**colour).as_ref()).collect();
        let places: Vec<Retained<NSNumber>> =
            stops.iter().map(|(place, _)| NSNumber::new_f64(*place)).collect();
        // Without this the colours would glide from one item's to the
        // next's by themselves, out of step with the fade of the rest.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        artwork::fill_with(&self.banner, &colour);
        // SAFETY: a gradient layer takes an array of CGColors.
        unsafe { self.fade.setColors(Some(&NSArray::from_slice(&colours))) };
        self.fade.setLocations(Some(&NSArray::from_retained_slice(&places)));
        CATransaction::commit();
    }

    /// Draws how much was watched, from 0 to 1. Zero hides the bar.
    fn set_progress(&self, fraction: f64) {
        self.track.setHidden(fraction <= 0.0);
        if let Some(old) = self.fill_width.take() {
            old.setActive(false);
        }
        let width = self.fill.widthAnchor().constraintEqualToAnchor_multiplier(
            &self.track.widthAnchor(),
            fraction.clamp(0.0, 1.0),
        );
        width.setActive(true);
        self.fill_width.replace(Some(width));
    }
}

/// The line under the title: which episode it is, or for a film its year,
/// rating and length.
fn facts(item: &Item) -> String {
    let mut facts: Vec<String> = Vec::new();
    if item.kind == Kind::Episode {
        facts.extend(text::episode_code(item));
        facts.push(item.title.clone());
    } else {
        facts.extend(item.year.map(|year| year.to_string()));
        facts.extend(item.content_rating.clone());
        facts.extend(item.duration.map(|length| text::length(length.into())));
    }
    facts.join(" · ")
}

/// One of the marks that say how many items there are and which is shown.
/// A click on it shows its item.
fn dot(main_thread: MainThreadMarker, on_click: impl Fn() + 'static) -> Retained<HoverView> {
    let dot = HoverView::pressable(main_thread, |_| {}, on_click);
    let mark = artwork::layered(main_thread);
    if let Some(layer) = mark.layer() {
        layer.setCornerRadius(theme::HOME_HERO_DOT_SIZE / 2.0);
    }
    dot.addSubview(&mark);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        dot.widthAnchor().constraintEqualToConstant(theme::HOME_HERO_DOT_SIZE + theme::SPACE_S),
        dot.heightAnchor().constraintEqualToConstant(theme::HOME_HERO_PAGER_HEIGHT),
        mark.widthAnchor().constraintEqualToConstant(theme::HOME_HERO_DOT_SIZE),
        mark.heightAnchor().constraintEqualToConstant(theme::HOME_HERO_DOT_SIZE),
        mark.centerXAnchor().constraintEqualToAnchor(&dot.centerXAnchor()),
        mark.centerYAnchor().constraintEqualToAnchor(&dot.centerYAnchor()),
    ]));
    dot
}

/// An arrow at one end of the pager, which VoiceOver calls `spoken`.
fn chevron(
    symbol: &str,
    spoken: &str,
    on_click: impl Fn() + 'static,
    main_thread: MainThreadMarker,
) -> Retained<HoverView> {
    let glyph = NSImageView::new(main_thread);
    glyph.setImage(artwork::symbol(symbol, theme::FONT_CAPTION, Weight::Bold).as_deref());
    glyph.setContentTintColor(Some(&theme::white(theme::SECONDARY_ON_ART_OPACITY)));
    glyph.setTranslatesAutoresizingMaskIntoConstraints(false);
    glyph.setAccessibilityElement(false);
    let lit = glyph.clone();
    let on_hover = move |over| {
        let opacity = if over { 1.0 } else { theme::SECONDARY_ON_ART_OPACITY };
        lit.setContentTintColor(Some(&theme::white(opacity)));
    };
    let arrow = HoverView::pressable(main_thread, on_hover, on_click);
    arrow.setAccessibilityElement(true);
    // SAFETY: the role is a constant that AppKit provides.
    arrow.setAccessibilityRole(Some(unsafe { NSAccessibilityButtonRole }));
    arrow.setAccessibilityLabel(Some(&NSString::from_str(spoken)));
    arrow.addSubview(&glyph);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        arrow.widthAnchor().constraintEqualToConstant(theme::HOME_HERO_PAGER_HEIGHT),
        arrow.heightAnchor().constraintEqualToConstant(theme::HOME_HERO_PAGER_HEIGHT),
        glyph.centerXAnchor().constraintEqualToAnchor(&arrow.centerXAnchor()),
        glyph.centerYAnchor().constraintEqualToAnchor(&arrow.centerYAnchor()),
    ]));
    arrow
}

/// A dark capsule holding `stops` in a row, so that they show on any
/// artwork.
fn pager(stops: &[&NSView], main_thread: MainThreadMarker) -> Retained<NSView> {
    let pager = artwork::layered(main_thread);
    artwork::fill_with(&pager, &theme::black(theme::HOME_HERO_PAGER_OPACITY));
    if let Some(layer) = pager.layer() {
        layer.setCornerRadius(theme::HOME_HERO_PAGER_HEIGHT / 2.0);
    }
    let row = NSStackView::stackViewWithViews(&NSArray::from_slice(stops), main_thread);
    row.setSpacing(0.0);
    layout::cover(&pager, &row);
    pager
}

#[cfg(test)]
mod tests {
    use super::super::poster::Shape;
    use super::super::posters::Style;
    use super::*;

    fn shelf(title: &str, keys: &[&str]) -> Shelf {
        let item = |key: &&str| {
            let json = serde_json::json!({ "ratingKey": key, "type": "movie", "title": "Title" });
            serde_json::from_value(json).unwrap()
        };
        Shelf::of(title, Style::Shelf(Shape::Poster), keys.iter().map(item).collect())
    }

    fn keys(items: &[Item]) -> Vec<&str> {
        items.iter().map(|item| item.rating_key.as_str()).collect()
    }

    #[test]
    fn the_featured_items_are_the_first_of_the_first_shelf() {
        let shelves = [shelf("a", &["1", "2", "3", "4"]), shelf("b", &["5"])];
        assert_eq!(keys(&features(&shelves)), ["1", "2", "3"]);
        let shelves = [shelf("a", &["1", "2"]), shelf("b", &["5"])];
        assert_eq!(keys(&features(&shelves)), ["1", "2"]);
        assert!(features(&[]).is_empty());
    }
}
