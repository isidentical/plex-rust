//! The page for one film, show or episode: what it is, the buttons to
//! watch it, and under those what goes with it. For a show that is its
//! episodes season by season, for an episode the rest of its season, and
//! for all of them the cast and whatever the server finds related.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAccessibilityButtonRole, NSAccessibilityImageRole, NSAccessibilityLinkRole,
    NSAttributedStringNSExtendedStringDrawing, NSButton, NSCellImagePosition, NSColor,
    NSControlSize, NSLayoutAttribute, NSLayoutConstraint, NSLayoutConstraintOrientation,
    NSLayoutPriorityDefaultHigh, NSLayoutPriorityDefaultLow, NSMenu, NSMenuItem, NSPopUpButton,
    NSPopover, NSPopoverBehavior, NSSegmentSwitchTracking, NSSegmentedControl, NSStackView,
    NSStackViewGravity, NSStackViewVisibilityPriorityDetachOnlyIfNecessary, NSStringDrawingOptions,
    NSTextField, NSUserInterfaceLayoutOrientation, NSView, NSViewController,
};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSArray, NSPoint, NSRect, NSRectEdge, NSSize, NSString, ns_string};
use plex::library::{appearances, next_up, of_genre};
use plex::model::{Hub, Item, Kind, Media, Part, RatingKey, Role, Stream, StreamKind, Tag};

use super::artwork::{self, ArtView};
use super::backdrop::Backdrop;
use super::banner::Banner;
use super::callback::Callback;
use super::formats;
use super::hover::HoverView;
use super::label::{self, Weight};
use super::layout;
use super::poster::{Card, Shape};
use super::posters::{Action, Header, OnAction, Outline, Playback, Posters, Shelf, Style};
use super::text;
use super::theme;
use crate::downloads::{Downloads, State};
use crate::images::{Artwork, Images};

/// How many genres, directors and cast members a page names.
const GENRES_SHOWN: usize = 3;
const DIRECTORS_SHOWN: usize = 3;
const CAST_SHOWN: usize = 20;
/// Up to this many seasons are side by side to click on, which the
/// smallest window has room for. More are in a menu.
const SEASON_TABS_MOST: usize = 5;

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
    /// Holds everything, and hears the keys the list has no use for.
    view: Retained<PageView>,
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
    /// The version the viewer picked to play, by the id of the media.
    /// Without one it is the first.
    version: Cell<Option<u64>>,
    /// How tall the header is, which is as tall as its content takes at
    /// the width of the window.
    header_height: Cell<f64>,
    /// Kept because controls do not retain their targets.
    _targets: [Retained<Callback>; 8],
}

/// The views at the top of the page.
struct Hero {
    view: Retained<HeaderView>,
    /// What the page leads with: the backdrop of the film or show, to the
    /// edges of the window.
    banner: Rc<Banner>,
    /// Whether there is a backdrop, which is then given room to be seen.
    has_backdrop: Rc<Cell<bool>>,
    /// On an episode's page, a frame of the episode.
    art: Rc<ArtView>,
    /// Around it. A click on it plays the episode.
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
    /// The title of a film or show drawn as on its poster, which stands
    /// in for the words when the server has it.
    logo: Logo,
    facts: Retained<NSTextField>,
    badges: Retained<NSStackView>,
    play: Pill,
    /// Beside the button that resumes: the way back to the beginning.
    restart: Pill,
    watched: Pill,
    download: Pill,
    /// Beside the download button while a download is on its way: how far
    /// it has got.
    downloaded: Retained<NSTextField>,
    /// What the two buttons do, again, in the menu a secondary click on
    /// the header opens.
    menu_play: Retained<NSMenuItem>,
    menu_restart: Retained<NSMenuItem>,
    menu_watched: Retained<NSMenuItem>,
    remaining: Remaining,
    /// The line from the poster.
    tagline: Retained<NSTextField>,
    summary: Retained<NSTextField>,
    /// Under a summary that is cut short. Leads to all of it.
    more: Retained<HoverView>,
    /// What to play of a film or an episode: which version of it, with
    /// which sound and which subtitles. Each is there only when there is
    /// more than one to choose from.
    choices: Retained<NSStackView>,
    version: Choice,
    audio: Choice,
    subtitles: Choice,
    /// Who directed it and who made it.
    directors: Fact,
    studio: Fact,
    /// One capsule for each genre, which leads to the others of it.
    genres: Retained<NSStackView>,
    seasons: Seasons,
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
                target(Self::press_restart),
                target(Self::press_watched),
                target(Self::press_download),
                target(Self::season_chosen),
                target(Self::version_chosen),
                target(Self::audio_chosen),
                target(Self::subtitles_chosen),
            ];
            let heard = Weak::clone(page);
            let hero = Hero::new(main_thread, kind, move |event| {
                if let Some(page) = heard.upgrade() {
                    page.hear(event);
                }
            });
            let [play, restart, watched, download, season, version, audio, subtitles] = &targets;
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
                hero.restart.button.setTarget(Some(restart));
                hero.restart.button.setAction(Some(Callback::action()));
                hero.menu_restart.setTarget(Some(restart));
                hero.menu_restart.setAction(Some(Callback::action()));
                hero.menu_watched.setTarget(Some(watched));
                hero.menu_watched.setAction(Some(Callback::action()));
                hero.seasons.tabs.setTarget(Some(season));
                hero.seasons.tabs.setAction(Some(Callback::action()));
                hero.seasons.menu.setTarget(Some(season));
                hero.seasons.menu.setAction(Some(Callback::action()));
                let choices =
                    [(&hero.version, version), (&hero.audio, audio), (&hero.subtitles, subtitles)];
                for (choice, target) in choices {
                    choice.picker.setTarget(Some(target));
                    choice.picker.setAction(Some(Callback::action()));
                }
            }

            // Until the header has been laid out and says how tall it is.
            let height = theme::HERO_HEIGHT;
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
            list.let_header_reach(hero.banner.reach());
            let backdrop = Backdrop::new(main_thread);
            layout::fill(&backdrop, list.view());
            let (keyed, header) = (Weak::clone(page), hero.view.clone());
            let view = PageView::new(
                main_thread,
                move |key| {
                    if let Some(page) = keyed.upgrade() {
                        page.key_pressed(key);
                    }
                },
                // The banner is as tall as the height of the page allows.
                move || header.setNeedsLayout(true),
            );
            layout::fill(&view, &backdrop);

            Self {
                view,
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
                version: Cell::new(None),
                header_height: Cell::new(height),
                _targets: targets,
            }
        })
    }

    pub fn view(&self) -> &NSView {
        &self.view
    }

    /// Does what a key means on the page as a whole, when no card had the
    /// keyboard to take it: Space plays, as it does in the player, and
    /// Escape goes back.
    fn key_pressed(&self, key: Key) {
        match key {
            Key::Space => self.press_play(),
            Key::Escape => (self.on_action)(Action::Back),
        }
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
        self.play(false);
    }

    /// Downloads the item, or deletes its download if it has one, be it
    /// whole or on its way. One that stopped short is carried on with.
    pub fn press_download(&self) {
        let item = self.item.borrow().clone();
        if let Some(mut item) = item.filter(is_downloadable) {
            // The version picked to play is the one to keep, and like the
            // one to play it goes first.
            Playback { from_start: false, version: self.version.get() }.apply(&mut item);
            (self.on_action)(Action::ToggleDownload(item));
        }
    }

    /// Plays from its beginning what the play button would resume.
    pub fn press_restart(&self) {
        self.play(true);
    }

    fn play(&self, from_start: bool) {
        if let Some(item) = self.playable() {
            let playback = Playback { from_start, version: self.version.get() };
            (self.on_action)(Action::PlayAs(item, playback));
        }
    }

    /// Takes the version picked in the menu as the one to play, and
    /// shows what that one is.
    fn version_chosen(&self) {
        let item = self.item.borrow().clone();
        let picked = self.hero.version.chosen();
        let version = item.as_ref().zip(picked).and_then(|(item, picked)| item.media.get(picked));
        if let (Some(item), Some(version)) = (&item, version) {
            self.version.set(Some(version.id));
            self.hero.show(item, Some(version.id), &self.images);
        }
    }

    /// Picks the version whose media has the id `version`, as choosing it
    /// in the menu does.
    pub fn pick_version(&self, version: u64) {
        let item = self.item.borrow().clone();
        let has = |item: &Item| item.media.iter().any(|media| media.id == version);
        if let Some(item) = item.filter(has) {
            self.version.set(Some(version));
            self.hero.show(&item, Some(version), &self.images);
        }
    }

    /// Has the server remember the sound picked in the menu, as the
    /// player does for one picked while playing.
    fn audio_chosen(&self) {
        let Some(part) = self.part() else {
            return;
        };
        let picked = self.hero.audio.chosen();
        let track = picked.and_then(|picked| part.embedded(StreamKind::Audio).nth(picked));
        if let Some(audio) = track.map(|track| track.id) {
            (self.on_action)(Action::ChooseStreams { part, audio: Some(audio), subtitles: None });
        }
    }

    /// Has the server remember the subtitles picked in the menu, where
    /// the first entry is none.
    fn subtitles_chosen(&self) {
        let Some(part) = self.part() else {
            return;
        };
        let subtitles = match self.hero.subtitles.chosen() {
            Some(0) => Some(None),
            Some(picked) => subtitles(&part).nth(picked - 1).map(|track| Some(track.id)),
            None => None,
        };
        if subtitles.is_some() {
            (self.on_action)(Action::ChooseStreams { part, audio: None, subtitles });
        }
    }

    /// The file that plays: the first of the version picked.
    fn part(&self) -> Option<Part> {
        let item = self.item.borrow();
        version_of(item.as_ref()?, self.version.get())?.parts.first().cloned()
    }

    /// Marks the item as watched if it is not, and the other way round.
    pub fn press_watched(&self) {
        let item = self.item.borrow().clone();
        if let Some(item) = item {
            let watched = !item.is_watched();
            // One episode is marked at once, and the button says so before
            // the server has. A show is asked about first.
            if !matches!(item.kind, Kind::Show | Kind::Season) {
                self.hero.show_watched(watched);
            }
            (self.on_action)(Action::SetWatched(item, watched));
        }
    }

    /// Puts the buttons and menus back to what the server last said, after
    /// a change the viewer made with them did not reach it.
    pub fn revert(&self) {
        if let Some(item) = &*self.item.borrow() {
            self.hero.show(item, self.version.get(), &self.images);
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
        self.hero.show(item, self.version.get(), &self.images);
        if let Some(colors) = item.colors {
            self.backdrop.set_colors(colors);
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

        let titles: Vec<&str> = seasons.iter().map(|(_, title)| title.as_str()).collect();
        self.hero.seasons.offer(&titles, chosen);
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
        let chosen = self.hero.seasons.chosen();
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
                let mut size = text::count(size, "episode");
                // An only season has no picker to name it.
                if let [(_, only)] = seasons(&episodes).as_slice() {
                    size = format!("{only} · {size}");
                }
                label::set(&self.hero.season_size, &size);
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
            hero.restart.button.setHidden(true);
            hero.menu_restart.setHidden(true);
            return hero.remaining.show("", 0.0);
        };
        let verb = if playable.view_offset.is_some() { "Resume" } else { "Play" };
        let is_show = self.item.borrow().as_ref().is_some_and(|item| item.kind == Kind::Show);
        let title = match text::episode_code(&playable).filter(|_| is_show) {
            Some(code) => format!("{verb} {code}"),
            None => verb.to_owned(),
        };
        hero.play.set_title(&title);
        hero.menu_play.setTitle(&NSString::from_str(&title));
        // Only what was stopped part of the way has a beginning to go
        // back to.
        let resumes = playable.view_offset.is_some();
        hero.restart.button.setHidden(!resumes);
        hero.menu_restart.setHidden(!resumes);
        // A show's button plays an episode, which the number on it does
        // not name.
        let mut beside: Vec<String> = Vec::new();
        beside.extend(is_show.then(|| playable.title.clone()));
        beside.extend(text::remaining(&playable));
        // An episode's own page shows how far it was watched on its frame.
        let watched =
            if is_show || playable.kind == Kind::Movie { playable.progress() } else { 0.0 };
        hero.remaining.show(&beside.join(" · "), watched);
    }
}

/// Whether `item` is something to keep on this Mac: a film or an episode
/// with a file whose size the server knows.
fn is_downloadable(item: &Item) -> bool {
    let part = item.media.first().and_then(|media| media.parts.first());
    matches!(item.kind, Kind::Movie | Kind::Episode) && part.is_some_and(|part| part.size.is_some())
}

/// The version of `item` with the id `version`, or without one or with
/// one the item no longer has, its first.
fn version_of(item: &Item, version: Option<u64>) -> Option<&Media> {
    item.media.iter().find(|media| Some(media.id) == version).or(item.media.first())
}

/// The subtitles there are for a file: those inside it and those in
/// files of their own beside it, in the server's order.
fn subtitles(part: &Part) -> impl Iterator<Item = &Stream> {
    part.streams.iter().filter(|stream| stream.kind == StreamKind::Subtitle)
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

        let view = HeaderView::new(main_thread);
        let banner = Rc::new(Banner::new(&view, main_thread));

        let art = Rc::new(ArtView::new(main_thread, theme::RADIUS_L));
        art.set_flat();
        art.set_playable(true);
        let (hovered, played) = (Rc::clone(&art), Rc::clone(&on_event));
        let art_link = HoverView::pressable(
            main_thread,
            move |over| hovered.set_hovered(over),
            move || played(Event::Play),
        );
        // Only an episode has a frame of its own to lead with.
        art_link.setHidden(!is_episode);
        art_link.setAccessibilityElement(true);
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
        let headline = theme::FONT_HEADLINE;
        let (show_link, show_name) = text_link(headline, follow(Link::Show), main_thread);
        let (season_link, season_name) = text_link(headline, follow(Link::Season), main_thread);
        let episode_number = line(headline, Weight::Semibold, &secondary);
        let step = || {
            let step = line(headline, Weight::Semibold, &secondary);
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
        eyebrow.setHidden(!is_episode);
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

        let play = Pill::large("Play", "play.fill", true, main_thread);
        // The default action of the page, which Return triggers.
        play.button.setKeyEquivalent(ns_string!("\r"));
        let restart = Pill::large("Play from Start", "arrow.counterclockwise", false, main_thread);
        restart.button.setHidden(true);
        let watched = Pill::large("Mark as Watched", "checkmark", false, main_thread);
        let download = Pill::large("Download", "arrow.down.circle", false, main_thread);
        download.button.setHidden(true);
        let downloaded = line(theme::FONT_BODY, Weight::Regular, &secondary);
        downloaded.setHidden(true);
        let remaining = Remaining::new(main_thread);
        let row: [&NSView; 5] =
            [&play.button, &restart.button, &watched.button, &download.button, &downloaded];
        let buttons = NSStackView::stackViewWithViews(&NSArray::from_slice(&row), main_thread);
        buttons.setSpacing(theme::SPACE_M);
        buttons.setCustomSpacing_afterView(theme::SPACE_L, &download.button);

        let seasons = Seasons::new(main_thread);

        let tagline = label::paragraph(
            theme::FONT_HEADLINE,
            Weight::Semibold,
            &primary,
            theme::HERO_TAGLINE_LINES,
            main_thread,
        );
        let summary = label::paragraph(
            theme::FONT_HEADLINE,
            Weight::Regular,
            &primary,
            theme::HERO_SUMMARY_LINES,
            main_thread,
        );
        let (headed, read) = (title.clone(), summary.clone());
        let (more, more_label) =
            text_link(theme::FONT_BODY, move || read_in_full(&headed, &read), main_thread);
        label::set(&more_label, "More");
        more.setAccessibilityLabel(Some(ns_string!("Read the whole summary")));
        more.setHidden(true);

        let choice = |caption| Choice::new(caption, main_thread);
        let (version, audio, subtitles) = (choice("Version"), choice("Audio"), choice("Subtitles"));
        let rows: [&NSView; 3] = [&version.row, &audio.row, &subtitles.row];
        let choices = NSStackView::stackViewWithViews(&NSArray::from_slice(&rows), main_thread);
        choices.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        choices.setAlignment(NSLayoutAttribute::Leading);
        choices.setSpacing(theme::SPACE_S);
        choices.setHidden(true);

        let (menu_play, menu_watched) =
            (NSMenuItem::new(main_thread), NSMenuItem::new(main_thread));
        let menu_restart = NSMenuItem::new(main_thread);
        menu_restart.setTitle(ns_string!("Play from Start"));
        menu_restart.setHidden(true);
        let menu = NSMenu::new(main_thread);
        menu.addItem(&menu_play);
        menu.addItem(&menu_restart);
        menu.addItem(&menu_watched);
        // SAFETY: the view keeps the menu, as a responder keeps the one it
        // is given.
        unsafe { view.setMenu(Some(&menu)) };

        let hero = Self {
            view,
            banner,
            has_backdrop: Rc::new(Cell::new(false)),
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
            logo: Logo::new(main_thread),
            // Two lines, so that a narrow window wraps the last facts
            // and does not cut them off.
            facts: label::paragraph(
                theme::FONT_HEADLINE,
                Weight::Medium,
                &theme::white(theme::SECONDARY_ON_ART_OPACITY),
                theme::HERO_FACTS_LINES,
                main_thread,
            ),
            badges,
            play,
            restart,
            watched,
            download,
            downloaded,
            menu_play,
            menu_restart,
            menu_watched,
            remaining,
            tagline,
            summary,
            more,
            choices,
            version,
            audio,
            subtitles,
            directors: Fact::new("Director", main_thread),
            studio: Fact::new("Studio", main_thread),
            genres,
            seasons,
            season_size: line(theme::FONT_BODY, Weight::Regular, &secondary),
            on_event,
        };
        hero.season_bar_hidden(true);
        hero.place(&buttons, kind);
        hero
    }

    /// What the item is and the buttons to watch it on the lower part of
    /// the banner. Under the banner what it is about and, beside that or
    /// under it, what there is to choose and to know. For a show the
    /// season picker in a strip along the bottom, where the episode list
    /// begins.
    fn place(&self, buttons: &NSStackView, kind: Kind) {
        let main_thread = self.view.mtm();
        let column = |lines: &[&NSView], spacing: f64| {
            let column = NSStackView::stackViewWithViews(&NSArray::from_slice(lines), main_thread);
            column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
            column.setAlignment(NSLayoutAttribute::Leading);
            column.setSpacing(spacing);
            column.setTranslatesAutoresizingMaskIntoConstraints(false);
            column
        };

        // The show's poster stands beside the lines that name the episode.
        let titles = column(&[&self.eyebrow, &self.title, &self.logo.view], theme::SPACE_XS);
        let headline: [&NSView; 2] = [&self.show_poster_link, &titles];
        let headline =
            NSStackView::stackViewWithViews(&NSArray::from_slice(&headline), main_thread);
        headline.setAlignment(NSLayoutAttribute::Top);
        headline.setSpacing(theme::SPACE_M);
        let lead = column(&[&headline, &self.facts, &self.badges, buttons], theme::SPACE_M);
        lead.setCustomSpacing_afterView(theme::SPACE_S, &headline);
        lead.setCustomSpacing_afterView(theme::SPACE_L, &self.badges);
        // Beside the buttons, but for an episode: its frame takes that
        // room in a narrow window, and has the bar on it.
        match kind {
            Kind::Episode => lead.addArrangedSubview(&self.remaining.view),
            _ => buttons.addArrangedSubview(&self.remaining.view),
        }
        let stage = NSView::new(main_thread);
        stage.setTranslatesAutoresizingMaskIntoConstraints(false);
        stage.addSubview(&self.art_link);
        stage.addSubview(&lead);

        let (summary, more) = (self.summary.clone(), self.more.clone());
        let story = Column::new(main_thread, move || offer_more(&summary, &more));
        let told: [&NSView; 3] = [&self.tagline, &self.summary, &self.more];
        for line in told {
            story.addArrangedSubview(line);
        }
        story.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        story.setAlignment(NSLayoutAttribute::Leading);
        story.setSpacing(theme::SPACE_S);
        let known = column(&[&self.choices, &self.directors.row, &self.studio.row], theme::SPACE_S);
        let side = column(&[&known, &self.genres], theme::SPACE_M);
        let about: [&NSView; 2] = [&story, &side];
        let about = NSStackView::stackViewWithViews(&NSArray::from_slice(&about), main_thread);
        about.setTranslatesAutoresizingMaskIntoConstraints(false);

        let season: [&NSView; 3] = [&self.seasons.tabs, &self.seasons.menu, &self.season_size];
        let season = NSStackView::stackViewWithViews(&NSArray::from_slice(&season), main_thread);
        season.setSpacing(theme::SPACE_M);
        season.setTranslatesAutoresizingMaskIntoConstraints(false);

        let (view, art) = (&self.view, &self.art_link);
        view.addSubview(&stage);
        view.addSubview(&about);
        view.addSubview(&season);

        let margin = theme::PAGE_MARGIN;
        // As tall as shows the backdrop, and taller where the text needs
        // it: a long title in a narrow window takes three lines.
        let room = stage.heightAnchor().constraintEqualToConstant(0.0);
        room.setPriority(NSLayoutPriorityDefaultHigh);
        // Beside what the item is about, the rest is this wide.
        let beside = side.widthAnchor().constraintEqualToConstant(theme::HERO_SIDE_WIDTH);
        let season_bar = if kind == Kind::Show { theme::SEASON_BAR_HEIGHT } else { 0.0 };
        let (banner, has_backdrop) = (Rc::clone(&self.banner), Rc::clone(&self.has_backdrop));
        let (on_event, staged, told) = (Rc::clone(&self.on_event), stage.clone(), about.clone());
        let (room_given, abreast) = (room.clone(), beside.clone());
        let (logo, worded) = (self.logo.clone(), lead.clone());
        view.on_layout(move || {
            let width = staged.frame().size.width;
            banner.fit(worded.frame().size.height);
            logo.fit(width);
            let page = staged.enclosingScrollView().map(|page| page.bounds().size.height);
            let wanted = match has_backdrop.get() {
                true => (width * theme::HERO_HEIGHT_PER_WIDTH)
                    .min(page.unwrap_or_default() * theme::HERO_MOST)
                    .round(),
                false => 0.0,
            };
            if room_given.constant() != wanted {
                room_given.setConstant(wanted);
            }
            arrange(&told, &abreast, width >= theme::HERO_ABREAST_FROM);
            let content = staged.frame().size.height + told.frame().size.height;
            on_event(Event::Resized((content + 2.0 * theme::SPACE_L + season_bar).ceil()));
        });
        arrange(&about, &beside, false);

        // The strip is the last thing in the header.
        let bar_middle = -theme::SEASON_BAR_HEIGHT / 2.0;
        let lead_from = match kind {
            Kind::Episode => {
                let (wide, tall) = (art.widthAnchor(), art.heightAnchor());
                let share = wide.constraintEqualToAnchor_multiplier(
                    &view.widthAnchor(),
                    theme::HERO_STILL_SHARE,
                );
                // As much of the page as it can have within its bounds.
                share.setPriority(NSLayoutPriorityDefaultHigh);
                NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
                    share,
                    wide.constraintGreaterThanOrEqualToConstant(theme::HERO_STILL_MIN_WIDTH),
                    wide.constraintLessThanOrEqualToConstant(theme::HERO_STILL_MAX_WIDTH),
                    tall.constraintEqualToAnchor_multiplier(&wide, theme::LANDSCAPE_ASPECT),
                    art.leadingAnchor()
                        .constraintEqualToAnchor_constant(&stage.leadingAnchor(), margin),
                    art.bottomAnchor().constraintEqualToAnchor(&stage.bottomAnchor()),
                    art.topAnchor().constraintGreaterThanOrEqualToAnchor_constant(
                        &stage.topAnchor(),
                        theme::SPACE_S,
                    ),
                ]));
                lead.leadingAnchor()
                    .constraintEqualToAnchor_constant(&art.trailingAnchor(), theme::SPACE_XL)
            }
            _ => lead
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&stage.leadingAnchor(), margin),
        };
        let small = self.show_poster_link.widthAnchor();
        let text_width = theme::HERO_TEXT_WIDTH;
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            small.constraintEqualToConstant(theme::HERO_SHOW_POSTER_WIDTH),
            self.show_poster_link
                .heightAnchor()
                .constraintEqualToAnchor_multiplier(&small, theme::POSTER_ASPECT),
            stage.topAnchor().constraintEqualToAnchor(&view.topAnchor()),
            stage.leadingAnchor().constraintEqualToAnchor(&view.leadingAnchor()),
            stage.trailingAnchor().constraintEqualToAnchor(&view.trailingAnchor()),
            room,
            self.banner.view().bottomAnchor().constraintEqualToAnchor(&stage.bottomAnchor()),
            lead_from,
            lead.bottomAnchor().constraintEqualToAnchor(&stage.bottomAnchor()),
            lead.topAnchor()
                .constraintGreaterThanOrEqualToAnchor_constant(&stage.topAnchor(), theme::SPACE_S),
            lead.trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&stage.trailingAnchor(), -margin),
            buttons.trailingAnchor().constraintLessThanOrEqualToAnchor(&lead.trailingAnchor()),
            about
                .topAnchor()
                .constraintEqualToAnchor_constant(&stage.bottomAnchor(), theme::SPACE_L),
            about.leadingAnchor().constraintEqualToAnchor_constant(&view.leadingAnchor(), margin),
            about
                .trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&view.trailingAnchor(), -margin),
            // A title has the width of the page. Only running text is
            // kept to a width that reads well.
            self.tagline.widthAnchor().constraintLessThanOrEqualToConstant(text_width),
            self.summary.widthAnchor().constraintLessThanOrEqualToConstant(text_width),
            side.widthAnchor().constraintLessThanOrEqualToAnchor(&about.widthAnchor()),
            season.leadingAnchor().constraintEqualToAnchor_constant(&view.leadingAnchor(), margin),
            season
                .centerYAnchor()
                .constraintEqualToAnchor_constant(&view.bottomAnchor(), bar_middle),
        ]));
    }

    fn season_bar_hidden(&self, hidden: bool) {
        self.seasons.set_hidden(hidden);
        self.season_size.setHidden(hidden);
    }

    /// Fills the header with `item`, and with `version` of it where it
    /// has several.
    fn show(&self, item: &Item, version: Option<u64>, images: &Rc<Images>) {
        let set = |label: &NSTextField, words: &str| {
            label::set(label, words);
            label.setHidden(words.is_empty());
        };
        let is_episode = item.kind == Kind::Episode;
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
        self.show_logo(item, images);
        // A line may break after a dot between two facts, never before it.
        set(&self.facts, &facts(item).replace(" · ", "\u{a0}· "));
        set(&self.tagline, item.tagline.as_deref().unwrap_or_default());
        set(&self.summary, item.summary.as_deref().unwrap_or_default());
        self.show_credits(item);

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
        let media = version_of(item, version);
        self.show_choices(item, media);
        let mut badges: Vec<Retained<NSView>> = media
            .map(formats::badges)
            .unwrap_or_default()
            .iter()
            .map(|words| badge(words, main_thread))
            .collect();
        let file = media.map(formats::file_facts).unwrap_or_default();
        if !file.is_empty() {
            let faint = theme::white(theme::SECONDARY_ON_ART_OPACITY);
            let plain = label::line(theme::FONT_CAPTION, Weight::Medium, &faint, main_thread);
            label::set(&plain, &file);
            badges.push(Retained::into_super(Retained::into_super(plain)));
        }
        self.badges.setViews_inGravity(
            &NSArray::from_retained_slice(&badges),
            NSStackViewGravity::Leading,
        );
        self.badges.setHidden(badges.is_empty());

        self.show_watched(item.is_watched());

        // An episode's own picture is a frame of it, which may give the
        // episode away when it is as large as the page. Its show's
        // backdrop does not.
        let backdrop = match item.kind {
            Kind::Episode => item.grandparent_art.as_deref().or(item.art.as_deref()),
            _ => item.art.as_deref(),
        };
        self.banner.paint(item.colors.map(|colors| colors.bottom_left));
        self.banner.show(backdrop, images);
        if self.has_backdrop.replace(backdrop.is_some()) != backdrop.is_some() {
            self.view.setNeedsLayout(true);
        }
        if is_episode {
            self.art.show(item.thumb.as_deref(), theme::HERO_STILL_REQUEST, images);
            self.art.set_progress(item.progress());
            let poster = item.grandparent_thumb.as_deref();
            self.show_poster.show(poster, theme::HERO_SHOW_POSTER_REQUEST, images);
        }
    }

    /// Says on the button and in the menu what a press of either now
    /// does to `watched`, which the button also shows by being lit.
    fn show_watched(&self, watched: bool) {
        let toggle = if watched { "Mark as Unwatched" } else { "Mark as Watched" };
        self.watched.set_title(toggle);
        self.menu_watched.setTitle(&NSString::from_str(toggle));
        self.watched.set_lit(watched);
    }

    /// Shows the title of a film or show as its logo, if the server has
    /// one. The words wait for a logo that is on its way and stand in for
    /// one that cannot be had. An episode goes by its own name: the logo
    /// it lists is that of its show.
    fn show_logo(&self, item: &Item, images: &Rc<Images>) {
        let path = item.logo().filter(|_| item.kind != Kind::Episode);
        let logo = &self.logo;
        label::set(&self.title, &item.title);
        logo.view.setAccessibilityLabel(Some(&NSString::from_str(&item.title)));
        if logo.wanted.borrow().as_deref() == path {
            return;
        }
        *logo.wanted.borrow_mut() = path.map(str::to_owned);
        let (width, height) = theme::HERO_LOGO_REQUEST;
        let (width, height) = (width * theme::RETINA_SCALE, height * theme::RETINA_SCALE);
        let ready = path.and_then(|path| images.cached_cutout(path, width, height));
        logo.set(ready.as_ref());
        self.title.setHidden(path.is_some());
        let (Some(path), None) = (path, ready) else {
            return;
        };
        let (logo, title, asked) = (logo.clone(), self.title.clone(), path.to_owned());
        images.load_cutout(path, width, height, move |artwork| {
            if logo.wanted.borrow().as_deref() != Some(asked.as_str()) {
                return;
            }
            logo.set(artwork);
            title.setHidden(artwork.is_some());
        });
    }

    /// Fills in who directed `item` and who made it.
    fn show_credits(&self, item: &Item) {
        let main_thread = self.view.mtm();
        let primary = NSColor::labelColor();
        let shown: Vec<&Role> = item.directors.iter().take(DIRECTORS_SHOWN).collect();
        let directors: Vec<Retained<NSView>> = shown
            .iter()
            .enumerate()
            .map(|(place, director)| {
                // Each name but the last ends in a comma.
                let name = match place + 1 < shown.len() {
                    true => format!("{},", director.tag),
                    false => director.tag.clone(),
                };
                let plain = label::line(theme::FONT_BODY, Weight::Regular, &primary, main_thread);
                label::set(&plain, &name);
                Retained::into_super(Retained::into_super(plain))
            })
            .collect();
        self.directors.set(directors);
        let studio: Vec<Retained<NSView>> = item
            .studio
            .iter()
            .map(|studio| {
                let plain = label::line(theme::FONT_BODY, Weight::Regular, &primary, main_thread);
                label::set(&plain, studio);
                Retained::into_super(Retained::into_super(plain))
            })
            .collect();
        self.studio.set(studio);
    }
}

/// Puts what there is to choose and to know beside what the item is
/// about, or under it where the page is too narrow for both: `beside`
/// holds the second to its width while they are side by side.
fn arrange(about: &NSStackView, beside: &NSLayoutConstraint, abreast: bool) {
    let orientation = match abreast {
        true => NSUserInterfaceLayoutOrientation::Horizontal,
        false => NSUserInterfaceLayoutOrientation::Vertical,
    };
    if about.orientation() == orientation && beside.isActive() == abreast {
        return;
    }
    about.setOrientation(orientation);
    about.setAlignment(if abreast { NSLayoutAttribute::Top } else { NSLayoutAttribute::Leading });
    about.setSpacing(if abreast { theme::SPACE_XXL } else { theme::SPACE_M });
    beside.setActive(abreast);
}

/// The title of a film or show as a picture of its own.
#[derive(Clone)]
struct Logo {
    view: Retained<NSView>,
    /// As large as the page has room for, and no larger than the picture.
    size: (Retained<NSLayoutConstraint>, Retained<NSLayoutConstraint>),
    /// How large the picture is, in points, and how wide the page.
    natural: Rc<Cell<NSSize>>,
    page: Rc<Cell<f64>>,
    /// The logo that is shown or on its way.
    wanted: Rc<RefCell<Option<String>>>,
}

impl Logo {
    fn new(main_thread: MainThreadMarker) -> Self {
        let view = artwork::layered(main_thread);
        view.setHidden(true);
        view.setAccessibilityElement(true);
        // SAFETY: the role is a constant that AppKit provides.
        view.setAccessibilityRole(Some(unsafe { NSAccessibilityImageRole }));
        if let Some(layer) = view.layer() {
            // A logo is drawn for its own poster, and the backdrop under
            // it here may be as light as it is.
            layer.setShadowOpacity(theme::HERO_SHADOW_OPACITY);
            layer.setShadowRadius(theme::HERO_LOGO_SHADOW_RADIUS);
            layer.setShadowOffset(CGSize::ZERO);
        }
        let size = (
            view.widthAnchor().constraintEqualToConstant(0.0),
            view.heightAnchor().constraintEqualToConstant(0.0),
        );
        size.0.setActive(true);
        size.1.setActive(true);
        let natural = Rc::new(Cell::new(NSSize::ZERO));
        Self { view, size, natural, page: Rc::default(), wanted: Rc::default() }
    }

    /// Sizes the logo for a page `page` points wide.
    fn fit(&self, page: f64) {
        self.page.set(page);
        let (natural, most) = (self.natural.get(), theme::HERO_LOGO_REQUEST);
        if natural.width <= 0.0 || natural.height <= 0.0 {
            return;
        }
        let wide = (page * theme::HERO_LOGO_SHARE).clamp(theme::HERO_LOGO_LEAST, f64::from(most.0));
        let tall = wide * f64::from(most.1) / f64::from(most.0);
        let scale = (wide / natural.width).min(tall / natural.height).min(1.0);
        let (wide, tall) = ((natural.width * scale).round(), (natural.height * scale).round());
        if self.size.0.constant() != wide || self.size.1.constant() != tall {
            self.size.0.setConstant(wide);
            self.size.1.setConstant(tall);
        }
    }

    /// Shows `logo`, or with none hides the view.
    fn set(&self, logo: Option<&Artwork>) {
        self.view.setHidden(logo.is_none());
        let Some((logo, layer)) = logo.zip(self.view.layer()) else {
            return;
        };
        let pixels = logo.size();
        let scale = f64::from(theme::RETINA_SCALE);
        self.natural.set(NSSize::new(pixels.width / scale, pixels.height / scale));
        self.fit(self.page.get());
        // SAFETY: the contents are a decoded CGImage.
        unsafe { layer.setContents(Some(logo.contents())) };
    }
}

/// Beside the buttons: what the main one plays and how much of it is
/// left, in words over a bar.
struct Remaining {
    view: Retained<NSStackView>,
    words: Retained<NSTextField>,
    track: Retained<NSView>,
    fill: Retained<NSView>,
    fill_width: RefCell<Option<Retained<NSLayoutConstraint>>>,
}

impl Remaining {
    fn new(main_thread: MainThreadMarker) -> Self {
        let faint = theme::white(theme::SECONDARY_ON_ART_OPACITY);
        let words = label::line(theme::FONT_BODY, Weight::Medium, &faint, main_thread);
        // In a narrow window the words are what gives, cut short with an
        // ellipsis, before any button does.
        words.setContentCompressionResistancePriority_forOrientation(
            NSLayoutPriorityDefaultLow,
            NSLayoutConstraintOrientation::Horizontal,
        );
        let track = artwork::layered(main_thread);
        artwork::fill_with(&track, &theme::white(theme::HOME_HERO_TRACK_OPACITY));
        let fill = artwork::layered(main_thread);
        artwork::fill_with(&fill, &NSColor::controlAccentColor());
        for bar in [&track, &fill] {
            if let Some(layer) = bar.layer() {
                layer.setCornerRadius(theme::PROGRESS_BAR_HEIGHT / 2.0);
            }
        }
        track.addSubview(&fill);
        let lines: [&NSView; 2] = [&words, &track];
        let view = NSStackView::stackViewWithViews(&NSArray::from_slice(&lines), main_thread);
        view.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        view.setAlignment(NSLayoutAttribute::Leading);
        view.setSpacing(theme::SPACE_XS);
        view.setHidden(true);
        let wide = track.widthAnchor().constraintEqualToConstant(theme::HOME_HERO_PROGRESS_WIDTH);
        wide.setPriority(NSLayoutPriorityDefaultHigh);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            wide,
            track.heightAnchor().constraintEqualToConstant(theme::PROGRESS_BAR_HEIGHT),
            fill.leadingAnchor().constraintEqualToAnchor(&track.leadingAnchor()),
            fill.topAnchor().constraintEqualToAnchor(&track.topAnchor()),
            fill.bottomAnchor().constraintEqualToAnchor(&track.bottomAnchor()),
        ]));
        Self { view, words, track, fill, fill_width: RefCell::new(None) }
    }

    /// Says `words` over a bar filled to `fraction`, from 0 to 1. No
    /// words hide both, and nothing watched hides the bar.
    fn show(&self, words: &str, fraction: f64) {
        label::set(&self.words, words);
        self.view.setHidden(words.is_empty());
        self.track.setHidden(fraction <= 0.0);
        if let Some(old) = self.fill_width.take() {
            old.setActive(false);
        }
        let width = self.fill.widthAnchor().constraintEqualToAnchor_multiplier(
            &self.track.widthAnchor(),
            fraction.clamp(0.0, 1.0),
        );
        width.setActive(true);
        self.fill_width.replace(Some(width));
    }
}

/// Something known about the item: what it is under a caption, as the
/// menus that choose what plays are under theirs.
struct Fact {
    row: Retained<NSStackView>,
    values: Retained<NSStackView>,
}

impl Fact {
    fn new(caption: &str, main_thread: MainThreadMarker) -> Self {
        let values = NSStackView::new(main_thread);
        values.setSpacing(theme::SPACE_XS);
        let row = captioned(caption, &values, main_thread);
        Self { row, values }
    }

    /// Shows `values` beside the caption, or with none hides both.
    fn set(&self, values: Vec<Retained<NSView>>) {
        self.row.setHidden(values.is_empty());
        self.values.setViews_inGravity(
            &NSArray::from_retained_slice(&values),
            NSStackViewGravity::Leading,
        );
    }
}

/// `value` beside the word for what it is, in a row that starts hidden.
/// The words of all such rows are one column.
fn captioned(
    caption: &str,
    value: &NSView,
    main_thread: MainThreadMarker,
) -> Retained<NSStackView> {
    let secondary = NSColor::secondaryLabelColor();
    let name = label::line(theme::FONT_BODY, Weight::Regular, &secondary, main_thread);
    label::set(&name, caption);
    name.widthAnchor().constraintEqualToConstant(theme::CHOICE_CAPTION_WIDTH).setActive(true);
    let row: [&NSView; 2] = [&name, value];
    let row = NSStackView::stackViewWithViews(&NSArray::from_slice(&row), main_thread);
    row.setSpacing(theme::SPACE_S);
    row.setHidden(true);
    row
}

impl Hero {
    /// Fills the menus that choose what plays, from the versions of
    /// `item` and the tracks of the one picked, `media`.
    fn show_choices(&self, item: &Item, media: Option<&Media>) {
        let versions: Vec<String> = item.media.iter().map(formats::version).collect();
        let picked = item.media.iter().position(|version| Some(version) == media);
        self.version.offer(&versions, picked.unwrap_or(0));

        let part = media.and_then(|media| media.parts.first());
        let name = |track: &Stream| part.map(|part| formats::track(track, part));
        let tracks = |kind| part.into_iter().flat_map(move |part| part.embedded(kind));
        let audio: Vec<String> = tracks(StreamKind::Audio).filter_map(name).collect();
        let heard = tracks(StreamKind::Audio).position(|track| track.selected);
        self.audio.offer(&audio, heard.unwrap_or(0));

        let written = || part.into_iter().flat_map(subtitles);
        let mut subtitles = vec!["Off".to_owned()];
        subtitles.extend(written().filter_map(name));
        // The first entry is none, so a track is one further down.
        let read = written().position(|track| track.selected).map_or(0, |track| track + 1);
        self.subtitles.offer(&subtitles, read);

        let rows = [&self.version, &self.audio, &self.subtitles];
        self.choices.setHidden(rows.iter().all(|choice| choice.row.isHidden()));
    }
}

/// The seasons of a show to pick one from: side by side while they are
/// few, as the tabs of the list under them, and in a menu when they are
/// many.
struct Seasons {
    tabs: Retained<NSSegmentedControl>,
    menu: Retained<NSPopUpButton>,
    /// How many seasons there are to pick from.
    count: Cell<usize>,
}

impl Seasons {
    fn new(main_thread: MainThreadMarker) -> Self {
        let tabs = NSSegmentedControl::new(main_thread);
        tabs.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
        tabs.setControlSize(NSControlSize::Large);
        tabs.setAccessibilityLabel(Some(ns_string!("Season")));
        let menu = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(main_thread),
            NSRect::ZERO,
            false,
        );
        menu.setControlSize(NSControlSize::Large);
        menu.setAccessibilityLabel(Some(ns_string!("Season")));
        Self { tabs, menu, count: Cell::new(0) }
    }

    /// Offers the seasons with these titles, the one at `picked` chosen.
    fn offer(&self, titles: &[&str], picked: usize) {
        self.count.set(titles.len());
        self.tabs.setSegmentCount(titles.len().cast_signed());
        for (segment, title) in titles.iter().enumerate() {
            self.tabs.setLabel_forSegment(&NSString::from_str(title), segment.cast_signed());
        }
        self.tabs.setSelectedSegment(picked.cast_signed());
        let titles: Vec<Retained<NSString>> =
            titles.iter().map(|title| NSString::from_str(title)).collect();
        self.menu.removeAllItems();
        self.menu.addItemsWithTitles(&NSArray::from_retained_slice(&titles));
        self.menu.selectItemAtIndex(picked.cast_signed());
    }

    /// Which season is chosen, counting from zero.
    fn chosen(&self) -> Option<usize> {
        let chosen = match self.count.get() <= SEASON_TABS_MOST {
            true => self.tabs.selectedSegment(),
            false => self.menu.indexOfSelectedItem(),
        };
        usize::try_from(chosen).ok()
    }

    /// Shows the tabs or the menu, whichever the number of seasons calls
    /// for, or with `hidden` neither. A single season is not a choice,
    /// and has neither as well.
    fn set_hidden(&self, hidden: bool) {
        let count = self.count.get();
        self.tabs.setHidden(hidden || !(2..=SEASON_TABS_MOST).contains(&count));
        self.menu.setHidden(hidden || count <= SEASON_TABS_MOST);
    }
}

/// One thing to choose about what plays: a caption and, beside it, a
/// menu of what there is.
struct Choice {
    row: Retained<NSStackView>,
    picker: Retained<NSPopUpButton>,
}

impl Choice {
    fn new(caption: &str, main_thread: MainThreadMarker) -> Self {
        let picker = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(main_thread),
            NSRect::ZERO,
            false,
        );
        picker.setControlSize(NSControlSize::Small);
        picker.setFont(Some(&label::font(theme::FONT_CAPTION, Weight::Regular)));
        picker.setAccessibilityLabel(Some(&NSString::from_str(caption)));
        picker.setTranslatesAutoresizingMaskIntoConstraints(false);
        let widest = theme::CHOICE_MENU_MAX_WIDTH;
        picker.widthAnchor().constraintLessThanOrEqualToConstant(widest).setActive(true);
        Self { row: captioned(caption, &picker, main_thread), picker }
    }

    /// Fills the menu with `titles`, the one at `picked` chosen. With
    /// fewer than two there is nothing to choose, and the row goes.
    fn offer(&self, titles: &[String], picked: usize) {
        self.row.setHidden(titles.len() < 2);
        self.picker.removeAllItems();
        let Some(menu) = self.picker.menu() else {
            return;
        };
        // Added to the menu one by one: given titles, the button drops
        // any it already has, and two tracks may both be "English".
        for title in titles {
            let entry = NSMenuItem::new(self.picker.mtm());
            entry.setTitle(&NSString::from_str(title));
            menu.addItem(&entry);
        }
        self.picker.selectItemAtIndex(picked.cast_signed());
    }

    /// Which entry of the menu is chosen, counting from zero.
    fn chosen(&self) -> Option<usize> {
        usize::try_from(self.picker.indexOfSelectedItem()).ok()
    }
}

define_class!(
    // SAFETY: NSView is designed to be subclassed, and `PageView` does not
    // implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustDetailPage"]
    #[ivars = PageHears]
    struct PageView;

    impl PageView {
        #[unsafe(method_id(initWithFrame:))]
        fn init_with_frame(this: Allocated<Self>, frame: NSRect) -> Retained<Self> {
            let this = this.set_ivars(PageHears::default());
            // SAFETY: forwards the designated initializer unchanged.
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        #[unsafe(method(layout))]
        fn layout(&self) {
            // SAFETY: forwards the call unchanged.
            let () = unsafe { msg_send![super(self), layout] };
            let (hears, height) = (self.ivars(), self.bounds().size.height);
            if let (Some(on_height), true) =
                (hears.on_height.get(), hears.height.replace(height) != height)
            {
                on_height();
            }
        }

        /// The list of cards hands a key it has no use for to the text
        /// system, which turns Escape into this command and sends it up
        /// from the list to whatever holds it.
        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) {
            if let Some(on_key) = self.ivars().on_key.get() {
                on_key(Key::Escape);
            }
        }

        /// And a key that types a character into this one, with the
        /// character.
        #[unsafe(method(insertText:))]
        fn insert_text(&self, text: &AnyObject) {
            let typed = text.downcast_ref::<NSString>().map(ToString::to_string);
            match (typed.as_deref(), self.ivars().on_key.get()) {
                (Some(" "), Some(on_key)) => on_key(Key::Space),
                // SAFETY: forwards the text unchanged.
                _ => unsafe { msg_send![super(self), insertText: text] },
            }
        }
    }
);

/// What a page tells whoever made it.
#[derive(Default)]
struct PageHears {
    on_key: OnceCell<Box<dyn Fn(Key)>>,
    /// Called when the page is another height than it was.
    on_height: OnceCell<Box<dyn Fn()>>,
    height: Cell<f64>,
}

/// A key that means something on the page as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Space,
    Escape,
}

impl PageView {
    /// A view that calls `on_key` when one of the page's keys is pressed
    /// and nothing inside the view took it, and `on_height` when it is laid
    /// out at another height than before.
    fn new(
        main_thread: MainThreadMarker,
        on_key: impl Fn(Key) + 'static,
        on_height: impl Fn() + 'static,
    ) -> Retained<Self> {
        let view: Retained<Self> =
            // SAFETY: `initWithFrame:` is NSView's designated initializer.
            unsafe { msg_send![Self::alloc(main_thread), initWithFrame: NSRect::ZERO] };
        // Cannot fail: the cells were created empty by the initializer.
        let _ = view.ivars().on_key.set(Box::new(on_key));
        let _ = view.ivars().on_height.set(Box::new(on_height));
        view
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
    /// How tall it is, and the size of its title and its glyph.
    height: f64,
    font: f64,
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
        let (height, font) = (theme::PILL_HEIGHT, theme::FONT_BODY);
        Self::sized(title, symbol, prominent, height, font, main_thread)
    }

    /// The same, as large as a finger needs it: the buttons of a page.
    fn large(title: &str, symbol: &str, prominent: bool, main_thread: MainThreadMarker) -> Self {
        let (height, font) = (theme::TOUCH_TARGET, theme::FONT_HEADLINE);
        Self::sized(title, symbol, prominent, height, font, main_thread)
    }

    fn sized(
        title: &str,
        symbol: &str,
        prominent: bool,
        height: f64,
        font: f64,
        main_thread: MainThreadMarker,
    ) -> Self {
        // SAFETY: the target and action are set once the page exists.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(ns_string!(""), None, None, main_thread)
        };
        button.setBordered(false);
        button.setFont(Some(&label::font(font, Weight::Semibold)));
        button.setWantsLayer(true);
        if let Some(layer) = button.layer() {
            layer.setCornerRadius(height / 2.0);
        }
        button.setTranslatesAutoresizingMaskIntoConstraints(false);
        button.heightAnchor().constraintEqualToConstant(height).setActive(true);
        if !prominent {
            button.widthAnchor().constraintEqualToConstant(height).setActive(true);
        }
        let pill = Self { button, prominent, height, font, width: RefCell::new(None) };
        pill.set_symbol(symbol);
        pill.set_lit(prominent);
        pill.set_title(title);
        pill
    }

    fn set_symbol(&self, symbol: &str) {
        let image = artwork::symbol(symbol, self.font, Weight::Semibold);
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
        let padding = theme::PILL_PADDING * self.height / theme::PILL_HEIGHT;
        let width = self.button.intrinsicContentSize().width + 2.0 * padding;
        let width = self.button.widthAnchor().constraintEqualToConstant(width.ceil());
        width.setActive(true);
        *self.width.borrow_mut() = Some(width);
    }
}

/// A few words that lead somewhere. They are as bright as a title, which
/// sets them apart from the plain text around them, and give way a little
/// under the pointer, which turns into a hand over them.
fn text_link(
    size: f64,
    on_click: impl Fn() + 'static,
    main_thread: MainThreadMarker,
) -> (Retained<HoverView>, Retained<NSTextField>) {
    let primary = NSColor::labelColor();
    let label = label::line(size, Weight::Semibold, &primary, main_thread);
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
