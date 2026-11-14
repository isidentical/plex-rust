//! The list of places on the left: Home, the libraries, Downloads.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSControlTextEditingDelegate, NSImage, NSImageView, NSLayoutConstraint,
    NSLineBreakMode, NSScrollView, NSTableCellView, NSTableColumn, NSTableView,
    NSTableViewDataSource, NSTableViewDelegate, NSTableViewStyle, NSTextField, NSView,
};
use objc2_foundation::{
    NSArray, NSIndexSet, NSInteger, NSNotification, NSObject, NSObjectProtocol, NSString, ns_string,
};
use plex::model::{Kind, Section};

use super::theme;

/// Somewhere the sidebar can take the user.
#[derive(Debug, Clone, PartialEq)]
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

    /// Replaces the list. Nothing is selected until [`select`](Self::select).
    pub fn show(&self, destinations: Vec<Destination>) {
        *self.ivars().destinations.borrow_mut() = destinations;
        self.ivars().table.reloadData();
    }

    /// Selects the row of `destination`, or none when the place on screen
    /// is not one of the sidebar's. `on_select` hears of it like of a click.
    pub fn select(&self, destination: Option<&Destination>) {
        let row = destination.and_then(|destination| {
            self.ivars().destinations.borrow().iter().position(|row| row == destination)
        });
        let rows = match row {
            Some(row) => NSIndexSet::indexSetWithIndex(row),
            None => NSIndexSet::indexSet(),
        };
        self.ivars().table.selectRowIndexes_byExtendingSelection(&rows, false);
    }
}

/// A row as the system lays one out in a source list. The table sizes and
/// colours the cell's own image and text for the sidebar, for a selected
/// row and for a window that is not in front.
fn row_view(destination: &Destination, main_thread: MainThreadMarker) -> Retained<NSView> {
    let cell = NSTableCellView::new(main_thread);
    let label = NSTextField::labelWithString(&NSString::from_str(destination.title()), main_thread);
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    let glyph = NSImageView::new(main_thread);
    let symbol = NSString::from_str(destination.symbol());
    glyph.setImage(
        NSImage::imageWithSystemSymbolName_accessibilityDescription(&symbol, None).as_deref(),
    );
    // The title next to it says what the row is.
    glyph.setAccessibilityElement(false);

    for view in [&**glyph, &**label] {
        view.setTranslatesAutoresizingMaskIntoConstraints(false);
        cell.addSubview(view);
    }
    // SAFETY: the cell does not retain these. They are its subviews, so
    // they live as long as it does.
    unsafe {
        cell.setImageView(Some(&glyph));
        cell.setTextField(Some(&label));
    }
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        glyph.leadingAnchor().constraintEqualToAnchor(&cell.leadingAnchor()),
        glyph.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
        glyph.widthAnchor().constraintEqualToConstant(theme::SIDEBAR_ICON_WIDTH),
        label
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&glyph.trailingAnchor(), theme::SIDEBAR_ICON_GAP),
        label.trailingAnchor().constraintEqualToAnchor(&cell.trailingAnchor()),
        label.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
    ]));
    Retained::into_super(cell)
}
