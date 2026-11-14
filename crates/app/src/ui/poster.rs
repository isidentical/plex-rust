//! One library item in a grid or shelf: its poster with two lines of text.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use objc2::rc::{Allocated, Retained, Weak};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSCollectionViewItem, NSColor, NSFont, NSFontWeightMedium, NSFontWeightRegular, NSImageScaling,
    NSImageView, NSLayoutAttribute, NSLineBreakMode, NSNibName, NSStackView, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSBundle, NSString};
use plex::model::{Item, Kind};

use super::theme;
use crate::images::Images;

#[derive(Default)]
pub struct Ivars {
    views: OnceCell<Views>,
    /// The artwork path this cell is waiting for. A cell is reused for
    /// another item while its image is still on the way, and the late image
    /// must not land on the wrong item.
    wanted: RefCell<Option<String>>,
}

struct Views {
    poster: Retained<NSImageView>,
    title: Retained<NSTextField>,
    subtitle: Retained<NSTextField>,
}

define_class!(
    // SAFETY: NSCollectionViewItem is designed to be subclassed, and
    // `PosterItem` does not implement `Drop`.
    #[unsafe(super(NSCollectionViewItem))]
    #[thread_kind = MainThreadOnly]
    #[name = "ReelPosterItem"]
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

    /// Fills the cell with `item`.
    pub fn show(&self, item: &Item, images: &Rc<Images>) {
        // Asking for the view makes AppKit call `loadView` if it has not.
        let _ = self.view();
        let Some(views) = self.ivars().views.get() else {
            return;
        };

        let (title, subtitle) = captions(item);
        views.title.setStringValue(&NSString::from_str(title));
        views
            .subtitle
            .setStringValue(&NSString::from_str(&subtitle));
        views.poster.setImage(None);

        let Some(path) = poster_path(item) else {
            *self.ivars().wanted.borrow_mut() = None;
            return;
        };
        *self.ivars().wanted.borrow_mut() = Some(path.to_owned());

        let cell = Weak::new(self);
        let wanted = path.to_owned();
        let scale = theme::RETINA_SCALE;
        images.load(
            path,
            theme::POSTER_WIDTH * scale,
            theme::POSTER_HEIGHT * scale,
            move |image| {
                let Some(cell) = cell.load() else {
                    return;
                };
                let still_wanted = cell.ivars().wanted.borrow().as_deref() == Some(wanted.as_str());
                if let (true, Some(views)) = (still_wanted, cell.ivars().views.get()) {
                    views.poster.setImage(Some(image));
                }
            },
        );
    }
}

/// An episode is shown under its show's poster and name, since its own
/// artwork is a still frame that does not fit a poster.
fn poster_path(item: &Item) -> Option<&str> {
    match item.kind {
        Kind::Episode => item.grandparent_thumb.as_deref().or(item.thumb.as_deref()),
        _ => item.thumb.as_deref(),
    }
}

fn captions(item: &Item) -> (&str, String) {
    match (
        item.kind,
        &item.grandparent_title,
        item.parent_index,
        item.index,
    ) {
        (Kind::Episode, Some(show), Some(season), Some(episode)) => {
            (show, format!("S{season} · E{episode}"))
        }
        _ => (
            &item.title,
            item.year.map(|year| year.to_string()).unwrap_or_default(),
        ),
    }
}

impl Views {
    fn new(main_thread: objc2::MainThreadMarker) -> Self {
        let poster = NSImageView::new(main_thread);
        poster.setImageScaling(NSImageScaling::ScaleAxesIndependently);
        poster.setWantsLayer(true);
        if let Some(layer) = poster.layer() {
            layer.setCornerRadius(theme::POSTER_CORNER_RADIUS);
            layer.setMasksToBounds(true);
            layer.setBackgroundColor(Some(&NSColor::quaternaryLabelColor().CGColor()));
        }
        poster.setTranslatesAutoresizingMaskIntoConstraints(false);
        let (width, height) = (
            f64::from(theme::POSTER_WIDTH),
            f64::from(theme::POSTER_HEIGHT),
        );
        poster
            .widthAnchor()
            .constraintEqualToConstant(width)
            .setActive(true);
        poster
            .heightAnchor()
            .constraintEqualToConstant(height)
            .setActive(true);

        // SAFETY: the weights are constants that AppKit provides.
        let (medium, regular) = unsafe { (NSFontWeightMedium, NSFontWeightRegular) };
        let title = caption(
            theme::TITLE_FONT_SIZE,
            medium,
            &NSColor::labelColor(),
            main_thread,
        );
        let subtitle = caption(
            theme::SUBTITLE_FONT_SIZE,
            regular,
            &NSColor::secondaryLabelColor(),
            main_thread,
        );
        Self {
            poster,
            title,
            subtitle,
        }
    }

    fn stack(&self) -> Retained<NSStackView> {
        let views: [&NSView; 3] = [&self.poster, &self.title, &self.subtitle];
        let stack =
            NSStackView::stackViewWithViews(&NSArray::from_slice(&views), self.poster.mtm());
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::Leading);
        stack.setSpacing(theme::POSTER_CAPTION_GAP);
        stack
    }
}

fn caption(
    size: f64,
    weight: objc2_app_kit::NSFontWeight,
    color: &NSColor,
    main_thread: objc2::MainThreadMarker,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::new(), main_thread);
    label.setFont(Some(&NSFont::systemFontOfSize_weight(size, weight)));
    label.setTextColor(Some(color));
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    label
}
