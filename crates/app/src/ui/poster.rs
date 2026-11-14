//! One library item in a grid or shelf: its artwork with two lines of text.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use objc2::rc::{Allocated, Retained, Weak};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSCollectionViewItem, NSColor, NSFont, NSFontWeight, NSFontWeightMedium, NSFontWeightRegular,
    NSImageScaling, NSImageView, NSLayoutAttribute, NSLayoutConstraint, NSLineBreakMode, NSNibName,
    NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSBundle, NSString};
use plex::model::{Item, Kind};

use super::theme;
use crate::images::Images;

/// The proportions of a cell's artwork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Upright, for films, shows and seasons.
    Poster,
    /// Wide, for an episode's still frame.
    Still,
}

impl Shape {
    /// Width and height of the artwork in points.
    fn artwork_size(self) -> (u32, u32) {
        match self {
            Self::Poster => (theme::POSTER_WIDTH, theme::POSTER_HEIGHT),
            Self::Still => (theme::STILL_WIDTH, theme::STILL_HEIGHT),
        }
    }

    /// Width and height of a whole cell in points: artwork plus captions.
    pub fn cell_size(self) -> (f64, f64) {
        let (width, height) = self.artwork_size();
        (f64::from(width), f64::from(height) + theme::CAPTIONS_HEIGHT)
    }
}

#[derive(Default)]
pub struct Ivars {
    views: OnceCell<Views>,
    /// The artwork path this cell is waiting for. A cell is reused for
    /// another item while its image is still on the way, and the late image
    /// must not land on the wrong item.
    wanted: RefCell<Option<String>>,
}

struct Views {
    artwork: Retained<NSImageView>,
    width: Retained<NSLayoutConstraint>,
    height: Retained<NSLayoutConstraint>,
    title: Retained<NSTextField>,
    subtitle: Retained<NSTextField>,
}

define_class!(
    // SAFETY: NSCollectionViewItem is designed to be subclassed, and
    // `PosterItem` does not implement `Drop`.
    #[unsafe(super(NSCollectionViewItem))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustPosterItem"]
    #[ivars = Ivars]
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
            let this = this.set_ivars(Ivars::default());
            // SAFETY: forwards the designated initializer unchanged.
            unsafe { msg_send![super(this), initWithNibName: nib, bundle: bundle] }
        }

        #[unsafe(method(loadView))]
        fn load_view(&self) {
            let views = Views::new(self.mtm());
            self.setView(&views.stack());
            // Cannot fail: AppKit loads a controller's view once.
            let _ = self.ivars().views.set(views);
        }
    }
);

impl PosterItem {
    pub const IDENTIFIER: &str = "poster";

    /// Fills the cell with `item`, drawn in `shape`.
    pub fn show(&self, item: &Item, shape: Shape, images: &Rc<Images>) {
        // Asking for the view makes AppKit call `loadView` if it has not.
        let _ = self.view();
        let Some(views) = self.ivars().views.get() else {
            return;
        };

        let (width, height) = shape.artwork_size();
        views.width.setConstant(f64::from(width));
        views.height.setConstant(f64::from(height));
        let (title, subtitle) = captions(item, shape);
        views.title.setStringValue(&NSString::from_str(&title));
        views.subtitle.setStringValue(&NSString::from_str(&subtitle));
        views.artwork.setImage(None);

        let path = artwork_path(item, shape);
        *self.ivars().wanted.borrow_mut() = path.map(str::to_owned);
        let Some(path) = path else {
            return;
        };

        let cell = Weak::new(self);
        let wanted = path.to_owned();
        let scale = theme::RETINA_SCALE;
        images.load(path, width * scale, height * scale, move |image| {
            let Some(cell) = cell.load() else {
                return;
            };
            let still_wanted = cell.ivars().wanted.borrow().as_deref() == Some(wanted.as_str());
            if let (true, Some(views)) = (still_wanted, cell.ivars().views.get()) {
                views.artwork.setImage(Some(image));
            }
        });
    }
}

/// On a poster, an episode stands for its show, since its own artwork is a
/// wide still frame. As a still, it shows that frame.
fn artwork_path(item: &Item, shape: Shape) -> Option<&str> {
    match (item.kind, shape) {
        (Kind::Episode, Shape::Poster) => {
            item.grandparent_thumb.as_deref().or(item.thumb.as_deref())
        }
        _ => item.thumb.as_deref(),
    }
}

/// The two lines under the artwork.
fn captions(item: &Item, shape: Shape) -> (String, String) {
    match (item.kind, shape, &item.grandparent_title, item.parent_index, item.index) {
        (Kind::Episode, Shape::Poster, Some(show), Some(season), Some(episode)) => {
            (show.clone(), format!("S{season} · E{episode}"))
        }
        (Kind::Episode, Shape::Still, _, _, Some(episode)) => {
            let minutes = item.duration.map(|duration| Duration::from(duration).as_secs() / 60);
            let length = minutes.map(|minutes| format!("{minutes} min")).unwrap_or_default();
            (format!("{episode}. {}", item.title), length)
        }
        _ => (item.title.clone(), item.year.map(|year| year.to_string()).unwrap_or_default()),
    }
}

impl Views {
    fn new(main_thread: MainThreadMarker) -> Self {
        let artwork = NSImageView::new(main_thread);
        artwork.setImageScaling(NSImageScaling::ScaleAxesIndependently);
        artwork.setWantsLayer(true);
        if let Some(layer) = artwork.layer() {
            layer.setCornerRadius(theme::ARTWORK_CORNER_RADIUS);
            layer.setMasksToBounds(true);
            // What shows while the image is on its way.
            layer.setBackgroundColor(Some(&NSColor::quaternaryLabelColor().CGColor()));
        }
        artwork.setTranslatesAutoresizingMaskIntoConstraints(false);
        // The real size is set each time the cell is filled.
        let width = artwork.widthAnchor().constraintEqualToConstant(0.0);
        let height = artwork.heightAnchor().constraintEqualToConstant(0.0);
        width.setActive(true);
        height.setActive(true);

        // SAFETY: the weights are constants that AppKit provides.
        let (medium, regular) = unsafe { (NSFontWeightMedium, NSFontWeightRegular) };
        let title = caption(theme::TITLE_FONT_SIZE, medium, &NSColor::labelColor(), main_thread);
        let subtitle = caption(
            theme::SUBTITLE_FONT_SIZE,
            regular,
            &NSColor::secondaryLabelColor(),
            main_thread,
        );
        Self { artwork, width, height, title, subtitle }
    }

    fn stack(&self) -> Retained<NSStackView> {
        let views: [&NSView; 3] = [&self.artwork, &self.title, &self.subtitle];
        let stack =
            NSStackView::stackViewWithViews(&NSArray::from_slice(&views), self.artwork.mtm());
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::Leading);
        stack.setSpacing(theme::CAPTION_GAP);
        stack
    }
}

fn caption(
    size: f64,
    weight: NSFontWeight,
    color: &NSColor,
    main_thread: MainThreadMarker,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::new(), main_thread);
    label.setFont(Some(&NSFont::systemFontOfSize_weight(size, weight)));
    label.setTextColor(Some(color));
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    label
}
