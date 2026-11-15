//! The Activity page: how much was watched, when, what and on what, over
//! a range of time the viewer picks, from what the server keeps.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSClickGestureRecognizer, NSColor, NSControlSize, NSLayoutConstraint, NSScrollView,
    NSSegmentSwitchTracking, NSSegmentedControl, NSStackView, NSStackViewDistribution, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{
    NSArray, NSDate, NSDateFormatter, NSDateFormatterStyle, NSEdgeInsets, NSPoint, NSRect, NSSize,
    NSString, NSTimeZone,
};
use plex::model::{Item, Kind};

use super::artwork::{self, ArtView};
use super::callback::Callback;
use super::label::{self, Weight};
use super::text;
use super::theme;
use crate::activity::{Bar, Clock, Heat, Range, Record, Share, Summary, Title, Unit, weekday};
use crate::images::Images;

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `Canvas` does not
    // implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustActivityCanvas"]
    #[ivars = OnceCell<Box<dyn Fn(NSSize)>>]
    struct Canvas;

    impl Canvas {
        /// Measured from the top, as a page is read.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(layout))]
        fn layout(&self) {
            // SAFETY: forwards the call unchanged.
            let () = unsafe { msg_send![super(self), layout] };
            if let Some(place) = self.ivars().get() {
                place(self.bounds().size);
            }
        }
    }
);

impl Canvas {
    /// A view placed by constraints that calls `place` with its size each
    /// time it is laid out, to set the frames of what it draws.
    fn new(main_thread: MainThreadMarker, place: Option<Box<dyn Fn(NSSize)>>) -> Retained<Self> {
        let this = Self::alloc(main_thread).set_ivars(OnceCell::new());
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let canvas: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        if let Some(place) = place {
            // Cannot fail: the cell was created empty a line up.
            let _ = canvas.ivars().set(place);
        }
        canvas.setTranslatesAutoresizingMaskIntoConstraints(false);
        canvas
    }
}

pub struct ActivityPage {
    main_thread: MainThreadMarker,
    scroll: Retained<NSScrollView>,
    /// Holds the heading, which stays, and the body, which is built again
    /// for each range.
    column: Retained<NSStackView>,
    body: RefCell<Option<Retained<NSView>>>,
    since: Retained<NSTextField>,
    ranges: Retained<NSSegmentedControl>,
    record: RefCell<Option<Record>>,
    range: Cell<Range>,
    images: Rc<Images>,
    on_open: Rc<dyn Fn(Item)>,
    /// What the views of the body hang on and do not retain themselves:
    /// the targets of its clicks and the pictures it shows.
    callbacks: RefCell<Vec<Retained<Callback>>>,
    pictures: RefCell<Vec<ArtView>>,
    /// The target of the range control.
    _picked: Retained<Callback>,
}

impl ActivityPage {
    /// `on_open` is called with the film or show of a row that is clicked.
    pub fn new(
        main_thread: MainThreadMarker,
        images: &Rc<Images>,
        on_open: impl Fn(Item) + 'static,
    ) -> Rc<Self> {
        Rc::new_cyclic(|page: &Weak<Self>| {
            let page = page.clone();
            let picked = Callback::new(main_thread, move |_| {
                if let Some(page) = page.upgrade() {
                    page.pick();
                }
            });
            let titles: Vec<Retained<NSString>> =
                Range::ALL.iter().map(|range| NSString::from_str(range.title())).collect();
            // SAFETY: the target is a `Callback`, whose `call:` takes the
            // control, and the page keeps it for as long as the control.
            let ranges = unsafe {
                NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                    &NSArray::from_retained_slice(&titles),
                    NSSegmentSwitchTracking::SelectOne,
                    Some(&picked),
                    Some(Callback::action()),
                    main_thread,
                )
            };
            ranges.setControlSize(NSControlSize::Large);
            let chosen = Range::ALL.iter().position(|range| *range == Range::default());
            ranges.setSelectedSegment(chosen.map_or(0, |index| index.cast_signed()));
            ranges.setTranslatesAutoresizingMaskIntoConstraints(false);

            let title =
                line(theme::FONT_LARGE_TITLE, Weight::Bold, &NSColor::labelColor(), main_thread);
            label::set(&title, "Activity");
            let since = line(
                theme::FONT_BODY,
                Weight::Regular,
                &NSColor::secondaryLabelColor(),
                main_thread,
            );
            let words = vertical(&[&title, &since], theme::SPACE_XS, main_thread);
            let heading = horizontal(&[&words, &ranges], theme::SPACE_L, main_thread);
            heading.setDistribution(NSStackViewDistribution::EqualCentering);

            let column = vertical(&[&heading], theme::SPACE_XL, main_thread);
            let sheet = Canvas::new(main_thread, None);
            sheet.addSubview(&column);
            let scroll = NSScrollView::new(main_thread);
            scroll.setHasVerticalScroller(true);
            scroll.setBackgroundColor(&theme::page_background());
            scroll.setDocumentView(Some(&sheet));
            let clip = scroll.contentView();
            let margin = theme::PAGE_MARGIN;
            // The sidebar floats over the left of the scroll view, which
            // places the page clear of it. The page is as wide as the
            // room it leaves.
            let clear = scroll.safeAreaLayoutGuide();
            let fill_width = column
                .widthAnchor()
                .constraintEqualToAnchor_constant(&sheet.widthAnchor(), -2.0 * margin);
            fill_width.setPriority(750.0);
            NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
                sheet.topAnchor().constraintEqualToAnchor(&clip.topAnchor()),
                sheet.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()),
                sheet.widthAnchor().constraintEqualToAnchor(&clear.widthAnchor()),
                column.topAnchor().constraintEqualToAnchor_constant(&sheet.topAnchor(), margin),
                column
                    .bottomAnchor()
                    .constraintEqualToAnchor_constant(&sheet.bottomAnchor(), -margin),
                column.centerXAnchor().constraintEqualToAnchor(&sheet.centerXAnchor()),
                column.widthAnchor().constraintLessThanOrEqualToConstant(theme::ACTIVITY_MAX_WIDTH),
                fill_width,
                heading.widthAnchor().constraintEqualToAnchor(&column.widthAnchor()),
            ]));

            Self {
                main_thread,
                scroll,
                column,
                body: RefCell::new(None),
                since,
                ranges,
                record: RefCell::new(None),
                range: Cell::new(Range::default()),
                images: Rc::clone(images),
                on_open: Rc::new(on_open),
                callbacks: RefCell::default(),
                pictures: RefCell::default(),
                _picked: picked,
            }
        })
    }

    pub fn view(&self) -> &NSView {
        &self.scroll
    }

    pub fn focus(&self) {
        if let Some(window) = self.scroll.window() {
            window.makeFirstResponder(Some(&self.ranges));
        }
    }

    /// Scrolls so that the content `y` points down is at the top.
    pub fn scroll_to(&self, y: f64) {
        let clip = self.scroll.contentView();
        let under_toolbar = self.scroll.contentInsets().top;
        // Across, the clip rests where it keeps the page clear of the
        // sidebar, which is not at 0.
        let across = clip.bounds().origin.x;
        clip.scrollToPoint(NSPoint { x: across, y: y - under_toolbar });
        self.scroll.reflectScrolledClipView(&clip);
    }

    /// Shows what `record` comes to over the range picked.
    pub fn show(&self, record: &Record) {
        *self.record.borrow_mut() = Some(record.clone());
        self.render();
    }

    /// Shows another range of what is there.
    fn pick(&self) {
        let index = usize::try_from(self.ranges.selectedSegment()).unwrap_or(0);
        self.range.set(Range::ALL.get(index).copied().unwrap_or_default());
        self.render();
    }

    fn render(&self) {
        let record = self.record.borrow();
        let Some(record) = record.as_ref() else {
            return;
        };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs());
        let clock = Clock { now, offset: &local_offset };
        let summary = record.summary(self.range.get(), &clock);
        label::set(&self.since, &since(&summary, record, &clock));

        self.callbacks.borrow_mut().clear();
        self.pictures.borrow_mut().clear();
        let body = self.body(&summary, record, &clock);
        if let Some(old) = self.body.replace(Some(body.clone())) {
            self.column.removeArrangedSubview(&old);
            old.removeFromSuperview();
        }
        self.column.addArrangedSubview(&body);
        body.widthAnchor().constraintEqualToAnchor(&self.column.widthAnchor()).setActive(true);
    }

    fn body(&self, summary: &Summary, record: &Record, clock: &Clock) -> Retained<NSView> {
        let main_thread = self.main_thread;
        if summary.plays() == 0 && summary.seconds() == 0 {
            let empty = format!("Nothing was watched in these {}.", noun(summary.range));
            let note = paragraph(&empty, main_thread);
            return Retained::into_super(vertical(&[&note], theme::SPACE_L, main_thread));
        }

        let mut parts: Vec<Retained<NSView>> = vec![self.tiles(summary)];
        if summary.timed {
            parts.push(card("Watch Time", &chart(summary, clock, main_thread), main_thread));
            parts.push(card("When You Watch", &heat(summary, clock, main_thread), main_thread));
        } else {
            let note = "Your server did not give the time watched, so only plays are counted.";
            parts.push(paragraph(note, main_thread));
        }
        if !summary.top.is_empty() {
            parts.push(card("Most Watched", &self.titles(&summary.top), main_thread));
        }

        let mut halves: Vec<Retained<NSView>> = Vec::new();
        let time = |share: &Share| measure(share, summary.timed);
        let plays = |share: &Share| (text::count(to_u32(share.plays), "play"), share.plays as u64);
        if !summary.genres.is_empty() {
            let genres = meters(&with_runtime(&summary.genres), &plays, main_thread);
            halves.push(card("Genres", &genres, main_thread));
        }
        if !summary.devices.is_empty() {
            let devices = meters(&summary.devices, &time, main_thread);
            halves.push(card("Devices", &devices, main_thread));
        }
        if !summary.decades.is_empty() {
            let decades = meters(&with_runtime(&summary.decades), &plays, main_thread);
            halves.push(card("Release Decades", &decades, main_thread));
        }
        if let Some(data) = summary.data.as_ref().filter(|data| data.bytes > 0) {
            halves.push(card("Data Streamed", &streamed(data, main_thread), main_thread));
        }
        if !summary.accounts.is_empty() {
            halves.push(card(
                "People",
                &meters(&summary.accounts, &time, main_thread),
                main_thread,
            ));
        }
        if !record.libraries.is_empty() {
            halves.push(card("Libraries", &libraries(record, main_thread), main_thread));
        }
        let filler = spacer(main_thread);
        for pair in halves.chunks(2) {
            let mut pair: Vec<&NSView> = pair.iter().map(|view| &**view).collect();
            // An odd one out takes half the width, as the others do.
            if pair.len() == 1 {
                pair.push(&filler);
            }
            let row = horizontal(&pair, theme::SPACE_L, main_thread);
            row.setDistribution(NSStackViewDistribution::FillEqually);
            row.setAlignment(objc2_app_kit::NSLayoutAttribute::Top);
            parts.push(Retained::into_super(row));
        }
        if !summary.recent.is_empty() {
            parts.push(card("Recently Watched", &self.recent(summary, record), main_thread));
        }
        parts.push(paragraph(FOOTNOTE, main_thread));

        let parts: Vec<&NSView> = parts.iter().map(|view| &**view).collect();
        let body = vertical(&parts, theme::SPACE_XL, main_thread);
        for part in &parts {
            part.widthAnchor().constraintEqualToAnchor(&body.widthAnchor()).setActive(true);
        }
        Retained::into_super(body)
    }

    fn tiles(&self, summary: &Summary) -> Retained<NSView> {
        let main_thread = self.main_thread;
        let length = |seconds: u64| text::length(Duration::from_secs(seconds));
        let mut tiles = Vec::new();
        if summary.timed {
            let split = format!(
                "{} episodes · {} films",
                length(summary.episode_seconds),
                length(summary.film_seconds)
            );
            tiles.push(tile(&length(summary.seconds()), "Watch time", &split, main_thread));
        }
        let split = format!(
            "{} · {}",
            text::count(to_u32(summary.episode_plays), "episode"),
            text::count(to_u32(summary.film_plays), "film")
        );
        tiles.push(tile(&summary.plays().to_string(), "Plays", &split, main_thread));
        tiles.push(tile(&summary.titles.to_string(), "Titles", "Films and shows", main_thread));
        let days = summary.last_day - summary.first_day + 1;
        let of = format!("Of {days} days");
        tiles.push(tile(&summary.days_watched.to_string(), "Days watched", &of, main_thread));
        let current = format!("{} now", text::count(to_u32(summary.current_streak), "day"));
        let longest = text::count(to_u32(summary.longest_streak), "day");
        tiles.push(tile(&longest, "Longest streak", &current, main_thread));
        if summary.timed && summary.days_watched > 0 {
            let average = summary.seconds() / summary.days_watched as u64;
            let per = "On a day you watched";
            tiles.push(tile(&length(average), "Daily average", per, main_thread));
        }
        if let Some((day, seconds)) = summary.busiest {
            let on = day_words(day, "EEEMMMd", &local_offset);
            tiles.push(tile(&length(seconds), "Busiest day", &on, main_thread));
        }
        if let Some(data) = summary.data.as_ref().filter(|data| data.bytes > 0) {
            let near = format!("{}% on your network", text::percent(data.nearby, data.bytes));
            tiles.push(tile(&text::size(data.bytes), "Data streamed", &near, main_thread));
        }

        let rows: Vec<Retained<NSStackView>> = tiles
            .chunks(theme::ACTIVITY_TILES_A_ROW)
            .map(|row| {
                let mut row: Vec<&NSView> = row.iter().map(|tile| &**tile).collect();
                let spacers: Vec<Retained<NSView>> =
                    (row.len()..theme::ACTIVITY_TILES_A_ROW).map(|_| spacer(main_thread)).collect();
                row.extend(spacers.iter().map(|spacer| &**spacer));
                let row = horizontal(&row, theme::SPACE_M, main_thread);
                row.setDistribution(NSStackViewDistribution::FillEqually);
                row
            })
            .collect();
        let rows: Vec<&NSView> = rows.iter().map(|row| &***row).collect();
        let grid = vertical(&rows, theme::SPACE_M, main_thread);
        for row in &rows {
            row.widthAnchor().constraintEqualToAnchor(&grid.widthAnchor()).setActive(true);
        }
        Retained::into_super(grid)
    }

    /// The films and shows watched the longest, each leading to its page.
    fn titles(&self, titles: &[Title]) -> Retained<NSView> {
        let main_thread = self.main_thread;
        let most = titles.iter().map(|title| title.plays).max().unwrap_or(1);
        let rows: Vec<Retained<NSView>> = titles
            .iter()
            .map(|title| {
                let what = if title.kind == Kind::Movie { "Film" } else { "Show" };
                let plays = text::count(to_u32(title.plays), "play");
                let detail = match title.seconds {
                    0 => what.to_owned(),
                    seconds => format!("{what} · {}", runtime(seconds)),
                };
                let fraction = title.plays as f64 / most as f64;
                let meter = meter(&title.name, &detail, &plays, fraction, main_thread);
                let row = self.row_with_poster(title.thumb.as_deref(), &meter);
                if let Some(item) = title.item.clone() {
                    self.on_click(&row, item);
                }
                row
            })
            .collect();
        stacked(&rows, main_thread)
    }

    /// The last plays of the range, newest first.
    fn recent(&self, summary: &Summary, record: &Record) -> Retained<NSView> {
        let main_thread = self.main_thread;
        let when = formatter(None);
        let rows: Vec<Retained<NSView>> = summary
            .recent
            .iter()
            .map(|play| {
                let (name, detail) = match (&play.grandparent_title, play.parent_index, play.index)
                {
                    (Some(show), Some(season), Some(episode)) => {
                        (show.clone(), format!("S{season} · E{episode}  {}", play.title))
                    }
                    (Some(show), ..) => (show.clone(), play.title.clone()),
                    (None, ..) => (play.title.clone(), "Film".to_owned()),
                };
                let at = format_at(&when, play.viewed_at);
                let device = record.device_name(play.device);
                let words = text_column(&name, &detail, main_thread);
                let right = text_column(&at, &device, main_thread);
                for label in right.arrangedSubviews().iter() {
                    if let Ok(label) = label.downcast::<NSTextField>() {
                        label.setAlignment(objc2_app_kit::NSTextAlignment::Right);
                    }
                }
                right.setAlignment(objc2_app_kit::NSLayoutAttribute::Trailing);
                let line = horizontal(&[&words, &right], theme::SPACE_L, main_thread);
                line.setDistribution(NSStackViewDistribution::Fill);
                right.setContentCompressionResistancePriority_forOrientation(
                    HOLDS_WIDTH,
                    objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
                );
                let thumb = play.grandparent_thumb.as_deref().or(play.thumb.as_deref());
                let row = self.row_with_poster(thumb, &line);
                let item = play.rating_key.as_ref().and_then(|key| record.items.get(key));
                if let Some(item) = item.cloned() {
                    self.on_click(&row, item);
                }
                row
            })
            .collect();
        stacked(&rows, main_thread)
    }

    /// A row a finger's height, with a poster before `content`.
    fn row_with_poster(&self, thumb: Option<&str>, content: &NSView) -> Retained<NSView> {
        let main_thread = self.main_thread;
        let poster = ArtView::new(main_thread, theme::RADIUS_S);
        let (width, height) = theme::ACTIVITY_POSTER_SIZE;
        poster.show(thumb, (width, height), &self.images);
        let row = Canvas::new(main_thread, None);
        let picture = poster.view();
        row.addSubview(picture);
        row.addSubview(content);
        content.setTranslatesAutoresizingMaskIntoConstraints(false);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            row.heightAnchor().constraintGreaterThanOrEqualToConstant(theme::ACTIVITY_ROW_HEIGHT),
            picture.leadingAnchor().constraintEqualToAnchor(&row.leadingAnchor()),
            picture.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()),
            picture.widthAnchor().constraintEqualToConstant(f64::from(width)),
            picture.heightAnchor().constraintEqualToConstant(f64::from(height)),
            content
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&picture.trailingAnchor(), theme::SPACE_M),
            content.trailingAnchor().constraintEqualToAnchor(&row.trailingAnchor()),
            content.centerYAnchor().constraintEqualToAnchor(&row.centerYAnchor()),
        ]));
        self.pictures.borrow_mut().push(poster);
        Retained::into_super(row)
    }

    /// Opens `item` when `row` is clicked or tapped.
    fn on_click(&self, row: &NSView, item: Item) {
        let on_open = Rc::clone(&self.on_open);
        let callback = Callback::new(self.main_thread, move |_| on_open(item.clone()));
        // SAFETY: the target is a `Callback`, whose `call:` takes the
        // recognizer, and the page keeps it for as long as the row.
        let click = unsafe {
            NSClickGestureRecognizer::initWithTarget_action(
                NSClickGestureRecognizer::alloc(self.main_thread),
                Some(&callback),
                Some(Callback::action()),
            )
        };
        row.addGestureRecognizer(&click);
        row.setToolTip(Some(&NSString::from_str("Open")));
        self.callbacks.borrow_mut().push(callback);
    }
}

/// How it reads under the title: from when, and how much.
fn since(summary: &Summary, record: &Record, clock: &Clock) -> String {
    let first = day_words(summary.first_day, "yMMMd", clock.offset);
    let last = day_words(summary.last_day, "yMMMd", clock.offset);
    let plays = text::count(to_u32(record.plays.len()), "play");
    format!("{first} to {last} · {plays} in your server's history")
}

const FOOTNOTE: &str = "From your server's play history and the time it counts watched on \
    each device. A play is counted when an item is watched to the end. Watch time is the time \
    the server counted playing. Runtime adds up the full length of each play, so it differs \
    from watch time when something was played faster or slower, or only partly watched.";

/// "38 h 12 min of runtime".
fn runtime(seconds: u64) -> String {
    format!("{} of runtime", text::length(Duration::from_secs(seconds)))
}

/// Shares whose words say their runtime, for lists ranked by plays.
fn with_runtime(shares: &[Share]) -> Vec<Share> {
    let said = |share: &Share| Share {
        detail: if share.seconds > 0 { runtime(share.seconds) } else { String::new() },
        ..share.clone()
    };
    shares.iter().map(said).collect()
}

/// What a range is called in a sentence.
fn noun(range: Range) -> &'static str {
    match range {
        Range::Week => "7 days",
        Range::Month => "30 days",
        Range::Year => "12 months",
        Range::All => "records",
    }
}

/// A share's value in words: the time watched, or the plays when there
/// is no time to go by.
fn measure(share: &Share, timed: bool) -> (String, u64) {
    if timed && share.seconds > 0 {
        (text::length(Duration::from_secs(share.seconds)), share.seconds)
    } else {
        (text::count(to_u32(share.plays), "play"), share.plays as u64)
    }
}

fn to_u32(number: usize) -> u32 {
    u32::try_from(number).unwrap_or(u32::MAX)
}

/// The offset of the time zone of this Mac from UTC at `at`, in seconds,
/// summer time included.
pub fn local_offset(at: u64) -> i64 {
    let date = NSDate::dateWithTimeIntervalSince1970(at as f64);
    NSTimeZone::localTimeZone().secondsFromGMTForDate(&date) as i64
}

/// Midday of local day `day`, as seconds since 1970: a moment that is in
/// that day whatever the offset.
fn noon(day: i64, offset: &dyn Fn(u64) -> i64) -> u64 {
    let guess = day * 86_400 + 43_200;
    u64::try_from(guess - offset(u64::try_from(guess).unwrap_or(0))).unwrap_or(0)
}

/// A formatter for dates in the viewer's language, from a template of
/// fields such as `MMMd`, or for a date and time said as people say it,
/// as in "Today at 9:42 PM".
fn formatter(template: Option<&str>) -> Retained<NSDateFormatter> {
    let formatter = NSDateFormatter::new();
    match template {
        Some(template) => {
            formatter.setLocalizedDateFormatFromTemplate(&NSString::from_str(template));
        }
        None => {
            formatter.setDateStyle(NSDateFormatterStyle::MediumStyle);
            formatter.setTimeStyle(NSDateFormatterStyle::ShortStyle);
            formatter.setDoesRelativeDateFormatting(true);
        }
    }
    formatter
}

fn format_at(formatter: &NSDateFormatter, at: u64) -> String {
    let date = NSDate::dateWithTimeIntervalSince1970(at as f64);
    formatter.stringFromDate(&date).to_string()
}

fn day_words(day: i64, template: &str, offset: &dyn Fn(u64) -> i64) -> String {
    format_at(&formatter(Some(template)), noon(day, offset))
}

/// The chart of time watched, a bar a day or a month, episodes under films.
fn chart(summary: &Summary, clock: &Clock, main_thread: MainThreadMarker) -> Retained<NSView> {
    let bars = summary.bars.clone();
    let most = bars.iter().map(|bar| bar.episodes + bar.films).max().unwrap_or(0);
    let top = nice_top(most);
    let episodes = NSColor::controlAccentColor();
    let films = NSColor::systemOrangeColor();

    let template = match (summary.unit, bars.len()) {
        (Unit::Day, 0..=7) => "EEE",
        (Unit::Day, _) => "MMMd",
        (Unit::Month, 0..=12) => "MMM",
        (Unit::Month, _) => "MMMyy",
    };
    let labels = formatter(Some(template));
    let tips = formatter(Some(match summary.unit {
        Unit::Day => "EEEEMMMMdy",
        Unit::Month => "MMMMy",
    }));

    let mut columns = Vec::new();
    for bar in &bars {
        let column = artwork::layered(main_thread);
        column.setTranslatesAutoresizingMaskIntoConstraints(true);
        let lower = segment(&episodes, main_thread);
        let upper = segment(&films, main_thread);
        column.addSubview(&lower);
        column.addSubview(&upper);
        column.setToolTip(Some(&NSString::from_str(&bar_tip(bar, &tips, clock))));
        columns.push((column, lower, upper));
    }
    let names: Vec<Retained<NSTextField>> = bars
        .iter()
        .map(|bar| {
            let name = caption(&format_at(&labels, noon(bar.day, clock.offset)), main_thread);
            name.setTranslatesAutoresizingMaskIntoConstraints(true);
            name.setAlignment(objc2_app_kit::NSTextAlignment::Center);
            name
        })
        .collect();
    let lines: Vec<(Retained<NSView>, Retained<NSTextField>)> = [1.0, 0.5]
        .iter()
        .map(|share: &f64| {
            let line = artwork::layered(main_thread);
            line.setTranslatesAutoresizingMaskIntoConstraints(true);
            artwork::fill_with(&line, &theme::white(theme::ACTIVITY_GRID_OPACITY));
            let seconds = (top as f64 * share) as u64;
            let words = caption(&hours(seconds), main_thread);
            words.setTranslatesAutoresizingMaskIntoConstraints(true);
            (line, words)
        })
        .collect();

    let place_columns = columns.clone();
    let (place_names, place_lines) = (names.clone(), lines.clone());
    let canvas = Canvas::new(
        main_thread,
        Some(Box::new(move |size: NSSize| {
            let left = theme::ACTIVITY_AXIS_WIDTH;
            let plot = (size.height - theme::ACTIVITY_AXIS_HEIGHT).max(0.0);
            let slot = (size.width - left) / place_columns.len().max(1) as f64;
            let width = (slot * theme::ACTIVITY_BAR_SHARE).max(1.0);
            for ((line, words), share) in place_lines.iter().zip([1.0, 0.5]) {
                let y = plot * (1.0 - share);
                line.setFrame(rect(left, y, size.width - left, 1.0));
                words.setFrame(rect(0.0, y - theme::FONT_CAPTION, left - theme::SPACE_S, 16.0));
            }
            let mut last_label = f64::NEG_INFINITY;
            for (index, ((column, lower, upper), bar)) in
                place_columns.iter().zip(&bars).enumerate()
            {
                let x = left + slot * index as f64;
                column.setFrame(rect(x, 0.0, slot, plot));
                let height = |seconds: u64| plot * seconds as f64 / top.max(1) as f64;
                let (low, high) = (height(bar.episodes), height(bar.films));
                let inset = (slot - width) / 2.0;
                // A bar's own view measures from its foot.
                lower.setFrame(rect(inset, 0.0, width, low));
                upper.setFrame(rect(inset, low, width, high));
                let name = &place_names[index];
                let centre = x + slot / 2.0;
                let shown = centre - last_label >= theme::ACTIVITY_LABEL_SPACING;
                name.setHidden(!shown);
                if shown {
                    last_label = centre;
                }
                let half = theme::ACTIVITY_LABEL_SPACING / 2.0;
                name.setFrame(rect(centre - half, plot + theme::SPACE_XS, 2.0 * half, 16.0));
            }
        })),
    );
    for (line, words) in &lines {
        canvas.addSubview(line);
        canvas.addSubview(words);
    }
    for (column, ..) in &columns {
        canvas.addSubview(column);
    }
    for name in &names {
        canvas.addSubview(name);
    }
    canvas.heightAnchor().constraintEqualToConstant(theme::ACTIVITY_CHART_HEIGHT).setActive(true);

    let legend = legend(&[("Episodes", &episodes), ("Films", &films)], main_thread);
    let chart = vertical(&[&legend, &canvas], theme::SPACE_M, main_thread);
    canvas.widthAnchor().constraintEqualToAnchor(&chart.widthAnchor()).setActive(true);
    Retained::into_super(chart)
}

/// A highest value for the chart's scale at or above `most`, whose half,
/// where the middle line is drawn, is a round time too: a whole number of
/// quarter hours, hours, or fives or tens of hours.
fn nice_top(most: u64) -> u64 {
    const UNITS: [u64; 5] = [900, 3600, 5 * 3600, 10 * 3600, 50 * 3600];
    // The smallest unit that takes no more than five to reach half way.
    let unit = UNITS.iter().copied().find(|unit| most <= 10 * unit).unwrap_or(100 * 3600);
    most.max(1).div_ceil(2 * unit) * 2 * unit
}

/// A time on the chart's scale: "45 min", "2 h", "1 h 30 min".
fn hours(seconds: u64) -> String {
    text::length(Duration::from_secs(seconds))
}

fn bar_tip(bar: &Bar, formatter: &NSDateFormatter, clock: &Clock) -> String {
    let when = format_at(formatter, noon(bar.day, clock.offset));
    let length = |seconds| text::length(Duration::from_secs(seconds));
    match (bar.episodes, bar.films) {
        (0, 0) => format!("{when}: nothing watched"),
        (episodes, 0) => format!("{when}: {} of episodes", length(episodes)),
        (0, films) => format!("{when}: {} of films", length(films)),
        (episodes, films) => format!(
            "{when}: {}, {} of episodes and {} of films",
            length(episodes + films),
            length(episodes),
            length(films)
        ),
    }
}

/// The grid of the week: a row a day, Monday first, a column an hour.
fn heat(summary: &Summary, clock: &Clock, main_thread: MainThreadMarker) -> Retained<NSView> {
    let grid: Heat = summary.heat;
    let most = grid.iter().flatten().copied().max().unwrap_or(0);
    let accent = NSColor::controlAccentColor();
    let empty = theme::white(theme::ACTIVITY_HEAT_EMPTY_OPACITY);
    // A Monday, from which each weekday's name and each hour is read.
    let monday = summary.last_day - weekday(summary.last_day) as i64;
    let day_names = formatter(Some("EEE"));
    let long_day_names = formatter(Some("EEEE"));
    let hour_names = formatter(Some("j"));
    let times = formatter(Some("jm"));
    let hour_at = |hour: usize| noon(monday, clock.offset) - 43_200 + hour as u64 * 3600;

    let mut cells = Vec::new();
    for (day, hours) in grid.iter().enumerate() {
        for (hour, seconds) in hours.iter().enumerate() {
            let cell = artwork::layered(main_thread);
            cell.setTranslatesAutoresizingMaskIntoConstraints(true);
            if let Some(layer) = cell.layer() {
                layer.setCornerRadius(theme::RADIUS_S);
            }
            let colour = match *seconds {
                0 => empty.clone(),
                seconds => {
                    let share = seconds as f64 / most.max(1) as f64;
                    let strength =
                        theme::ACTIVITY_HEAT_LEAST + (1.0 - theme::ACTIVITY_HEAT_LEAST) * share;
                    accent.colorWithAlphaComponent(strength)
                }
            };
            artwork::fill_with(&cell, &colour);
            let name = format_at(&long_day_names, noon(monday + day as i64, clock.offset));
            let from = format_at(&times, hour_at(hour));
            let length = text::length(Duration::from_secs(*seconds));
            cell.setToolTip(Some(&NSString::from_str(&format!("{name}s, {from}: {length}"))));
            cells.push(cell);
        }
    }
    let row_names: Vec<Retained<NSTextField>> = (0..7)
        .map(|day| {
            let name = format_at(&day_names, noon(monday + day, clock.offset));
            let name = caption(&name, main_thread);
            name.setTranslatesAutoresizingMaskIntoConstraints(true);
            name
        })
        .collect();
    let column_names: Vec<Retained<NSTextField>> = [0, 6, 12, 18]
        .iter()
        .map(|hour| {
            let name = caption(&format_at(&hour_names, hour_at(*hour)), main_thread);
            name.setTranslatesAutoresizingMaskIntoConstraints(true);
            name
        })
        .collect();

    let (place_cells, place_rows, place_columns) =
        (cells.clone(), row_names.clone(), column_names.clone());
    let canvas = Canvas::new(
        main_thread,
        Some(Box::new(move |size: NSSize| {
            let left = theme::ACTIVITY_AXIS_WIDTH;
            let pitch = ((size.width - left) / 24.0)
                .min(theme::ACTIVITY_HEAT_CELL + theme::ACTIVITY_HEAT_GAP);
            let cell = (pitch - theme::ACTIVITY_HEAT_GAP).max(1.0);
            for (index, view) in place_cells.iter().enumerate() {
                let (day, hour) = (index / 24, index % 24);
                view.setFrame(rect(left + pitch * hour as f64, pitch * day as f64, cell, cell));
            }
            for (day, name) in place_rows.iter().enumerate() {
                let y = pitch * day as f64 + (cell - 16.0) / 2.0;
                name.setFrame(rect(0.0, y, left - theme::SPACE_S, 16.0));
            }
            for (index, name) in place_columns.iter().enumerate() {
                let x = left + pitch * (index * 6) as f64;
                name.setFrame(rect(
                    x,
                    pitch * 7.0 + theme::SPACE_XS,
                    4.0 * pitch * 6.0 / 4.0,
                    16.0,
                ));
            }
        })),
    );
    for view in &cells {
        canvas.addSubview(view);
    }
    for name in row_names.iter().chain(&column_names) {
        canvas.addSubview(name);
    }
    let pitch = theme::ACTIVITY_HEAT_CELL + theme::ACTIVITY_HEAT_GAP;
    let height = pitch * 7.0 + theme::SPACE_XS + 16.0;
    canvas.heightAnchor().constraintEqualToConstant(height).setActive(true);

    let peak = grid
        .iter()
        .enumerate()
        .flat_map(|(day, hours)| hours.iter().enumerate().map(move |(hour, s)| (*s, day, hour)))
        .max();
    let words = match peak {
        Some((seconds, day, hour)) if seconds > 0 => {
            let name = format_at(&long_day_names, noon(monday + day as i64, clock.offset));
            let at = format_at(&times, hour_at(hour));
            format!("Most often on {name}s around {at}.")
        }
        _ => "Nothing watched hour by hour in this range.".to_owned(),
    };
    let mut parts: Vec<Retained<NSView>> = vec![paragraph(&words, main_thread)];
    if let Some(from) = summary.hourly_from {
        let first = noon(clock.day(from), clock.offset);
        if clock.day(from) > summary.first_day {
            let since = format_at(&formatter(Some("yMMMd")), first);
            parts.push(paragraph(&format!("Your server counts hours since {since}."), main_thread));
        }
    }
    parts.push(Retained::into_super(canvas));
    let parts: Vec<&NSView> = parts.iter().map(|part| &**part).collect();
    let stack = vertical(&parts, theme::SPACE_M, main_thread);
    for part in &parts {
        part.widthAnchor().constraintEqualToAnchor(&stack.widthAnchor()).setActive(true);
    }
    Retained::into_super(stack)
}

/// What was streamed: how much in the server's network and how much from
/// outside it, then by device.
fn streamed(data: &crate::activity::Data, main_thread: MainThreadMarker) -> Retained<NSView> {
    let nearby = NSColor::systemGreenColor();
    let remote = NSColor::systemPurpleColor();
    let share = data.nearby as f64 / data.bytes.max(1) as f64;
    let split = artwork::layered(main_thread);
    artwork::fill_with(&split, &remote);
    if let Some(layer) = split.layer() {
        layer.setCornerRadius(theme::ACTIVITY_METER_HEIGHT / 2.0);
        layer.setMasksToBounds(true);
    }
    let near = artwork::layered(main_thread);
    artwork::fill_with(&near, &nearby);
    split.addSubview(&near);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        split.heightAnchor().constraintEqualToConstant(theme::ACTIVITY_METER_HEIGHT),
        near.leadingAnchor().constraintEqualToAnchor(&split.leadingAnchor()),
        near.topAnchor().constraintEqualToAnchor(&split.topAnchor()),
        near.bottomAnchor().constraintEqualToAnchor(&split.bottomAnchor()),
        near.widthAnchor().constraintEqualToAnchor_multiplier(&split.widthAnchor(), share),
    ]));
    let percent = text::percent(data.nearby, data.bytes);
    let near_words = format!("On your network, {}", text::size(data.nearby));
    let remote_words = format!("Remote, {}", text::size(data.bytes - data.nearby));
    let legend = legend(&[(&near_words, &nearby), (&remote_words, &remote)], main_thread);
    let summary = paragraph(&format!("{percent}% stayed on your network."), main_thread);
    let size = |share: &Share| (text::size(share.seconds), share.seconds);
    let devices = meters(&data.devices, &size, main_thread);
    let parts: [&NSView; 4] = [&summary, &split, &legend, &devices];
    let stack = vertical(&parts, theme::SPACE_M, main_thread);
    for part in parts {
        part.widthAnchor().constraintEqualToAnchor(&stack.widthAnchor()).setActive(true);
    }
    Retained::into_super(stack)
}

/// How much of each library has been watched.
fn libraries(record: &Record, main_thread: MainThreadMarker) -> Retained<NSView> {
    let rows: Vec<Retained<NSView>> = record
        .libraries
        .iter()
        .map(|library| {
            let noun = if library.kind == Kind::Movie { "film" } else { "episode" };
            let detail =
                format!("{} of {} watched", library.watched, text::count(library.total, noun));
            let percent = text::percent(u64::from(library.watched), u64::from(library.total));
            let fraction = f64::from(library.watched) / f64::from(library.total.max(1));
            meter(&library.title, &detail, &format!("{percent}%"), fraction, main_thread)
        })
        .collect();
    stacked(&rows, main_thread)
}

/// Rows of names with a bar each as long as its share of the largest.
fn meters(
    shares: &[Share],
    value: &dyn Fn(&Share) -> (String, u64),
    main_thread: MainThreadMarker,
) -> Retained<NSView> {
    let most = shares.iter().map(|share| value(share).1).max().unwrap_or(0).max(1);
    let rows: Vec<Retained<NSView>> = shares
        .iter()
        .map(|share| {
            let (words, amount) = value(share);
            let fraction = amount as f64 / most as f64;
            meter(&share.name, &share.detail, &words, fraction, main_thread)
        })
        .collect();
    stacked(&rows, main_thread)
}

/// A name and its value on a line, words under the name, and a bar
/// `fraction` of the width under both.
fn meter(
    name: &str,
    detail: &str,
    value: &str,
    fraction: f64,
    main_thread: MainThreadMarker,
) -> Retained<NSView> {
    let name_label = line(theme::FONT_BODY, Weight::Semibold, &NSColor::labelColor(), main_thread);
    label::set(&name_label, name);
    let value_label =
        line(theme::FONT_BODY, Weight::Medium, &NSColor::secondaryLabelColor(), main_thread);
    label::set(&value_label, value);
    value_label.setContentCompressionResistancePriority_forOrientation(
        HOLDS_WIDTH,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    );
    // The name takes the room left over, so values line up on the right.
    name_label.setContentHuggingPriority_forOrientation(
        1.0,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    );
    let top = horizontal(&[&name_label, &value_label], theme::SPACE_S, main_thread);
    top.setDistribution(NSStackViewDistribution::Fill);

    let track = artwork::layered(main_thread);
    artwork::fill_with(&track, &theme::white(theme::ACTIVITY_GRID_OPACITY));
    let fill = artwork::layered(main_thread);
    artwork::fill_with(&fill, &NSColor::controlAccentColor());
    for bar in [&track, &fill] {
        if let Some(layer) = bar.layer() {
            layer.setCornerRadius(theme::ACTIVITY_METER_HEIGHT / 2.0);
        }
    }
    track.addSubview(&fill);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        track.heightAnchor().constraintEqualToConstant(theme::ACTIVITY_METER_HEIGHT),
        fill.leadingAnchor().constraintEqualToAnchor(&track.leadingAnchor()),
        fill.topAnchor().constraintEqualToAnchor(&track.topAnchor()),
        fill.bottomAnchor().constraintEqualToAnchor(&track.bottomAnchor()),
        fill.widthAnchor()
            .constraintEqualToAnchor_multiplier(&track.widthAnchor(), fraction.clamp(0.01, 1.0)),
    ]));

    let mut parts: Vec<&NSView> = vec![&top];
    let detail_label = caption(detail, main_thread);
    if !detail.is_empty() {
        parts.push(&detail_label);
    }
    parts.push(&track);
    let stack = vertical(&parts, theme::SPACE_XS, main_thread);
    for part in &parts {
        part.widthAnchor().constraintEqualToAnchor(&stack.widthAnchor()).setActive(true);
    }
    Retained::into_super(stack)
}

/// A box of rounded corners with a heading over `content`.
fn card(title: &str, content: &NSView, main_thread: MainThreadMarker) -> Retained<NSView> {
    let heading = line(theme::FONT_HEADLINE, Weight::Semibold, &NSColor::labelColor(), main_thread);
    label::set(&heading, title);
    let stack = vertical(&[&heading, content], theme::SPACE_M, main_thread);
    content.widthAnchor().constraintEqualToAnchor(&stack.widthAnchor()).setActive(true);
    let card = artwork::layered(main_thread);
    artwork::fill_with(&card, &theme::white(theme::ACTIVITY_CARD_OPACITY));
    if let Some(layer) = card.layer() {
        layer.setCornerRadius(theme::RADIUS_XL);
    }
    card.addSubview(&stack);
    let inset = theme::SPACE_L;
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        stack.topAnchor().constraintEqualToAnchor_constant(&card.topAnchor(), inset),
        stack.leadingAnchor().constraintEqualToAnchor_constant(&card.leadingAnchor(), inset),
        stack.trailingAnchor().constraintEqualToAnchor_constant(&card.trailingAnchor(), -inset),
        stack
            .bottomAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&card.bottomAnchor(), -inset),
    ]));
    card
}

/// A big number with what it counts and a line about it.
fn tile(value: &str, title: &str, detail: &str, main_thread: MainThreadMarker) -> Retained<NSView> {
    let title_label = caption(title, main_thread);
    let value_label = line(theme::FONT_TITLE, Weight::Bold, &NSColor::labelColor(), main_thread);
    label::set(&value_label, value);
    let detail_label =
        line(theme::FONT_CAPTION, Weight::Regular, &NSColor::tertiaryLabelColor(), main_thread);
    label::set(&detail_label, detail);
    let stack =
        vertical(&[&title_label, &value_label, &detail_label], theme::SPACE_XS, main_thread);
    let tile = artwork::layered(main_thread);
    artwork::fill_with(&tile, &theme::white(theme::ACTIVITY_CARD_OPACITY));
    if let Some(layer) = tile.layer() {
        layer.setCornerRadius(theme::RADIUS_L);
    }
    tile.addSubview(&stack);
    let inset = theme::SPACE_L;
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        tile.heightAnchor().constraintEqualToConstant(theme::ACTIVITY_TILE_HEIGHT),
        stack.leadingAnchor().constraintEqualToAnchor_constant(&tile.leadingAnchor(), inset),
        stack
            .trailingAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&tile.trailingAnchor(), -inset),
        stack.centerYAnchor().constraintEqualToAnchor(&tile.centerYAnchor()),
    ]));
    tile
}

/// Swatches of colour with what each stands for.
fn legend(entries: &[(&str, &NSColor)], main_thread: MainThreadMarker) -> Retained<NSView> {
    let mut views: Vec<Retained<NSView>> = Vec::new();
    for (name, colour) in entries {
        let swatch = artwork::layered(main_thread);
        artwork::fill_with(&swatch, colour);
        if let Some(layer) = swatch.layer() {
            layer.setCornerRadius(theme::RADIUS_S / 2.0);
        }
        let size = theme::ACTIVITY_SWATCH_SIZE;
        swatch.widthAnchor().constraintEqualToConstant(size).setActive(true);
        swatch.heightAnchor().constraintEqualToConstant(size).setActive(true);
        let words = caption(name, main_thread);
        let entry = horizontal(&[&swatch, &words], theme::SPACE_XS, main_thread);
        views.push(Retained::into_super(entry));
    }
    let views: Vec<&NSView> = views.iter().map(|view| &**view).collect();
    let legend = horizontal(&views, theme::SPACE_L, main_thread);
    let spacer = spacer(main_thread);
    legend.addArrangedSubview(&spacer);
    Retained::into_super(legend)
}

fn segment(colour: &NSColor, main_thread: MainThreadMarker) -> Retained<NSView> {
    let segment = artwork::layered(main_thread);
    segment.setTranslatesAutoresizingMaskIntoConstraints(true);
    artwork::fill_with(&segment, colour);
    if let Some(layer) = segment.layer() {
        layer.setCornerRadius(theme::ACTIVITY_BAR_RADIUS);
    }
    segment
}

/// A name over a line of smaller words.
fn text_column(name: &str, detail: &str, main_thread: MainThreadMarker) -> Retained<NSStackView> {
    let name_label = line(theme::FONT_BODY, Weight::Semibold, &NSColor::labelColor(), main_thread);
    label::set(&name_label, name);
    let detail_label = caption(detail, main_thread);
    vertical(&[&name_label, &detail_label], theme::SPACE_XXS, main_thread)
}

fn caption(words: &str, main_thread: MainThreadMarker) -> Retained<NSTextField> {
    let caption =
        line(theme::FONT_CAPTION, Weight::Regular, &NSColor::secondaryLabelColor(), main_thread);
    label::set(&caption, words);
    caption
}

fn paragraph(words: &str, main_thread: MainThreadMarker) -> Retained<NSView> {
    let paragraph = yielding(label::paragraph(
        theme::FONT_BODY,
        Weight::Regular,
        &NSColor::secondaryLabelColor(),
        0,
        main_thread,
    ));
    label::set(&paragraph, words);
    Retained::into_super(Retained::into_super(paragraph))
}

/// Rows one under the other, each as wide as the stack.
fn stacked(rows: &[Retained<NSView>], main_thread: MainThreadMarker) -> Retained<NSView> {
    let rows: Vec<&NSView> = rows.iter().map(|row| &**row).collect();
    let stack = vertical(&rows, theme::SPACE_M, main_thread);
    for row in &rows {
        row.widthAnchor().constraintEqualToAnchor(&stack.widthAnchor()).setActive(true);
    }
    Retained::into_super(stack)
}

fn spacer(main_thread: MainThreadMarker) -> Retained<NSView> {
    let spacer = NSView::new(main_thread);
    spacer.setTranslatesAutoresizingMaskIntoConstraints(false);
    spacer
}

fn vertical(
    views: &[&NSView],
    spacing: f64,
    main_thread: MainThreadMarker,
) -> Retained<NSStackView> {
    stack(views, NSUserInterfaceLayoutOrientation::Vertical, spacing, main_thread)
}

fn horizontal(
    views: &[&NSView],
    spacing: f64,
    main_thread: MainThreadMarker,
) -> Retained<NSStackView> {
    stack(views, NSUserInterfaceLayoutOrientation::Horizontal, spacing, main_thread)
}

fn stack(
    views: &[&NSView],
    orientation: NSUserInterfaceLayoutOrientation,
    spacing: f64,
    main_thread: MainThreadMarker,
) -> Retained<NSStackView> {
    let stack = NSStackView::stackViewWithViews(&NSArray::from_slice(views), main_thread);
    stack.setOrientation(orientation);
    stack.setSpacing(spacing);
    if orientation == NSUserInterfaceLayoutOrientation::Vertical {
        stack.setAlignment(objc2_app_kit::NSLayoutAttribute::Leading);
    }
    stack.setEdgeInsets(NSEdgeInsets { top: 0.0, left: 0.0, bottom: 0.0, right: 0.0 });
    // A stack keeps all it holds in view by default, at a priority that
    // would widen the window. Here the window decides, and text gives way.
    stack.setClippingResistancePriority_forOrientation(
        GIVES_WIDTH,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    );
    stack.setTranslatesAutoresizingMaskIntoConstraints(false);
    stack
}

/// How hard a line of text holds its width: below the window's hold on
/// its size, so that text that does not fit is cut short and never makes
/// the window grow. A value next to a name holds a little harder, so the
/// name gives way first.
const GIVES_WIDTH: f32 = 250.0;
const HOLDS_WIDTH: f32 = 400.0;

/// One line of text that gives way to the width it is given.
fn line(
    size: f64,
    weight: Weight,
    color: &NSColor,
    main_thread: MainThreadMarker,
) -> Retained<NSTextField> {
    yielding(label::line(size, weight, color, main_thread))
}

fn yielding(label: Retained<NSTextField>) -> Retained<NSTextField> {
    label.setContentCompressionResistancePriority_forOrientation(
        GIVES_WIDTH,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    );
    label
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width.max(0.0), height.max(0.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scale_of_the_chart_ends_on_a_round_time() {
        assert_eq!(nice_top(0), 1800);
        assert_eq!(nice_top(1000), 1800);
        // 9 h 45 min: up to 10 h, with 5 h half way.
        assert_eq!(nice_top(35_100), 36_000);
        // 41 h: up to 50 h, with 25 h half way.
        assert_eq!(nice_top(41 * 3600), 50 * 3600);
    }
}
