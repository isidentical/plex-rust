//! The list of places on the left: Home, the libraries, Downloads.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSControlTextEditingDelegate, NSImage, NSImageView, NSScrollView, NSStackView, NSTableColumn,
    NSTableView, NSTableViewDataSource, NSTableViewDelegate, NSTableViewStyle, NSTextField, NSView,
};
use objc2_foundation::{
    NSArray, NSIndexSet, NSInteger, NSNotification, NSObject, NSObjectProtocol, NSString, ns_string,
};
use plex::model::{Kind, Section};

use super::theme;

/// Somewhere the sidebar can take the user.
#[derive(Debug, Clone)]
pub enum Destination {
    Home,
    Section(Section),
    Downloads,
}

impl Destination {
    pub fn title(&self) -> &str {
        match self {
            Self::Home => "Home",
            Self::Section(section) => &section.title,
            Self::Downloads => "Downloads",
        }
    }

    /// The SF Symbol shown next to the title.
    fn symbol(&self) -> &'static str {
        match self {
            Self::Home => "house",
            Self::Section(section) if section.kind == Kind::Show => "tv",
            Self::Section(_) => "film",
            Self::Downloads => "arrow.down.circle",
        }
    }
}

pub struct Ivars {
    scroll: Retained<NSScrollView>,
    table: Retained<NSTableView>,
    destinations: RefCell<Vec<Destination>>,
    on_select: Box<dyn Fn(&Destination)>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Sidebar` does
    // not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustSidebar"]
    #[ivars = Ivars]
    pub struct Sidebar;

    unsafe impl NSObjectProtocol for Sidebar {}

    unsafe impl NSTableViewDataSource for Sidebar {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn row_count(&self, _table: &NSTableView) -> NSInteger {
            self.ivars().destinations.borrow().len().cast_signed()
        }
    }

    unsafe impl NSControlTextEditingDelegate for Sidebar {}

    unsafe impl NSTableViewDelegate for Sidebar {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn row_view(
            &self,
            _table: &NSTableView,
            _column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<NSView>> {
            self.view_for_row(row)
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, _notification: &NSNotification) {
            let row = self.ivars().table.selectedRow();
            let destinations = self.ivars().destinations.borrow();
            let selected = usize::try_from(row).ok().and_then(|row| destinations.get(row));
            if let Some(destination) = selected {
                (self.ivars().on_select)(destination);
            }
        }
    }
);

impl Sidebar {
    /// `on_select` is called whenever the selected destination changes.
    pub fn new(
        main_thread: MainThreadMarker,
        on_select: impl Fn(&Destination) + 'static,
    ) -> Retained<Self> {
        let table = NSTableView::new(main_thread);
        table.setStyle(NSTableViewStyle::SourceList);
        table.setHeaderView(None);
        table.setRowHeight(theme::SIDEBAR_ROW_HEIGHT);
        let column = NSTableColumn::initWithIdentifier(
            NSTableColumn::alloc(main_thread),
            ns_string!("destination"),
        );
        table.addTableColumn(&column);

        let scroll = NSScrollView::new(main_thread);
        scroll.setDocumentView(Some(&table));
        scroll.setHasVerticalScroller(true);
        scroll.setDrawsBackground(false);

        let this = Self::alloc(main_thread).set_ivars(Ivars {
            scroll,
            table,
            destinations: RefCell::new(Vec::new()),
            on_select: Box::new(on_select),
        });
        // SAFETY: `init` is NSObject's designated initializer.
        let sidebar: Retained<Self> = unsafe { msg_send![super(this), init] };

        // SAFETY: the table does not retain these. Whoever owns the sidebar
        // keeps it alive for as long as its view is on screen.
        unsafe {
            sidebar.ivars().table.setDataSource(Some(ProtocolObject::from_ref(&*sidebar)));
            sidebar.ivars().table.setDelegate(Some(ProtocolObject::from_ref(&*sidebar)));
        }
        sidebar
    }

    fn view_for_row(&self, row: NSInteger) -> Option<Retained<NSView>> {
        let destinations = self.ivars().destinations.borrow();
        let destination = destinations.get(usize::try_from(row).ok()?)?;
        Some(row_view(destination, self.mtm()))
    }

    pub fn view(&self) -> &NSView {
        &self.ivars().scroll
    }

    /// Replaces the list and selects the entry at `selected`.
    pub fn show(&self, destinations: Vec<Destination>, selected: usize) {
        *self.ivars().destinations.borrow_mut() = destinations;
        let table = &self.ivars().table;
        table.reloadData();
        let selected = NSIndexSet::indexSetWithIndex(selected);
        table.selectRowIndexes_byExtendingSelection(&selected, false);
    }
}

fn row_view(destination: &Destination, main_thread: MainThreadMarker) -> Retained<NSView> {
    let label = NSTextField::labelWithString(&NSString::from_str(destination.title()), main_thread);
    let mut views: Vec<Retained<NSView>> = Vec::new();

    let symbol = NSString::from_str(destination.symbol());
    if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(&symbol, None)
    {
        views.push(Retained::into_super(Retained::into_super(NSImageView::imageViewWithImage(
            &image,
            main_thread,
        ))));
    }
    views.push(Retained::into_super(Retained::into_super(label)));

    let row = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&views), main_thread);
    row.setSpacing(theme::SIDEBAR_ICON_GAP);
    Retained::into_super(row)
}
