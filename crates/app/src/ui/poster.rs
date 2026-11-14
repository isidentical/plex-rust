//! One thing in a grid or shelf as a card: artwork with two lines of text
//! under it.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use objc2::rc::{Allocated, Retained};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityButtonRole, NSCollectionViewItem,
    NSCollectionViewItemHighlightState, NSColor, NSLayoutConstraint, NSNibName, NSTextField,
    NSView,
};
use objc2_foundation::{NSArray, NSBundle, NSPoint, NSString};
use plex::model::{Item, Kind, Role};

use super::artwork::{self, ArtView, Badge};
use super::hover::HoverView;
use super::label::{self, Weight};
use super::layout;
use super::text;
use super::theme;
use crate::images::Images;

/// What a card stands for.
// Nearly every card is an item, so boxing the item to even out the sizes
// would cost an allocation per card to save space on a handful of people.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Card {
    Item(Item),
    /// A member of the cast.
    Person(Role),
}

impl Card {
    /// Whether `other` stands for the same thing, whatever else changed
    /// about it, such as how far it was watched.
    pub fn is_same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Item(item), Self::Item(other)) => item.rating_key == other.rating_key,
            (Self::Person(person), Self::Person(other)) => person == other,
            _ => false,
        }
    }

    pub fn item(&self) -> Option<&Item> {
        match self {
            Self::Item(item) => Some(item),
            Self::Person(_) => None,
        }
    }
}

/// The proportions of a card's artwork, and with them what the card says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Upright, for films, shows and seasons.
    Poster,
    /// Wide, for something to carry on with: a frame of an episode under
    /// its show's name, or a film's backdrop.
    Landscape,
    /// Wide, for an episode among the others of its season: a frame under
    /// the episode's own number and title.
    Still,
    /// A round photo of a person.
    Portrait,
}

impl Shape {
    /// The least width of a card in a grid, which widens its cards to
    /// fill each row.
    pub fn least_width(self) -> f64 {
        match self {
            Self::Poster => theme::POSTER_GRID_MIN_WIDTH,
            Self::Landscape | Self::Still | Self::Portrait => self.width(),
        }
    }

    /// The width of a card in a shelf.
    pub fn width(self) -> f64 {
        match self {
            Self::Poster => theme::POSTER_WIDTH,
            Self::Landscape => theme::LANDSCAPE_WIDTH,
            Self::Still => theme::STILL_WIDTH,
            Self::Portrait => theme::PORTRAIT_WIDTH,
        }
    }

    /// The height of a whole card `width` wide: artwork plus captions.
    pub fn height(self, width: f64) -> f64 {
        let aspect = match self {
            Self::Poster => theme::POSTER_ASPECT,
            Self::Landscape | Self::Still => theme::LANDSCAPE_ASPECT,
            Self::Portrait => 1.0,
        };
        (width * aspect).round() + theme::CAPTIONS_HEIGHT
    }

    /// The size artwork is requested at, in points.
    fn request(self) -> (u32, u32) {
        match self {
            Self::Poster => theme::POSTER_REQUEST,
            Self::Landscape | Self::Still => theme::LANDSCAPE_REQUEST,
            Self::Portrait => theme::PORTRAIT_REQUEST,
        }
    }

    /// How round the corners of the artwork are. A portrait is a circle,
    /// and only ever shown at its shelf width.
    fn radius(self) -> f64 {
        match self {
            Self::Portrait => theme::PORTRAIT_WIDTH / 2.0,
            Self::Poster | Self::Landscape | Self::Still => theme::RADIUS_M,
        }
    }
}

pub struct Views {
    art: ArtView,
    title: Retained<NSTextField>,
    subtitle: Retained<NSTextField>,
    /// Stands in for the photo of a person there is none of: a disc with
    /// their initials on it.
    monogram: Retained<NSView>,
    initials: Retained<NSTextField>,
    /// What the card shows, to tell a new look at the same thing from
    /// another thing taking the card over.
    shown: RefCell<Option<Card>>,
}

define_class!(
    // SAFETY: NSCollectionViewItem is designed to be subclassed, and
    // `PosterItem` does not implement `Drop`.
    #[unsafe(super(NSCollectionViewItem))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustPosterItem"]
    #[ivars = OnceCell<Rc<Views>>]
    pub struct PosterItem;

    impl PosterItem {
        /// The collection view creates items itself, through this
        /// initializer.
        #[unsafe(method_id(initWithNibName:bundle:))]
        fn init_with_nib(
            this: Allocated<Self>,
            nib: Option<&NSNibName>,
            bundle: Option<&NSBundle>,
        ) -> Retained<Self> {
            let this = this.set_ivars(OnceCell::new());
            // SAFETY: forwards the designated initializer unchanged.
            unsafe { msg_send![super(this), initWithNibName: nib, bundle: bundle] }
        }

        #[unsafe(method(loadView))]
        fn load_view(&self) {
            let views = Rc::new(Views::new(self.mtm()));
            // The whole card reacts to the pointer, captions included.
            let hovered = Rc::clone(&views);
            let card = HoverView::new(self.mtm(), move |over| hovered.art.set_hovered(over));
            views.place_in(&card);
            // VoiceOver reads a card as one button, not as a picture and
            // two loose pieces of text.
            card.setAccessibilityElement(true);
            // SAFETY: the role is a constant that AppKit provides.
            card.setAccessibilityRole(Some(unsafe { NSAccessibilityButtonRole }));
            self.setView(&card);
            // Cannot fail: AppKit loads a controller's view once.
            let _ = self.ivars().set(views);
        }

        /// The collection view selects the card the keyboard is on.
        #[unsafe(method(setSelected:))]
        fn set_selected(&self, selected: bool) {
            // SAFETY: forwards the setter unchanged.
            let () = unsafe { msg_send![super(self), setSelected: selected] };
            if let Some(views) = self.ivars().get() {
                views.art.set_focused(selected);
            }
        }

        /// The collection view highlights the card under a pressed button.
        #[unsafe(method(setHighlightState:))]
        fn set_highlight_state(&self, state: NSCollectionViewItemHighlightState) {
            // SAFETY: forwards the setter unchanged.
            let () = unsafe { msg_send![super(self), setHighlightState: state] };
            if let Some(views) = self.ivars().get() {
                views.art.set_pressed(state == NSCollectionViewItemHighlightState::ForSelection);
            }
        }
    }
);

impl PosterItem {
    pub const IDENTIFIER: &str = "poster";

    /// Fills the card with `card`, drawn in `shape`. `is_current` marks it
    /// as the one the page is about, among others like it.
    pub fn show(&self, card: &Card, shape: Shape, is_current: bool, images: &Rc<Images>) {
        // Asking for the view makes AppKit call `loadView` if it has not.
        let view = self.view();
        let Some(views) = self.ivars().get() else {
            return;
        };
        // A card taken over by something else starts at rest. One that is
        // only brought up to date keeps the pointer's highlight.
        let was = views.shown.replace(Some(card.clone()));
        if !was.is_some_and(|was| was.is_same(card)) {
            views.art.reset();
        }
        views.art.set_radius(shape.radius());
        views.art.set_current(is_current);
        let faceless = matches!(card, Card::Person(person) if person.thumb.is_none());
        views.monogram.setHidden(!faceless);
        let (title, subtitle) = match card {
            Card::Item(item) => {
                views.art.set_playable(is_playable(item));
                views.art.set_progress(item.progress());
                views.art.set_badge(badge(item));
                views.art.show(artwork_path(item, shape), shape.request(), None, images);
                captions(item, shape)
            }
            Card::Person(person) => {
                views.art.set_playable(false);
                views.art.set_progress(0.0);
                views.art.set_badge(Badge::None);
                views.art.show(person.thumb.as_deref(), shape.request(), None, images);
                label::set(&views.initials, &initials(&person.tag));
                (person.tag.clone(), person.role.clone().unwrap_or_default())
            }
        };
        label::set(&views.title, &title);
        label::set(&views.subtitle, &subtitle);
        let spoken = spoken(card, &title, &subtitle, is_current);
        view.setAccessibilityLabel(Some(&NSString::from_str(&spoken)));
    }

    /// Whether the card was told what a press through VoiceOver does.
    pub fn has_on_press(&self) -> bool {
        self.view().downcast_ref::<HoverView>().is_some_and(HoverView::has_on_press)
    }

    /// Says what pressing the card through VoiceOver does. A card is
    /// reused for many items, so `on_press` has to find out which one it
    /// shows when it is called.
    pub fn set_on_press(&self, on_press: impl Fn() + 'static) {
        if let Some(card) = self.view().downcast_ref::<HoverView>() {
            card.set_on_press(on_press);
        }
    }

    /// Whether `point`, in the window's coordinates, is on the play glyph.
    pub fn is_on_play(&self, point: NSPoint) -> bool {
        self.ivars().get().is_some_and(|views| views.art.is_on_play(point))
    }
}

/// Whether `item` is a file to play, not a show or season to look inside.
pub fn is_playable(item: &Item) -> bool {
    matches!(item.kind, Kind::Movie | Kind::Episode | Kind::Clip)
}

/// The mark in the corner of `item`. What was never started has none: a
/// library starts out that way, and a mark on everything says nothing.
/// What was watched to the end has a tick, and a show or season with
/// episodes left says how many.
pub fn badge(item: &Item) -> Badge {
    if item.view_offset.is_some() || item.kind == Kind::Clip {
        // The progress bar says it, and a trailer is not there to be
        // ticked off.
        return Badge::None;
    }
    match (item.leaf_count, item.viewed_leaf_count) {
        (Some(episodes), Some(watched)) if watched < episodes => Badge::Left(episodes - watched),
        _ if item.is_watched() => Badge::Watched,
        _ => Badge::None,
    }
}

/// What VoiceOver says for a card: its captions and its watched state.
pub fn spoken(card: &Card, title: &str, subtitle: &str, is_current: bool) -> String {
    let mut parts: Vec<String> = vec![title.to_owned()];
    if !subtitle.is_empty() {
        parts.push(subtitle.to_owned());
    }
    if is_current {
        parts.push("this episode".to_owned());
    }
    if let Card::Item(item) = card {
        match badge(item) {
            Badge::Watched => parts.push("watched".to_owned()),
            Badge::Left(episodes) => {
                parts.push(format!("{} not watched", text::count(episodes, "episode")));
            }
            Badge::None => {}
        }
        if item.view_offset.is_some() {
            parts.push(format!("{:.0} percent watched", item.progress() * 100.0));
        }
    }
    parts.join(", ")
}

/// The first letters of the first two words of `name`, as a person
/// without a photo is drawn.
fn initials(name: &str) -> String {
    let letters = name.split_whitespace().take(2).filter_map(|word| word.chars().next());
    letters.flat_map(char::to_uppercase).collect()
}

/// On a poster, an episode stands for its show, since its own artwork is a
/// wide frame. A wide card shows that frame, or a film's backdrop.
fn artwork_path(item: &Item, shape: Shape) -> Option<&str> {
    match (item.kind, shape) {
        (Kind::Episode, Shape::Poster) => {
            item.grandparent_thumb.as_deref().or(item.thumb.as_deref())
        }
        (Kind::Episode | Kind::Clip, _) => item.thumb.as_deref(),
        (_, Shape::Landscape | Shape::Still) => item.art.as_deref().or(item.thumb.as_deref()),
        (_, Shape::Poster | Shape::Portrait) => item.thumb.as_deref(),
    }
}

/// The two lines under the artwork.
fn captions(item: &Item, shape: Shape) -> (String, String) {
    let year = || item.year.map(|year| year.to_string()).unwrap_or_default();
    let length = || item.duration.map(|length| text::length(length.into()));
    let show = || item.grandparent_title.clone().unwrap_or_else(|| item.title.clone());
    match (item.kind, shape) {
        (Kind::Episode, Shape::Still) => {
            let title = match item.index {
                Some(number) => format!("{number}. {}", item.title),
                None => item.title.clone(),
            };
            (title, text::remaining(item).or_else(length).unwrap_or_default())
        }
        (Kind::Episode, Shape::Landscape) => {
            let mut second: Vec<String> = Vec::new();
            second.extend(text::episode_code(item));
            second.push(text::remaining(item).unwrap_or_else(|| item.title.clone()));
            (show(), second.join(" · "))
        }
        (Kind::Episode, _) => (show(), text::episode_code(item).unwrap_or_default()),
        (Kind::Clip, _) => {
            let mut second = vec![text::extra(item.subtype.as_deref()).to_owned()];
            second.extend(length());
            (item.title.clone(), second.join(" · "))
        }
        (Kind::Season, _) => match &item.parent_title {
            Some(show) => (show.clone(), item.title.clone()),
            None => (item.title.clone(), year()),
        },
        (_, Shape::Landscape) => (item.title.clone(), text::remaining(item).unwrap_or_else(year)),
        _ => (item.title.clone(), year()),
    }
}

impl Views {
    fn new(main_thread: MainThreadMarker) -> Self {
        let secondary = NSColor::secondaryLabelColor();
        let monogram = artwork::layered(main_thread);
        artwork::fill_with(&monogram, &theme::white(theme::MONOGRAM_DISC_OPACITY));
        if let Some(layer) = monogram.layer() {
            layer.setCornerRadius(theme::PORTRAIT_WIDTH / 2.0);
        }
        monogram.setHidden(true);
        let initials = label::line(
            theme::FONT_LARGE_TITLE,
            Weight::Medium,
            &theme::white(theme::MONOGRAM_INK_OPACITY),
            main_thread,
        );
        // The name is right under it.
        initials.setAccessibilityElement(false);
        layout::float(&monogram, &initials);
        Self {
            monogram,
            initials,
            art: ArtView::new(main_thread, theme::RADIUS_M),
            title: label::line(
                theme::FONT_BODY,
                Weight::Medium,
                &NSColor::labelColor(),
                main_thread,
            ),
            subtitle: label::line(theme::FONT_CAPTION, Weight::Regular, &secondary, main_thread),
            shown: RefCell::new(None),
        }
    }

    /// Artwork on top, filling the card down to the captions.
    fn place_in(&self, card: &HoverView) {
        let (art, title, subtitle) = (self.art.view(), &self.title, &self.subtitle);
        card.addSubview(art);
        // On the artwork, to grow with it under the pointer.
        layout::cover(art, &self.monogram);
        card.addSubview(title);
        card.addSubview(subtitle);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            art.topAnchor().constraintEqualToAnchor(&card.topAnchor()),
            art.leadingAnchor().constraintEqualToAnchor(&card.leadingAnchor()),
            art.trailingAnchor().constraintEqualToAnchor(&card.trailingAnchor()),
            art.bottomAnchor()
                .constraintEqualToAnchor_constant(&card.bottomAnchor(), -theme::CAPTIONS_HEIGHT),
            title.topAnchor().constraintEqualToAnchor_constant(&art.bottomAnchor(), theme::SPACE_S),
            title.leadingAnchor().constraintEqualToAnchor(&card.leadingAnchor()),
            title.trailingAnchor().constraintEqualToAnchor(&card.trailingAnchor()),
            subtitle
                .topAnchor()
                .constraintEqualToAnchor_constant(&title.bottomAnchor(), theme::SPACE_XXS),
            subtitle.leadingAnchor().constraintEqualToAnchor(&card.leadingAnchor()),
            subtitle.trailingAnchor().constraintEqualToAnchor(&card.trailingAnchor()),
        ]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(json: serde_json::Value) -> Item {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn an_episode_is_captioned_by_where_it_is_shown() {
        let episode = item(serde_json::json!({
            "ratingKey": "1", "type": "episode", "title": "Pilot", "index": 6, "parentIndex": 2,
            "grandparentTitle": "Show", "duration": 1_260_000, "viewOffset": 600_000,
        }));
        let (show, code) = (String::from("Show"), String::from("S2 · E6"));
        assert_eq!(captions(&episode, Shape::Poster), (show.clone(), code));
        assert_eq!(captions(&episode, Shape::Landscape), (show, "S2 · E6 · 11 min left".into()));
        assert_eq!(captions(&episode, Shape::Still), ("6. Pilot".into(), "11 min left".into()));
    }

    #[test]
    fn a_person_without_a_photo_is_drawn_as_two_initials() {
        assert_eq!(initials("Reggie Dees II"), "RD");
        assert_eq!(initials("e.j. holowicki"), "EH");
        assert_eq!(initials("Zendaya"), "Z");
    }

    #[test]
    fn a_card_is_spoken_with_what_is_left_to_watch() {
        let show = item(serde_json::json!({
            "ratingKey": "1", "type": "show", "title": "Show", "year": 2014,
            "leafCount": 10, "viewedLeafCount": 7,
        }));
        let card = Card::Item(show);
        assert_eq!(spoken(&card, "Show", "2014", false), "Show, 2014, 3 episodes not watched");
        let film = item(serde_json::json!({
            "ratingKey": "1", "type": "movie", "title": "Film",
            "duration": 100, "viewOffset": 25, "viewCount": 1,
        }));
        assert_eq!(spoken(&Card::Item(film), "Film", "", false), "Film, 25 percent watched");
        let seen = item(serde_json::json!({
            "ratingKey": "1", "type": "movie", "title": "Film", "viewCount": 2,
        }));
        assert_eq!(spoken(&Card::Item(seen), "Film", "", false), "Film, watched");
        let new = item(serde_json::json!({ "ratingKey": "1", "type": "movie", "title": "Film" }));
        assert_eq!(spoken(&Card::Item(new), "Film", "", false), "Film");
    }
}
