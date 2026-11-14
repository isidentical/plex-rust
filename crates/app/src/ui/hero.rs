//! The top of the home screen: the first thing to watch, large, on its
//! backdrop, with a button that plays it.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityButtonRole, NSButton, NSCellImagePosition, NSColor,
    NSLayoutConstraint, NSTextField, NSView,
};
use objc2_core_foundation::CGPoint;
use objc2_foundation::{NSArray, NSString, ns_string};
use objc2_quartz_core::{CAAutoresizingMask, CAGradientLayer, kCACornerCurveContinuous};
use plex::model::{Item, Kind};

use super::artwork::{self, ArtView};
use super::backdrop;
use super::callback::Callback;
use super::hover::HoverView;
use super::label::{self, Weight};
use super::layout;
use super::poster::{self, Card};
use super::posters::{Action, Shelf};
use super::text;
use super::theme;
use crate::images::Images;

/// What the hero shows: an item, and the title of the shelf it leads.
#[derive(Debug, Clone, PartialEq)]
pub struct Feature {
    pub shelf: String,
    pub item: Item,
}

/// Takes the first item of the first shelf out of `shelves` to show it
/// large above them, so that it is not on the screen twice. A shelf left
/// with nothing goes too.
pub fn feature(shelves: &mut Vec<Shelf>) -> Option<Feature> {
    let first = shelves.first_mut()?;
    let Some(Card::Item(item)) = first.cards.first().cloned() else {
        return None;
    };
    first.cards.remove(0);
    let shelf = first.title.clone();
    if first.cards.is_empty() {
        shelves.remove(0);
    }
    Some(Feature { shelf, item })
}

pub struct Hero {
    view: Retained<NSView>,
    card: Retained<HoverView>,
    art: Rc<ArtView>,
    /// Everything drawn over the artwork, hidden until there is an item.
    overlay: Retained<NSView>,
    shelf: Retained<NSTextField>,
    title: Retained<NSTextField>,
    facts: Retained<NSTextField>,
    play: Retained<NSButton>,
    /// How much was watched, beside the button: a track and its filled
    /// part.
    track: Retained<NSView>,
    fill: Retained<NSView>,
    fill_width: RefCell<Option<Retained<NSLayoutConstraint>>>,
    shown: Rc<RefCell<Option<Item>>>,
    images: Rc<Images>,
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
        let shown: Rc<RefCell<Option<Item>>> = Rc::new(RefCell::new(None));
        let on_action = Rc::new(on_action);
        // Cloned out of the cell first, so that the handler is free to
        // show something else.
        let act = |make: fn(Item) -> Action| {
            let (shown, on_action) = (Rc::clone(&shown), Rc::clone(&on_action));
            move || {
                let item = shown.borrow().clone();
                if let Some(item) = item {
                    on_action(make(item));
                }
            }
        };

        let art = Rc::new(ArtView::new(main_thread, theme::RADIUS_L));
        art.set_flat();
        let lit = Rc::clone(&art);
        let card =
            HoverView::pressable(main_thread, move |over| lit.set_hovered(over), act(Action::Open));
        card.setTranslatesAutoresizingMaskIntoConstraints(false);
        card.set_live(false);
        // VoiceOver reads the hero as one button that opens the item, with
        // the play button inside it as another.
        card.setAccessibilityElement(true);
        // SAFETY: the role is a constant that AppKit provides.
        card.setAccessibilityRole(Some(unsafe { NSAccessibilityButtonRole }));
        layout::cover(&card, art.view());

        let overlay = scrim(main_thread);
        overlay.setHidden(true);
        layout::cover(&card, &overlay);

        let white = NSColor::whiteColor();
        let faint = theme::white(theme::HOME_HERO_FACTS_OPACITY);
        let shelf = label::line(theme::FONT_BODY, Weight::Semibold, &faint, main_thread);
        let title = label::line(theme::FONT_LARGE_TITLE, Weight::Bold, &white, main_thread);
        let facts = label::line(theme::FONT_HEADLINE, Weight::Medium, &faint, main_thread);

        let press = act(Action::Play);
        let pressed = Callback::new(main_thread, move |_| press());
        let play = play_button(&pressed, main_thread);

        let track = artwork::layered(main_thread);
        artwork::fill_with(&track, &theme::white(theme::HOME_HERO_TRACK_OPACITY));
        let fill = artwork::layered(main_thread);
        artwork::fill_with(&fill, &NSColor::controlAccentColor());
        for bar in [&track, &fill] {
            if let Some(layer) = bar.layer() {
                layer.setCornerRadius(theme::PROGRESS_BAR_HEIGHT / 2.0);
            }
        }
        overlay.addSubview(&track);
        track.addSubview(&fill);

        for view in [&**shelf, &**title, &**facts, &**play] {
            let view: &NSView = view;
            overlay.addSubview(view);
            view.leadingAnchor()
                .constraintEqualToAnchor_constant(&overlay.leadingAnchor(), theme::SPACE_XXL)
                .setActive(true);
        }
        // Only now, with the button in the overlay too: a constraint needs
        // both its views in one hierarchy.
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            track
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&play.trailingAnchor(), theme::SPACE_L),
            track.centerYAnchor().constraintEqualToAnchor(&play.centerYAnchor()),
            track.widthAnchor().constraintEqualToConstant(theme::HOME_HERO_PROGRESS_WIDTH),
            track.heightAnchor().constraintEqualToConstant(theme::PROGRESS_BAR_HEIGHT),
            fill.leadingAnchor().constraintEqualToAnchor(&track.leadingAnchor()),
            fill.topAnchor().constraintEqualToAnchor(&track.topAnchor()),
            fill.bottomAnchor().constraintEqualToAnchor(&track.bottomAnchor()),
        ]));
        let text_width = theme::HERO_TEXT_WIDTH;
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            // From the bottom up: the button, the facts, the title and
            // the shelf's name.
            play.bottomAnchor()
                .constraintEqualToAnchor_constant(&overlay.bottomAnchor(), -theme::SPACE_XXL),
            facts
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&play.topAnchor(), -theme::SPACE_L),
            title
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&facts.topAnchor(), -theme::SPACE_XS),
            shelf
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&title.topAnchor(), -theme::SPACE_XXS),
            title.widthAnchor().constraintLessThanOrEqualToConstant(text_width),
            title.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(
                &overlay.trailingAnchor(),
                -theme::SPACE_XXL,
            ),
            facts.widthAnchor().constraintLessThanOrEqualToConstant(text_width),
            facts.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(
                &overlay.trailingAnchor(),
                -theme::SPACE_XXL,
            ),
        ]));

        let view = NSView::new(main_thread);
        view.addSubview(&card);
        let margin = theme::PAGE_MARGIN;
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            card.leadingAnchor().constraintEqualToAnchor_constant(&view.leadingAnchor(), margin),
            card.trailingAnchor().constraintEqualToAnchor_constant(&view.trailingAnchor(), -margin),
            card.topAnchor().constraintEqualToAnchor_constant(&view.topAnchor(), theme::SPACE_L),
            card.bottomAnchor()
                .constraintEqualToAnchor_constant(&view.bottomAnchor(), -theme::SPACE_L),
        ]));

        Self {
            view,
            card,
            art,
            overlay,
            shelf,
            title,
            facts,
            play,
            track,
            fill,
            fill_width: RefCell::new(None),
            shown,
            images,
            _pressed: pressed,
        }
    }

    pub fn view(&self) -> &NSView {
        &self.view
    }

    /// Shows `feature`, or with none leaves the space empty.
    pub fn show(&self, feature: Option<Feature>) {
        self.card.setHidden(feature.is_none());
        self.card.set_live(feature.is_some());
        self.overlay.setHidden(feature.is_none());
        let Some(Feature { shelf, item }) = feature else {
            self.shown.replace(None);
            return;
        };

        // An episode's own picture is a frame of it, which may give the
        // episode away. Its show's backdrop does not.
        let art = match item.kind {
            Kind::Episode => item.grandparent_art.as_deref().or(item.art.as_deref()),
            _ => item.art.as_deref(),
        };
        self.art.show(art.or(item.thumb.as_deref()), theme::HOME_HERO_REQUEST, None, &self.images);
        self.set_progress(item.progress());

        let title = match item.kind {
            Kind::Episode => item.grandparent_title.as_deref().unwrap_or(&item.title),
            _ => &item.title,
        };
        let facts = facts(&item);
        label::set(&self.shelf, &shelf);
        label::set(&self.title, title);
        label::set(&self.facts, &facts);
        let verb = if item.view_offset.is_some() { "Resume" } else { "Play" };
        self.play.setTitle(&NSString::from_str(verb));
        self.play.setHidden(!poster::is_playable(&item));
        let spoken = format!("{shelf}, {title}, {facts}");
        self.card.setAccessibilityLabel(Some(&NSString::from_str(&spoken)));
        self.shown.replace(Some(item));
    }
}

impl Hero {
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

/// The line under the title: which episode it is and how much is left, or
/// for something not started its year and length.
fn facts(item: &Item) -> String {
    let mut facts: Vec<String> = Vec::new();
    if item.kind == Kind::Episode {
        facts.extend(text::episode_code(item));
        facts.push(item.title.clone());
    } else {
        facts.extend(item.year.map(|year| year.to_string()));
    }
    let length = || item.duration.map(|length| text::length(length.into()));
    facts.extend(text::remaining(item).or_else(length));
    facts.join(" · ")
}

/// A view that darkens the left of the artwork under it, where the text
/// is, and holds that text.
fn scrim(main_thread: MainThreadMarker) -> Retained<NSView> {
    let view = artwork::layered(main_thread);
    let gradient = CAGradientLayer::new();
    gradient.setStartPoint(CGPoint { x: 0.0, y: 0.5 });
    gradient.setEndPoint(CGPoint { x: theme::HOME_HERO_SCRIM_REACH, y: 0.5 });
    backdrop::set_gradient(
        &gradient,
        &theme::black(theme::HOME_HERO_SCRIM_OPACITY),
        &NSColor::clearColor(),
    );
    // The gradient is not the view's own layer, so it has to be told to
    // follow the view's size.
    gradient.setAutoresizingMask(
        CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
    );
    if let Some(layer) = view.layer() {
        gradient.setFrame(layer.bounds());
        layer.setCornerRadius(theme::RADIUS_L);
        // SAFETY: the curve is a constant that Core Animation provides.
        unsafe { layer.setCornerCurve(kCACornerCurveContinuous) };
        layer.setMasksToBounds(true);
        layer.addSublayer(&gradient);
    }
    view
}

/// A white capsule with a play glyph, drawn by us so that it looks the
/// same in a window that is not in front.
fn play_button(pressed: &Callback, main_thread: MainThreadMarker) -> Retained<NSButton> {
    // SAFETY: the hero keeps the target for as long as it keeps the button.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            ns_string!("Play"),
            Some(pressed),
            Some(Callback::action()),
            main_thread,
        )
    };
    button.setBordered(false);
    button.setFont(Some(&label::font(theme::FONT_BODY, Weight::Semibold)));
    button.setImage(artwork::symbol("play.fill", theme::FONT_BODY).as_deref());
    button.setImagePosition(NSCellImagePosition::ImageLeading);
    button.setImageHugsTitle(true);
    button.setContentTintColor(Some(&NSColor::blackColor()));
    button.setWantsLayer(true);
    if let Some(layer) = button.layer() {
        layer.setBackgroundColor(Some(&NSColor::whiteColor().CGColor()));
        layer.setCornerRadius(theme::PILL_HEIGHT / 2.0);
    }
    button.setTranslatesAutoresizingMaskIntoConstraints(false);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        button.heightAnchor().constraintEqualToConstant(theme::PILL_HEIGHT),
        button.widthAnchor().constraintEqualToConstant(theme::HOME_HERO_BUTTON_WIDTH),
    ]));
    button
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

    #[test]
    fn the_featured_item_leaves_its_shelf() {
        let mut shelves = vec![shelf("a", &["1", "2"]), shelf("b", &["3"])];
        let feature = feature(&mut shelves).unwrap();
        assert_eq!((feature.shelf.as_str(), feature.item.rating_key.as_str()), ("a", "1"));
        assert_eq!(shelves, [shelf("a", &["2"]), shelf("b", &["3"])]);

        let mut shelves = vec![shelf("a", &["1"]), shelf("b", &["3"])];
        assert!(super::feature(&mut shelves).is_some());
        assert_eq!(shelves, [shelf("b", &["3"])]);
        assert_eq!(super::feature(&mut Vec::new()), None);
    }
}
