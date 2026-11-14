//! A scrolling collection of library items, used for every screen that
//! lists them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
};
use objc2_app_kit::{
    NSCollectionElementKindSectionHeader, NSCollectionLayoutBoundarySupplementaryItem,
    NSCollectionLayoutDimension, NSCollectionLayoutGroup, NSCollectionLayoutItem,
    NSCollectionLayoutSection, NSCollectionLayoutSectionOrthogonalScrollingBehavior,
    NSCollectionLayoutSize, NSCollectionLayoutSpacing, NSCollectionView,
    NSCollectionViewCompositionalLayout, NSCollectionViewDataSource, NSCollectionViewDelegate,
    NSCollectionViewItem, NSCollectionViewSupplementaryElementKind, NSColor,
    NSDirectionalEdgeInsets, NSFont, NSFontWeightBold, NSIndexPathNSCollectionViewAdditions,
    NSRectAlignment, NSScrollView, NSTextField, NSView,
};
use objc2_foundation::{
    NSArray, NSIndexPath, NSInteger, NSObject, NSObjectProtocol, NSSet, NSString, ns_string,
};
use plex::model::Item;

use super::poster::{PosterItem, Shape};
use super::theme;
use crate::images::Images;

/// How the items are arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrangement {
    /// One row per shelf, scrolling sideways under the shelf's title.
    Shelves,
    /// Rows that wrap to the width of the view, scrolling down.
    Grid(Shape),
}

/// A titled group of items.
#[derive(Debug, Clone)]
pub struct Shelf {
    pub title: String,
    pub items: Vec<Item>,
}

/// When the item this close to the end of a grid is drawn, more are needed.
const NEAR_END: usize = 20;

pub struct Ivars {
    scroll: Retained<NSScrollView>,
    collection: Retained<NSCollectionView>,
    arrangement: Arrangement,
    shelves: RefCell<Vec<Shelf>>,
    images: Rc<Images>,
    on_open: Box<dyn Fn(&Item)>,
    on_near_end: Box<dyn Fn()>,
    /// Set from asking for more items until they arrive, so that one page
    /// is not asked for twice.
    awaiting_more: Cell<bool>,
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
            self.ivars().shelves.borrow().len().cast_signed()
        }

        #[unsafe(method(collectionView:numberOfItemsInSection:))]
        fn item_count(&self, _collection: &NSCollectionView, shelf: NSInteger) -> NSInteger {
            let shelves = self.ivars().shelves.borrow();
            let shelf = usize::try_from(shelf).ok().and_then(|shelf| shelves.get(shelf));
            shelf.map_or(0, |shelf| shelf.items.len().cast_signed())
        }

        #[unsafe(method_id(collectionView:itemForRepresentedObjectAtIndexPath:))]
        fn cell(
            &self,
            collection: &NSCollectionView,
            index_path: &NSIndexPath,
        ) -> Retained<NSCollectionViewItem> {
            let cell = collection.makeItemWithIdentifier_forIndexPath(
                &NSString::from_str(PosterItem::IDENTIFIER),
                index_path,
            );
            self.fill(&cell, index_path);
            cell
        }

        #[unsafe(method_id(collectionView:viewForSupplementaryElementOfKind:atIndexPath:))]
        fn heading(
            &self,
            collection: &NSCollectionView,
            kind: &NSCollectionViewSupplementaryElementKind,
            index_path: &NSIndexPath,
        ) -> Retained<NSView> {
            let view = collection.makeSupplementaryViewOfKind_withIdentifier_forIndexPath(
                kind,
                ns_string!("heading"),
                index_path,
            );
            self.title(&view, index_path);
            view
        }
    }

    unsafe impl NSCollectionViewDelegate for Posters {
        #[unsafe(method(collectionView:didSelectItemsAtIndexPaths:))]
        fn selected(&self, collection: &NSCollectionView, index_paths: &NSSet<NSIndexPath>) {
            // Opening an item replaces this collection, which would free it
            // while this method is still running.
            let _alive = self.retain();
            // A click is an action here, not a selection to keep.
            // SAFETY: the sender is optional.
            unsafe { collection.deselectAll(None) };
            let Some(index_path) = index_paths.anyObject() else {
                return;
            };
            // Cloned so the handler is free to change what is shown.
            let item = self.item_at(&index_path);
            if let Some(item) = item {
                (self.ivars().on_open)(&item);
            }
        }
    }
);

impl Posters {
    /// `on_open` is called with an item when it is clicked. `on_near_end` is
    /// called when a grid is scrolled close to its last item, as the cue to
    /// [`append`](Self::append) more.
    pub fn new(
        main_thread: MainThreadMarker,
        images: Rc<Images>,
        arrangement: Arrangement,
        on_open: impl Fn(&Item) + 'static,
        on_near_end: impl Fn() + 'static,
    ) -> Retained<Self> {
        let collection = NSCollectionView::new(main_thread);
        collection.setCollectionViewLayout(Some(&layout(arrangement, main_thread)));
        collection
            .setBackgroundColors(Some(&NSArray::from_retained_slice(&[NSColor::clearColor()])));
        collection.setSelectable(true);
        // SAFETY: both classes are valid for their roles: an item class for
        // items, a view class for supplementary views.
        unsafe {
            collection.registerClass_forItemWithIdentifier(
                Some(PosterItem::class()),
                &NSString::from_str(PosterItem::IDENTIFIER),
            );
            collection.registerClass_forSupplementaryViewOfKind_withIdentifier(
                Some(NSTextField::class()),
                NSCollectionElementKindSectionHeader,
                ns_string!("heading"),
            );
        }

        let scroll = NSScrollView::new(main_thread);
        scroll.setDocumentView(Some(&collection));
        scroll.setHasVerticalScroller(true);
        scroll.setDrawsBackground(false);

        let this = Self::alloc(main_thread).set_ivars(Ivars {
            scroll,
            collection,
            arrangement,
            shelves: RefCell::new(Vec::new()),
            images,
            on_open: Box::new(on_open),
            on_near_end: Box::new(on_near_end),
            awaiting_more: Cell::new(false),
        });
        // SAFETY: `init` is NSObject's designated initializer.
        let posters: Retained<Self> = unsafe { msg_send![super(this), init] };
        // The collection view retains neither. Whoever owns `Posters` keeps
        // it alive while its view is on screen.
        let collection = &posters.ivars().collection;
        collection.setDataSource(Some(ProtocolObject::from_ref(&*posters)));
        collection.setDelegate(Some(ProtocolObject::from_ref(&*posters)));
        posters
    }

    pub fn view(&self) -> &NSView {
        &self.ivars().scroll
    }

    /// Replaces everything shown.
    pub fn show(&self, shelves: Vec<Shelf>) {
        self.ivars().awaiting_more.set(false);
        *self.ivars().shelves.borrow_mut() = shelves;
        self.ivars().collection.reloadData();
    }

    /// Adds `items` to the end of the last shelf. An empty `items` means
    /// there is nothing more, and no more will be asked for.
    pub fn append(&self, items: Vec<Item>) {
        if items.is_empty() {
            return;
        }
        self.ivars().awaiting_more.set(false);
        if let Some(shelf) = self.ivars().shelves.borrow_mut().last_mut() {
            shelf.items.extend(items);
        }
        self.ivars().collection.reloadData();
    }

    /// How many items are shown, across all shelves.
    pub fn len(&self) -> usize {
        self.ivars().shelves.borrow().iter().map(|shelf| shelf.items.len()).sum()
    }

    fn item_at(&self, index_path: &NSIndexPath) -> Option<Item> {
        let shelves = self.ivars().shelves.borrow();
        let shelf = shelves.get(usize::try_from(index_path.section()).ok()?)?;
        shelf.items.get(usize::try_from(index_path.item()).ok()?).cloned()
    }

    fn fill(&self, cell: &NSCollectionViewItem, index_path: &NSIndexPath) {
        let (Some(item), Some(cell)) =
            (self.item_at(index_path), cell.downcast_ref::<PosterItem>())
        else {
            return;
        };
        let shape = match self.ivars().arrangement {
            Arrangement::Shelves => Shape::Poster,
            Arrangement::Grid(shape) => shape,
        };
        cell.show(&item, shape, &self.ivars().images);

        let is_grid = matches!(self.ivars().arrangement, Arrangement::Grid(_));
        let drawn = usize::try_from(index_path.item()).unwrap_or(0);
        let near_end = self.len().saturating_sub(drawn) <= NEAR_END;
        if is_grid && near_end && !self.ivars().awaiting_more.replace(true) {
            (self.ivars().on_near_end)();
        }
    }

    fn title(&self, view: &NSView, index_path: &NSIndexPath) {
        let shelves = self.ivars().shelves.borrow();
        let shelf = usize::try_from(index_path.section()).ok().and_then(|shelf| shelves.get(shelf));
        let (Some(shelf), Some(label)) = (shelf, view.downcast_ref::<NSTextField>()) else {
            return;
        };
        // The collection view creates a plain text field. Make it a label.
        label.setBezeled(false);
        label.setDrawsBackground(false);
        label.setEditable(false);
        label.setSelectable(false);
        // SAFETY: the weight is a constant that AppKit provides.
        let bold = unsafe { NSFontWeightBold };
        label.setFont(Some(&NSFont::systemFontOfSize_weight(theme::HEADING_FONT_SIZE, bold)));
        label.setStringValue(&NSString::from_str(&shelf.title));
    }
}

fn layout(
    arrangement: Arrangement,
    main_thread: MainThreadMarker,
) -> Retained<NSCollectionViewCompositionalLayout> {
    let absolute = |points| NSCollectionLayoutDimension::absoluteDimension(points, main_thread);
    let full_width = NSCollectionLayoutDimension::fractionalWidthDimension(1.0, main_thread);
    let shape = match arrangement {
        Arrangement::Shelves => Shape::Poster,
        Arrangement::Grid(shape) => shape,
    };
    let (width, height) = shape.cell_size();
    let cell = NSCollectionLayoutSize::sizeWithWidthDimension_heightDimension(
        &absolute(width),
        &absolute(height),
    );
    let item = NSCollectionLayoutItem::itemWithLayoutSize(&cell);
    let margins = NSDirectionalEdgeInsets {
        top: theme::PAGE_MARGIN,
        leading: theme::PAGE_MARGIN,
        bottom: theme::PAGE_MARGIN,
        trailing: theme::PAGE_MARGIN,
    };

    let section = match arrangement {
        Arrangement::Shelves => {
            let group = NSCollectionLayoutGroup::horizontalGroupWithLayoutSize_subitems(
                &cell,
                &NSArray::from_retained_slice(&[item]),
            );
            let shelf = NSCollectionLayoutSection::sectionWithGroup(&group);
            shelf.setOrthogonalScrollingBehavior(
                NSCollectionLayoutSectionOrthogonalScrollingBehavior::Continuous,
            );
            shelf.setInterGroupSpacing(theme::CELL_SPACING);
            shelf.setContentInsets(NSDirectionalEdgeInsets { top: 0.0, ..margins });

            let heading_size = NSCollectionLayoutSize::sizeWithWidthDimension_heightDimension(
                &full_width,
                &absolute(theme::SHELF_HEADING_HEIGHT),
            );
            // SAFETY: the element kind is a constant that AppKit provides.
            let kind = unsafe { NSCollectionElementKindSectionHeader };
            let heading = NSCollectionLayoutBoundarySupplementaryItem::boundarySupplementaryItemWithLayoutSize_elementKind_alignment(
                &heading_size,
                kind,
                NSRectAlignment::Top,
            );
            shelf.setBoundarySupplementaryItems(&NSArray::from_retained_slice(&[heading]));
            shelf
        }
        Arrangement::Grid(_) => {
            // A row as wide as the view holds as many cells as fit.
            let row = NSCollectionLayoutSize::sizeWithWidthDimension_heightDimension(
                &full_width,
                &absolute(height),
            );
            let group = NSCollectionLayoutGroup::horizontalGroupWithLayoutSize_subitems(
                &row,
                &NSArray::from_retained_slice(&[item]),
            );
            let spacing = NSCollectionLayoutSpacing::fixedSpacing(theme::CELL_SPACING, main_thread);
            group.setInterItemSpacing(Some(&spacing));
            let grid = NSCollectionLayoutSection::sectionWithGroup(&group);
            grid.setInterGroupSpacing(theme::CELL_SPACING);
            grid.setContentInsets(margins);
            grid
        }
    };

    NSCollectionViewCompositionalLayout::initWithSection(
        NSCollectionViewCompositionalLayout::alloc(main_thread),
        &section,
    )
}
