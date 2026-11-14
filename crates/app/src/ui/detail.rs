//! The page for one film, show or episode: what it is, the buttons to
//! watch it, and under those what goes with it. For a show that is its
//! episodes season by season, for an episode the rest of its season, and
//! for all of them the cast and whatever the server finds related.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

use objc2::rc::{Allocated, Retained};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityButtonRole, NSAccessibilityLinkRole,
    NSAttributedStringNSExtendedStringDrawing, NSButton, NSCellImagePosition, NSColor,
    NSControlSize, NSLayoutAttribute, NSLayoutConstraint, NSLayoutConstraintOrientation,
    NSLayoutPriorityDefaultHigh, NSLayoutPriorityDefaultLow, NSMenu, NSMenuItem, NSPopUpButton,
    NSPopover, NSPopoverBehavior, NSStackView, NSStackViewGravity,
    NSStackViewVisibilityPriorityDetachOnlyIfNecessary, NSStringDrawingOptions, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSViewController,
};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSArray, NSPoint, NSRect, NSRectEdge, NSSize, NSString, ns_string};
use plex::library::{appearances, next_up, of_genre};
use plex::model::{Hub, Item, Kind, RatingKey, Role, Tag};

use super::artwork::{self, ArtView};
use super::backdrop::Backdrop;
use super::callback::Callback;
use super::formats;
use super::hover::HoverView;
use super::label::{self, Weight};
use super::layout;
use super::poster::{Card, Shape};
use super::posters::{Action, Header, OnAction, Outline, Posters, Shelf, Style};
use super::text;
use super::theme;
use crate::downloads::{Downloads, State};
use crate::images::Images;

/// How many genres, directors and cast members a page names.
const GENRES_SHOWN: usize = 3;
const DIRECTORS_SHOWN: usize = 3;
const CAST_SHOWN: usize = 20;

/// How a show's page opens.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Opening {
    /// The season whose episodes are listed. Without one, it is the season
    /// of the episode to watch next.
    pub season: Option<RatingKey>,
    /// Whether the page starts scrolled down to that list, as the page of
    /// the season and not of the show.
    pub at_episodes: bool,
    /// The episode the keyboard starts on.
    pub episode: Option<RatingKey>,
}

/// A link in the header of an episode's page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Show,
    Season,
}

/// What the header tells the page.
enum Event {
    /// A link from an episode was clicked.
    Follow(Link),
    /// The frame of an episode was clicked, which plays it.
    Play,
    /// A genre was clicked.
    OpenGenre(Tag),
    /// The header was laid out, and its content takes this many points.
    Resized(f64),
}

pub struct Detail {
    backdrop: Retained<Backdrop>,
    /// The header and, under it, the shelves.
    list: Retained<Posters>,
    hero: Hero,
    images: Rc<Images>,
    downloads: Rc<Downloads>,
    on_action: Rc<OnAction>,
    item: RefCell<Option<Item>>,
    /// For a show, every episode in order. For an episode, those of its
    /// season.
    episodes: RefCell<Vec<Item>>,
    /// The rows the server relates to the item.
    related: RefCell<Vec<Hub>>,
    /// For a show, the season whose episodes are listed.
    season: RefCell<Option<RatingKey>>,
    /// Where to scroll and what to select once the episodes are there.
    /// Taken when that is done, so it happens once.
    opening: RefCell<Option<Opening>>,
    /// How tall the header is, which is as tall as its content takes at
    /// the width of the window.
    header_height: Cell<f64>,
    /// Kept because controls do not retain their targets.
    _targets: [Retained<Callback>; 4],
}

/// The views at the top of the page.
struct Hero {
    view: Retained<HeaderView>,
    /// What the page leads with: the poster of a film or show, or a frame
    /// of an episode.
    art: Rc<ArtView>,
    /// Around it. On an episode's page a click on it plays the episode.
    art_link: Retained<HoverView>,
    /// On an episode's page, the poster of its show, small, beside the
    /// way down to the episode. It leads to the show.
    show_poster: Rc<ArtView>,
    show_poster_link: Retained<HoverView>,
    /// The line above an episode's title, as a path down to it: its show
    /// and its season, both links, and its number.
    eyebrow: Retained<NSStackView>,
    show_link: Retained<HoverView>,
    show_name: Retained<NSTextField>,
    season_link: Retained<HoverView>,
    season_name: Retained<NSTextField>,
    episode_number: Retained<NSTextField>,
    title: Retained<NSTextField>,
    facts: Retained<NSTextField>,
    badges: Retained<NSStackView>,
    /// One capsule for each genre, which leads to the others of it.
    genres: Retained<NSStackView>,
    play: Pill,
    watched: Pill,
    download: Pill,
    /// Beside the download button while a download is on its way: how far
    /// it has got.
    downloaded: Retained<NSTextField>,
    /// What the two buttons do, again, in the menu a secondary click on
    /// the header opens.
    menu_play: Retained<NSMenuItem>,
    menu_watched: Retained<NSMenuItem>,
    remaining: Retained<NSTextField>,
    summary: Retained<NSTextField>,
    /// Under a summary that is cut short. Leads to all of it.
    more: Retained<HoverView>,
    /// Who directed it and who made it.
    credits: Retained<NSTextField>,
    seasons: Retained<NSPopUpButton>,
    season_size: Retained<NSTextField>,
    on_event: Rc<dyn Fn(Event)>,
}

impl Detail {
    /// An empty page for an item of `kind`. `on_action` is called when the
    /// viewer asks to open, play or mark the item or something listed
    /// under it.
    pub fn new(
        main_thread: MainThreadMarker,
        images: &Rc<Images>,
        downloads: &Rc<Downloads>,
        kind: Kind,
        opening: Opening,
        on_action: impl Fn(Action) + 'static,
    ) -> Rc<Self> {
        let on_action: Rc<OnAction> = Rc::new(on_action);
        Rc::new_cyclic(|page: &Weak<Self>| {
            let target = |act: fn(&Self)| {
                let page = Weak::clone(page);
                Callback::new(main_thread, move |_| {
                    if let Some(page) = page.upgrade() {
                        act(&page);
                    }
                })
            };
            let targets = [
                target(Self::press_play),
                target(Self::press_watched),
                target(Self::press_download),
                target(Self::season_chosen),
            ];
            let heard = Weak::clone(page);
            let hero = Hero::new(main_thread, kind, move |event| {
                if let Some(page) = heard.upgrade() {
                    page.hear(event);
                }
            });
            let [play, watched, download, season] = &targets;
            // SAFETY: the page keeps the targets for as long as it keeps
            // the controls.
            unsafe {
                hero.play.button.setTarget(Some(play));
                hero.play.button.setAction(Some(Callback::action()));
                hero.watched.button.setTarget(Some(watched));
                hero.watched.button.setAction(Some(Callback::action()));
                hero.download.button.setTarget(Some(download));
                hero.download.button.setAction(Some(Callback::action()));
                hero.menu_play.setTarget(Some(play));
                hero.menu_play.setAction(Some(Callback::action()));
                hero.menu_watched.setTarget(Some(watched));
                hero.menu_watched.setAction(Some(Callback::action()));
                hero.seasons.setTarget(Some(season));
                hero.seasons.setAction(Some(Callback::action()));
            }

            // Until the header has been laid out and says how tall it is.
            // Only a show has seasons to pick from under it.
            let height = match kind {
                Kind::Show => theme::HERO_HEIGHT + theme::SEASON_BAR_HEIGHT,
                _ => theme::HERO_HEIGHT,
            };
            let header = Header { view: Retained::into_super(hero.view.clone()), height };
            let (listed, from_list) = (Weak::clone(page), Rc::clone(&on_action));
            let list = Posters::new(
                main_thread,
                Rc::clone(images),
                Some(header),
                // The header belongs to the first shelf's place, so there
                // is always a shelf, if only an empty one.
                vec![Outline { title: "", style: Style::List, count: 0 }],
                None,
                move |action| match (action, listed.upgrade()) {
                    // Only the page knows which library the person is to
                    // be looked up in.
                    (Action::OpenPerson(person), Some(page)) => page.open_person(&person),
                    (action, _) => from_list(action),
                },
                || {},
            );
            let backdrop = Backdrop::new(main_thread);
            layout::fill(&backdrop, list.view());

            Self {
                backdrop,
                list,
                hero,
                images: Rc::clone(images),
                downloads: Rc::clone(downloads),
                on_action,
                item: RefCell::new(None),
                episodes: RefCell::new(Vec::new()),
                related: RefCell::new(Vec::new()),
                season: RefCell::new(opening.season.clone()),
                opening: RefCell::new(Some(opening)),
                header_height: Cell::new(height),
                _targets: targets,
            }
        })
    }

    pub fn view(&self) -> &NSView {
        &self.backdrop
    }

    pub fn focus(&self) {
        self.list.focus();
    }

    /// Scrolls the page so that the content `y` points down is at the top.
    pub fn scroll_to(&self, y: f64) {
        self.list.scroll_to(y);
    }

    fn hear(&self, event: Event) {
        match event {
            Event::Follow(link) => self.follow_link(link),
            Event::Play => self.press_play(),
            Event::OpenGenre(genre) => self.open_genre(&genre),
            Event::Resized(height) => self.fit_header(height),
        }
    }

    /// Gives the header the height its content takes.
    fn fit_header(&self, height: f64) {
        if self.header_height.replace(height) != height {
            self.list.set_header_height(height);
        }
    }

    /// Plays the item, or for a show, the episode to watch next.
    pub fn press_play(&self) {
        if let Some(item) = self.playable() {
            (self.on_action)(Action::Play(item));
        }
    }

    /// Downloads the item, or deletes its download if it has one, be it
    /// whole or on its way. One that stopped short is carried on with.
    pub fn press_download(&self) {
        let item = self.item.borrow().clone();
        if let Some(item) = item.filter(is_downloadable) {
            (self.on_action)(Action::ToggleDownload(item));
        }
    }

    /// Marks the item as watched if it is not, and the other way round.
    pub fn press_watched(&self) {
        let item = self.item.borrow().clone();
        if let Some(item) = item {
            let watched = !item.is_watched();
            (self.on_action)(Action::SetWatched(item, watched));
        }
    }

    /// Does what a click on `link` does on an episode's page. The poster
    /// and the show's name lead to the show, the season's name to the
    /// season.
    pub fn follow_link(&self, link: Link) {
        // Cloned so the handler is free to replace the page.
        let item = self.item.borrow().clone();
        let Some(episode) = item.filter(|item| item.kind == Kind::Episode) else {
            return;
        };
        (self.on_action)(match link {
            Link::Show => Action::OpenShow(episode),
            Link::Season => Action::OpenSeason(episode),
        });
    }

    /// Fills the page with `item`. Called again when the server has more
    /// to say about it, such as how far it was watched.
    pub fn show(&self, item: &Item) {
        self.hero.show(item, &self.images);
        if let Some(colors) = item.colors {
            self.backdrop.set_colors(colors);
        }
        let art = match item.kind {
            Kind::Episode => item.grandparent_art.as_deref().or(item.art.as_deref()),
            _ => item.art.as_deref(),
        };
        if let Some(path) = art {
            self.backdrop.show_art(path, &self.images);
        }
        *self.item.borrow_mut() = Some(item.clone());
        self.show_play_button();
        self.show_download();
        self.list.show(self.shelves());
    }

    /// Brings the download button up to date with where the download of
    /// the item stands. Only a film or an episode has the button.
    pub fn show_download(&self) {
        let item = self.item.borrow();
        let (pill, so_far) = (&self.hero.download, &self.hero.downloaded);
        let entry = item
            .as_ref()
            .filter(|item| is_downloadable(item))
            .map(|item| self.downloads.find(&item.rating_key));
        pill.button.setHidden(entry.is_none());
        let state = entry.flatten().map(|entry| (entry.state, entry.download.size));
        let (symbol, title, percent) = match &state {
            None => ("arrow.down.circle", "Download", None),
            Some((State::Downloading(transfer), size)) => {
                ("xmark", "Cancel Download", Some(text::percent(transfer.received(), *size)))
            }
            Some((State::Incomplete { .. }, _)) => ("arrow.clockwise", "Resume Download", None),
            Some((State::Downloaded, _)) => ("arrow.down.circle.fill", "Delete Download", None),
        };
        pill.set_symbol(symbol);
        pill.set_title(title);
        pill.set_lit(matches!(state, Some((State::Downloaded, _))));
        label::set(so_far, &percent.map(|percent| format!("{percent}%")).unwrap_or_default());
        so_far.setHidden(percent.is_none());
    }

    /// Fills in the episodes: every one of the show on the page, in order,
    /// or for an episode's page those of its season.
    pub fn show_episodes(&self, episodes: Vec<Item>) {
        let seasons = seasons(&episodes);
        // Without a season to open on, open on the one to watch next.
        let wanted = self.season.borrow().clone();
        let wanted = wanted.or_else(|| next_up(&episodes)?.parent_rating_key.clone());
        let chosen = seasons.iter().position(|(key, _)| Some(key) == wanted.as_ref()).unwrap_or(0);
        *self.season.borrow_mut() = seasons.get(chosen).map(|(key, _)| key.clone());
        *self.episodes.borrow_mut() = episodes;

        let titles: Vec<Retained<NSString>> =
            seasons.iter().map(|(_, title)| NSString::from_str(title)).collect();
        let picker = &self.hero.seasons;
        picker.removeAllItems();
        picker.addItemsWithTitles(&NSArray::from_retained_slice(&titles));
        picker.selectItemAtIndex(chosen.cast_signed());
        self.show_play_button();
        self.list.show(self.shelves());
        self.open_as_asked();
    }

    /// Fills in the rows the server relates to the item.
    pub fn show_related(&self, related: Vec<Hub>) {
        *self.related.borrow_mut() = related;
        self.list.show(self.shelves());
    }

    /// Once the episodes are listed, goes where the page was opened to
    /// go: for a show, down to the season's list and onto an episode in
    /// it, and for an episode, sideways to its place in the season.
    fn open_as_asked(&self) {
        let (Some(item), Some(opening)) = (self.item.borrow().clone(), self.opening.take()) else {
            return;
        };
        match item.kind {
            Kind::Show => {
                if opening.at_episodes {
                    // The header says how tall it is when it is laid out.
                    if let Some(window) = self.hero.view.window() {
                        window.layoutIfNeeded();
                    }
                    self.list.scroll_to(self.header_height.get() - theme::SEASON_BAR_HEIGHT);
                }
                if let Some(episode) = &opening.episode {
                    self.list.select(episode);
                }
            }
            Kind::Episode => self.list.reveal(&item.rating_key),
            Kind::Movie | Kind::Season | Kind::Clip | Kind::Other => {}
        }
    }

    fn season_chosen(&self) {
        let chosen = usize::try_from(self.hero.seasons.indexOfSelectedItem()).ok();
        let seasons = seasons(&self.episodes.borrow());
        if let Some((key, _)) = chosen.and_then(|chosen| seasons.into_iter().nth(chosen)) {
            *self.season.borrow_mut() = Some(key);
            self.list.show(self.shelves());
        }
    }

    /// Asks for the list of what else `person` is in, in the library the
    /// item on the page is in.
    fn open_person(&self, person: &Role) {
        let library = self.item.borrow().as_ref().and_then(|item| item.library_section_id);
        if let Some(path) = library.and_then(|library| appearances(library, person)) {
            (self.on_action)(Action::OpenList { title: person.tag.clone(), path });
        }
    }

    /// Asks for the list of everything of `genre` in the library the item
    /// on the page is in.
    fn open_genre(&self, genre: &Tag) {
        let library = self.item.borrow().as_ref().and_then(|item| item.library_section_id);
        if let Some(path) = library.and_then(|library| of_genre(library, genre)) {
            (self.on_action)(Action::OpenList { title: genre.tag.clone(), path });
        }
    }

    /// Everything under the header, from what is known so far.
    fn shelves(&self) -> Vec<Shelf> {
        let item = self.item.borrow();
        let Some(item) = &*item else {
            return Vec::new();
        };
        let episodes = self.episodes.borrow();
        let mut shelves: Vec<Shelf> = Vec::new();
        match item.kind {
            Kind::Show => {
                let season = self.season.borrow();
                let listed: Vec<Item> = episodes
                    .iter()
                    .filter(|episode| episode.parent_rating_key == *season)
                    .cloned()
                    .collect();
                let size = u32::try_from(listed.len()).unwrap_or(u32::MAX);
                label::set(&self.hero.season_size, &text::count(size, "episode"));
                self.hero.season_bar_hidden(listed.is_empty());
                let mut list = Shelf::of("", Style::List, listed);
                list.current = next_up(&episodes).map(|episode| episode.rating_key.clone());
                shelves.push(list);
            }
            // A season of one has no others to show.
            Kind::Episode if episodes.len() > 1 => {
                let title = item.parent_title.clone().unwrap_or_else(|| "Season".to_owned());
                let style = Style::Shelf(Shape::Still);
                let mut season = Shelf::of(title, style, episodes.clone());
                season.current = Some(item.rating_key.clone());
                shelves.push(season);
            }
            _ => {}
        }

        // A list knows the cast only by name. The photos and the rest
        // come with the item in full, and the shelf waits for them.
        let cast: Vec<Card> = item
            .roles
            .iter()
            .filter(|person| person.id.is_some())
            .take(CAST_SHOWN)
            .cloned()
            .map(Card::Person)
            .collect();
        if !cast.is_empty() {
            let (title, style) = ("Cast".to_owned(), Style::Shelf(Shape::Portrait));
            shelves.push(Shelf { title, style, cards: cast, more: None, current: None });
        }
        if !item.extras.items.is_empty() {
            let style = Style::Shelf(Shape::Landscape);
            shelves.push(Shelf::of("Trailers & Extras", style, item.extras.items.clone()));
        }
        for hub in self.related.borrow().iter() {
            let style = Style::Shelf(Shape::Poster);
            let mut shelf = Shelf::of(hub.title.clone(), style, hub.items.clone());
            shelf.more = hub.more.clone();
            shelves.push(shelf);
        }
        if shelves.is_empty() {
            // The header belongs to the first shelf's place.
            shelves.push(Shelf::of("", Style::List, Vec::new()));
        }
        shelves
    }

    /// What the play button plays: the item itself, or for a show, the
    /// episode to watch next.
    fn playable(&self) -> Option<Item> {
        let item = self.item.borrow().clone()?;
        match item.kind {
            Kind::Show => next_up(&self.episodes.borrow()).cloned(),
            Kind::Movie | Kind::Episode | Kind::Clip => Some(item),
            Kind::Season | Kind::Other => None,
        }
    }

    /// "Play", "Resume", or for a show "Play S2 · E15".
    fn show_play_button(&self) {
        let hero = &self.hero;
        let playable = self.playable();
        hero.play.button.setHidden(playable.is_none());
        hero.menu_play.setHidden(playable.is_none());
        let Some(playable) = playable else {
            return;
        };
        let verb = if playable.view_offset.is_some() { "Resume" } else { "Play" };
        let is_show = self.item.borrow().as_ref().is_some_and(|item| item.kind == Kind::Show);
        let title = match text::episode_code(&playable).filter(|_| is_show) {
            Some(code) => format!("{verb} {code}"),
            None => verb.to_owned(),
        };
        hero.play.set_title(&title);
        hero.menu_play.setTitle(&NSString::from_str(&title));
        let remaining = text::remaining(&playable);
        label::set(&hero.remaining, remaining.as_deref().unwrap_or_default());
        hero.remaining.setHidden(remaining.is_none());
    }
}

/// Whether `item` is something to keep on this Mac: a film or an episode
/// with a file whose size the server knows.
fn is_downloadable(item: &Item) -> bool {
    let part = item.media.first().and_then(|media| media.parts.first());
    matches!(item.kind, Kind::Movie | Kind::Episode) && part.is_some_and(|part| part.size.is_some())
}

/// The seasons that `episodes` belong to, in order, as key and title.
fn seasons(episodes: &[Item]) -> Vec<(RatingKey, String)> {
    let mut seasons: Vec<(RatingKey, String)> = Vec::new();
    for episode in episodes {
        let Some(key) = &episode.parent_rating_key else {
            continue;
        };
        if seasons.last().is_none_or(|(last, _)| last != key) {
            let title = episode.parent_title.clone().unwrap_or_else(|| "Season".to_owned());
            seasons.push((key.clone(), title));
        }
    }
    seasons
}

impl Hero {
    /// The header for an item of `kind`. `on_event` is called when a link
    /// in the header is clicked and each time the header is laid out.
    fn new(main_thread: MainThreadMarker, kind: Kind, on_event: impl Fn(Event) + 'static) -> Self {
        let is_episode = kind == Kind::Episode;
        let on_event: Rc<dyn Fn(Event)> = Rc::new(on_event);
        let follow = |link| {
            let on_event = Rc::clone(&on_event);
            move || on_event(Event::Follow(link))
        };
        let (primary, secondary) = (NSColor::labelColor(), NSColor::secondaryLabelColor());
        let line = |size, weight, color| label::line(size, weight, color, main_thread);

        let art = Rc::new(ArtView::new(main_thread, theme::RADIUS_L));
        art.set_flat();
        art.set_playable(is_episode);
        let (hovered, played) = (Rc::clone(&art), Rc::clone(&on_event));
        let art_link = HoverView::pressable(
            main_thread,
            move |over| hovered.set_hovered(over),
            move || played(Event::Play),
        );
        // A poster stands for the film. Only a frame of an episode, with
        // the play glyph on it, is there to be pressed.
        art_link.set_live(is_episode);
        art_link.setAccessibilityElement(is_episode);
        // SAFETY: the role is a constant that AppKit provides.
        art_link.setAccessibilityRole(Some(unsafe { NSAccessibilityButtonRole }));
        art_link.setAccessibilityLabel(Some(ns_string!("Play")));
        art_link.setTranslatesAutoresizingMaskIntoConstraints(false);
        layout::cover(&art_link, art.view());
        // The shadow sits on the link, since the artwork's own layer has
        // a shadow for the lift under the pointer.
        art_link.setWantsLayer(true);
        if let Some(layer) = art_link.layer() {
            layer.setShadowOpacity(theme::HERO_SHADOW_OPACITY);
            layer.setShadowRadius(theme::HERO_SHADOW_RADIUS);
            layer.setShadowOffset(CGSize { width: 0.0, height: -theme::HERO_SHADOW_DROP });
        }
        let show_poster = Rc::new(ArtView::new(main_thread, theme::RADIUS_S));
        let lifted = Rc::clone(&show_poster);
        let show_poster_link =
            HoverView::link(main_thread, move |over| lifted.set_hovered(over), follow(Link::Show));
        show_poster_link.setTranslatesAutoresizingMaskIntoConstraints(false);
        layout::cover(&show_poster_link, show_poster.view());
        show_poster_link.setHidden(!is_episode);
        show_poster_link.setAccessibilityElement(true);
        let (show_link, show_name) = text_link(follow(Link::Show), main_thread);
        let (season_link, season_name) = text_link(follow(Link::Season), main_thread);
        let episode_number = line(theme::FONT_HEADLINE, Weight::Semibold, &secondary);
        let step = || {
            let step = line(theme::FONT_HEADLINE, Weight::Semibold, &secondary);
            label::set(&step, "›");
            // Punctuation between links, not something to read out.
            step.setAccessibilityElement(false);
            step
        };
        let (first_step, second_step) = (step(), step());
        let last: [&NSView; 2] = [&second_step, &episode_number];
        let last = NSStackView::stackViewWithViews(&NSArray::from_slice(&last), main_thread);
        last.setSpacing(theme::SPACE_S);
        let eyebrow: [&NSView; 4] = [&show_link, &first_step, &season_link, &last];
        let path = NSArray::from_slice(&eyebrow);
        let eyebrow = NSStackView::new(main_thread);
        eyebrow.setViews_inGravity(&path, NSStackViewGravity::Leading);
        eyebrow.setSpacing(theme::SPACE_S);
        // Where the two links do not fit beside the number, the number
        // goes: the title under it is the episode's own.
        eyebrow.setClippingResistancePriority_forOrientation(
            NSLayoutPriorityDefaultLow,
            NSLayoutConstraintOrientation::Horizontal,
        );
        let if_necessary = NSStackViewVisibilityPriorityDetachOnlyIfNecessary;
        eyebrow.setVisibilityPriority_forView(if_necessary, &last);
        let title = label::paragraph(
            theme::FONT_LARGE_TITLE,
            Weight::Bold,
            &primary,
            theme::HERO_TITLE_LINES,
            main_thread,
        );
        let badges = NSStackView::new(main_thread);
        badges.setSpacing(theme::SPACE_S);
        let genres = NSStackView::new(main_thread);
        genres.setSpacing(theme::SPACE_S);

        let play = Pill::new("Play", "play.fill", true, main_thread);
        // The default action of the page, which Return triggers.
        play.button.setKeyEquivalent(ns_string!("\r"));
        let watched = Pill::new("Mark as Watched", "checkmark", false, main_thread);
        let download = Pill::new("Download", "arrow.down.circle", false, main_thread);
        download.button.setHidden(true);
        let downloaded = line(theme::FONT_BODY, Weight::Regular, &secondary);
        downloaded.setHidden(true);
        let remaining = line(theme::FONT_BODY, Weight::Regular, &secondary);
        // Placed by gravity, which is what lets the stack drop a view that
        // does not fit.
        let row: [&NSView; 5] =
            [&play.button, &watched.button, &download.button, &downloaded, &remaining];
        let buttons = NSStackView::new(main_thread);
        buttons.setViews_inGravity(&NSArray::from_slice(&row), NSStackViewGravity::Leading);
        buttons.setSpacing(theme::SPACE_M);
        // In a narrow window the row gives up how much is left to watch,
        // whole, before anything in it is cut short.
        buttons.setClippingResistancePriority_forOrientation(
            NSLayoutPriorityDefaultLow,
            NSLayoutConstraintOrientation::Horizontal,
        );
        buttons.setVisibilityPriority_forView(if_necessary, &remaining);

        let seasons = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(main_thread),
            NSRect::ZERO,
            false,
        );
        seasons.setControlSize(NSControlSize::Large);
        seasons.setAccessibilityLabel(Some(ns_string!("Season")));

        let summary = label::paragraph(
            theme::FONT_HEADLINE,
            Weight::Regular,
            &primary,
            theme::HERO_SUMMARY_LINES,
            main_thread,
        );
        let (headed, read) = (title.clone(), summary.clone());
        let (more, more_label) = text_link(move || read_in_full(&headed, &read), main_thread);
        more_label.setFont(Some(&label::font(theme::FONT_BODY, Weight::Semibold)));
        label::set(&more_label, "More");
        more.setAccessibilityLabel(Some(ns_string!("Read the whole summary")));
        more.setHidden(true);

        let (menu_play, menu_watched) =
            (NSMenuItem::new(main_thread), NSMenuItem::new(main_thread));
        let menu = NSMenu::new(main_thread);
        menu.addItem(&menu_play);
        menu.addItem(&menu_watched);
        let view = HeaderView::new(main_thread);
        // SAFETY: the view keeps the menu, as a responder keeps the one it
        // is given.
        unsafe { view.setMenu(Some(&menu)) };

        let hero = Self {
            view,
            art,
            art_link,
            show_poster,
            show_poster_link,
            eyebrow,
            show_link,
            show_name,
            season_link,
            season_name,
            episode_number,
            title,
            // Two lines, so that a narrow window wraps the last facts
            // and does not cut them off.
            facts: label::paragraph(
                theme::FONT_HEADLINE,
                Weight::Medium,
                &secondary,
                theme::HERO_FACTS_LINES,
                main_thread,
            ),
            badges,
            genres,
            play,
            watched,
            download,
            downloaded,
            menu_play,
            menu_watched,
            remaining,
            summary,
            more,
            credits: line(theme::FONT_BODY, Weight::Regular, &secondary),
            seasons,
            season_size: line(theme::FONT_BODY, Weight::Regular, &secondary),
            on_event,
        };
        hero.season_bar_hidden(true);
        hero.place(&buttons, kind);
        hero
    }

    /// The artwork on the left, the text in a column beside it, and for a
    /// show the season picker in a strip along the bottom, where the
    /// episode list begins.
    fn place(&self, buttons: &NSStackView, kind: Kind) {
        let main_thread = self.view.mtm();
        let (summary, more) = (self.summary.clone(), self.more.clone());
        let column = Column::new(main_thread, move || offer_more(&summary, &more));

        // The show's poster stands beside the lines that name the episode.
        let titles: [&NSView; 2] = [&self.eyebrow, &self.title];
        let titles = NSStackView::stackViewWithViews(&NSArray::from_slice(&titles), main_thread);
        titles.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        titles.setAlignment(NSLayoutAttribute::Leading);
        titles.setSpacing(theme::SPACE_XS);
        let headline: [&NSView; 2] = [&self.show_poster_link, &titles];
        let headline =
            NSStackView::stackViewWithViews(&NSArray::from_slice(&headline), main_thread);
        headline.setAlignment(NSLayoutAttribute::Top);
        headline.setSpacing(theme::SPACE_M);

        let lines: [&NSView; 8] = [
            &headline,
            &self.facts,
            &self.badges,
            buttons,
            &self.summary,
            &self.more,
            &self.genres,
            &self.credits,
        ];
        for line in lines {
            column.addArrangedSubview(line);
        }
        column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        column.setAlignment(NSLayoutAttribute::Leading);
        column.setSpacing(theme::SPACE_M);
        column.setCustomSpacing_afterView(theme::SPACE_S, &headline);
        column.setCustomSpacing_afterView(theme::SPACE_L, &self.badges);
        column.setCustomSpacing_afterView(theme::SPACE_L, buttons);
        column.setCustomSpacing_afterView(theme::SPACE_S, &self.summary);
        column.setTranslatesAutoresizingMaskIntoConstraints(false);

        let season: [&NSView; 2] = [&self.seasons, &self.season_size];
        let season = NSStackView::stackViewWithViews(&NSArray::from_slice(&season), main_thread);
        season.setSpacing(theme::SPACE_M);
        season.setTranslatesAutoresizingMaskIntoConstraints(false);

        let (view, art) = (&self.view, &self.art_link);
        view.addSubview(art);
        view.addSubview(&column);
        view.addSubview(&season);
        // The text may run past the artwork: a long title in a narrow
        // window takes three lines and leaves the rest no less room.
        let season_bar = if kind == Kind::Show { theme::SEASON_BAR_HEIGHT } else { 0.0 };
        let (on_event, pictured, written) =
            (Rc::clone(&self.on_event), art.clone(), column.clone());
        view.on_layout(move || {
            let content = written.frame().size.height.max(pictured.frame().size.height).ceil();
            on_event(Event::Resized(2.0 * theme::SPACE_L + content + season_bar));
        });

        let margin = theme::PAGE_MARGIN;
        // The strip is the last thing in the header.
        let bar_middle = -theme::SEASON_BAR_HEIGHT / 2.0;
        let (wide, tall) = (art.widthAnchor(), art.heightAnchor());
        let shape = match kind {
            Kind::Episode => {
                let share = wide.constraintEqualToAnchor_multiplier(
                    &view.widthAnchor(),
                    theme::HERO_STILL_SHARE,
                );
                // As much of the page as it can have within its bounds.
                share.setPriority(NSLayoutPriorityDefaultHigh);
                vec![
                    share,
                    wide.constraintGreaterThanOrEqualToConstant(theme::HERO_STILL_MIN_WIDTH),
                    wide.constraintLessThanOrEqualToConstant(theme::HERO_STILL_MAX_WIDTH),
                    tall.constraintEqualToAnchor_multiplier(&wide, theme::LANDSCAPE_ASPECT),
                ]
            }
            _ => vec![
                wide.constraintEqualToConstant(theme::HERO_POSTER_WIDTH),
                tall.constraintEqualToAnchor_multiplier(&wide, theme::POSTER_ASPECT),
            ],
        };
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&shape));
        let small = self.show_poster_link.widthAnchor();
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            small.constraintEqualToConstant(theme::HERO_SHOW_POSTER_WIDTH),
            self.show_poster_link
                .heightAnchor()
                .constraintEqualToAnchor_multiplier(&small, theme::POSTER_ASPECT),
            art.leadingAnchor().constraintEqualToAnchor_constant(&view.leadingAnchor(), margin),
            art.topAnchor().constraintEqualToAnchor_constant(&view.topAnchor(), theme::SPACE_L),
            column
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&art.trailingAnchor(), theme::SPACE_XXL),
            column.topAnchor().constraintEqualToAnchor(&art.topAnchor()),
            column
                .trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&view.trailingAnchor(), -margin),
            // A title has the width of the page. Only running text is
            // kept to a width that reads well.
            self.summary.widthAnchor().constraintLessThanOrEqualToConstant(theme::HERO_TEXT_WIDTH),
            buttons.trailingAnchor().constraintLessThanOrEqualToAnchor(&column.trailingAnchor()),
            season.leadingAnchor().constraintEqualToAnchor_constant(&view.leadingAnchor(), margin),
            season
                .centerYAnchor()
                .constraintEqualToAnchor_constant(&view.bottomAnchor(), bar_middle),
        ]));
    }

    fn season_bar_hidden(&self, hidden: bool) {
        self.seasons.setHidden(hidden);
        self.season_size.setHidden(hidden);
    }

    fn show(&self, item: &Item, images: &Rc<Images>) {
        let set = |label: &NSTextField, words: &str| {
            label::set(label, words);
            label.setHidden(words.is_empty());
        };
        let is_episode = item.kind == Kind::Episode;
        self.eyebrow.setHidden(!is_episode);
        let show = item.grandparent_title.as_deref().unwrap_or_default();
        let season = item.parent_title.as_deref().unwrap_or_default();
        set(&self.show_name, show);
        set(&self.season_name, season);
        for (link, leads_to) in [(&self.show_link, show), (&self.season_link, season)] {
            link.setAccessibilityLabel(Some(&NSString::from_str(&format!("Go to {leads_to}"))));
        }
        self.show_poster_link
            .setAccessibilityLabel(Some(&NSString::from_str(&format!("Poster. Go to {show}"))));
        let number = item.index.map(|number| format!("Episode {number}"));
        set(&self.episode_number, number.as_deref().unwrap_or_default());
        set(&self.title, &item.title);
        // A line may break after a dot between two facts, never before it.
        set(&self.facts, &facts(item).replace(" · ", "\u{a0}· "));
        set(&self.summary, item.summary.as_deref().unwrap_or_default());
        set(&self.credits, &credits(item));

        let main_thread = self.view.mtm();
        let genres: Vec<Retained<NSView>> = item
            .genres
            .iter()
            .take(GENRES_SHOWN)
            .map(|genre| {
                let (on_event, opened) = (Rc::clone(&self.on_event), genre.clone());
                chip(genre, move || on_event(Event::OpenGenre(opened.clone())), main_thread)
            })
            .collect();
        self.genres.setViews_inGravity(
            &NSArray::from_retained_slice(&genres),
            NSStackViewGravity::Leading,
        );
        self.genres.setHidden(genres.is_empty());
        let media = item.media.first();
        let mut badges: Vec<Retained<NSView>> = media
            .map(formats::badges)
            .unwrap_or_default()
            .iter()
            .map(|words| badge(words, main_thread))
            .collect();
        let file = media.map(formats::file_facts).unwrap_or_default();
        if !file.is_empty() {
            let secondary = NSColor::secondaryLabelColor();
            let plain = label::line(theme::FONT_CAPTION, Weight::Medium, &secondary, main_thread);
            label::set(&plain, &file);
            badges.push(Retained::into_super(Retained::into_super(plain)));
        }
        self.badges.setViews_inGravity(
            &NSArray::from_retained_slice(&badges),
            NSStackViewGravity::Leading,
        );
        self.badges.setHidden(badges.is_empty());

        let toggle = match item.is_watched() {
            true => "Mark as Unwatched",
            false => "Mark as Watched",
        };
        self.watched.set_title(toggle);
        self.menu_watched.setTitle(&NSString::from_str(toggle));
        self.watched.set_lit(item.is_watched());

        // An episode's own artwork is a frame of it, a film's is its poster.
        let request = match is_episode {
            true => theme::HERO_STILL_REQUEST,
            false => theme::HERO_POSTER_REQUEST,
        };
        self.art.show(item.thumb.as_deref(), request, images);
        self.art.set_progress(item.progress());
        if is_episode {
            let poster = item.grandparent_thumb.as_deref();
            self.show_poster.show(poster, theme::HERO_SHOW_POSTER_REQUEST, images);
        }
    }
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `HeaderView` does
    // not implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustDetailHeader"]
    #[ivars = OnceCell<Box<dyn Fn()>>]
    struct HeaderView;

    impl HeaderView {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(OnceCell::new());
            // SAFETY: forwards the designated initializer unchanged.
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        #[unsafe(method(layout))]
        fn layout(&self) {
            // SAFETY: forwards the call unchanged.
            let () = unsafe { msg_send![super(self), layout] };
            if let Some(on_layout) = self.ivars().get() {
                on_layout();
            }
        }
    }
);

impl HeaderView {
    fn new(main_thread: MainThreadMarker) -> Retained<Self> {
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        unsafe { msg_send![Self::alloc(main_thread), initWithFrame: NSRect::ZERO] }
    }

    /// Has `on_layout` called each time the view has placed what is
    /// directly in it, which is when it is known how tall each part is.
    fn on_layout(&self, on_layout: impl Fn() + 'static) {
        // Cannot matter if it fails: the header is placed once.
        let _ = self.ivars().set(Box::new(on_layout));
    }
}

define_class!(
    // SAFETY: NSStackView may be subclassed, and `Column` does not
    // implement `Drop`.
    #[unsafe(super(NSStackView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustDetailColumn"]
    #[ivars = OnceCell<Box<dyn Fn()>>]
    struct Column;

    impl Column {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(OnceCell::new());
            // SAFETY: forwards the designated initializer unchanged.
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        #[unsafe(method(layout))]
        fn layout(&self) {
            // SAFETY: forwards the call unchanged.
            let () = unsafe { msg_send![super(self), layout] };
            if let Some(on_layout) = self.ivars().get() {
                on_layout();
            }
        }
    }
);

impl Column {
    /// A stack that calls `on_layout` each time it has placed its lines,
    /// which is when it is known how much room each one got.
    fn new(main_thread: MainThreadMarker, on_layout: impl Fn() + 'static) -> Retained<Self> {
        let column: Retained<Self> =
            // SAFETY: `initWithFrame:` is NSView's designated initializer.
            unsafe { msg_send![Self::alloc(main_thread), initWithFrame: NSRect::ZERO] };
        // Cannot fail: the cell was created empty by the initializer.
        let _ = column.ivars().set(Box::new(on_layout));
        column
    }
}

/// Shows the link to the whole summary when the summary is cut short: by
/// its limit of lines, or by the room the column had left for it. The
/// room counts the link's own line, so that showing the link cannot be
/// what cuts the summary short.
fn offer_more(summary: &NSTextField, more: &NSView) {
    let shown = summary.frame().size;
    if summary.isHidden() || shown.width <= 0.0 {
        return more.setHidden(true);
    }
    let unbounded = NSSize::new(shown.width, f64::MAX);
    // The label's own measure stops at its limit of lines. That of the
    // text, wrapped to the same width, does not.
    let limit = summary.sizeThatFits(unbounded).height;
    let wrapped = NSStringDrawingOptions::UsesLineFragmentOrigin;
    let whole = summary.attributedStringValue();
    let whole = whole.boundingRectWithSize_options_context(unbounded, wrapped, None).size.height;
    let room = match more.isHidden() {
        true => shown.height,
        false => shown.height + theme::SPACE_S + more.frame().size.height,
    };
    // Half a point of slack: sizes come back rounded to the pixel.
    let cut_short = whole > limit.min(room) + 0.5;
    if more.isHidden() == cut_short {
        more.setHidden(!cut_short);
    }
}

/// Opens the whole of `summary` under it, headed by `title`, in a popover
/// that goes away with a click anywhere else.
fn read_in_full(title: &NSTextField, summary: &NSTextField) {
    let controller = NSViewController::new(summary.mtm());
    controller.setView(&whole_summary(title, summary));
    let popover = NSPopover::new(summary.mtm());
    popover.setContentViewController(Some(&controller));
    popover.setBehavior(NSPopoverBehavior::Transient);
    // Below the text it continues. AppKit keeps the popover while it is
    // shown.
    popover.showRelativeToRect_ofView_preferredEdge(summary.bounds(), summary, NSRectEdge::MinY);
}

/// The text of `summary` in full under the text of `title`, as tall as
/// that takes.
fn whole_summary(title: &NSTextField, summary: &NSTextField) -> Retained<NSView> {
    let main_thread = summary.mtm();
    let primary = NSColor::labelColor();
    let heading = label::line(theme::FONT_HEADLINE, Weight::Semibold, &primary, main_thread);
    heading.setStringValue(&title.stringValue());
    let body = label::paragraph(theme::FONT_BODY, Weight::Regular, &primary, 0, main_thread);
    body.setStringValue(&summary.stringValue());
    // So that a line of it can be copied.
    body.setSelectable(true);

    let page = NSView::new(main_thread);
    page.addSubview(&heading);
    page.addSubview(&body);
    let inset = theme::SPACE_L;
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        page.widthAnchor().constraintEqualToConstant(theme::SUMMARY_POPOVER_WIDTH),
        heading.topAnchor().constraintEqualToAnchor_constant(&page.topAnchor(), inset),
        heading.leadingAnchor().constraintEqualToAnchor_constant(&page.leadingAnchor(), inset),
        heading.trailingAnchor().constraintEqualToAnchor_constant(&page.trailingAnchor(), -inset),
        body.topAnchor().constraintEqualToAnchor_constant(&heading.bottomAnchor(), theme::SPACE_S),
        body.leadingAnchor().constraintEqualToAnchor(&heading.leadingAnchor()),
        body.trailingAnchor().constraintEqualToAnchor(&heading.trailingAnchor()),
        body.bottomAnchor().constraintEqualToAnchor_constant(&page.bottomAnchor(), -inset),
    ]));
    page
}

/// A button drawn as a capsule of our own, so that it looks the same in a
/// window that is not in front: AppKit greys the coloured bezel of a
/// system button there, and the main action of a page must not look
/// switched off.
pub(super) struct Pill {
    pub(super) button: Retained<NSButton>,
    prominent: bool,
    /// For the prominent pill: as wide as the title needs. Replaced when
    /// the title changes.
    width: RefCell<Option<Retained<NSLayoutConstraint>>>,
}

impl Pill {
    /// A prominent pill is white with dark text, for the one main action.
    /// Any other is a round glyph on a faint wash, which says what it does
    /// to a pointer resting on it and to VoiceOver, so that a row of them
    /// fits beside the main action in the narrowest window.
    pub(super) fn new(
        title: &str,
        symbol: &str,
        prominent: bool,
        main_thread: MainThreadMarker,
    ) -> Self {
        // SAFETY: the target and action are set once the page exists.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(ns_string!(""), None, None, main_thread)
        };
        button.setBordered(false);
        button.setFont(Some(&label::font(theme::FONT_BODY, Weight::Semibold)));
        button.setWantsLayer(true);
        if let Some(layer) = button.layer() {
            layer.setCornerRadius(theme::PILL_HEIGHT / 2.0);
        }
        button.setTranslatesAutoresizingMaskIntoConstraints(false);
        button.heightAnchor().constraintEqualToConstant(theme::PILL_HEIGHT).setActive(true);
        if !prominent {
            button.widthAnchor().constraintEqualToConstant(theme::PILL_HEIGHT).setActive(true);
        }
        let pill = Self { button, prominent, width: RefCell::new(None) };
        pill.set_symbol(symbol);
        pill.set_lit(prominent);
        pill.set_title(title);
        pill
    }

    fn set_symbol(&self, symbol: &str) {
        let image = artwork::symbol(symbol, theme::FONT_BODY, Weight::Semibold);
        if let (Some(image), true) = (&image, self.prominent) {
            // A button sets its title right against its image. Widening
            // the rectangle the image is laid out by puts a gap between.
            let mut size = image.size();
            size.width += theme::PILL_GLYPH_GAP;
            image.setAlignmentRect(NSRect::new(NSPoint::ZERO, size));
        }
        self.button.setImage(image.as_deref());
        self.button.setImagePosition(match self.prominent {
            true => NSCellImagePosition::ImageLeading,
            false => NSCellImagePosition::ImageOnly,
        });
        // Glyph and title together in the middle, not one at each end.
        self.button.setImageHugsTitle(true);
    }

    /// Lit, a pill is white with dark ink: the main action, or a switch
    /// that is on. Unlit it is light ink on a faint wash.
    fn set_lit(&self, lit: bool) {
        let (fill, ink) = match lit {
            true => (NSColor::whiteColor(), NSColor::blackColor()),
            false => (theme::white(theme::PILL_WASH_OPACITY), NSColor::whiteColor()),
        };
        self.button.setContentTintColor(Some(&ink));
        if let Some(layer) = self.button.layer() {
            layer.setBackgroundColor(Some(&fill.CGColor()));
        }
    }

    /// What the pill says: on its face if it is the prominent one, and
    /// otherwise to a resting pointer and to VoiceOver.
    pub(super) fn set_title(&self, title: &str) {
        let title = NSString::from_str(title);
        if !self.prominent {
            self.button.setToolTip(Some(&title));
            self.button.setAccessibilityLabel(Some(&title));
            return;
        }
        self.button.setTitle(&title);
        if let Some(old) = self.width.take() {
            old.setActive(false);
        }
        // A button without a bezel is exactly as wide as its content. The
        // capsule needs room at both ends.
        let width = self.button.intrinsicContentSize().width + 2.0 * theme::PILL_PADDING;
        let width = self.button.widthAnchor().constraintEqualToConstant(width.ceil());
        width.setActive(true);
        *self.width.borrow_mut() = Some(width);
    }
}

/// A few words that lead somewhere. They are as bright as a title, which
/// sets them apart from the plain text around them, and give way a little
/// under the pointer, which turns into a hand over them.
fn text_link(
    on_click: impl Fn() + 'static,
    main_thread: MainThreadMarker,
) -> (Retained<HoverView>, Retained<NSTextField>) {
    let primary = NSColor::labelColor();
    let label = label::line(theme::FONT_HEADLINE, Weight::Semibold, &primary, main_thread);
    let lit = label.clone();
    let link = HoverView::link(
        main_thread,
        move |over| {
            let color = if over { NSColor::secondaryLabelColor() } else { NSColor::labelColor() };
            lit.setTextColor(Some(&color));
        },
        on_click,
    );
    layout::cover(&link, &label);
    link.setAccessibilityElement(true);
    // SAFETY: the role is a constant that AppKit provides.
    link.setAccessibilityRole(Some(unsafe { NSAccessibilityLinkRole }));
    (link, label)
}

/// The line under the title, such as "2015 · 2 h 18 min · PG-13 · ★ 8.4".
fn facts(item: &Item) -> String {
    let mut facts: Vec<String> = Vec::new();
    match (item.kind, item.originally_available_at) {
        // A film is known by its year, an episode by the day it aired.
        (Kind::Episode, Some(aired)) => facts.push(text::date(aired)),
        _ => facts.extend(item.year.map(|year| year.to_string())),
    }
    match item.kind {
        // The server gives a show the length of one episode, which reads
        // as the length of the show. Its size says more.
        Kind::Show => {
            facts.extend(item.child_count.map(|seasons| text::count(seasons, "season")));
            facts.extend(item.leaf_count.map(|episodes| text::count(episodes, "episode")));
        }
        _ => facts.extend(item.duration.map(|duration| text::length(duration.into()))),
    }
    facts.extend(item.content_rating.clone());
    facts.extend(ratings(item));
    facts.join(" · ")
}

/// What critics and viewers gave `item`, each under the name of who asked
/// them: "RT 93%", "Audience 84%", "IMDb 7.7".
fn ratings(item: &Item) -> Vec<String> {
    // The server names the source by a picture of its own apps, such as
    // `rottentomatoes://image.rating.ripe`.
    fn source(image: Option<&str>) -> Option<&str> {
        image?.split_once("://").map(|(source, _)| source)
    }
    let critics = source(item.rating_image.as_deref());
    let viewers = source(item.audience_rating_image.as_deref());
    let percent = |rating: f32| format!("{:.0}%", rating * 10.0);
    let by = |source: Option<&str>, whose: &str, rating: f32| match source {
        Some("rottentomatoes") => format!("{whose} {}", percent(rating)),
        Some("imdb") => format!("IMDb {rating:.1}"),
        Some("themoviedb") => format!("TMDB {rating:.1}"),
        _ => format!("★ {rating:.1}"),
    };
    // Said once, when both numbers are from the same place.
    let viewers_of = match critics == viewers && item.rating.is_some() {
        true => "Audience",
        false => "RT Audience",
    };
    let mut ratings: Vec<String> = Vec::new();
    ratings.extend(item.rating.map(|rating| by(critics, "RT", rating)));
    ratings.extend(item.audience_rating.map(|rating| by(viewers, viewers_of, rating)));
    ratings
}

/// Who directed `item` and who made it: "Directed by A, B · Studio".
fn credits(item: &Item) -> String {
    let directors: Vec<&str> =
        item.directors.iter().take(DIRECTORS_SHOWN).map(|director| director.tag.as_str()).collect();
    let mut credits: Vec<String> = Vec::new();
    if !directors.is_empty() {
        credits.push(format!("Directed by {}", directors.join(", ")));
    }
    credits.extend(item.studio.clone());
    credits.join(" · ")
}

/// A few words in an outlined box.
fn badge(words: &str, main_thread: MainThreadMarker) -> Retained<NSView> {
    let frame = artwork::layered(main_thread);
    if let Some(layer) = frame.layer() {
        layer.setCornerRadius(theme::RADIUS_S);
        layer.setBorderWidth(theme::ARTWORK_OUTLINE_WIDTH);
        layer.setBorderColor(Some(&theme::white(theme::FACT_BADGE_BORDER_OPACITY).CGColor()));
    }
    let secondary = NSColor::secondaryLabelColor();
    let label = label::line(theme::FONT_CAPTION, Weight::Semibold, &secondary, main_thread);
    label::set(&label, words);
    frame.addSubview(&label);
    let (across, down) = (theme::SPACE_S - theme::SPACE_XXS, theme::SPACE_XXS);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        label.leadingAnchor().constraintEqualToAnchor_constant(&frame.leadingAnchor(), across),
        label.trailingAnchor().constraintEqualToAnchor_constant(&frame.trailingAnchor(), -across),
        label.topAnchor().constraintEqualToAnchor_constant(&frame.topAnchor(), down),
        label.bottomAnchor().constraintEqualToAnchor_constant(&frame.bottomAnchor(), -down),
    ]));
    frame
}

/// The name of `genre` in a capsule that leads, by `on_click`, to the
/// others of it. A genre from a listing has no number to look it up by,
/// and is a plain capsule until the item is known in full.
fn chip(
    genre: &Tag,
    on_click: impl Fn() + 'static,
    main_thread: MainThreadMarker,
) -> Retained<NSView> {
    let secondary = NSColor::secondaryLabelColor();
    let label = label::line(theme::FONT_CAPTION, Weight::Medium, &secondary, main_thread);
    label::set(&label, &genre.tag);
    let (lit, washed) = (label.clone(), Rc::new(OnceCell::<Retained<HoverView>>::new()));
    let wash = Rc::clone(&washed);
    let capsule = HoverView::link(
        main_thread,
        move |over| {
            let (ink, opacity) = match over {
                true => (NSColor::labelColor(), theme::CHIP_HOVER_OPACITY),
                false => (NSColor::secondaryLabelColor(), theme::CHIP_WASH_OPACITY),
            };
            lit.setTextColor(Some(&ink));
            if let Some(capsule) = wash.get() {
                artwork::fill_with(capsule, &theme::white(opacity));
            }
        },
        on_click,
    );
    capsule.set_live(genre.id.is_some());
    capsule.setWantsLayer(true);
    capsule.setTranslatesAutoresizingMaskIntoConstraints(false);
    artwork::fill_with(&capsule, &theme::white(theme::CHIP_WASH_OPACITY));
    if let Some(layer) = capsule.layer() {
        layer.setCornerRadius(theme::CHIP_HEIGHT / 2.0);
    }
    capsule.addSubview(&label);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        capsule.heightAnchor().constraintEqualToConstant(theme::CHIP_HEIGHT),
        label
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&capsule.leadingAnchor(), theme::CHIP_PADDING),
        label
            .trailingAnchor()
            .constraintEqualToAnchor_constant(&capsule.trailingAnchor(), -theme::CHIP_PADDING),
        label.centerYAnchor().constraintEqualToAnchor(&capsule.centerYAnchor()),
    ]));
    capsule.setAccessibilityElement(true);
    // SAFETY: the role is a constant that AppKit provides.
    capsule.setAccessibilityRole(Some(unsafe { NSAccessibilityLinkRole }));
    capsule.setAccessibilityLabel(Some(&NSString::from_str(&format!("More of {}", genre.tag))));
    // Cannot fail: the cell was made empty above.
    let _ = washed.set(capsule.clone());
    Retained::into_super(capsule)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(json: serde_json::Value) -> Item {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_show_states_its_size_and_not_the_length_of_an_episode() {
        let show = item(serde_json::json!({
            "ratingKey": "1", "type": "show", "title": "Title", "year": 2014,
            "duration": 2_520_000, "childCount": 7, "leafCount": 100,
            "contentRating": "TV-14", "audienceRating": 7.9,
        }));
        assert_eq!(facts(&show), "2014 · 7 seasons · 100 episodes · TV-14 · ★ 7.9");
    }

    #[test]
    fn a_rating_goes_under_the_name_of_who_gave_it() {
        let rated = |json: serde_json::Value| {
            let mut film = serde_json::json!({ "ratingKey": "1", "type": "movie", "title": "T" });
            film.as_object_mut().unwrap().extend(json.as_object().unwrap().clone());
            ratings(&item(film))
        };
        let both = rated(serde_json::json!({
            "rating": 9.3, "ratingImage": "rottentomatoes://image.rating.ripe",
            "audienceRating": 8.4, "audienceRatingImage": "rottentomatoes://image.rating.upright",
        }));
        assert_eq!(both, ["RT 93%", "Audience 84%"]);
        let viewers_only = rated(serde_json::json!({
            "audienceRating": 8.4, "audienceRatingImage": "rottentomatoes://image.rating.upright",
        }));
        assert_eq!(viewers_only, ["RT Audience 84%"]);
        let other_places = rated(serde_json::json!({
            "rating": 7.7, "ratingImage": "imdb://image.rating",
            "audienceRating": 7.9, "audienceRatingImage": "themoviedb://image.rating",
        }));
        assert_eq!(other_places, ["IMDb 7.7", "TMDB 7.9"]);
    }

    #[test]
    fn an_episode_states_the_day_it_aired() {
        let episode = item(serde_json::json!({
            "ratingKey": "1", "type": "episode", "title": "Title", "year": 2011,
            "originallyAvailableAt": "2011-04-24", "duration": 3_317_386,
        }));
        assert_eq!(facts(&episode), "Apr 24, 2011 · 55 min");
    }

    #[test]
    fn a_film_states_its_length() {
        let film = item(serde_json::json!({
            "ratingKey": "1", "type": "movie", "title": "Title", "year": 2015,
            "duration": 8_286_624, "contentRating": "PG-13",
        }));
        assert_eq!(facts(&film), "2015 · 2 h 18 min · PG-13");
    }

    #[test]
    fn seasons_come_from_the_episodes_in_order() {
        let episode = |season: &str| {
            item(serde_json::json!({
                "ratingKey": "9", "type": "episode", "title": "Title",
                "parentRatingKey": season, "parentTitle": format!("Season {season}"),
            }))
        };
        let titles: Vec<String> = seasons(&[episode("1"), episode("1"), episode("2")])
            .into_iter()
            .map(|(_, title)| title)
            .collect();
        assert_eq!(titles, ["Season 1", "Season 2"]);
    }
}
