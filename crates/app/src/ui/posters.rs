//! A scrolling collection of library items, used for every screen that
//! lists them.

use std::cell::{Cell, OnceCell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::{Allocated, Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{
    ClassType, DefinedClass, Encode, Encoding, MainThreadMarker, MainThreadOnly, Message,
    define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSApplication, NSCollectionElementKindSectionHeader,
    NSCollectionLayoutBoundarySupplementaryItem, NSCollectionLayoutContainer,
    NSCollectionLayoutDimension, NSCollectionLayoutEnvironment, NSCollectionLayoutGroup,
    NSCollectionLayoutItem, NSCollectionLayoutSection,
    NSCollectionLayoutSectionOrthogonalScrollingBehavior, NSCollectionLayoutSize,
    NSCollectionLayoutSpacing, NSCollectionView, NSCollectionViewCompositionalLayout,
    NSCollectionViewCompositionalLayoutConfiguration, NSCollectionViewDataSource,
    NSCollectionViewDelegate, NSCollectionViewItem, NSCollectionViewSupplementaryElementKind,
    NSColor, NSEvent, NSEventType, NSFontWeightBold, NSFontWeightRegular,
    NSIndexPathNSCollectionViewAdditions, NSMenu, NSMenuItem, NSRectAlignment, NSScrollView,
    NSTextField, NSView,
};
use objc2_foundation::{
    NSArray, NSIndexPath, NSInteger, NSMutableIndexSet, NSObject, NSObjectProtocol, NSPoint,
    NSRect, NSSet, NSString, ns_string,
};
use plex::model::Item;

use super::episode::EpisodeItem;
use super::hover::HoverView;
use super::layout;
use super::poster::{self, PosterItem, Shape};
use super::text;
use super::theme;
use crate::images::Images;

/// How the items of one shelf are arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// One row of cards, scrolling sideways.
    Shelf(Shape),
    /// Rows of cards that wrap to the width of the view.
    Grid(Shape),
    /// Episodes, one to a row.
    List,
}

/// A titled group of items. An empty title means no heading.
#[derive(Debug, Clone, PartialEq)]
pub struct Shelf {
    pub title: String,
    pub style: Style,
    pub items: Vec<Item>,
}

/// The shape of a shelf whose items are not known yet.
#[derive(Debug, Clone)]
pub struct Outline {
    pub title: &'static str,
    pub style: Style,
    pub count: usize,
}

/// What the viewer asked to do with an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Open,
    Play,
    SetWatched(bool),
    /// Go from an episode to its show, opened on the episode's season.
    OpenShow,
}

/// Hears what the viewer asked to do, and with which item.
pub type OnAction = dyn Fn(&Item, Action);

/// A view shown above the shelves, scrolling with them.
pub struct Header {
    pub view: Retained<NSView>,
    pub height: f64,
}

/// What the collection shows: grey outlines until the first answer, then
/// the items.
enum Content {
    Outlines(Vec<Outline>),
    Shelves(Vec<Shelf>),
}

impl Content {
    fn shelf_count(&self) -> usize {
        match self {
            Self::Outlines(outlines) => outlines.len(),
            Self::Shelves(shelves) => shelves.len(),
        }
    }

    fn item_count(&self, shelf: usize) -> usize {
        match self {
            Self::Outlines(outlines) => outlines.get(shelf).map_or(0, |outline| outline.count),
            Self::Shelves(shelves) => shelves.get(shelf).map_or(0, |shelf| shelf.items.len()),
        }
    }

    fn style(&self, shelf: usize) -> Option<Style> {
        match self {
            Self::Outlines(outlines) => outlines.get(shelf).map(|outline| outline.style),
            Self::Shelves(shelves) => shelves.get(shelf).map(|shelf| shelf.style),
        }
    }

    fn title(&self, shelf: usize) -> &str {
        match self {
            Self::Outlines(outlines) => outlines.get(shelf).map_or("", |outline| outline.title),
            Self::Shelves(shelves) => shelves.get(shelf).map_or("", |shelf| shelf.title.as_str()),
        }
    }

    fn item(&self, shelf: usize, item: usize) -> Option<&Item> {
        match self {
            Self::Outlines(_) => None,
            Self::Shelves(shelves) => shelves.get(shelf)?.items.get(item),
        }
    }
}

/// When the item this close to the end of a grid is drawn, more are needed.
const NEAR_END: usize = 30;

const HEADING: &str = "heading";
const PAGE_HEADER: &str = "page-header";

/// The Return and Space keys, as `NSEvent::keyCode` reports them.
const KEY_RETURN: u16 = 36;
const KEY_SPACE: u16 = 49;

pub struct Ivars {
    scroll: Retained<NSScrollView>,
    collection: Retained<ItemsView>,
    /// Shared with the layout, which asks it how each shelf is arranged.
    content: Rc<RefCell<Content>>,
    header: Option<Header>,
    /// Shown in the middle when there turned out to be nothing to list.
    nothing: Retained<NSTextField>,
    images: Rc<Images>,
    on_action: Box<OnAction>,
    on_near_end: Box<dyn Fn()>,
    /// Set from asking for more items until they arrive, so that one page
    /// is not asked for twice.
    awaiting_more: Cell<bool>,
    /// The item a context menu is open on.
    menu_item: RefCell<Option<Item>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Posters` does
    // not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustPosters"]
    #[ivars = Ivars]
    pub struct Posters;

    unsafe impl NSObjectProtocol for Posters {}

    unsafe impl NSCollectionViewDataSource for Posters {
        #[unsafe(method(numberOfSectionsInCollectionView:))]
        fn shelf_count(&self, _collection: &NSCollectionView) -> NSInteger {
            self.ivars().content.borrow().shelf_count().cast_signed()
        }

        #[unsafe(method(collectionView:numberOfItemsInSection:))]
        fn item_count(&self, _collection: &NSCollectionView, shelf: NSInteger) -> NSInteger {
            let shelf = usize::try_from(shelf).unwrap_or(usize::MAX);
            self.ivars().content.borrow().item_count(shelf).cast_signed()
        }

        #[unsafe(method_id(collectionView:itemForRepresentedObjectAtIndexPath:))]
        fn cell(
            &self,
            collection: &NSCollectionView,
            index_path: &NSIndexPath,
        ) -> Retained<NSCollectionViewItem> {
            let (shelf, _) = position(index_path);
            let style = self.ivars().content.borrow().style(shelf);
            let identifier = match style {
                Some(Style::List) => EpisodeItem::IDENTIFIER,
                _ => PosterItem::IDENTIFIER,
            };
            let cell = collection
                .makeItemWithIdentifier_forIndexPath(&NSString::from_str(identifier), index_path);
            self.fill(&cell, index_path);
            cell
        }

        #[unsafe(method_id(collectionView:viewForSupplementaryElementOfKind:atIndexPath:))]
        fn supplementary(
            &self,
            collection: &NSCollectionView,
            kind: &NSCollectionViewSupplementaryElementKind,
            index_path: &NSIndexPath,
        ) -> Retained<NSView> {
            let is_page_header = kind.to_string() == PAGE_HEADER;
            let identifier = if is_page_header { PAGE_HEADER } else { HEADING };
            let view = collection.makeSupplementaryViewOfKind_withIdentifier_forIndexPath(
                kind,
                &NSString::from_str(identifier),
                index_path,
            );
            match (&self.ivars().header, is_page_header) {
                (Some(header), true) => layout::fill(&view, &header.view),
                _ => self.title(&view, index_path),
            }
            view
        }
    }

    unsafe impl NSCollectionViewDelegate for Posters {
        #[unsafe(method(collectionView:didSelectItemsAtIndexPaths:))]
        fn selected(&self, collection: &NSCollectionView, index_paths: &NSSet<NSIndexPath>) {
            let event = NSApplication::sharedApplication(self.mtm()).currentEvent();
            let click = event.filter(|event| {
                matches!(event.r#type(), NSEventType::LeftMouseDown | NSEventType::LeftMouseUp)
            });
            // A selection made with the arrow keys stays: it is where the
            // keyboard is. One made with a click is an action.
            let (Some(click), Some(index_path)) = (click, index_paths.anyObject()) else {
                return;
            };
            // SAFETY: the sender is optional.
            unsafe { collection.deselectAll(None) };
            let on_play = collection
                .itemAtIndexPath(&index_path)
                .is_some_and(|cell| is_on_play(&cell, click.locationInWindow()));
            self.act(&index_path, if on_play { Action::Play } else { Action::Open });
        }
    }

    impl Posters {
        #[unsafe(method(menuOpen:))]
        fn menu_open(&self, _sender: Option<&AnyObject>) {
            self.act_on_menu_item(|_| Action::Open);
        }

        #[unsafe(method(menuPlay:))]
        fn menu_play(&self, _sender: Option<&AnyObject>) {
            self.act_on_menu_item(|_| Action::Play);
        }

        #[unsafe(method(menuToggleWatched:))]
        fn menu_toggle_watched(&self, _sender: Option<&AnyObject>) {
            self.act_on_menu_item(|item| Action::SetWatched(!item.is_watched()));
        }
    }
);

impl Posters {
    /// `outlines` is what to draw until [`show`](Self::show) is first
    /// called, and `nothing` what to say if it is then called with no
    /// items. `on_action` is called when the viewer clicks an item or picks
    /// from its menu. `on_near_end` is called when a grid is scrolled close
    /// to its last item, as the cue to [`append`](Self::append) more.
    pub fn new(
        main_thread: MainThreadMarker,
        images: Rc<Images>,
        header: Option<Header>,
        outlines: Vec<Outline>,
        nothing: &str,
        on_action: impl Fn(&Item, Action) + 'static,
        on_near_end: impl Fn() + 'static,
    ) -> Retained<Self> {
        let content = Rc::new(RefCell::new(Content::Outlines(outlines)));
        let header_height = header.as_ref().map(|header| header.height);
        let collection = ItemsView::new(main_thread);
        collection.setCollectionViewLayout(Some(&layout(
            Rc::clone(&content),
            header_height,
            main_thread,
        )));
        collection
            .setBackgroundColors(Some(&NSArray::from_retained_slice(&[NSColor::clearColor()])));
        collection.setSelectable(true);
        // SAFETY: the classes are valid for their roles: item classes for
        // items, view classes for supplementary views.
        unsafe {
            collection.registerClass_forItemWithIdentifier(
                Some(PosterItem::class()),
                &NSString::from_str(PosterItem::IDENTIFIER),
            );
            collection.registerClass_forItemWithIdentifier(
                Some(EpisodeItem::class()),
                &NSString::from_str(EpisodeItem::IDENTIFIER),
            );
            collection.registerClass_forSupplementaryViewOfKind_withIdentifier(
                Some(NSView::class()),
                NSCollectionElementKindSectionHeader,
                &NSString::from_str(HEADING),
            );
            collection.registerClass_forSupplementaryViewOfKind_withIdentifier(
                Some(NSView::class()),
                &NSString::from_str(PAGE_HEADER),
                &NSString::from_str(PAGE_HEADER),
            );
        }

        let scroll = NSScrollView::new(main_thread);
        scroll.setDocumentView(Some(&collection));
        scroll.setHasVerticalScroller(true);
        scroll.setDrawsBackground(false);

        // SAFETY: the weight is a constant that AppKit provides.
        let regular = unsafe { NSFontWeightRegular };
        let nothing_label = text::label(
            theme::FONT_HEADLINE,
            regular,
            &NSColor::secondaryLabelColor(),
            main_thread,
        );
        text::set(&nothing_label, nothing);
        nothing_label.setHidden(true);
        scroll.addSubview(&nothing_label);
        nothing_label
            .centerXAnchor()
            .constraintEqualToAnchor(&scroll.centerXAnchor())
            .setActive(true);
        nothing_label
            .centerYAnchor()
            .constraintEqualToAnchor(&scroll.centerYAnchor())
            .setActive(true);

        let this = Self::alloc(main_thread).set_ivars(Ivars {
            scroll,
            collection,
            content,
            header,
            nothing: nothing_label,
            images,
            on_action: Box::new(on_action),
            on_near_end: Box::new(on_near_end),
            awaiting_more: Cell::new(false),
            menu_item: RefCell::new(None),
        });
        // SAFETY: `init` is NSObject's designated initializer.
        let posters: Retained<Self> = unsafe { msg_send![super(this), init] };
        // The collection view retains neither. Whoever owns `Posters` keeps
        // it alive while its view is on screen.
        let collection = &posters.ivars().collection;
        collection.setDataSource(Some(ProtocolObject::from_ref(&*posters)));
        collection.setDelegate(Some(ProtocolObject::from_ref(&*posters)));
        // Cannot fail: the cell was created empty a few lines up.
        let _ = collection.ivars().set(Weak::new(&posters));
        posters
    }

    pub fn view(&self) -> &NSView {
        &self.ivars().scroll
    }

    /// Gives the keyboard to the collection, so the arrow keys move in it.
    pub fn focus(&self) {
        if let Some(window) = self.ivars().collection.window() {
            window.makeFirstResponder(Some(&self.ivars().collection));
        }
    }

    /// Replaces everything shown, disturbing as little as it can. When the
    /// shelves are the same ones, only those whose items changed are laid
    /// out again, and the cells of the others are filled where they are,
    /// so nothing moves and nothing flashes.
    pub fn show(&self, shelves: Vec<Shelf>) {
        let ivars = self.ivars();
        ivars.awaiting_more.set(false);
        ivars.nothing.setHidden(shelves.iter().any(|shelf| !shelf.items.is_empty()));
        let new = Content::Shelves(shelves);
        let changed = changed_shelves(&ivars.content.borrow(), &new);
        *ivars.content.borrow_mut() = new;
        let Some(changed) = changed else {
            ivars.collection.reloadData();
            return;
        };
        if !changed.is_empty() {
            let sections = NSMutableIndexSet::new();
            for shelf in changed {
                sections.addIndex(shelf);
            }
            ivars.collection.reloadSections(&sections);
        }
        for cell in ivars.collection.visibleItems() {
            if let Some(index_path) = ivars.collection.indexPathForItem(&cell) {
                self.fill(&cell, &index_path);
            }
        }
    }

    /// Adds `items` to the end of the last shelf, where they are the ones
    /// from index `start` on. A page that starts anywhere but at the end
    /// of what is shown was asked for before the list changed under it,
    /// and is dropped. An empty page means there is nothing more, and no
    /// more will be asked for.
    pub fn append(&self, start: usize, items: Vec<Item>) {
        let ivars = self.ivars();
        if items.is_empty() || start != self.len() {
            return;
        }
        let Some(last) = ivars.content.borrow().shelf_count().checked_sub(1) else {
            return;
        };
        // The collection view counts its items lazily. Asking it now makes
        // it count before the list grows: counting afterwards, it would
        // find the new items already there and refuse to insert them.
        let _ = ivars.collection.numberOfItemsInSection(last.cast_signed());
        let added: Vec<Retained<NSIndexPath>> = (start..start + items.len())
            .map(|item| {
                NSIndexPath::indexPathForItem_inSection(item.cast_signed(), last.cast_signed())
            })
            .collect();
        if let Content::Shelves(shelves) = &mut *ivars.content.borrow_mut()
            && let Some(shelf) = shelves.last_mut()
        {
            shelf.items.extend(items);
        }
        // Inserting leaves the cells on screen and the scroll position
        // alone, which reloading everything would not.
        ivars.collection.insertItemsAtIndexPaths(&NSSet::from_retained_slice(&added));
        ivars.awaiting_more.set(false);
    }

    /// How many items are shown, across all shelves.
    pub fn len(&self) -> usize {
        let content = self.ivars().content.borrow();
        match &*content {
            Content::Outlines(_) => 0,
            Content::Shelves(shelves) => shelves.iter().map(|shelf| shelf.items.len()).sum(),
        }
    }

    /// Scrolls so that the content `y` points down is at the top.
    pub fn scroll_to(&self, y: f64) {
        self.ivars().collection.scrollPoint(NSPoint { x: 0.0, y });
    }

    /// Puts the pointer's highlight on the first item, for a snapshot.
    pub fn hover_first(&self) {
        let first = NSIndexPath::indexPathForItem_inSection(0, 0);
        if let Some(cell) = self.ivars().collection.itemAtIndexPath(&first)
            && let Some(view) = cell.view().downcast_ref::<HoverView>()
        {
            view.hovered(true);
        }
    }

    fn item_at(&self, index_path: &NSIndexPath) -> Option<Item> {
        let (shelf, item) = position(index_path);
        self.ivars().content.borrow().item(shelf, item).cloned()
    }

    /// Tells the owner what the viewer wants done with the item at
    /// `index_path`.
    fn act(&self, index_path: &NSIndexPath, action: Action) {
        // What the handler shows next may replace this collection, which
        // would free it while this method is still running.
        let _alive = self.retain();
        // Cloned so the handler is free to change what is shown.
        if let Some(item) = self.item_at(index_path) {
            (self.ivars().on_action)(&item, action);
        }
    }

    fn act_on_menu_item(&self, action: impl Fn(&Item) -> Action) {
        let _alive = self.retain();
        let item = self.ivars().menu_item.borrow_mut().take();
        if let Some(item) = item {
            (self.ivars().on_action)(&item, action(&item));
        }
    }

    fn fill(&self, cell: &NSCollectionViewItem, index_path: &NSIndexPath) {
        let ivars = self.ivars();
        let (shelf, index) = position(index_path);
        let content = ivars.content.borrow();
        let (Some(style), item) = (content.style(shelf), content.item(shelf, index)) else {
            return;
        };
        match (style, item) {
            (Style::List, Some(item)) => {
                if let Some(cell) = cell.downcast_ref::<EpisodeItem>() {
                    cell.show(item, &ivars.images);
                }
            }
            (Style::Shelf(shape) | Style::Grid(shape), Some(item)) => {
                if let Some(cell) = cell.downcast_ref::<PosterItem>() {
                    cell.show(item, shape, &ivars.images);
                }
            }
            // An outline: a cell with nothing in it yet, and nothing after
            // it to ask for.
            (_, None) => return,
        }

        let is_grid = matches!(style, Style::Grid(_));
        let left = content.item_count(shelf).saturating_sub(index);
        drop(content);
        if is_grid && left <= NEAR_END && !ivars.awaiting_more.replace(true) {
            (ivars.on_near_end)();
        }
    }

    fn title(&self, view: &NSView, index_path: &NSIndexPath) {
        // The collection view hands back a view it made earlier, with the
        // label this put in it then, or a new, empty one.
        let reused = view.subviews().firstObject();
        let label = match reused.and_then(|label| label.downcast::<NSTextField>().ok()) {
            Some(label) => label,
            None => {
                // SAFETY: the weight is a constant that AppKit provides.
                let bold = unsafe { NSFontWeightBold };
                let label =
                    text::label(theme::FONT_TITLE, bold, &NSColor::labelColor(), self.mtm());
                view.addSubview(&label);
                // At the bottom of its strip, close to the shelf it names.
                label
                    .leadingAnchor()
                    .constraintEqualToAnchor(&view.leadingAnchor())
                    .setActive(true);
                label
                    .bottomAnchor()
                    .constraintEqualToAnchor_constant(&view.bottomAnchor(), -theme::SPACE_XS)
                    .setActive(true);
                label
            }
        };
        let (shelf, _) = position(index_path);
        text::set(&label, self.ivars().content.borrow().title(shelf));
    }

    /// Return and Space open the item the keyboard is on.
    fn key_pressed(&self, event: &NSEvent) -> bool {
        if !matches!(event.keyCode(), KEY_RETURN | KEY_SPACE) {
            return false;
        }
        let Some(index_path) = self.ivars().collection.selectionIndexPaths().anyObject() else {
            return false;
        };
        self.act(&index_path, Action::Open);
        true
    }

    /// The menu for a secondary click at the place of `event`.
    fn menu_at(&self, event: &NSEvent) -> Option<Retained<NSMenu>> {
        let collection = &self.ivars().collection;
        let point = collection.convertPoint_fromView(event.locationInWindow(), None);
        let index_path = collection.indexPathForItemAtPoint(point)?;
        let item = self.item_at(&index_path)?;

        let menu = NSMenu::new(self.mtm());
        let add = |title: &str, action| {
            // SAFETY: the menu item does not retain its target. `Posters`
            // owns the collection the menu belongs to, so it outlives it.
            let entry = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm()),
                    &NSString::from_str(title),
                    Some(action),
                    ns_string!(""),
                )
            };
            // SAFETY: as above.
            unsafe { entry.setTarget(Some(self)) };
            menu.addItem(&entry);
        };
        if poster::is_playable(&item) {
            add("Play", sel!(menuPlay:));
        }
        add("Open", sel!(menuOpen:));
        menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
        let toggle = if item.is_watched() { "Mark as Unwatched" } else { "Mark as Watched" };
        add(toggle, sel!(menuToggleWatched:));
        *self.ivars().menu_item.borrow_mut() = Some(item);
        Some(menu)
    }
}

/// The shelf and the place in it that `index_path` names.
fn position(index_path: &NSIndexPath) -> (usize, usize) {
    let part = |number: NSInteger| usize::try_from(number).unwrap_or(usize::MAX);
    (part(index_path.section()), part(index_path.item()))
}

fn is_on_play(cell: &NSCollectionViewItem, point: NSPoint) -> bool {
    match (cell.downcast_ref::<PosterItem>(), cell.downcast_ref::<EpisodeItem>()) {
        (Some(card), _) => card.is_on_play(point),
        (_, Some(row)) => row.is_on_play(point),
        _ => false,
    }
}

/// Which shelves of `new` hold other items, or the same in another order,
/// than the shelf in that place in `old`. `None` when the shelves
/// themselves differ, in number, title or arrangement, or when `old` was
/// only outlines: then nothing on screen can stay.
fn changed_shelves(old: &Content, new: &Content) -> Option<Vec<usize>> {
    let (Content::Shelves(old), Content::Shelves(new)) = (old, new) else {
        return None;
    };
    let same_shelf =
        |(old, new): (&Shelf, &Shelf)| old.title == new.title && old.style == new.style;
    if old.len() != new.len() || !old.iter().zip(new).all(same_shelf) {
        return None;
    }
    let same_items = |old: &Shelf, new: &Shelf| {
        let mut pairs = old.items.iter().zip(&new.items);
        old.items.len() == new.items.len()
            && pairs.all(|(old, new)| old.rating_key == new.rating_key)
    };
    let pairs = old.iter().zip(new).enumerate();
    Some(pairs.filter(|(_, (old, new))| !same_items(old, new)).map(|(shelf, _)| shelf).collect())
}

/// How many cards fit across `available` points when each is at least
/// `least` wide with `spacing` between them, and how wide each then is.
fn columns(available: f64, least: f64, spacing: f64) -> (usize, f64) {
    let count = ((available + spacing) / (least + spacing)).floor().max(1.0);
    let width = ((available - (count - 1.0) * spacing) / count).floor();
    (count as usize, width.max(1.0))
}

define_class!(
    // SAFETY: NSCollectionView may be subclassed, and `ItemsView` does not
    // implement `Drop`.
    #[unsafe(super(NSCollectionView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustItemsView"]
    #[ivars = OnceCell<Weak<Posters>>]
    struct ItemsView;

    impl ItemsView {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(OnceCell::new());
            // SAFETY: forwards the designated initializer unchanged.
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let owner = self.ivars().get().and_then(Weak::load);
            if owner.is_some_and(|posters| posters.key_pressed(event)) {
                return;
            }
            // SAFETY: forwards the event unchanged.
            let () = unsafe { msg_send![super(self), keyDown: event] };
        }

        /// Scrolling moves the cards under a pointer that stays where it
        /// is, so the highlight has to move to the card now under it.
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            // SAFETY: forwards the event unchanged.
            let () = unsafe { msg_send![super(self), scrollWheel: event] };
            for cell in self.visibleItems() {
                if let Some(view) = cell.view().downcast_ref::<HoverView>() {
                    view.follow(event.locationInWindow());
                }
            }
        }

        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, event: &NSEvent) -> Option<Retained<NSMenu>> {
            let owner = self.ivars().get().and_then(Weak::load);
            owner.and_then(|posters| posters.menu_at(event))
        }
    }
);

impl ItemsView {
    fn new(main_thread: MainThreadMarker) -> Retained<Self> {
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        unsafe { msg_send![Self::alloc(main_thread), initWithFrame: NSRect::ZERO] }
    }
}

fn layout(
    content: Rc<RefCell<Content>>,
    header_height: Option<f64>,
    main_thread: MainThreadMarker,
) -> Retained<NSCollectionViewCompositionalLayout> {
    let provider = RcBlock::new(
        move |shelf: NSInteger,
              environment: NonNull<ProtocolObject<dyn NSCollectionLayoutEnvironment>>|
              -> *mut NSCollectionLayoutSection {
            let shelf = usize::try_from(shelf).unwrap_or(usize::MAX);
            let content = content.borrow();
            let Some(style) = content.style(shelf) else {
                return std::ptr::null_mut();
            };
            // SAFETY: AppKit passes a valid environment for the call.
            let width = unsafe { environment.as_ref() }.container().effectiveContentSize().width;
            let has_heading = !content.title(shelf).is_empty();
            Retained::autorelease_return(section(style, has_heading, width, main_thread))
        },
    );

    let configuration = NSCollectionViewCompositionalLayoutConfiguration::new(main_thread);
    if let Some(height) = header_height {
        let header = boundary_item(PAGE_HEADER, height, main_thread);
        configuration.setBoundarySupplementaryItems(&NSArray::from_retained_slice(&[header]));
    }
    // SAFETY: the block has the signature the layout calls it with, and the
    // layout copies it.
    unsafe {
        NSCollectionViewCompositionalLayout::initWithSectionProvider_configuration(
            NSCollectionViewCompositionalLayout::alloc(main_thread),
            RcBlock::as_ptr(&provider),
            &configuration,
        )
    }
}

/// A full-width strip above a shelf or above the whole collection.
fn boundary_item(
    kind: &str,
    height: f64,
    main_thread: MainThreadMarker,
) -> Retained<NSCollectionLayoutBoundarySupplementaryItem> {
    let size = NSCollectionLayoutSize::sizeWithWidthDimension_heightDimension(
        &NSCollectionLayoutDimension::fractionalWidthDimension(1.0, main_thread),
        &NSCollectionLayoutDimension::absoluteDimension(height, main_thread),
    );
    NSCollectionLayoutBoundarySupplementaryItem::boundarySupplementaryItemWithLayoutSize_elementKind_alignment(
        &size,
        &NSString::from_str(kind),
        NSRectAlignment::Top,
    )
}

/// The space around a shelf's items, as `NSDirectionalEdgeInsets` lays it
/// out. AppKit's headers call the type that, but the method that takes it
/// was compiled under the name `CVCDirectionalEdgeInsets`, and a debug
/// build of objc2 refuses to send a struct under any other name. So this
/// one carries the name the method expects.
#[repr(C)]
#[derive(Clone, Copy)]
struct SectionInsets {
    top: f64,
    leading: f64,
    bottom: f64,
    trailing: f64,
}

// SAFETY: four doubles in the order of `NSDirectionalEdgeInsets`, under
// the name `-[NSCollectionLayoutSection setContentInsets:]` reports.
unsafe impl Encode for SectionInsets {
    const ENCODING: Encoding = Encoding::Struct(
        "CVCDirectionalEdgeInsets",
        &[f64::ENCODING, f64::ENCODING, f64::ENCODING, f64::ENCODING],
    );
}

/// The layout of one shelf in a collection `width` points wide.
fn section(
    style: Style,
    has_heading: bool,
    width: f64,
    main_thread: MainThreadMarker,
) -> Retained<NSCollectionLayoutSection> {
    let absolute = |points| NSCollectionLayoutDimension::absoluteDimension(points, main_thread);
    let full_width = NSCollectionLayoutDimension::fractionalWidthDimension(1.0, main_thread);
    let size = |width: &NSCollectionLayoutDimension, height: f64| {
        NSCollectionLayoutSize::sizeWithWidthDimension_heightDimension(width, &absolute(height))
    };
    // Room above and below for a card to grow under the pointer and cast
    // its shadow without being cut off by the shelf's edge.
    let mut insets = SectionInsets {
        top: theme::SPACE_S,
        leading: theme::PAGE_MARGIN,
        bottom: theme::SHELF_SPACING,
        trailing: theme::PAGE_MARGIN,
    };

    let section = match style {
        Style::Shelf(shape) => {
            let card = size(&absolute(shape.width()), shape.height(shape.width()));
            let item = NSCollectionLayoutItem::itemWithLayoutSize(&card);
            let group = NSCollectionLayoutGroup::horizontalGroupWithLayoutSize_subitems(
                &card,
                &NSArray::from_retained_slice(&[item]),
            );
            let shelf = NSCollectionLayoutSection::sectionWithGroup(&group);
            shelf.setOrthogonalScrollingBehavior(
                NSCollectionLayoutSectionOrthogonalScrollingBehavior::Continuous,
            );
            shelf.setInterGroupSpacing(theme::CELL_SPACING);
            shelf
        }
        Style::Grid(shape) => {
            let available = width - 2.0 * theme::PAGE_MARGIN;
            let (_, card_width) = columns(available, shape.width(), theme::CELL_SPACING);
            let height = shape.height(card_width);
            let item =
                NSCollectionLayoutItem::itemWithLayoutSize(&size(&absolute(card_width), height));
            // A row as wide as the view holds as many cards as fit.
            let group = NSCollectionLayoutGroup::horizontalGroupWithLayoutSize_subitems(
                &size(&full_width, height),
                &NSArray::from_retained_slice(&[item]),
            );
            let spacing = NSCollectionLayoutSpacing::fixedSpacing(theme::CELL_SPACING, main_thread);
            group.setInterItemSpacing(Some(&spacing));
            let grid = NSCollectionLayoutSection::sectionWithGroup(&group);
            grid.setInterGroupSpacing(theme::CELL_SPACING);
            grid
        }
        Style::List => {
            let row = size(&full_width, theme::EPISODE_ROW_HEIGHT);
            let item = NSCollectionLayoutItem::itemWithLayoutSize(&row);
            let group = NSCollectionLayoutGroup::verticalGroupWithLayoutSize_subitems(
                &row,
                &NSArray::from_retained_slice(&[item]),
            );
            // A row keeps its frame a little inside its own edge, so it
            // starts that much outside the margin to line up with the rest.
            insets.leading -= theme::SPACE_S;
            insets.trailing -= theme::SPACE_S;
            let list = NSCollectionLayoutSection::sectionWithGroup(&group);
            list.setInterGroupSpacing(theme::SPACE_XS);
            list
        }
    };
    // SAFETY: `setContentInsets:` takes one struct of four doubles, which
    // `SectionInsets` is, under the name the method declares.
    let () = unsafe { msg_send![&section, setContentInsets: insets] };
    if has_heading {
        // SAFETY: the element kind is a constant that AppKit provides.
        let kind = unsafe { NSCollectionElementKindSectionHeader };
        let heading = boundary_item(&kind.to_string(), theme::SHELF_HEADING_HEIGHT, main_thread);
        section.setBoundarySupplementaryItems(&NSArray::from_retained_slice(&[heading]));
    }
    section
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shelf(title: &str, keys: &[&str]) -> Shelf {
        let item = |key: &&str| {
            let json = serde_json::json!({ "ratingKey": key, "type": "movie", "title": "Title" });
            serde_json::from_value(json).unwrap()
        };
        Shelf {
            title: title.to_owned(),
            style: Style::Shelf(Shape::Poster),
            items: keys.iter().map(item).collect(),
        }
    }

    #[test]
    fn only_shelves_with_other_items_are_laid_out_again() {
        let shown = |shelves: Vec<Shelf>| Content::Shelves(shelves);
        let old = shown(vec![shelf("a", &["1", "2"]), shelf("b", &["3"])]);

        let same = shown(vec![shelf("a", &["1", "2"]), shelf("b", &["3"])]);
        assert_eq!(changed_shelves(&old, &same), Some(vec![]));
        let reordered = shown(vec![shelf("a", &["2", "1"]), shelf("b", &["3"])]);
        assert_eq!(changed_shelves(&old, &reordered), Some(vec![0]));
        let longer = shown(vec![shelf("a", &["1", "2"]), shelf("b", &["3", "4"])]);
        assert_eq!(changed_shelves(&old, &longer), Some(vec![1]));

        let renamed = shown(vec![shelf("a", &["1", "2"]), shelf("c", &["3"])]);
        assert_eq!(changed_shelves(&old, &renamed), None);
        assert_eq!(changed_shelves(&old, &shown(vec![shelf("a", &["1", "2"])])), None);
        assert_eq!(changed_shelves(&Content::Outlines(Vec::new()), &same), None);
    }

    #[test]
    fn a_grid_fills_its_width_with_as_many_cards_as_fit() {
        // 1060 wide holds six cards of at least 160 with gaps of 16.
        assert_eq!(columns(1060.0, 160.0, 16.0), (6, 163.0));
        // One point narrower than seven need, so still six, each wider.
        assert_eq!(columns(1215.0, 160.0, 16.0), (6, 189.0));
        assert_eq!(columns(1216.0, 160.0, 16.0), (7, 160.0));
        // Narrower than one card: one column, as wide as there is room.
        assert_eq!(columns(120.0, 160.0, 16.0), (1, 120.0));
    }
}
