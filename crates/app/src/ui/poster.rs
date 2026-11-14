//! One library item as a card in a grid or shelf: its artwork with two
//! lines of text under it.

use std::cell::OnceCell;
use std::rc::Rc;

use objc2::rc::{Allocated, Retained};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSCollectionViewItem, NSCollectionViewItemHighlightState, NSColor, NSLayoutConstraint,
    NSNibName, NSTextField,
};
use objc2_foundation::{NSArray, NSBundle, NSPoint};
use plex::model::{Item, Kind};

use super::artwork::{ArtView, Badge};
use super::hover::HoverView;
use super::label::{self, Weight};
use super::text;
use super::theme;
use crate::images::Images;

/// The proportions of a card's artwork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Upright, for films, shows and seasons.
    Poster,
    /// Wide, for a frame of an episode or a film's backdrop.
    Landscape,
}

impl Shape {
    /// The width of a card in a shelf, and the least it gets in a grid.
    pub fn width(self) -> f64 {
        match self {
            Self::Poster => theme::POSTER_WIDTH,
            Self::Landscape => theme::LANDSCAPE_WIDTH,
        }
    }

    /// The height of a whole card `width` wide: artwork plus captions.
    pub fn height(self, width: f64) -> f64 {
        let aspect = match self {
            Self::Poster => theme::POSTER_ASPECT,
            Self::Landscape => theme::LANDSCAPE_ASPECT,
        };
        (width * aspect).round() + theme::CAPTIONS_HEIGHT
    }

    /// The size artwork is requested at, in points.
    fn request(self) -> (u32, u32) {
        match self {
            Self::Poster => theme::POSTER_REQUEST,
            Self::Landscape => theme::LANDSCAPE_REQUEST,
        }
    }
}

pub struct Views {
    art: ArtView,
    title: Retained<NSTextField>,
    subtitle: Retained<NSTextField>,
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

    /// Fills the card with `item`, drawn in `shape`.
    pub fn show(&self, item: &Item, shape: Shape, images: &Rc<Images>) {
        // Asking for the view makes AppKit call `loadView` if it has not.
        let _ = self.view();
        let Some(views) = self.ivars().get() else {
            return;
        };
        let (title, subtitle) = captions(item, shape);
        label::set(&views.title, &title);
        label::set(&views.subtitle, &subtitle);
        views.art.reset();
        views.art.set_playable(is_playable(item));
        views.art.set_progress(item.progress());
        views.art.set_badge(badge(item));
        views.art.show(artwork_path(item, shape), shape.request(), images);
    }

    /// Whether `point`, in the window's coordinates, is on the play glyph.
    pub fn is_on_play(&self, point: NSPoint) -> bool {
        self.ivars().get().is_some_and(|views| views.art.is_on_play(point))
    }
}

/// Whether `item` is a file to play, not a show or season to look inside.
pub fn is_playable(item: &Item) -> bool {
    matches!(item.kind, Kind::Movie | Kind::Episode)
}

/// What marks `item` as having something left to watch.
pub fn badge(item: &Item) -> Badge {
    if item.view_offset.is_some() {
        // The progress bar says it.
        return Badge::None;
    }
    match (item.leaf_count, item.viewed_leaf_count) {
        (Some(episodes), Some(watched)) if watched < episodes => Badge::Left(episodes - watched),
        (Some(_), Some(_)) => Badge::None,
        _ if item.view_count == 0 => Badge::Unwatched,
        _ => Badge::None,
    }
}

/// On a poster, an episode stands for its show, since its own artwork is a
/// wide frame. A wide card shows that frame, or a film's backdrop.
fn artwork_path(item: &Item, shape: Shape) -> Option<&str> {
    match (item.kind, shape) {
        (Kind::Episode, Shape::Poster) => {
            item.grandparent_thumb.as_deref().or(item.thumb.as_deref())
        }
        (Kind::Episode, Shape::Landscape) => item.thumb.as_deref(),
        (_, Shape::Landscape) => item.art.as_deref().or(item.thumb.as_deref()),
        (_, Shape::Poster) => item.thumb.as_deref(),
    }
}

/// The two lines under the artwork.
fn captions(item: &Item, shape: Shape) -> (String, String) {
    let year = || item.year.map(|year| year.to_string()).unwrap_or_default();
    match (item.kind, shape) {
        (Kind::Episode, Shape::Poster) => (
            item.grandparent_title.clone().unwrap_or_else(|| item.title.clone()),
            text::episode_code(item).unwrap_or_default(),
        ),
        (Kind::Episode, Shape::Landscape) => {
            let mut second: Vec<String> = Vec::new();
            second.extend(text::episode_code(item));
            second.push(text::remaining(item).unwrap_or_else(|| item.title.clone()));
            (
                item.grandparent_title.clone().unwrap_or_else(|| item.title.clone()),
                second.join(" · "),
            )
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
        Self {
            art: ArtView::new(main_thread, theme::RADIUS_M),
            title: label::line(
                theme::FONT_BODY,
                Weight::Medium,
                &NSColor::labelColor(),
                main_thread,
            ),
            subtitle: label::line(
                theme::FONT_CAPTION,
                Weight::Regular,
                &NSColor::secondaryLabelColor(),
                main_thread,
            ),
        }
    }

    /// Artwork on top, filling the card down to the captions.
    fn place_in(&self, card: &HoverView) {
        let (art, title, subtitle) = (self.art.view(), &self.title, &self.subtitle);
        card.addSubview(art);
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
