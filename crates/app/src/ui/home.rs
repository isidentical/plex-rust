//! The home screen: one horizontally scrolling shelf per hub.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSCollectionElementKindSectionHeader, NSCollectionLayoutBoundarySupplementaryItem,
    NSCollectionLayoutDimension, NSCollectionLayoutGroup, NSCollectionLayoutItem,
    NSCollectionLayoutSection, NSCollectionLayoutSectionOrthogonalScrollingBehavior,
    NSCollectionLayoutSize, NSCollectionView, NSCollectionViewCompositionalLayout,
    NSCollectionViewDataSource, NSCollectionViewItem, NSCollectionViewSupplementaryElementKind,
    NSColor, NSDirectionalEdgeInsets, NSFont, NSFontWeightBold,
    NSIndexPathNSCollectionViewAdditions, NSRectAlignment, NSScrollView, NSTextField, NSView,
};
use objc2_foundation::{
    NSArray, NSIndexPath, NSInteger, NSObject, NSObjectProtocol, NSString, ns_string,
};
use plex::model::Hub;

use super::poster::PosterItem;
use super::theme;
use crate::images::Images;

pub struct Ivars {
    scroll: Retained<NSScrollView>,
    collection: Retained<NSCollectionView>,
    hubs: RefCell<Vec<Hub>>,
    images: Rc<Images>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Home` does not
    // implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ReelHome"]
    #[ivars = Ivars]
    pub struct Home;

    unsafe impl NSObjectProtocol for Home {}

    unsafe impl NSCollectionViewDataSource for Home {
        #[unsafe(method(numberOfSectionsInCollectionView:))]
        fn shelf_count(&self, _collection: &NSCollectionView) -> NSInteger {
            self.ivars().hubs.borrow().len().cast_signed()
        }

        #[unsafe(method(collectionView:numberOfItemsInSection:))]
        fn item_count(&self, _collection: &NSCollectionView, shelf: NSInteger) -> NSInteger {
            let hubs = self.ivars().hubs.borrow();
            let hub = usize::try_from(shelf)
                .ok()
                .and_then(|shelf| hubs.get(shelf));
            hub.map_or(0, |hub| hub.items.len().cast_signed())
        }

        #[unsafe(method_id(collectionView:itemForRepresentedObjectAtIndexPath:))]
        fn item(
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
);

impl Home {
    pub fn new(main_thread: MainThreadMarker, images: Rc<Images>) -> Retained<Self> {
        let collection = NSCollectionView::new(main_thread);
        collection.setCollectionViewLayout(Some(&shelves_layout(main_thread)));
        collection.setBackgroundColors(Some(&NSArray::from_retained_slice(&[
            NSColor::clearColor(),
        ])));
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
            hubs: RefCell::new(Vec::new()),
            images,
        });
        // SAFETY: `init` is NSObject's designated initializer.
        let home: Retained<Self> = unsafe { msg_send![super(this), init] };
        // The collection view does not retain its data source. Whoever owns
        // `Home` keeps it alive while its view is on screen.
        home.ivars()
            .collection
            .setDataSource(Some(ProtocolObject::from_ref(&*home)));
        home
    }

    pub fn view(&self) -> &NSView {
        &self.ivars().scroll
    }

    pub fn show(&self, hubs: Vec<Hub>) {
        *self.ivars().hubs.borrow_mut() = hubs;
        self.ivars().collection.reloadData();
    }

    fn fill(&self, cell: &NSCollectionViewItem, index_path: &NSIndexPath) {
        let hubs = self.ivars().hubs.borrow();
        let item = usize::try_from(index_path.section())
            .ok()
            .and_then(|shelf| hubs.get(shelf))
            .zip(usize::try_from(index_path.item()).ok())
            .and_then(|(hub, index)| hub.items.get(index));
        if let (Some(item), Some(cell)) = (item, cell.downcast_ref::<PosterItem>()) {
            cell.show(item, &self.ivars().images);
        }
    }

    fn title(&self, view: &NSView, index_path: &NSIndexPath) {
        let hubs = self.ivars().hubs.borrow();
        let hub = usize::try_from(index_path.section())
            .ok()
            .and_then(|shelf| hubs.get(shelf));
        let (Some(hub), Some(label)) = (hub, view.downcast_ref::<NSTextField>()) else {
            return;
        };
        // The collection view creates a plain text field. Make it a label.
        label.setBezeled(false);
        label.setDrawsBackground(false);
        label.setEditable(false);
        label.setSelectable(false);
        // SAFETY: the weight is a constant that AppKit provides.
        let bold = unsafe { NSFontWeightBold };
        label.setFont(Some(&NSFont::systemFontOfSize_weight(
            theme::HEADING_FONT_SIZE,
            bold,
        )));
        label.setStringValue(&NSString::from_str(&hub.title));
    }
}

/// Every shelf is one row of posters that scrolls sideways under a heading.
fn shelves_layout(main_thread: MainThreadMarker) -> Retained<NSCollectionViewCompositionalLayout> {
    let absolute = |points| NSCollectionLayoutDimension::absoluteDimension(points, main_thread);
    let poster = NSCollectionLayoutSize::sizeWithWidthDimension_heightDimension(
        &absolute(f64::from(theme::POSTER_WIDTH)),
        &absolute(theme::POSTER_CELL_HEIGHT),
    );
    let item = NSCollectionLayoutItem::itemWithLayoutSize(&poster);
    let group = NSCollectionLayoutGroup::horizontalGroupWithLayoutSize_subitems(
        &poster,
        &NSArray::from_retained_slice(&[item]),
    );

    let heading_size = NSCollectionLayoutSize::sizeWithWidthDimension_heightDimension(
        &NSCollectionLayoutDimension::fractionalWidthDimension(1.0, main_thread),
        &absolute(theme::SHELF_HEADING_HEIGHT),
    );
    // SAFETY: the element kind is a constant that AppKit provides.
    let heading = NSCollectionLayoutBoundarySupplementaryItem::boundarySupplementaryItemWithLayoutSize_elementKind_alignment(
        &heading_size,
        unsafe { NSCollectionElementKindSectionHeader },
        NSRectAlignment::Top,
    );

    let shelf = NSCollectionLayoutSection::sectionWithGroup(&group);
    shelf.setOrthogonalScrollingBehavior(
        NSCollectionLayoutSectionOrthogonalScrollingBehavior::Continuous,
    );
    shelf.setInterGroupSpacing(theme::SHELF_SPACING);
    shelf.setContentInsets(NSDirectionalEdgeInsets {
        top: 0.0,
        leading: theme::PAGE_MARGIN,
        bottom: theme::PAGE_MARGIN,
        trailing: theme::PAGE_MARGIN,
    });
    shelf.setBoundarySupplementaryItems(&NSArray::from_retained_slice(&[heading]));

    NSCollectionViewCompositionalLayout::initWithSection(
        NSCollectionViewCompositionalLayout::alloc(main_thread),
        &shelf,
    )
}
