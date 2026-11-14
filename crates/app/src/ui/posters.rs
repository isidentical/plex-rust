//! A scrolling collection of cards, used for every screen that lists
//! things: shelves that scroll sideways, grids, and lists of episodes.

use std::cell::{Cell, OnceCell, RefCell};
use std::ops::Range;
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
    NSApplication, NSCarriageReturnCharacter, NSCollectionElementKindSectionHeader,
    NSCollectionLayoutBoundarySupplementaryItem, NSCollectionLayoutContainer,
    NSCollectionLayoutDimension, NSCollectionLayoutEnvironment, NSCollectionLayoutGroup,
    NSCollectionLayoutItem, NSCollectionLayoutSection,
    NSCollectionLayoutSectionOrthogonalScrollingBehavior, NSCollectionLayoutSize,
    NSCollectionLayoutSpacing, NSCollectionView, NSCollectionViewCompositionalLayout,
    NSCollectionViewCompositionalLayoutConfiguration, NSCollectionViewDataSource,
    NSCollectionViewDelegate, NSCollectionViewItem, NSCollectionViewScrollPosition,
    NSCollectionViewSupplementaryElementKind, NSColor, NSDownArrowFunctionKey, NSEvent,
    NSEventModifierFlags, NSEventType, NSIndexPathNSCollectionViewAdditions,
    NSLeftArrowFunctionKey, NSMenu, NSMenuItem, NSRectAlignment, NSRightArrowFunctionKey,
    NSScrollView, NSUpArrowFunctionKey, NSView, NSViewBoundsDidChangeNotification,
};
use objc2_foundation::{
    NSArray, NSIndexPath, NSIndexSet, NSInteger, NSMutableIndexSet, NSNotification,
    NSNotificationCenter, NSObject, NSObjectProtocol, NSPoint, NSRange, NSRect, NSSet, NSString,
    ns_string,
};
use plex::model::{Item, Kind, RatingKey, Role};

use super::episode::EpisodeItem;
use super::heading;
use super::hover::HoverView;
use super::layout;
use super::notice::Notice;
use super::poster::{self, Card, PosterItem, Shape};
use super::theme;
use crate::images::Images;

/// How the cards of one shelf are arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// One row of cards, scrolling sideways.
    Shelf(Shape),
    /// Rows of cards that wrap to the width of the view.
    Grid(Shape),
    /// Episodes, one to a row.
    List,
}

/// A titled group of cards. An empty title means no heading.
#[derive(Debug, Clone, PartialEq)]
pub struct Shelf {
    pub title: String,
    pub style: Style,
    pub cards: Vec<Card>,
    /// Server path of the whole list this shelf is the start of. With it,
    /// the heading is a link to that list.
    pub more: Option<String>,
    /// The item the page is about, which is marked among the others.
    pub current: Option<RatingKey>,
}

impl Shelf {
    /// A shelf of library items that leads nowhere further.
    pub fn of(title: impl Into<String>, style: Style, items: Vec<Item>) -> Self {
        let cards = items.into_iter().map(Card::Item).collect();
        Self { title: title.into(), style, cards, more: None, current: None }
    }
}

/// The shape of a shelf whose cards are not known yet.
#[derive(Debug, Clone)]
pub struct Outline {
    pub title: &'static str,
    pub style: Style,
    pub count: usize,
}

/// What the viewer asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Open(Item),
    Play(Item),
    SetWatched(Item, bool),
    /// Go from this episode to its show, with the episode's season chosen.
    OpenShow(Item),
    /// Go from this episode to its season: the list it is in.
    OpenSeason(Item),
    /// Show what else this member of the cast is in.
    OpenPerson(Role),
    /// Show the whole list at this server path, under this title.
    OpenList {
        title: String,
        path: String,
    },
}

/// A key the viewer moves through the cards with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    Left,
    Right,
    Up,
    Down,
    Return,
}

impl Press {
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "return" => Some(Self::Return),
            _ => None,
        }
    }

    /// The key as an event carries it: its code and the character it types.
    fn code_and_character(self) -> (u16, u32) {
        match self {
            Self::Left => (KEY_LEFT, NSLeftArrowFunctionKey),
            Self::Right => (KEY_RIGHT, NSRightArrowFunctionKey),
            Self::Up => (KEY_UP, NSUpArrowFunctionKey),
            Self::Down => (KEY_DOWN, NSDownArrowFunctionKey),
            Self::Return => (KEY_RETURN, NSCarriageReturnCharacter),
        }
    }
}

/// Hears what the viewer asked for.
pub type OnAction = dyn Fn(Action);

/// A view shown above the shelves, scrolling with them.
pub struct Header {
    pub view: Retained<NSView>,
    pub height: f64,
}

/// What the collection shows: grey outlines until the first answer, then
/// the shelves.
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

    fn card_count(&self, shelf: usize) -> usize {
        match self {
            Self::Outlines(outlines) => outlines.get(shelf).map_or(0, |outline| outline.count),
            Self::Shelves(shelves) => shelves.get(shelf).map_or(0, |shelf| shelf.cards.len()),
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

    fn shelf(&self, shelf: usize) -> Option<&Shelf> {
        match self {
            Self::Outlines(_) => None,
            Self::Shelves(shelves) => shelves.get(shelf),
        }
    }

    fn card(&self, shelf: usize, card: usize) -> Option<&Card> {
        self.shelf(shelf)?.cards.get(card)
    }

    /// Where the card for the item with `key` is.
    fn find(&self, key: &RatingKey) -> Option<Retained<NSIndexPath>> {
        let Self::Shelves(shelves) = self else {
            return None;
        };
        let is_it = |card: &Card| card.item().is_some_and(|item| &item.rating_key == key);
        shelves.iter().enumerate().find_map(|(shelf, held)| {
            let card = held.cards.iter().position(is_it)?;
            Some(NSIndexPath::indexPathForItem_inSection(card.cast_signed(), shelf.cast_signed()))
        })
    }
}

/// When the card this close to the end of a grid is drawn, more are needed.
const NEAR_END: usize = 30;

const HEADING: &str = "heading";
const PAGE_HEADER: &str = "page-header";

/// The Return and Space keys, as `NSEvent::keyCode` reports them.
const KEY_RETURN: u16 = 36;
const KEY_SPACE: u16 = 49;
const KEY_LEFT: u16 = 123;
const KEY_RIGHT: u16 = 124;
const KEY_DOWN: u16 = 125;
const KEY_UP: u16 = 126;

pub struct Ivars {
    /// Holds the scroll view and, over it, the words for an empty list.
    root: Retained<NSView>,
    scroll: Retained<NSScrollView>,
    collection: Retained<ItemsView>,
    /// Shared with the layout, which asks it how each shelf is arranged.
    content: Rc<RefCell<Content>>,
    header: Option<Header>,
    /// Shown in the middle when there turned out to be nothing to list.
    nothing: Option<Notice>,
    images: Rc<Images>,
    on_action: Box<OnAction>,
    on_near_end: Box<dyn Fn()>,
    /// Set from asking for more cards until they arrive, so that one page
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
        fn card_count(&self, _collection: &NSCollectionView, shelf: NSInteger) -> NSInteger {
            let shelf = usize::try_from(shelf).unwrap_or(usize::MAX);
            self.ivars().content.borrow().card_count(shelf).cast_signed()
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
                _ => self.fill_heading(&view, index_path),
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
            self.open(&index_path, on_play);
        }
    }

    impl Posters {
        #[unsafe(method(menuOpen:))]
        fn menu_open(&self, _sender: Option<&AnyObject>) {
            self.act_on_menu_item(Action::Open);
        }

        #[unsafe(method(menuPlay:))]
        fn menu_play(&self, _sender: Option<&AnyObject>) {
            self.act_on_menu_item(Action::Play);
        }

        #[unsafe(method(menuToggleWatched:))]
        fn menu_toggle_watched(&self, _sender: Option<&AnyObject>) {
            self.act_on_menu_item(|item| {
                let watched = !item.is_watched();
                Action::SetWatched(item, watched)
            });
        }
    }
);

impl Posters {
    /// `outlines` is what to draw until [`show`](Self::show) is first
    /// called, and `nothing` what to say if it is then called with no
    /// cards. `on_action` is called when the viewer clicks a card or a
    /// heading, or picks from a card's menu. `on_near_end` is called when a
    /// grid is scrolled close to its last card, as the cue to
    /// [`append`](Self::append) more.
    pub fn new(
        main_thread: MainThreadMarker,
        images: Rc<Images>,
        header: Option<Header>,
        outlines: Vec<Outline>,
        nothing: Option<Notice>,
        on_action: impl Fn(Action) + 'static,
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
        // The collection hears of every scroll through these, to keep the
        // pointer's highlight on the card under the pointer.
        scroll.contentView().setPostsBoundsChangedNotifications(true);
        // SAFETY: the selector is one `ItemsView` implements, taking the
        // notification. The centre drops an observer that is freed.
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &collection,
                sel!(scrolled:),
                Some(NSViewBoundsDidChangeNotification),
                None,
            );
        }

        // Beside the scroll view, not inside it: a scroll view places its
        // own subviews and leaves any other where it was made, at no size.
        let root = NSView::new(main_thread);
        layout::fill(&root, &scroll);
        if let Some(nothing) = &nothing {
            nothing.view().setHidden(true);
            layout::float(&root, nothing.view());
        }

        let this = Self::alloc(main_thread).set_ivars(Ivars {
            root,
            scroll,
            collection,
            content,
            header,
            nothing,
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
        &self.ivars().root
    }

    /// Gives the keyboard to the collection, so the arrow keys move in it.
    pub fn focus(&self) {
        if let Some(window) = self.ivars().collection.window() {
            window.makeFirstResponder(Some(&self.ivars().collection));
        }
    }

    /// Replaces everything shown, disturbing as little as it can. While
    /// the shelves stay the ones they were, only those whose cards changed
    /// are laid out again, shelves that are new are added below, and the
    /// cards of the others are brought up to date where they are: nothing
    /// moves and nothing flashes.
    pub fn show(&self, shelves: Vec<Shelf>) {
        let ivars = self.ivars();
        ivars.awaiting_more.set(false);
        if let Some(nothing) = &ivars.nothing {
            nothing.view().setHidden(shelves.iter().any(|shelf| !shelf.cards.is_empty()));
        }
        let changes = changes(&ivars.content.borrow(), &shelves);
        let Some(changes) = changes else {
            *ivars.content.borrow_mut() = Content::Shelves(shelves);
            ivars.collection.reloadData();
            return;
        };

        // The collection view counts its shelves and cards lazily. Asking
        // it now makes it count before they change: counting afterwards, it
        // would take the new numbers for the old ones and refuse the update.
        let _ = ivars.collection.numberOfSections();
        *ivars.content.borrow_mut() = Content::Shelves(shelves);
        if !changes.is_empty() {
            let collection = ivars.collection.clone();
            let update = RcBlock::new(move || {
                collection.reloadSections(&index_set(&changes.reloaded));
                collection.deleteSections(&index_range(&changes.removed));
                collection.insertSections(&index_range(&changes.added));
            });
            ivars.collection.performBatchUpdates_completionHandler(Some(&update), None);
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
        // As in `show`: the collection view has to count before the list
        // grows.
        let _ = ivars.collection.numberOfItemsInSection(last.cast_signed());
        let added: Vec<Retained<NSIndexPath>> = (start..start + items.len())
            .map(|card| {
                NSIndexPath::indexPathForItem_inSection(card.cast_signed(), last.cast_signed())
            })
            .collect();
        if let Content::Shelves(shelves) = &mut *ivars.content.borrow_mut()
            && let Some(shelf) = shelves.last_mut()
        {
            shelf.cards.extend(items.into_iter().map(Card::Item));
        }
        // Inserting leaves the cells on screen and the scroll position
        // alone, which reloading everything would not.
        ivars.collection.insertItemsAtIndexPaths(&NSSet::from_retained_slice(&added));
        ivars.awaiting_more.set(false);
    }

    /// Hears that the cards asked for through `on_near_end` did not come.
    /// The next card drawn near the end asks for them again.
    pub fn more_failed(&self) {
        self.ivars().awaiting_more.set(false);
    }

    /// How many cards are shown, across all shelves.
    pub fn len(&self) -> usize {
        let content = self.ivars().content.borrow();
        match &*content {
            Content::Outlines(_) => 0,
            Content::Shelves(shelves) => shelves.iter().map(|shelf| shelf.cards.len()).sum(),
        }
    }

    /// Scrolls so that the content `y` points down is at the top of what
    /// can be seen, which is under the toolbar.
    pub fn scroll_to(&self, y: f64) {
        self.lay_out();
        let under_toolbar = self.ivars().scroll.contentInsets().top;
        self.ivars().collection.scrollPoint(NSPoint { x: 0.0, y: y - under_toolbar });
    }

    /// Gives everything its place now. What was just shown has no place
    /// to scroll to until then, and a screen that was just put in the
    /// window has no size, which only the window can give it.
    fn lay_out(&self) {
        if let Some(window) = self.ivars().scroll.window() {
            window.layoutIfNeeded();
        }
        self.ivars().collection.layoutSubtreeIfNeeded();
    }

    /// Puts the keyboard on the card for the item with `key`, scrolling
    /// up or down no further than it takes to bring the card into view.
    pub fn select(&self, key: &RatingKey) {
        let Some(index_path) = self.ivars().content.borrow().find(key) else {
            return;
        };
        self.ivars().collection.selectItemsAtIndexPaths_scrollPosition(
            &NSSet::from_retained_slice(&[index_path]),
            NSCollectionViewScrollPosition::NearestVerticalEdge,
        );
    }

    /// Scrolls the shelf that holds the item with `key` sideways until its
    /// card is in the middle.
    pub fn reveal(&self, key: &RatingKey) {
        let Some(index_path) = self.ivars().content.borrow().find(key) else {
            return;
        };
        self.lay_out();
        self.ivars().collection.scrollToItemsAtIndexPaths_scrollPosition(
            &NSSet::from_retained_slice(&[index_path]),
            NSCollectionViewScrollPosition::CenteredHorizontally,
        );
        self.fill_scrolled_shelves();
    }

    /// A shelf scrolled by the program, not by the viewer, does not fetch
    /// the cards that came into view by itself. Laying the collection out
    /// again does.
    fn fill_scrolled_shelves(&self) {
        let collection = &self.ivars().collection;
        if let Some(layout) = collection.collectionViewLayout() {
            layout.invalidateLayout();
        }
        collection.layoutSubtreeIfNeeded();
    }

    /// Puts the pointer's highlight on the first card, for a snapshot.
    pub fn hover_first(&self) {
        let first = NSIndexPath::indexPathForItem_inSection(0, 0);
        if let Some(cell) = self.ivars().collection.itemAtIndexPath(&first)
            && let Some(view) = cell.view().downcast_ref::<HoverView>()
        {
            view.hovered(true);
        }
    }

    /// Presses `press` in the collection as the keyboard would, for a
    /// snapshot.
    pub fn press(&self, press: Press) {
        let (code, character) = press.code_and_character();
        let characters: String = char::from_u32(character).into_iter().collect();
        let characters = NSString::from_str(&characters);
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,
            NSPoint::ZERO,
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            &characters,
            &characters,
            false,
            code,
        );
        if let Some(event) = event {
            self.ivars().collection.keyDown(&event);
        }
    }

    /// Scrolls the first shelf `x` points sideways, for a snapshot.
    pub fn scroll_first_shelf_to(&self, x: f64) {
        if let Some(shelf) = first_scroller_in(&self.ivars().collection) {
            let clip = shelf.contentView();
            clip.scrollToPoint(NSPoint { x, y: clip.bounds().origin.y });
            shelf.reflectScrolledClipView(&clip);
            self.fill_scrolled_shelves();
        }
    }

    /// Tells the owner what the viewer asked for.
    fn act(&self, action: Action) {
        // What the handler shows next may replace this collection, which
        // would free it while this method is still running.
        let _alive = self.retain();
        (self.ivars().on_action)(action);
    }

    /// Does what a click or the Return key does to the card at
    /// `index_path`. `on_play` says the click was on the play glyph.
    fn open(&self, index_path: &NSIndexPath, on_play: bool) {
        let (shelf, card) = position(index_path);
        // Cloned so the handler is free to change what is shown.
        let card = self.ivars().content.borrow().card(shelf, card).cloned();
        match card {
            Some(Card::Item(item)) if on_play => self.act(Action::Play(item)),
            Some(Card::Item(item)) => self.act(Action::Open(item)),
            Some(Card::Person(person)) => self.act(Action::OpenPerson(person)),
            None => {}
        }
    }

    fn act_on_menu_item(&self, action: impl Fn(Item) -> Action) {
        let item = self.ivars().menu_item.borrow_mut().take();
        if let Some(item) = item {
            self.act(action(item));
        }
    }

    fn fill(&self, cell: &NSCollectionViewItem, index_path: &NSIndexPath) {
        let ivars = self.ivars();
        let (shelf, index) = position(index_path);
        let content = ivars.content.borrow();
        let (Some(held), Some(card)) = (content.shelf(shelf), content.card(shelf, index)) else {
            // An outline: a cell with nothing in it yet, and nothing after
            // it to ask for.
            return;
        };
        let is_current =
            card.item().is_some_and(|item| held.current.as_ref() == Some(&item.rating_key));
        match (held.style, card) {
            (Style::List, Card::Item(item)) => {
                if let Some(cell) = cell.downcast_ref::<EpisodeItem>() {
                    cell.show(item, &ivars.images);
                }
            }
            (Style::Shelf(shape) | Style::Grid(shape), card) => {
                if let Some(cell) = cell.downcast_ref::<PosterItem>() {
                    cell.show(card, shape, is_current, &ivars.images);
                }
            }
            // A list is of episodes only.
            (Style::List, Card::Person(_)) => {}
        }

        let is_grid = matches!(held.style, Style::Grid(_));
        let left = held.cards.len().saturating_sub(index);
        drop(content);
        if is_grid && left <= NEAR_END && !ivars.awaiting_more.replace(true) {
            (ivars.on_near_end)();
        }
    }

    fn fill_heading(&self, strip: &NSView, index_path: &NSIndexPath) {
        let (shelf, _) = position(index_path);
        let content = self.ivars().content.borrow();
        let title = content.title(shelf).to_owned();
        let more = content.shelf(shelf).and_then(|shelf| shelf.more.clone());
        drop(content);
        let on_more = more.map(|path| -> Box<dyn Fn()> {
            let (posters, title) = (Weak::new(self), title.clone());
            Box::new(move || {
                if let Some(posters) = posters.load() {
                    posters.act(Action::OpenList { title: title.clone(), path: path.clone() });
                }
            })
        });
        heading::fill(strip, &title, on_more, self.mtm());
    }

    /// Return and Space open the card the keyboard is on. An arrow key
    /// pressed while it is on none puts it on the first card in view:
    /// the collection view only ever moves a selection it already has.
    fn key_pressed(&self, event: &NSEvent) -> bool {
        let collection = &self.ivars().collection;
        let selected = collection.selectionIndexPaths().anyObject();
        match (event.keyCode(), selected) {
            (KEY_RETURN | KEY_SPACE, Some(index_path)) => self.open(&index_path, false),
            (KEY_LEFT | KEY_RIGHT | KEY_UP | KEY_DOWN, None) => {
                let Some(first) = first_visible(collection) else {
                    return false;
                };
                collection.selectItemsAtIndexPaths_scrollPosition(
                    &NSSet::from_retained_slice(&[first]),
                    NSCollectionViewScrollPosition::NearestVerticalEdge,
                );
            }
            _ => return false,
        }
        true
    }

    /// The menu for a secondary click at the place of `event`.
    fn menu_at(&self, event: &NSEvent) -> Option<Retained<NSMenu>> {
        let collection = &self.ivars().collection;
        let point = collection.convertPoint_fromView(event.locationInWindow(), None);
        let index_path = collection.indexPathForItemAtPoint(point)?;
        let (shelf, card) = position(&index_path);
        let item = self.ivars().content.borrow().card(shelf, card)?.item()?.clone();

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
        // A trailer has no page to open and no place in what was watched.
        if item.kind != Kind::Clip {
            add("Open", sel!(menuOpen:));
            menu.addItem(&NSMenuItem::separatorItem(self.mtm()));
            let toggle = if item.is_watched() { "Mark as Unwatched" } else { "Mark as Watched" };
            add(toggle, sel!(menuToggleWatched:));
        }
        *self.ivars().menu_item.borrow_mut() = Some(item);
        Some(menu)
    }
}

/// The shelf and the place in it that `index_path` names.
fn position(index_path: &NSIndexPath) -> (usize, usize) {
    let part = |number: NSInteger| usize::try_from(number).unwrap_or(usize::MAX);
    (part(index_path.section()), part(index_path.item()))
}

/// The card in view that comes first in reading order.
fn first_visible(collection: &NSCollectionView) -> Option<Retained<NSIndexPath>> {
    collection.indexPathsForVisibleItems().iter().min_by_key(|index_path| position(index_path))
}

fn is_on_play(cell: &NSCollectionViewItem, point: NSPoint) -> bool {
    match (cell.downcast_ref::<PosterItem>(), cell.downcast_ref::<EpisodeItem>()) {
        (Some(card), _) => card.is_on_play(point),
        (_, Some(row)) => row.is_on_play(point),
        _ => false,
    }
}

fn index_set(indexes: &[usize]) -> Retained<NSMutableIndexSet> {
    let set = NSMutableIndexSet::new();
    for index in indexes {
        set.addIndex(*index);
    }
    set
}

fn index_range(range: &Range<usize>) -> Retained<NSIndexSet> {
    NSIndexSet::indexSetWithIndexesInRange(NSRange::new(range.start, range.len()))
}

/// The first scroll view inside `view`, searching depth first. Inside a
/// collection those are the shelves that scroll sideways.
fn first_scroller_in(view: &NSView) -> Option<Retained<NSScrollView>> {
    view.subviews().iter().find_map(|child| match child.downcast::<NSScrollView>() {
        Ok(scroller) => Some(scroller),
        Err(child) => first_scroller_in(&child),
    })
}

/// What turns the shelves on screen into other ones without starting over.
#[derive(Debug, PartialEq)]
struct Changes {
    /// Shelves that stay but hold other cards, or the same in another
    /// order, or lead somewhere else.
    reloaded: Vec<usize>,
    /// Shelves past the end of the new ones.
    removed: Range<usize>,
    /// Shelves past the end of the old ones.
    added: Range<usize>,
}

impl Changes {
    fn is_empty(&self) -> bool {
        self.reloaded.is_empty() && self.removed.is_empty() && self.added.is_empty()
    }
}

/// How to get from `old` to `new`. `None` when a shelf that both have
/// differs in title or arrangement, or when `old` was only outlines: then
/// nothing on screen can stay.
fn changes(old: &Content, new: &[Shelf]) -> Option<Changes> {
    let Content::Shelves(old) = old else {
        return None;
    };
    let same_shelf =
        |(old, new): (&Shelf, &Shelf)| old.title == new.title && old.style == new.style;
    if !old.iter().zip(new).all(same_shelf) {
        return None;
    }
    let same_cards = |old: &Shelf, new: &Shelf| {
        let mut pairs = old.cards.iter().zip(&new.cards);
        old.cards.len() == new.cards.len()
            && old.more == new.more
            && pairs.all(|(old, new)| old.is_same(new))
    };
    let shared = old.iter().zip(new).enumerate();
    Some(Changes {
        reloaded: shared
            .filter(|(_, (old, new))| !same_cards(old, new))
            .map(|(at, _)| at)
            .collect(),
        removed: new.len().min(old.len())..old.len(),
        added: old.len().min(new.len())..new.len(),
    })
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

        /// Each shelf that scrolls sideways sits in a scroll view that the
        /// layout makes, and that paints the window's colour behind its
        /// cards: a dark band across a page that has colours of its own.
        #[unsafe(method(layout))]
        fn layout(&self) {
            // SAFETY: forwards the call unchanged.
            let () = unsafe { msg_send![super(self), layout] };
            for view in self.subviews() {
                if let Ok(shelf) = view.downcast::<NSScrollView>() {
                    shelf.setDrawsBackground(false);
                }
            }
        }

        /// Some view scrolled. If it was this collection, up and down, or
        /// one of its shelves, sideways, the cards moved under a pointer
        /// that stayed where it was, and the highlight has to move to the
        /// card now under it. AppKit reports entering and leaving only when
        /// the pointer moves.
        #[unsafe(method(scrolled:))]
        fn scrolled(&self, notification: &NSNotification) {
            let moved = notification.object().and_then(|view| view.downcast::<NSView>().ok());
            let is_ours =
                moved.is_some_and(|view| view.isDescendantOf(self) || self.isDescendantOf(&view));
            let (true, Some(window)) = (is_ours, self.window()) else {
                return;
            };
            let pointer = window.mouseLocationOutsideOfEventStream();
            for cell in self.visibleItems() {
                if let Some(view) = cell.view().downcast_ref::<HoverView>() {
                    view.follow(pointer);
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

/// The space around a shelf's cards, as `NSDirectionalEdgeInsets` lays it
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
        Shelf::of(title, Style::Shelf(Shape::Poster), keys.iter().map(item).collect())
    }

    fn reloaded(old: &Content, new: &[Shelf]) -> Option<Vec<usize>> {
        changes(old, new).map(|changes| changes.reloaded)
    }

    #[test]
    fn only_shelves_with_other_cards_are_laid_out_again() {
        let old = Content::Shelves(vec![shelf("a", &["1", "2"]), shelf("b", &["3"])]);

        let same = [shelf("a", &["1", "2"]), shelf("b", &["3"])];
        assert!(changes(&old, &same).unwrap().is_empty());
        let reordered = [shelf("a", &["2", "1"]), shelf("b", &["3"])];
        assert_eq!(reloaded(&old, &reordered), Some(vec![0]));
        let longer = [shelf("a", &["1", "2"]), shelf("b", &["3", "4"])];
        assert_eq!(reloaded(&old, &longer), Some(vec![1]));
        let mut leads_on = same.clone();
        leads_on[1].more = Some("/path".to_owned());
        assert_eq!(reloaded(&old, &leads_on), Some(vec![1]));

        let renamed = [shelf("a", &["1", "2"]), shelf("c", &["3"])];
        assert_eq!(changes(&old, &renamed), None);
        assert_eq!(changes(&Content::Outlines(Vec::new()), &same), None);
    }

    #[test]
    fn shelves_are_added_and_removed_at_the_end() {
        let old = Content::Shelves(vec![shelf("a", &["1"]), shelf("b", &["2"])]);

        let more = [shelf("a", &["1"]), shelf("b", &["2"]), shelf("c", &["3"])];
        let expected = Changes { reloaded: vec![], removed: 2..2, added: 2..3 };
        assert_eq!(changes(&old, &more), Some(expected));
        let fewer = [shelf("a", &["9"])];
        let expected = Changes { reloaded: vec![0], removed: 1..2, added: 1..1 };
        assert_eq!(changes(&old, &fewer), Some(expected));
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
