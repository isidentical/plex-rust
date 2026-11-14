//! The list of what is kept on this Mac and what is on its way: one row a
//! download, with how far it has got and the buttons to play it, carry on
//! with it or delete it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSButton, NSColor, NSControlSize, NSControlTextEditingDelegate, NSImage, NSImageScaling,
    NSImageView, NSLayoutAttribute, NSLayoutConstraint, NSLayoutConstraintOrientation,
    NSLayoutPriorityDefaultLow, NSProgressIndicator, NSProgressIndicatorStyle, NSScrollView,
    NSStackView, NSTableColumn, NSTableView, NSTableViewColumnAutoresizingStyle,
    NSTableViewDataSource, NSTableViewDelegate, NSTableViewSelectionHighlightStyle,
    NSTableViewStyle, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{
    NSArray, NSEdgeInsets, NSInteger, NSObject, NSObjectProtocol, NSPoint, NSSize, NSString,
    ns_string,
};
use plex::model::{Item, RatingKey};

use super::artwork;
use super::callback::Callback;
use super::label::{self, Weight};
use super::layout;
use super::notice::{Mark, Notice};
use super::text;
use super::theme;
use crate::downloads::{Entry, State};

/// What the viewer asked for in a row.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    Play(RatingKey),
    /// Carry on with a download that stopped short.
    Resume(RatingKey),
    /// Delete a download, stopping it first if it is on its way.
    Remove(RatingKey),
    /// Look for the server again.
    Reconnect,
}

/// Which of its three looks a row has. One look differs from another in
/// its buttons, so a row that changes its look is built again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    Downloading,
    Incomplete,
    Downloaded,
}

impl Look {
    fn of(state: &State) -> Self {
        match state {
            State::Downloading(_) => Self::Downloading,
            State::Incomplete { .. } => Self::Incomplete,
            State::Downloaded => Self::Downloaded,
        }
    }

    /// The buttons of a row, as title and what each asks for.
    fn buttons(self, key: &RatingKey) -> Vec<(&'static str, Act)> {
        let key = || key.clone();
        match self {
            Self::Downloading => vec![("Cancel", Act::Remove(key()))],
            Self::Incomplete => {
                vec![("Resume", Act::Resume(key())), ("Delete", Act::Remove(key()))]
            }
            Self::Downloaded => vec![("Play", Act::Play(key())), ("Delete", Act::Remove(key()))],
        }
    }
}

struct Row {
    key: RatingKey,
    look: Look,
    view: Retained<NSView>,
    poster: Retained<NSImageView>,
    /// The poster arrives a moment after its download begins.
    has_poster: Cell<bool>,
    status: Retained<NSTextField>,
    bar: Retained<NSProgressIndicator>,
    buttons: Vec<Retained<NSButton>>,
    /// Kept because buttons do not retain their targets.
    _targets: Vec<Retained<Callback>>,
}

pub struct Ivars {
    root: Retained<NSView>,
    /// Holds the list, or `nothing` in its place.
    body: Retained<NSView>,
    scroll: Retained<NSScrollView>,
    table: Retained<NSTableView>,
    /// Shown in place of the list while it is empty.
    nothing: Notice,
    /// A strip across the top that says the server is away.
    away: Retained<NSView>,
    /// The strip's button, which looks for the server again.
    reconnect: Retained<NSButton>,
    rows: RefCell<Vec<Row>>,
    on_act: Rc<dyn Fn(Act)>,
    /// Kept because a button does not retain its target.
    _reconnect: Retained<Callback>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `DownloadList`
    // does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustDownloadList"]
    #[ivars = Ivars]
    pub struct DownloadList;

    unsafe impl NSObjectProtocol for DownloadList {}

    unsafe impl NSTableViewDataSource for DownloadList {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn row_count(&self, _table: &NSTableView) -> NSInteger {
            self.ivars().rows.borrow().len().cast_signed()
        }
    }

    unsafe impl NSControlTextEditingDelegate for DownloadList {}

    unsafe impl NSTableViewDelegate for DownloadList {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn row_view(
            &self,
            _table: &NSTableView,
            _column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<NSView>> {
            self.view_for_row(row)
        }
    }
);

impl DownloadList {
    /// An empty list. `on_act` is called with what the viewer asks for.
    pub fn new(main_thread: MainThreadMarker, on_act: impl Fn(Act) + 'static) -> Retained<Self> {
        let table = NSTableView::new(main_thread);
        table.setStyle(NSTableViewStyle::FullWidth);
        table.setHeaderView(None);
        table.setRowHeight(theme::DOWNLOAD_ROW_HEIGHT);
        table.setIntercellSpacing(NSSize::ZERO);
        table.setBackgroundColor(&NSColor::clearColor());
        // The rows are acted on through their buttons, not by choosing one.
        table.setSelectionHighlightStyle(NSTableViewSelectionHighlightStyle::None);
        table.setColumnAutoresizingStyle(
            NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle,
        );
        let column =
            NSTableColumn::initWithIdentifier(NSTableColumn::alloc(main_thread), ns_string!("row"));
        table.addTableColumn(&column);

        let scroll = NSScrollView::new(main_thread);
        scroll.setDocumentView(Some(&table));
        scroll.setHasVerticalScroller(true);
        scroll.setDrawsBackground(false);

        let nothing = Notice::new(
            main_thread,
            Mark::Symbol("arrow.down.circle"),
            "No downloads",
            "Download a film or an episode from its page to watch it without your server.",
            None,
        );
        let on_act: Rc<dyn Fn(Act)> = Rc::new(on_act);
        let pressed = Rc::clone(&on_act);
        let reconnect = Callback::new(main_thread, move |_| pressed(Act::Reconnect));
        let (away, reconnect_button) = away_strip(&reconnect, main_thread);
        away.setHidden(true);

        let (root, body) = (NSView::new(main_thread), NSView::new(main_thread));
        layout::fill(&root, &body);
        layout::centre(&body, nothing.view());
        root.addSubview(&away);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            away.topAnchor().constraintEqualToAnchor(&root.safeAreaLayoutGuide().topAnchor()),
            away.leadingAnchor().constraintEqualToAnchor(&root.leadingAnchor()),
            away.trailingAnchor().constraintEqualToAnchor(&root.trailingAnchor()),
            away.heightAnchor().constraintEqualToConstant(theme::DOWNLOAD_STRIP_HEIGHT),
        ]));

        let this = Self::alloc(main_thread).set_ivars(Ivars {
            root,
            body,
            scroll,
            table,
            nothing,
            away,
            reconnect: reconnect_button,
            rows: RefCell::new(Vec::new()),
            on_act,
            _reconnect: reconnect,
        });
        // SAFETY: `init` is NSObject's designated initializer.
        let screen: Retained<Self> = unsafe { msg_send![super(this), init] };
        // SAFETY: the table does not retain these. Whoever owns the screen
        // keeps it alive for as long as its view is on screen.
        unsafe {
            screen.ivars().table.setDataSource(Some(ProtocolObject::from_ref(&*screen)));
            screen.ivars().table.setDelegate(Some(ProtocolObject::from_ref(&*screen)));
        }
        screen
    }

    fn view_for_row(&self, row: NSInteger) -> Option<Retained<NSView>> {
        let rows = self.ivars().rows.borrow();
        Some(rows.get(usize::try_from(row).ok()?)?.view.clone())
    }

    pub fn view(&self) -> &NSView {
        &self.ivars().root
    }

    pub fn focus(&self) {
        if let Some(window) = self.ivars().table.window() {
            window.makeFirstResponder(Some(&self.ivars().table));
        }
    }

    /// Scrolls the list so that the content `y` points down is at the top.
    pub fn scroll_to(&self, y: f64) {
        let under_toolbar = self.ivars().scroll.contentInsets().top;
        self.ivars().table.scrollPoint(NSPoint { x: 0.0, y: y - under_toolbar });
    }

    /// Clicks the first button with `title` that is to be seen: the
    /// strip's, or one in the topmost row that has such a button. For a
    /// walk, which has no pointer. Returns whether there was one.
    pub fn press(&self, title: &str) -> bool {
        let ivars = self.ivars();
        let titled = |button: &&Retained<NSButton>| button.title().to_string() == title;
        let strip = (!ivars.away.isHidden()).then_some(&ivars.reconnect);
        // Cloned out, because the click may build the rows again.
        let button = {
            let rows = ivars.rows.borrow();
            strip.into_iter().chain(rows.iter().flat_map(|row| &row.buttons)).find(titled).cloned()
        };
        if let Some(button) = &button {
            // SAFETY: the sender may be nil, and the button's target is a
            // `Callback` that its row or the list keeps alive.
            unsafe { button.performClick(None) };
        }
        button.is_some()
    }

    /// Shows `entries`, and with `away` that the server is not there.
    /// While the entries are the downloads already listed, each in the
    /// state it was in, only the progress in their rows changes, so that a
    /// button is not replaced under the pointer once a second.
    pub fn show(&self, entries: &[Entry], away: bool) {
        let ivars = self.ivars();
        ivars.away.setHidden(!away);
        // The list starts below the strip.
        let top = if away { theme::DOWNLOAD_STRIP_HEIGHT } else { 0.0 };
        ivars.scroll.setAdditionalSafeAreaInsets(NSEdgeInsets {
            top,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        });
        let listed = |(row, entry): (&Row, &Entry)| {
            row.key == *entry.download.key() && row.look == Look::of(&entry.state)
        };
        let same = {
            let rows = ivars.rows.borrow();
            rows.len() == entries.len() && rows.iter().zip(entries).all(listed)
        };
        if !same {
            let row = |entry: &Entry| Row::new(entry, &ivars.on_act, self.mtm());
            *ivars.rows.borrow_mut() = entries.iter().map(row).collect();
            ivars.table.reloadData();
            match entries.is_empty() {
                true => layout::centre(&ivars.body, ivars.nothing.view()),
                false => layout::fill(&ivars.body, &ivars.scroll),
            }
        }
        for (row, entry) in ivars.rows.borrow().iter().zip(entries) {
            row.update(entry);
        }
    }
}

impl Row {
    fn new(entry: &Entry, on_act: &Rc<dyn Fn(Act)>, main_thread: MainThreadMarker) -> Self {
        let item = &entry.download.item;
        let (primary, secondary) = (NSColor::labelColor(), NSColor::secondaryLabelColor());
        let line = |size, weight, color| label::line(size, weight, color, main_thread);

        let poster = NSImageView::new(main_thread);
        poster.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        poster.setWantsLayer(true);
        if let Some(layer) = poster.layer() {
            layer.setCornerRadius(theme::RADIUS_S);
            layer.setMasksToBounds(true);
            let empty = theme::white(theme::ARTWORK_PLACEHOLDER_OPACITY);
            layer.setBackgroundColor(Some(&empty.CGColor()));
        }
        poster.setTranslatesAutoresizingMaskIntoConstraints(false);

        let title = line(theme::FONT_HEADLINE, Weight::Semibold, &primary);
        label::set(&title, &item.title);
        let context = line(theme::FONT_BODY, Weight::Regular, &secondary);
        label::set(&context, &context_of(item));
        let status = line(theme::FONT_BODY, Weight::Regular, &secondary);
        let bar = NSProgressIndicator::new(main_thread);
        bar.setStyle(NSProgressIndicatorStyle::Bar);
        bar.setControlSize(NSControlSize::Small);
        bar.setIndeterminate(false);
        bar.setMaxValue(1.0);
        bar.setTranslatesAutoresizingMaskIntoConstraints(false);
        let words: [&NSView; 4] = [&title, &context, &status, &bar];
        let words = NSStackView::stackViewWithViews(&NSArray::from_slice(&words), main_thread);
        words.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        words.setAlignment(NSLayoutAttribute::Leading);
        words.setSpacing(theme::SPACE_XS);
        words.setTranslatesAutoresizingMaskIntoConstraints(false);
        // A long title is cut short before it pushes the buttons out.
        words.setContentCompressionResistancePriority_forOrientation(
            NSLayoutPriorityDefaultLow,
            NSLayoutConstraintOrientation::Horizontal,
        );

        let look = Look::of(&entry.state);
        let (mut targets, mut pressable) = (Vec::new(), Vec::new());
        let buttons = NSStackView::new(main_thread);
        buttons.setSpacing(theme::SPACE_S);
        buttons.setTranslatesAutoresizingMaskIntoConstraints(false);
        for (name, act) in look.buttons(entry.download.key()) {
            let on_act = Rc::clone(on_act);
            let pressed = Callback::new(main_thread, move |_| on_act(act.clone()));
            // SAFETY: the row keeps the target for as long as it keeps the
            // button.
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(name),
                    Some(&pressed),
                    Some(Callback::action()),
                    main_thread,
                )
            };
            button.setControlSize(NSControlSize::Large);
            buttons.addArrangedSubview(&button);
            pressable.push(button);
            targets.push(pressed);
        }

        let view = NSView::new(main_thread);
        for part in [&**poster, &**words, &**buttons] {
            let part: &NSView = part;
            view.addSubview(part);
            part.centerYAnchor().constraintEqualToAnchor(&view.centerYAnchor()).setActive(true);
        }
        let margin = theme::PAGE_MARGIN;
        let poster_height = (theme::DOWNLOAD_POSTER_WIDTH * theme::POSTER_ASPECT).round();
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            poster.leadingAnchor().constraintEqualToAnchor_constant(&view.leadingAnchor(), margin),
            poster.widthAnchor().constraintEqualToConstant(theme::DOWNLOAD_POSTER_WIDTH),
            poster.heightAnchor().constraintEqualToConstant(poster_height),
            words
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&poster.trailingAnchor(), theme::SPACE_L),
            words
                .trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&buttons.leadingAnchor(), -margin),
            bar.widthAnchor().constraintEqualToConstant(theme::DOWNLOAD_BAR_WIDTH),
            buttons
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&view.trailingAnchor(), -margin),
        ]));

        Self {
            key: entry.download.key().clone(),
            look,
            view,
            poster,
            has_poster: Cell::new(false),
            status,
            bar,
            buttons: pressable,
            _targets: targets,
        }
    }

    /// Brings the row up to date with how far `entry` has got.
    fn update(&self, entry: &Entry) {
        let size = entry.download.size;
        label::set(&self.status, &status(&entry.state, size));
        let received = match &entry.state {
            State::Downloading(transfer) => Some(transfer.received()),
            State::Incomplete { received, .. } => Some(*received),
            State::Downloaded => None,
        };
        self.bar.setHidden(received.is_none());
        if let Some(received) = received {
            self.bar.setDoubleValue(received as f64 / size.max(1) as f64);
        }

        if !self.has_poster.get() && entry.download.poster.exists() {
            let path = NSString::from_str(&entry.download.poster.to_string_lossy());
            let image = NSImage::initWithContentsOfFile(NSImage::alloc(), &path);
            self.poster.setImage(image.as_deref());
            self.has_poster.set(image.is_some());
        }
    }
}

/// The strip that says the server is away, and its button, which looks
/// for the server again.
fn away_strip(
    reconnect: &Callback,
    main_thread: MainThreadMarker,
) -> (Retained<NSView>, Retained<NSButton>) {
    let words = label::line(
        theme::FONT_BODY,
        Weight::Regular,
        &NSColor::secondaryLabelColor(),
        main_thread,
    );
    label::set(&words, "Your server is not answering. Downloads play without it.");
    // SAFETY: the list keeps the target for as long as it keeps the strip.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            ns_string!("Try Again"),
            Some(reconnect),
            Some(Callback::action()),
            main_thread,
        )
    };
    button.setTranslatesAutoresizingMaskIntoConstraints(false);

    let strip = artwork::layered(main_thread);
    // Rows scroll under the strip and must not show through it.
    artwork::fill_with(&strip, &theme::page_background());
    strip.addSubview(&words);
    strip.addSubview(&button);
    let margin = theme::PAGE_MARGIN;
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        words.leadingAnchor().constraintEqualToAnchor_constant(&strip.leadingAnchor(), margin),
        words.centerYAnchor().constraintEqualToAnchor(&strip.centerYAnchor()),
        button.trailingAnchor().constraintEqualToAnchor_constant(&strip.trailingAnchor(), -margin),
        button.centerYAnchor().constraintEqualToAnchor(&strip.centerYAnchor()),
        words
            .trailingAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&button.leadingAnchor(), -theme::SPACE_M),
    ]));
    (strip, button)
}

/// What a film or an episode is besides its title: "A Show · S2 · E7" or
/// "2015".
fn context_of(item: &Item) -> String {
    let mut context: Vec<String> = Vec::new();
    context.extend(item.grandparent_title.clone());
    context.extend(text::episode_code(item));
    if context.is_empty() {
        context.extend(item.year.map(|year| year.to_string()));
    }
    context.join(" · ")
}

/// The line that says where a download of `size` bytes stands.
fn status(state: &State, size: u64) -> String {
    let so_far = |received: u64| {
        let percent = text::percent(received, size);
        format!("{} of {} · {percent}%", text::size(received), text::size(size))
    };
    match state {
        State::Downloading(transfer) => so_far(transfer.received()),
        State::Incomplete { received, error: None } => format!("Stopped · {}", so_far(*received)),
        State::Incomplete { received, error: Some(error) } => {
            format!("Stopped · {} · {error}", so_far(*received))
        }
        State::Downloaded => format!("Downloaded · {}", text::size(size)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_line_says_how_far_a_download_has_got() {
        let stopped = |error: Option<&str>| State::Incomplete {
            received: 5_000_000,
            error: error.map(str::to_owned),
        };
        let size = 14_902_335;
        assert_eq!(status(&stopped(None), size), "Stopped · 5 MB of 15 MB · 33%");
        assert_eq!(
            status(&stopped(Some("the server stopped sending the file")), size),
            "Stopped · 5 MB of 15 MB · 33% · the server stopped sending the file"
        );
        assert_eq!(status(&State::Downloaded, size), "Downloaded · 15 MB");
    }
}
