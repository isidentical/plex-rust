//! A panel over the window that lists the subtitles the server finds
//! online for a film or an episode, in a language the viewer picks, and
//! has one added. The item's page and the player both open it.
//!
//! It is a view in the window and not a sheet: the player fills the
//! window, and a snapshot is a picture of the window alone.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityButtonRole, NSBorderType, NSButton, NSColor, NSControlSize,
    NSEvent, NSGlassEffectView, NSLayoutAttribute, NSLayoutConstraint,
    NSLayoutConstraintOrientation, NSLayoutPriorityDefaultLow, NSMenuItem, NSPopUpButton,
    NSResponder, NSScrollView, NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSEdgeInsets, NSLocale, NSRect, NSString, ns_string};
use plex::subtitles::Candidate;

use super::artwork;
use super::callback::Callback;
use super::failure::Failure;
use super::hover::HoverView;
use super::label::{self, Weight};
use super::{layout, text, theme};
use crate::error::with_causes;

/// The languages offered, as the two letter codes of ISO 639-1 that the
/// server takes. The viewer's own is added when it is not one of them.
const LANGUAGES: [&str; 32] = [
    "ar", "bg", "ca", "cs", "da", "de", "el", "en", "es", "et", "fa", "fi", "fr", "he", "hi", "hr",
    "hu", "id", "it", "ja", "ko", "nl", "no", "pl", "pt", "ro", "ru", "sv", "th", "tr", "uk", "zh",
];

/// Why nothing can be searched for or added: there is no server to ask.
pub const NO_SERVER: &str = "There is no server to ask from here.";

/// Why nothing is added: a snapshot or a check leaves the server as it
/// found it.
pub const READ_ONLY: &str = "A snapshot or a check adds nothing to the server.";

/// Called with the number of a search and the code of its language.
type OnSearch = Box<dyn Fn(u64, &str)>;
type OnPick = Box<dyn Fn(Candidate)>;

/// The key code of Escape on every Mac keyboard (`kVK_Escape` in Carbon's
/// `Events.h`).
const ESCAPE: u16 = 53;

#[derive(Default)]
pub struct KeysIvars {
    on_escape: OnceCell<Box<dyn Fn()>>,
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `Keys` does not
    // implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustSubtitleSearch"]
    #[ivars = KeysIvars]
    struct Keys;

    impl Keys {
        /// The keys the panel's controls leave come here, and go no
        /// further: the page or the player behind the panel must not act
        /// on them. Escape closes the panel.
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if let (ESCAPE, Some(on_escape)) = (event.keyCode(), self.ivars().on_escape.get()) {
                on_escape();
            }
        }
    }
);

impl Keys {
    fn new(main_thread: MainThreadMarker, on_escape: impl Fn() + 'static) -> Retained<Self> {
        let this = Self::alloc(main_thread).set_ivars(KeysIvars::default());
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        // Cannot fail: the cell was created empty just above.
        let _ = view.ivars().on_escape.set(Box::new(on_escape));
        view
    }
}

pub struct SubtitleSearch {
    /// Covers the window while open: a dim layer that closes the panel
    /// when pressed, and the panel in the middle.
    view: Retained<Keys>,
    heading: Retained<NSTextField>,
    language: Retained<NSPopUpButton>,
    /// The codes of the languages in the menu, in its order.
    codes: Vec<String>,
    status: Retained<NSTextField>,
    list: Retained<NSStackView>,
    rows: RefCell<Vec<Retained<HoverView>>>,
    /// The panel itself, for its rows to call back to.
    this: Weak<Self>,
    on_search: RefCell<Option<OnSearch>>,
    /// What had the keyboard before the panel took it, to give it back.
    had_keys: RefCell<Option<Retained<NSResponder>>>,
    on_pick: RefCell<Option<OnPick>>,
    /// The number of the latest search. The answer to one that another
    /// has overtaken is not shown.
    searches: Cell<u64>,
    /// Kept because controls do not retain their targets.
    _targets: [Retained<Callback>; 2],
}

impl SubtitleSearch {
    pub fn new(main_thread: MainThreadMarker) -> Rc<Self> {
        Rc::new_cyclic(|this: &Weak<Self>| {
            let (primary, secondary) = (NSColor::labelColor(), NSColor::secondaryLabelColor());
            let heading =
                label::line(theme::FONT_HEADLINE, Weight::Semibold, &primary, main_thread);
            let status =
                label::paragraph(theme::FONT_BODY, Weight::Regular, &secondary, 3, main_thread);
            let caption = label::line(theme::FONT_BODY, Weight::Regular, &secondary, main_thread);
            label::set(&caption, "Language");

            let codes = languages();
            let language = NSPopUpButton::initWithFrame_pullsDown(
                NSPopUpButton::alloc(main_thread),
                NSRect::ZERO,
                false,
            );
            language.setControlSize(NSControlSize::Large);
            language.setAccessibilityLabel(Some(ns_string!("Language")));
            if let Some(menu) = language.menu() {
                for code in &codes {
                    let entry = NSMenuItem::new(main_thread);
                    entry.setTitle(&NSString::from_str(&language_name(code)));
                    menu.addItem(&entry);
                }
            }

            let changed = Weak::clone(this);
            let language_changed = Callback::new(main_thread, move |_| {
                if let Some(panel) = changed.upgrade() {
                    panel.search();
                }
            });
            let closed = Weak::clone(this);
            let close_pressed = Callback::new(main_thread, move |_| {
                if let Some(panel) = closed.upgrade() {
                    panel.close();
                }
            });
            // SAFETY: the panel keeps the targets for as long as it keeps
            // the controls.
            let close = unsafe {
                language.setTarget(Some(&language_changed));
                language.setAction(Some(Callback::action()));
                NSButton::buttonWithTitle_target_action(
                    ns_string!("Close"),
                    Some(&close_pressed),
                    Some(Callback::action()),
                    main_thread,
                )
            };
            close.setControlSize(NSControlSize::Large);

            let list = NSStackView::new(main_thread);
            list.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            list.setAlignment(NSLayoutAttribute::Leading);
            list.setSpacing(0.0);
            list.setTranslatesAutoresizingMaskIntoConstraints(false);
            let scroll = NSScrollView::new(main_thread);
            scroll.setDrawsBackground(false);
            scroll.setBorderType(NSBorderType::NoBorder);
            scroll.setHasVerticalScroller(true);
            scroll.setAutohidesScrollers(true);
            scroll.setDocumentView(Some(&list));
            scroll.setTranslatesAutoresizingMaskIntoConstraints(false);

            let bar: [&NSView; 3] = [&caption, &language, &close];
            let bar = NSStackView::stackViewWithViews(&NSArray::from_slice(&bar), main_thread);
            bar.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
            bar.setSpacing(theme::SPACE_S);
            bar.setCustomSpacing_afterView(theme::SPACE_XL, &language);
            let inset = theme::SPACE_XL;
            let column: [&NSView; 4] = [&heading, &bar, &status, &scroll];
            let column =
                NSStackView::stackViewWithViews(&NSArray::from_slice(&column), main_thread);
            column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            column.setAlignment(NSLayoutAttribute::Leading);
            column.setSpacing(theme::SPACE_M);
            column.setEdgeInsets(NSEdgeInsets {
                top: inset,
                left: inset,
                bottom: inset,
                right: inset,
            });

            let glass = NSGlassEffectView::new(main_thread);
            glass.setCornerRadius(theme::RADIUS_XXL);
            glass.setContentView(Some(&column));
            // A press on the panel between its controls goes nowhere: not
            // to the dim layer, and not to the picture behind it.
            let panel = HoverView::pressable(main_thread, |_| {}, || {});
            layout::cover(&panel, &glass);
            let dimmed = Weak::clone(this);
            let dim = HoverView::pressable(
                main_thread,
                |_| {},
                move || {
                    if let Some(panel) = dimmed.upgrade() {
                        panel.close();
                    }
                },
            );
            dim.setWantsLayer(true);
            if let Some(layer) = dim.layer() {
                let shade = theme::black(theme::SUBTITLE_SEARCH_DIM_OPACITY);
                layer.setBackgroundColor(Some(&shade.CGColor()));
            }
            // The Close button says the same to a viewer who cannot see.
            dim.setAccessibilityElement(false);

            let escaped = Weak::clone(this);
            let view = Keys::new(main_thread, move || {
                if let Some(panel) = escaped.upgrade() {
                    panel.close();
                }
            });
            layout::cover(&view, &dim);
            layout::float(&view, &panel);
            let inside = theme::SUBTITLE_SEARCH_WIDTH - 2.0 * inset;
            NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
                panel.widthAnchor().constraintEqualToConstant(theme::SUBTITLE_SEARCH_WIDTH),
                scroll.widthAnchor().constraintEqualToConstant(inside),
                scroll.heightAnchor().constraintEqualToConstant(theme::SUBTITLE_SEARCH_LIST_HEIGHT),
                list.widthAnchor().constraintEqualToConstant(inside),
                status.widthAnchor().constraintEqualToConstant(inside),
            ]));
            view.setHidden(true);

            Self {
                view,
                heading,
                language,
                codes,
                status,
                list,
                rows: RefCell::new(Vec::new()),
                this: Weak::clone(this),
                on_search: RefCell::new(None),
                had_keys: RefCell::new(None),
                on_pick: RefCell::new(None),
                searches: Cell::new(0),
                _targets: [language_changed, close_pressed],
            }
        })
    }

    /// Puts the panel over all of `parent`, hidden until it is opened.
    pub fn float_in(&self, parent: &NSView) {
        layout::cover(parent, &self.view);
    }

    pub fn is_open(&self) -> bool {
        !self.view.isHidden()
    }

    /// Opens the panel for the item called `title` and searches at once,
    /// in the language picked last or the first time in the viewer's own.
    /// `on_search` is called for each search with its number and the code
    /// of its language, and is answered through [`show`](Self::show) or
    /// [`fail`](Self::fail) with that number. `on_pick` is called with
    /// the subtitles the viewer picks, and is answered through
    /// [`fail`](Self::fail) or [`close`](Self::close).
    pub fn open(
        &self,
        title: &str,
        on_search: impl Fn(u64, &str) + 'static,
        on_pick: impl Fn(Candidate) + 'static,
    ) {
        label::set(&self.heading, &format!("Subtitles for {title}"));
        *self.on_search.borrow_mut() = Some(Box::new(on_search));
        *self.on_pick.borrow_mut() = Some(Box::new(on_pick));
        self.view.setHidden(false);
        // The keyboard is the panel's while it is open.
        if let Some(window) = self.view.window() {
            *self.had_keys.borrow_mut() = window.firstResponder();
            window.makeFirstResponder(Some(&self.language));
        }
        self.search();
    }

    /// Puts the panel away, and gives the keyboard back.
    pub fn close(&self) {
        let had_keys = self.had_keys.borrow_mut().take();
        if let (true, Some(window)) = (self.is_open(), self.view.window()) {
            // Not to a view that has left the window since, with the page
            // it was on.
            let in_window = |responder: &Retained<NSResponder>| {
                let view = responder.downcast_ref::<NSView>();
                view.is_none_or(|view| view.window().is_some_and(|its| its == window))
            };
            window.makeFirstResponder(had_keys.filter(in_window).as_deref());
        }
        self.view.setHidden(true);
        // An answer still on its way is for nobody now.
        self.searches.set(self.searches.get() + 1);
        self.on_search.borrow_mut().take();
        self.on_pick.borrow_mut().take();
        self.fill(&[]);
    }

    /// Lists `candidates`, the answer to search `number`.
    pub fn show(&self, number: u64, candidates: &[Candidate]) {
        if number != self.searches.get() {
            return;
        }
        let language = self.picked().map(|code| language_name(&code)).unwrap_or_default();
        let found = match candidates.len() {
            0 => format!("The server found no subtitles in {language}."),
            count => {
                let count = u32::try_from(count).unwrap_or(u32::MAX);
                format!("The server found {} in {language}.", text::count(count, "file"))
            }
        };
        label::set(&self.status, &found);
        self.fill(candidates);
    }

    /// Says why search `number`, or adding the subtitles picked, which is
    /// the latest search's, did not work. The list stays to pick again.
    pub fn fail(&self, number: u64, why: &str) {
        if number != self.searches.get() {
            return;
        }
        label::set(&self.status, why);
        self.set_live(true);
    }

    /// The number of the search on screen, for the answer to a pick.
    pub fn latest(&self) -> u64 {
        self.searches.get()
    }

    /// Asks for subtitles in the language picked.
    fn search(&self) {
        let Some(code) = self.picked() else {
            return;
        };
        let number = self.searches.get() + 1;
        self.searches.set(number);
        self.fill(&[]);
        label::set(&self.status, &format!("Searching in {}\u{2026}", language_name(&code)));
        if let Some(on_search) = &*self.on_search.borrow() {
            on_search(number, &code);
        }
    }

    fn picked(&self) -> Option<String> {
        let index = usize::try_from(self.language.indexOfSelectedItem()).ok()?;
        self.codes.get(index).cloned()
    }

    /// Replaces the rows of the list with one for each of `candidates`.
    fn fill(&self, candidates: &[Candidate]) {
        for row in self.rows.borrow_mut().drain(..) {
            row.removeFromSuperview();
        }
        let main_thread = self.view.mtm();
        let rows: Vec<Retained<HoverView>> =
            candidates.iter().map(|candidate| self.row(candidate, main_thread)).collect();
        for row in &rows {
            self.list.addArrangedSubview(row);
            row.widthAnchor().constraintEqualToAnchor(&self.list.widthAnchor()).setActive(true);
        }
        // The list scrolls to its top, the highest scored, once it knows
        // how long it is.
        self.view.layoutSubtreeIfNeeded();
        if let Some(first) = rows.first() {
            first.scrollRectToVisible(first.bounds());
        }
        *self.rows.borrow_mut() = rows;
    }

    /// A row for `candidate`: its file's name, and under it where it is
    /// from and what kind it is. Pressed, it is added.
    fn row(&self, candidate: &Candidate, main_thread: MainThreadMarker) -> Retained<HoverView> {
        let (primary, secondary) = (NSColor::labelColor(), NSColor::secondaryLabelColor());
        let name = label::line(theme::FONT_BODY, Weight::Medium, &primary, main_thread);
        label::set(&name, candidate.title.as_deref().unwrap_or(&candidate.key));
        let detail = label::line(theme::FONT_CAPTION, Weight::Regular, &secondary, main_thread);
        label::set(&detail, &described(candidate));
        // A long name is cut short inside the row, not past its edges.
        for line in [&name, &detail] {
            line.setContentCompressionResistancePriority_forOrientation(
                NSLayoutPriorityDefaultLow,
                NSLayoutConstraintOrientation::Horizontal,
            );
        }
        let lines: [&NSView; 2] = [&name, &detail];
        let lines = NSStackView::stackViewWithViews(&NSArray::from_slice(&lines), main_thread);
        lines.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        lines.setAlignment(NSLayoutAttribute::Leading);
        lines.setSpacing(theme::SPACE_XXS);
        let pad = theme::SPACE_S;
        lines.setEdgeInsets(NSEdgeInsets { top: pad, left: pad, bottom: pad, right: pad });

        let wash = artwork::layered(main_thread);
        artwork::fill_with(&wash, &theme::white(theme::ROW_HOVER_OPACITY));
        if let Some(layer) = wash.layer() {
            layer.setCornerRadius(theme::RADIUS_M);
        }
        wash.setAlphaValue(0.0);
        let (hovered, picked, candidate) =
            (wash.clone(), Weak::clone(&self.this), candidate.clone());
        let row = HoverView::pressable(
            main_thread,
            move |over| hovered.setAlphaValue(if over { 1.0 } else { 0.0 }),
            move || {
                if let Some(panel) = picked.upgrade() {
                    panel.pick(candidate.clone());
                }
            },
        );
        layout::cover(&row, &wash);
        layout::cover(&row, &lines);
        row.heightAnchor()
            .constraintGreaterThanOrEqualToConstant(theme::TOUCH_TARGET)
            .setActive(true);
        row.setAccessibilityElement(true);
        // SAFETY: the role is a constant that AppKit provides.
        row.setAccessibilityRole(Some(unsafe { NSAccessibilityButtonRole }));
        let spoken = format!("{}, {}", name.stringValue(), detail.stringValue());
        row.setAccessibilityLabel(Some(&NSString::from_str(&spoken)));
        row
    }

    /// Has `candidate` added, and says so until it is.
    fn pick(&self, candidate: Candidate) {
        let name = candidate.title.clone().unwrap_or_else(|| candidate.key.clone());
        label::set(&self.status, &format!("Adding {name}\u{2026}"));
        self.set_live(false);
        if let Some(on_pick) = &*self.on_pick.borrow() {
            on_pick(candidate);
        }
    }

    fn set_live(&self, live: bool) {
        for row in self.rows.borrow().iter() {
            row.set_live(live);
        }
    }
}

/// Why a search did not work, in words for the viewer.
pub fn not_searched(error: &plex::Error) -> String {
    because("The server could not search for subtitles", error)
}

/// Why subtitles picked were not added, in words for the viewer.
pub fn not_added(error: &plex::Error) -> String {
    because("The server did not add those subtitles", error)
}

/// `what` happened, and why as far as `error` says. A server without Plex
/// Pass, or without its OpenSubtitles agent, answers with an error status
/// that only the error itself names.
fn because(what: &str, error: &plex::Error) -> String {
    match Failure::of(error).title() {
        Some(title) => format!("{what}. {title}."),
        None => format!("{what}: {}.", with_causes(error)),
    }
}

/// What sets `candidate` apart besides its name, such as "OpenSubtitles ·
/// English · SRT · Hearing impaired · Score 7,342".
fn described(candidate: &Candidate) -> String {
    let codec = candidate.codec.as_deref().map(str::to_uppercase);
    let score = candidate.score.map(|score| format!("Score {}", text::thousands(score)));
    let mut facts = vec![candidate.provider_title.clone(), candidate.language.clone(), codec];
    facts.push(candidate.hearing_impaired.then(|| "Hearing impaired".to_owned()));
    facts.push(candidate.forced.then(|| "Forced".to_owned()));
    facts.push(score);
    facts.into_iter().flatten().collect::<Vec<String>>().join(" \u{b7} ")
}

/// The codes of the languages to offer, in the order of their names, and
/// the viewer's own first so that it is the one picked at the start.
fn languages() -> Vec<String> {
    let preferred = NSLocale::preferredLanguages();
    let own = preferred.firstObject().map(|tag| base_language(&tag.to_string()).to_owned());
    let mut codes: Vec<String> = LANGUAGES.iter().map(|&code| code.to_owned()).collect();
    codes.sort_by_key(|code| language_name(code));
    if let Some(own) = own {
        codes.retain(|code| *code != own);
        codes.insert(0, own);
    }
    codes
}

/// The language of a tag such as `en-GB` or `zh-Hans-CN`: `en`, `zh`.
fn base_language(tag: &str) -> &str {
    tag.split(['-', '_']).next().unwrap_or(tag)
}

/// The name of the language with `code` in the viewer's own language.
fn language_name(code: &str) -> String {
    let name = NSLocale::currentLocale().localizedStringForLanguageCode(&NSString::from_str(code));
    name.map_or_else(|| code.to_owned(), |name| name.to_string())
}
