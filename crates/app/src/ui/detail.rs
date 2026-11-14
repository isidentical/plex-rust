//! The page for one item: what it is, and for a show or season, what is
//! inside it.

use std::rc::Rc;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSButton, NSCellImagePosition, NSColor, NSControlSize, NSFont, NSFontWeightBold, NSImage,
    NSImageScaling, NSImageView, NSLayoutAttribute, NSStackView, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSEdgeInsets, NSObject, NSObjectProtocol, NSString, ns_string};
use plex::model::{Item, Kind};

use super::player::clock;
use super::poster::Shape;
use super::posters::{Arrangement, Posters, Shelf};
use super::theme;
use crate::images::Images;

pub struct Ivars {
    page: Retained<NSStackView>,
    /// The seasons of a show or the episodes of a season.
    children: Retained<Posters>,
    item: Item,
    on_play: Box<dyn Fn(&Item)>,
    on_back: Box<dyn Fn()>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Detail` does
    // not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ReelDetail"]
    #[ivars = Ivars]
    pub struct Detail;

    unsafe impl NSObjectProtocol for Detail {}

    impl Detail {
        #[unsafe(method(play:))]
        fn play(&self, _sender: Option<&AnyObject>) {
            (self.ivars().on_play)(&self.ivars().item);
        }

        #[unsafe(method(back:))]
        fn back(&self, _sender: Option<&AnyObject>) {
            (self.ivars().on_back)();
        }
    }
);

impl Detail {
    /// `on_open` is called when one of the item's children is clicked,
    /// `on_play` when the user asks to watch the item, and `on_back` when
    /// they ask to leave the page.
    pub fn new(
        main_thread: MainThreadMarker,
        item: &Item,
        images: &Rc<Images>,
        on_open: impl Fn(&Item) + 'static,
        on_play: impl Fn(&Item) + 'static,
        on_back: impl Fn() + 'static,
    ) -> Retained<Self> {
        // A season lists episodes, which are wide stills. A show lists
        // seasons, which have posters.
        let shape = if item.kind == Kind::Season {
            Shape::Still
        } else {
            Shape::Poster
        };
        let children = Posters::new(
            main_thread,
            Rc::clone(images),
            Arrangement::Grid(shape),
            on_open,
            || {},
        );

        let page = NSStackView::new(main_thread);
        page.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        page.setAlignment(NSLayoutAttribute::Leading);
        page.setSpacing(theme::PAGE_MARGIN);
        page.setEdgeInsets(NSEdgeInsets {
            top: theme::CELL_SPACING,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        });

        let this = Self::alloc(main_thread).set_ivars(Ivars {
            page,
            children,
            item: item.clone(),
            on_play: Box::new(on_play),
            on_back: Box::new(on_back),
        });
        // SAFETY: `init` is NSObject's designated initializer.
        let detail: Retained<Self> = unsafe { msg_send![super(this), init] };

        let page = &detail.ivars().page;
        page.addArrangedSubview(&detail.back_button(main_thread));
        let play = detail.play_button(main_thread);
        page.addArrangedSubview(&header(item, images, play.as_deref(), main_thread));
        page.addArrangedSubview(detail.ivars().children.view());
        detail
    }

    pub fn view(&self) -> &NSView {
        &self.ivars().page
    }

    /// Fills in the item's seasons or episodes.
    pub fn show_children(&self, children: Vec<Item>) {
        self.ivars().children.show(vec![Shelf {
            title: String::new(),
            items: children,
        }]);
    }

    /// "Play", or "Resume at 1:16:05" when the item was left partway. `None`
    /// for shows and seasons, which have no file of their own.
    fn play_button(&self, main_thread: MainThreadMarker) -> Option<Retained<NSButton>> {
        let item = &self.ivars().item;
        if item.media.is_empty() {
            return None;
        }
        let title = match item.view_offset {
            Some(offset) => format!("Resume at {}", clock(offset.into())),
            None => "Play".to_owned(),
        };
        // SAFETY: the button does not retain its target. `Detail` owns the
        // page the button is in, so it outlives every click.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(&title),
                Some(self),
                Some(sel!(play:)),
                main_thread,
            )
        };
        button.setImage(
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                ns_string!("play.fill"),
                None,
            )
            .as_deref(),
        );
        button.setImagePosition(NSCellImagePosition::ImageLeading);
        button.setControlSize(NSControlSize::Large);
        Some(button)
    }

    fn back_button(&self, main_thread: MainThreadMarker) -> Retained<NSView> {
        let chevron = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            ns_string!("chevron.left"),
            Some(ns_string!("Back")),
        );
        // SAFETY: the button does not retain its target. `Detail` owns the
        // page the button is in, so it outlives every click.
        let button = unsafe {
            match chevron {
                Some(image) => NSButton::buttonWithImage_target_action(
                    &image,
                    Some(self),
                    Some(sel!(back:)),
                    main_thread,
                ),
                None => NSButton::buttonWithTitle_target_action(
                    ns_string!("Back"),
                    Some(self),
                    Some(sel!(back:)),
                    main_thread,
                ),
            }
        };
        inset(&button, main_thread)
    }
}

/// Artwork on the left, title and description on the right.
fn header(
    item: &Item,
    images: &Rc<Images>,
    play: Option<&NSButton>,
    main_thread: MainThreadMarker,
) -> Retained<NSView> {
    let poster = NSImageView::new(main_thread);
    poster.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    poster.setWantsLayer(true);
    if let Some(layer) = poster.layer() {
        layer.setCornerRadius(theme::ARTWORK_CORNER_RADIUS);
        layer.setMasksToBounds(true);
    }
    let (width, height) = (theme::DETAIL_POSTER_WIDTH, theme::DETAIL_POSTER_HEIGHT);
    poster
        .widthAnchor()
        .constraintEqualToConstant(f64::from(width))
        .setActive(true);
    poster
        .heightAnchor()
        .constraintEqualToConstant(f64::from(height))
        .setActive(true);
    // An episode's own artwork is a wide still, so it borrows its show's.
    let artwork = match item.kind {
        Kind::Episode => item.grandparent_thumb.as_deref().or(item.thumb.as_deref()),
        _ => item.thumb.as_deref(),
    };
    if let Some(path) = artwork {
        let poster = poster.clone();
        let scale = theme::RETINA_SCALE;
        images.load(path, width * scale, height * scale, move |image| {
            poster.setImage(Some(image));
        });
    }

    let title = NSTextField::labelWithString(&NSString::from_str(&item.title), main_thread);
    // SAFETY: the weight is a constant that AppKit provides.
    let bold = unsafe { NSFontWeightBold };
    title.setFont(Some(&NSFont::systemFontOfSize_weight(
        theme::DETAIL_TITLE_FONT_SIZE,
        bold,
    )));

    let facts = NSTextField::labelWithString(&NSString::from_str(&facts(item)), main_thread);
    facts.setTextColor(Some(&NSColor::secondaryLabelColor()));

    let summary = item.summary.as_deref().unwrap_or_default();
    let summary = NSTextField::wrappingLabelWithString(&NSString::from_str(summary), main_thread);
    summary.setPreferredMaxLayoutWidth(theme::DETAIL_TEXT_WIDTH);
    summary.setMaximumNumberOfLines(theme::DETAIL_SUMMARY_LINES);

    let mut text: Vec<&NSView> = vec![&title, &facts];
    text.extend(play.map(|play| -> &NSView { play }));
    text.push(&summary);
    let text = NSStackView::stackViewWithViews(&NSArray::from_slice(&text), main_thread);
    text.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    text.setAlignment(NSLayoutAttribute::Leading);
    text.setSpacing(theme::CELL_SPACING / 2.0);

    let row: [&NSView; 2] = [&poster, &text];
    let row = NSStackView::stackViewWithViews(&NSArray::from_slice(&row), main_thread);
    row.setAlignment(NSLayoutAttribute::Top);
    row.setSpacing(theme::PAGE_MARGIN);
    inset(&row, main_thread)
}

/// The line under the title, such as "Show · Season 2 · Episode 3 · 44 min".
fn facts(item: &Item) -> String {
    let mut facts: Vec<String> = Vec::new();
    facts.extend(item.grandparent_title.clone());
    if item.kind == Kind::Episode {
        facts.extend(item.parent_index.map(|season| format!("Season {season}")));
        facts.extend(item.index.map(|episode| format!("Episode {episode}")));
    }
    facts.extend(item.year.map(|year| year.to_string()));
    facts.extend(
        item.leaf_count
            .map(|episodes| format!("{episodes} episodes")),
    );
    facts.extend(
        item.duration
            .map(|duration| format!("{} min", Duration::from(duration).as_secs() / 60)),
    );
    facts.join(" · ")
}

/// Wraps `view` so it sits one page margin in from the left edge. The grid
/// below it brings its own margin, so the page itself has none.
fn inset(view: &NSView, main_thread: MainThreadMarker) -> Retained<NSView> {
    let wrapper = NSStackView::stackViewWithViews(&NSArray::from_slice(&[view]), main_thread);
    wrapper.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: theme::PAGE_MARGIN,
        bottom: 0.0,
        right: theme::PAGE_MARGIN,
    });
    Retained::into_super(wrapper)
}
