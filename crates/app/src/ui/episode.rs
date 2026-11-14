//! One episode as a row in a season's list: a frame on the left, its
//! number, title, length and summary beside it.

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
use plex::model::{Item, RatingKey};

use super::artwork::{self, ArtView};
use super::hover::HoverView;
use super::label::{self, Weight};
use super::layout;
use super::poster::{self, Card};
use super::text;
use super::theme;
use crate::images::Images;

pub struct Views {
    art: ArtView,
    title: Retained<NSTextField>,
    facts: Retained<NSTextField>,
    summary: Retained<NSTextField>,
    /// Lights up the whole row under the pointer.
    wash: Retained<NSView>,
    /// The episode the row shows.
    shown: RefCell<Option<RatingKey>>,
}

define_class!(
    // SAFETY: NSCollectionViewItem is designed to be subclassed, and
    // `EpisodeItem` does not implement `Drop`.
    #[unsafe(super(NSCollectionViewItem))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustEpisodeItem"]
    #[ivars = OnceCell<Rc<Views>>]
    pub struct EpisodeItem;

    impl EpisodeItem {
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
            let hovered = Rc::clone(&views);
            let row = HoverView::new(self.mtm(), move |over| hovered.set_hovered(over));
            views.place_in(&row);
            // VoiceOver reads a row as one button, not as loose pieces.
            row.setAccessibilityElement(true);
            // SAFETY: the role is a constant that AppKit provides.
            row.setAccessibilityRole(Some(unsafe { NSAccessibilityButtonRole }));
            self.setView(&row);
            // Cannot fail: AppKit loads a controller's view once.
            let _ = self.ivars().set(views);
        }

        /// The collection view selects the row the keyboard is on.
        #[unsafe(method(setSelected:))]
        fn set_selected(&self, selected: bool) {
            // SAFETY: forwards the setter unchanged.
            let () = unsafe { msg_send![super(self), setSelected: selected] };
            if let Some(views) = self.ivars().get() {
                views.art.set_focused(selected);
            }
        }

        /// The collection view highlights the row under a pressed button.
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

impl EpisodeItem {
    pub const IDENTIFIER: &str = "episode";

    pub fn show(&self, episode: &Item, images: &Rc<Images>) {
        // Asking for the view makes AppKit call `loadView` if it has not.
        let _ = self.view();
        let Some(views) = self.ivars().get() else {
            return;
        };
        let title = match episode.index {
            Some(number) => format!("{number}. {}", episode.title),
            None => episode.title.clone(),
        };
        label::set(&views.title, &title);
        label::set(&views.facts, &facts(episode));
        label::set(&views.summary, episode.summary.as_deref().unwrap_or_default());
        // A row taken over by another episode starts at rest. One that is
        // only brought up to date keeps the pointer's highlight.
        let was = views.shown.replace(Some(episode.rating_key.clone()));
        if was.as_ref() != Some(&episode.rating_key) {
            views.set_hovered(false);
            views.art.reset();
        }
        views.art.set_playable(true);
        views.art.set_progress(episode.progress());
        views.art.set_badge(poster::badge(episode));
        views.art.show(episode.thumb.as_deref(), theme::LANDSCAPE_REQUEST, None, images);
        let spoken = poster::spoken(&Card::Item(episode.clone()), &title, &facts(episode), false);
        self.view().setAccessibilityLabel(Some(&NSString::from_str(&spoken)));
    }

    /// Whether `point`, in the window's coordinates, is on the play glyph.
    pub fn is_on_play(&self, point: NSPoint) -> bool {
        self.ivars().get().is_some_and(|views| views.art.is_on_play(point))
    }
}

/// The line under the title: "Apr 24, 2011 · 42 min", and after those
/// "23 min left" when it was started.
fn facts(episode: &Item) -> String {
    let mut facts: Vec<String> = Vec::new();
    facts.extend(episode.originally_available_at.map(text::date));
    facts.extend(episode.duration.map(|duration| text::length(duration.into())));
    facts.extend(text::remaining(episode));
    facts.join(" · ")
}

impl Views {
    fn new(main_thread: MainThreadMarker) -> Self {
        let secondary = NSColor::secondaryLabelColor();
        let wash = artwork::layered(main_thread);
        artwork::fill_with(&wash, &theme::white(theme::ROW_HOVER_OPACITY));
        if let Some(layer) = wash.layer() {
            layer.setCornerRadius(theme::RADIUS_L);
        }
        wash.setAlphaValue(0.0);
        Self {
            art: ArtView::new(main_thread, theme::RADIUS_M),
            title: label::line(
                theme::FONT_HEADLINE,
                Weight::Semibold,
                &NSColor::labelColor(),
                main_thread,
            ),
            facts: label::line(theme::FONT_CAPTION, Weight::Regular, &secondary, main_thread),
            summary: label::paragraph(
                theme::FONT_BODY,
                Weight::Regular,
                &secondary,
                theme::EPISODE_SUMMARY_LINES,
                main_thread,
            ),
            wash,
            shown: RefCell::new(None),
        }
    }

    fn set_hovered(&self, hovered: bool) {
        self.wash.setAlphaValue(if hovered { 1.0 } else { 0.0 });
        self.art.set_hovered(hovered);
    }

    /// The frame at the left edge, the text in a column beside it.
    fn place_in(&self, row: &HoverView) {
        layout::cover(row, &self.wash);
        let art = self.art.view();
        row.addSubview(art);
        for line in [&self.title, &self.facts, &self.summary] {
            row.addSubview(line);
        }
        let inset = theme::SPACE_S;
        let still_height = (theme::EPISODE_STILL_WIDTH * theme::LANDSCAPE_ASPECT).round();
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            art.leadingAnchor().constraintEqualToAnchor_constant(&row.leadingAnchor(), inset),
            art.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()),
            art.widthAnchor().constraintEqualToConstant(theme::EPISODE_STILL_WIDTH),
            art.heightAnchor().constraintEqualToConstant(still_height),
            self.title
                .topAnchor()
                .constraintEqualToAnchor_constant(&art.topAnchor(), theme::SPACE_XS),
            self.facts
                .topAnchor()
                .constraintEqualToAnchor_constant(&self.title.bottomAnchor(), theme::SPACE_XS),
            self.summary
                .topAnchor()
                .constraintEqualToAnchor_constant(&self.facts.bottomAnchor(), theme::SPACE_S),
        ]));
        for line in [&self.title, &self.facts, &self.summary] {
            let narrow = line.widthAnchor().constraintEqualToConstant(theme::EPISODE_TEXT_WIDTH);
            // As wide as the text column gets, but never past the row's edge.
            narrow.setPriority(objc2_app_kit::NSLayoutPriorityDefaultHigh);
            NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
                line.leadingAnchor()
                    .constraintEqualToAnchor_constant(&art.trailingAnchor(), theme::SPACE_L),
                line.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(
                    &row.trailingAnchor(),
                    -theme::SPACE_L,
                ),
                narrow,
            ]));
        }
    }
}
