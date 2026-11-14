//! The main window: sidebar on the left, the chosen place on the right,
//! and the way from one place to the next.

use std::cell::{Cell, OnceCell, RefCell};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameDarkAqua, NSApplication, NSApplicationActivationPolicy,
    NSApplicationDelegate, NSBackingStoreType, NSSplitViewController, NSSplitViewItem, NSView,
    NSViewController, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSString, ns_string,
};
use objc2_quartz_core::{CAMediaTiming, CATransition, kCATransitionFade};
use plex::library::{Listing, Page};
use plex::model::{Hub, Item, Kind, RatingKey, Section};
use plex::{Client, MediaServer, Token};

use super::callback::Callback;
use super::detail::Detail;
use super::layout;
use super::library_bar::LibraryBar;
use super::menus::{self, Commands};
use super::notice::Notice;
use super::player::input::Key;
use super::player::{Outcome, PlayerScreen, Purpose};
use super::poster::Shape;
use super::posters::{Action, Header, Outline, Posters, Shelf, Style};
use super::sidebar::{Destination, Sidebar};
use super::snapshot;
use super::theme;
use super::toolbar::Toolbar;
use crate::account;
use crate::background::Background;
use crate::error::{Error, Result, with_causes};
use crate::images::Images;
use crate::settings::Settings;

/// How many items each shelf of the home screen starts with.
const SHELF_LENGTH: u32 = 20;

/// How many items of a library are fetched at a time.
const PAGE_SIZE: u32 = 100;

/// How many results a search shows of each kind.
const SEARCH_LIMIT: u32 = 30;

/// How long a snapshot waits after the window has its content, so that
/// work started by showing it, such as loading artwork, can finish.
const SNAPSHOT_SETTLE: Duration = Duration::from_secs(2);

/// How long a snapshot then waits after scrolling and hovering as it was
/// asked to, so that what those bring on screen can load too.
const SNAPSHOT_TWEAKS_SETTLE: Duration = Duration::from_secs(1);

/// How long a snapshot of the player waits before pressing its keys, so
/// that the file is open and the keys have something to act on.
const SNAPSHOT_KEYS_AFTER: Duration = Duration::from_millis(1500);

const UNREACHABLE: &str = "Could not reach your server";
const NOT_LOADED: &str = "This could not be loaded";

struct Shell {
    main_thread: MainThreadMarker,
    background: Rc<Background>,
    window: Retained<NSWindow>,
    /// The area right of the sidebar. Its one subview is the current place.
    content: Retained<NSView>,
    // The next three are set once, right after the shell exists, because
    // what they call back into is the shell.
    sidebar: OnceCell<Retained<Sidebar>>,
    toolbar: OnceCell<Retained<Toolbar>>,
    commands: OnceCell<Commands>,
    /// The targets of the Go menu's entries for Home and the libraries.
    places: RefCell<Vec<menus::Place>>,
    /// Set once a server is known, from the last launch or from asking.
    session: RefCell<Option<Session>>,
    /// Every place visited, oldest first, each with its screen kept as it
    /// was left. Going back shows the same views again, scrolled as before.
    history: RefCell<Vec<Entry>>,
    /// Which entry of the history is on screen.
    position: Cell<usize>,
    /// Counts the moves from place to place, so that an answer arriving
    /// late can tell that the viewer has moved on.
    moves: Cell<u64>,
    /// A message shown in place of the current entry, such as an error.
    notice: RefCell<Option<Notice>>,
    /// The sidebar and content area, which the player replaces while open.
    split: Retained<NSSplitViewController>,
    playing: RefCell<Option<Retained<PlayerScreen>>>,
    /// Set while an item is being fetched in order to play it.
    opening_player: Cell<bool>,
    /// Speed and volume, kept from one item to the next and across launches.
    settings: Cell<Settings>,
}

/// What exists only while a server is known.
#[derive(Clone)]
struct Session {
    server: MediaServer,
    images: Rc<Images>,
    sections: Vec<Section>,
}

/// Somewhere the viewer can be.
#[derive(Clone, PartialEq)]
enum Route {
    Home,
    Library(Section, Listing),
    /// The page of an item. For a show, optionally the season to open on.
    Item(Box<Item>, Option<RatingKey>),
    Search(String),
}

struct Entry {
    route: Route,
    screen: Screen,
    /// Asks the server again and updates the screen where it differs.
    reload: Rc<dyn Fn()>,
}

#[derive(Clone)]
enum Screen {
    Posters(Retained<Posters>),
    Detail(Retained<Detail>),
}

impl Screen {
    fn view(&self) -> &NSView {
        match self {
            Self::Posters(posters) => posters.view(),
            Self::Detail(detail) => detail.view(),
        }
    }

    fn focus(&self) {
        match self {
            Self::Posters(posters) => posters.focus(),
            Self::Detail(detail) => detail.focus(),
        }
    }

    fn scroll_to(&self, y: f64) {
        match self {
            Self::Posters(posters) => posters.scroll_to(y),
            Self::Detail(detail) => detail.scroll_to(y),
        }
    }
}

/// What a snapshot shows.
#[derive(Clone)]
pub enum Subject {
    Home,
    /// The library at this position in the sidebar, counting from zero.
    Library(usize),
    /// The detail page of the item with this rating key.
    Item(String),
    /// The player, playing the item with this rating key as a check, after
    /// these keys were pressed in it.
    Play(String, Vec<Key>),
    /// The results of searching for this.
    Search(String),
    /// The screen a signed-out viewer starts on.
    SignIn,
}

/// Changes to the state a snapshot is taken in.
#[derive(Clone, Default)]
pub struct Tweaks {
    /// The window at its smallest size.
    pub narrow: bool,
    /// The pointer over the first item.
    pub hover: bool,
    /// On an episode's page, its link to the show followed.
    pub follow_link: bool,
    /// Scrolled this many points down.
    pub scroll: f64,
    /// For a library, which items and in what order.
    pub listing: Listing,
}

/// A request to draw one state of the app into a PNG and exit.
#[derive(Clone)]
pub struct Snapshot {
    pub subject: Subject,
    pub tweaks: Tweaks,
    pub path: PathBuf,
}

/// Opens the main window and runs the app until it quits.
///
/// With `snapshot`, the window stays off screen: once the subject has
/// loaded, the window is saved to the path as a PNG and the app exits.
pub fn run(snapshot: Option<Snapshot>) -> Result<()> {
    let main_thread =
        MainThreadMarker::new().unwrap_or_else(|| panic!("main() runs on the main thread"));
    let background = Rc::new(Background::new()?);

    let app = NSApplication::sharedApplication(main_thread);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    // A film library looks right on dark, whatever the system is set to.
    // SAFETY: the name is a constant that AppKit provides.
    let dark = unsafe { NSAppearanceNameDarkAqua };
    app.setAppearance(NSAppearance::appearanceNamed(dark).as_deref());

    let shell = Shell::new(main_thread, Rc::clone(&background), Settings::load()?);
    match &snapshot {
        Some(snapshot) if snapshot.tweaks.narrow => {
            shell.window.setContentSize(theme::WINDOW_MIN_SIZE);
        }
        Some(_) => {}
        None => {
            // Opens where and as large as it was last closed.
            shell.window.setFrameAutosaveName(ns_string!("main"));
            shell.window.makeKeyAndOrderFront(None);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
    }

    let wants_sign_in = matches!(&snapshot, Some(Snapshot { subject: Subject::SignIn, .. }));
    match account::signed_in() {
        Ok((client, token)) if !wants_sign_in => shell.connect(client, token, snapshot),
        Ok(_) | Err(Error::SignedOut) => {
            shell.show_sign_in();
            if let Some(snapshot) = snapshot {
                shell.snapshot_later(snapshot);
            }
        }
        Err(error) => return Err(error),
    }

    let delegate = AppDelegate::new(main_thread, Rc::clone(&shell));
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
    Ok(())
}

impl Shell {
    fn new(
        main_thread: MainThreadMarker,
        background: Rc<Background>,
        settings: Settings,
    ) -> Rc<Self> {
        let content = NSView::new(main_thread);
        // The fade from one screen to the next is drawn by its layer.
        content.setWantsLayer(true);
        let content_controller = view_controller(main_thread);
        content_controller.setView(&content);
        let shell = Rc::new(Self {
            main_thread,
            background,
            window: window(main_thread),
            content,
            sidebar: OnceCell::new(),
            toolbar: OnceCell::new(),
            commands: OnceCell::new(),
            places: RefCell::new(Vec::new()),
            session: RefCell::new(None),
            history: RefCell::new(Vec::new()),
            position: Cell::new(0),
            moves: Cell::new(0),
            notice: RefCell::new(None),
            split: NSSplitViewController::new(main_thread),
            playing: RefCell::new(None),
            opening_player: Cell::new(false),
            settings: Cell::new(settings),
        });

        let selected = Rc::downgrade(&shell);
        let sidebar = Sidebar::new(main_thread, move |destination| {
            if let Some(shell) = selected.upgrade() {
                shell.go_to(destination);
            }
        });
        let sidebar_controller = view_controller(main_thread);
        sidebar_controller.setView(sidebar.view());
        let sidebar_item = NSSplitViewItem::sidebarWithViewController(&sidebar_controller);
        sidebar_item.setMinimumThickness(theme::SIDEBAR_WIDTH);
        sidebar_item.setMaximumThickness(theme::SIDEBAR_WIDTH);

        shell.split.addSplitViewItem(&sidebar_item);
        shell.split.addSplitViewItem(&NSSplitViewItem::splitViewItemWithViewController(
            &content_controller,
        ));
        shell.window.setContentViewController(Some(&shell.split));
        shell.window.setContentSize(theme::WINDOW_SIZE);
        shell.window.center();

        let searched = Rc::downgrade(&shell);
        let toolbar = Toolbar::new(
            main_thread,
            shell.command(Self::back),
            shell.command(Self::forward),
            move |query: &str| {
                if let Some(shell) = searched.upgrade() {
                    shell.search(query);
                }
            },
        );
        toolbar.install(&shell.window);

        let menu = |act: fn(&Rc<Self>)| {
            let act = shell.command(act);
            Callback::new(main_thread, move |_| act())
        };
        let commands = Commands {
            back: menu(Self::back),
            forward: menu(Self::forward),
            reload: menu(Self::reload),
            find: menu(Self::focus_search),
            play: menu(Self::play_current),
            toggle_watched: menu(Self::toggle_watched_current),
        };
        menus::install(main_thread, &commands, &[]);

        // None of these can fail: the cells were created empty above.
        let _ = shell.sidebar.set(sidebar);
        let _ = shell.toolbar.set(toolbar);
        let _ = shell.commands.set(commands);
        shell
    }

    /// A closure that runs `act` on the shell for as long as there is one.
    fn command(self: &Rc<Self>, act: fn(&Rc<Self>)) -> impl Fn() + 'static {
        let shell = Rc::downgrade(self);
        move || {
            if let Some(shell) = shell.upgrade() {
                act(&shell);
            }
        }
    }

    // Starting up.

    /// Finds the server and shows what is on it.
    fn connect(self: &Rc<Self>, client: Client, token: Token, snapshot: Option<Snapshot>) {
        let (responses, saved) = match saved_server(&client) {
            Ok(saved) => saved,
            Err(error) => return self.show_failure(UNREACHABLE, &error, Self::restart),
        };
        let subject = snapshot.clone();

        // First, what the last launch left: the same server and the
        // libraries it had then. This puts the sidebar and Home on screen
        // at once.
        if let Some(server) = saved.clone() {
            let (shell, subject) = (Rc::clone(self), subject.clone());
            let remembered = server.remembered();
            let ask = async move { remembered.sections().await };
            self.background.run(self.main_thread, ask, move |sections| {
                if let (Ok(sections), None) = (sections, shell.session()) {
                    shell.connected(server, sections, subject.as_ref());
                }
            });
        } else {
            self.show_notice(Notice::new(
                self.main_thread,
                "server.rack",
                "Looking for your server",
                "Asking plex.tv where it is.",
                None,
            ));
        }

        // Then the truth. The saved server is asked directly, which is one
        // request. Only if it does not answer, say on another network, is
        // plex.tv asked where the server is now.
        let find = async move {
            if let Some(server) = saved
                && let Ok(sections) = server.sections().await
            {
                return Ok((server, sections));
            }
            let servers = plex::discovery::servers(&client, &token).await?;
            let server = servers.first().ok_or(Error::NoServer)?;
            let server = MediaServer::connect(&client, server, None).await?.with_cache(responses);
            let sections = server.sections().await?;
            Ok::<_, Error>((server, sections))
        };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, find, move |found| {
            match found {
                Ok((server, sections)) => {
                    if let Err(error) = account::save_server(&server) {
                        eprintln!("plex-rust: {}", with_causes(&error));
                    }
                    // Showing the same server and libraries again would
                    // reload the screen the viewer is already looking at.
                    let known = shell.session().is_some_and(|session| {
                        session.server.address() == server.address() && session.sections == sections
                    });
                    if !known {
                        shell.connected(server, sections, subject.as_ref());
                    }
                }
                Err(error) if shell.session().is_some() => {
                    eprintln!("plex-rust: {}", with_causes(&error));
                }
                Err(error) => shell.show_failure(UNREACHABLE, &error, Self::restart),
            }
            if let Some(snapshot) = snapshot {
                shell.snapshot_later(snapshot);
            }
        });
    }

    /// Starts over from the stored account, after a failure to connect.
    fn restart(self: &Rc<Self>) {
        match account::signed_in() {
            Ok((client, token)) => self.connect(client, token, None),
            Err(Error::SignedOut) => self.show_sign_in(),
            Err(error) => self.show_failure(UNREACHABLE, &error, Self::restart),
        }
    }

    fn connected(
        self: &Rc<Self>,
        server: MediaServer,
        sections: Vec<Section>,
        snapshot: Option<&Snapshot>,
    ) {
        let images =
            match Images::new(self.main_thread, Rc::clone(&self.background), server.clone()) {
                Ok(images) => Rc::new(images),
                Err(error) => return self.show_failure(UNREACHABLE, &error, Self::restart),
            };
        let session = Session { server, images, sections: sections.clone() };
        *self.session.borrow_mut() = Some(session);
        self.history.borrow_mut().clear();

        let mut destinations = vec![Destination::Home];
        destinations.extend(sections.iter().cloned().map(Destination::Section));
        self.install_places(&destinations);
        if let Some(sidebar) = self.sidebar.get() {
            sidebar.show(destinations);
        }

        let tweaks = snapshot.map(|snapshot| snapshot.tweaks.clone()).unwrap_or_default();
        match snapshot.map(|snapshot| &snapshot.subject) {
            Some(Subject::Library(index)) => match sections.get(*index) {
                Some(section) => self.go(Route::Library(section.clone(), tweaks.listing)),
                None => self.go(Route::Home),
            },
            Some(Subject::Item(key)) => self.fetch(RatingKey::new(key.as_str()), Self::open),
            Some(Subject::Play(key, keys)) => {
                let keys = keys.clone();
                self.fetch(RatingKey::new(key.as_str()), move |shell, item| {
                    shell.open_player(item, Purpose::Check);
                    shell.press_later(keys);
                });
            }
            Some(Subject::Search(query)) => {
                if let Some(toolbar) = self.toolbar.get() {
                    toolbar.set_search_text(query);
                }
                self.go(Route::Search(query.clone()));
            }
            Some(Subject::Home | Subject::SignIn) | None => self.go(Route::Home),
        }
    }

    /// Gives Home and the libraries their entries in the Go menu.
    fn install_places(self: &Rc<Self>, destinations: &[Destination]) {
        let place = |destination: &Destination| {
            let (shell, destination) = (Rc::downgrade(self), destination.clone());
            let title = destination.title().to_owned();
            let go = Callback::new(self.main_thread, move |_| {
                if let Some(shell) = shell.upgrade() {
                    shell.go_to(&destination);
                }
            });
            (title, go)
        };
        let places: Vec<menus::Place> = destinations.iter().map(place).collect();
        if let Some(commands) = self.commands.get() {
            menus::install(self.main_thread, commands, &places);
        }
        *self.places.borrow_mut() = places;
    }

    fn session(&self) -> Option<Session> {
        self.session.borrow().clone()
    }

    // Signing in.

    fn show_sign_in(self: &Rc<Self>) {
        let pressed = self.command(Self::sign_in);
        self.show_notice(Notice::new(
            self.main_thread,
            "person.crop.circle",
            "Sign in to Plex",
            "plex-rust opens plex.tv in your browser. Approve the sign-in there and come back.",
            Some(("Sign In", Callback::new(self.main_thread, move |_| pressed()))),
        ));
    }

    /// Runs the sign-in: asks plex.tv for a link, opens it in the browser,
    /// and waits for the viewer to approve it there.
    fn sign_in(self: &Rc<Self>) {
        const FAILED: &str = "The sign-in did not go through";
        if let Some(notice) = &*self.notice.borrow() {
            notice.set_busy("Waiting for your browser…");
        }
        let client = match account::client() {
            Ok(client) => client,
            Err(error) => return self.show_failure(FAILED, &error, Self::show_sign_in),
        };
        let asking = client.clone();
        let ask = async move { plex::auth::request_pin(&asking).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, ask, move |pin| {
            let pin = match pin {
                Ok(pin) => pin,
                Err(error) => return shell.show_failure(FAILED, &error, Self::show_sign_in),
            };
            account::open_in_browser(&pin.approval_url(&client));
            let waiting = client.clone();
            let wait = async move { plex::auth::wait_for_token(&waiting, &pin).await };
            let signed_in = Rc::clone(&shell);
            shell.background.run(shell.main_thread, wait, move |token| {
                let stored = token
                    .map_err(Error::from)
                    .and_then(|token| account::store_token(&token).map(|()| token));
                match stored {
                    Ok(token) => signed_in.connect(client, token, None),
                    Err(error) => signed_in.show_failure(FAILED, &error, Self::show_sign_in),
                }
            });
        });
    }

    // Moving from place to place.

    /// Goes somewhere new, leaving the current place one step back.
    fn go(self: &Rc<Self>, route: Route) {
        if self.playing.borrow().is_some() {
            return;
        }
        self.moves.set(self.moves.get() + 1);
        let Some(entry) = self.build(route) else {
            return;
        };
        let next = if self.history.borrow().is_empty() { 0 } else { self.position.get() + 1 };
        {
            let mut history = self.history.borrow_mut();
            history.truncate(next);
            history.push(entry);
        }
        self.position.set(next);
        self.display(true);
    }

    /// Puts another place where the current one is, without a step back to
    /// it: the same library in another order, the next search.
    fn replace(self: &Rc<Self>, route: Route) {
        self.moves.set(self.moves.get() + 1);
        let Some(entry) = self.build(route) else {
            return;
        };
        match self.history.borrow_mut().get_mut(self.position.get()) {
            Some(current) => *current = entry,
            None => return,
        }
        self.display(false);
    }

    fn back(self: &Rc<Self>) {
        if self.position.get() > 0 {
            self.step_to(self.position.get() - 1);
        }
    }

    fn forward(self: &Rc<Self>) {
        if self.position.get() + 1 < self.history.borrow().len() {
            self.step_to(self.position.get() + 1);
        }
    }

    /// Shows the entry at `position` as it was left, then brings it up to
    /// date: what was watched since may have changed it.
    fn step_to(self: &Rc<Self>, position: usize) {
        if self.playing.borrow().is_some() {
            return;
        }
        self.moves.set(self.moves.get() + 1);
        self.position.set(position);
        self.display(true);
        self.refresh();
    }

    /// Asks the server about the current place again and updates the
    /// screen where the answer differs.
    fn refresh(&self) {
        let reload = self.current(|entry| Rc::clone(&entry.reload));
        if let Some(reload) = reload {
            reload();
        }
    }

    /// Builds the current place again from nothing. For Command-R, and for
    /// trying again after a failure.
    fn reload(self: &Rc<Self>) {
        if let Some(route) = self.current(|entry| entry.route.clone()) {
            self.replace(route);
        }
    }

    /// Shows a place from the sidebar or the Go menu.
    fn go_to(self: &Rc<Self>, destination: &Destination) {
        let route = match destination {
            Destination::Home => Route::Home,
            Destination::Section(section) => Route::Library(section.clone(), Listing::default()),
        };
        // Selecting the row of the place already on screen, as `display`
        // does to keep the sidebar in step, must not go there again.
        let here = self.current(|entry| destination_of(&entry.route).as_ref() == Some(destination));
        if here != Some(true) {
            self.go(route);
        }
    }

    fn current<T>(&self, read: impl FnOnce(&Entry) -> T) -> Option<T> {
        self.history.borrow().get(self.position.get()).map(read)
    }

    /// Puts the current entry's screen in the window.
    fn display(self: &Rc<Self>, fade: bool) {
        let Some((screen, route)) =
            self.current(|entry| (entry.screen.clone(), entry.route.clone()))
        else {
            return;
        };
        *self.notice.borrow_mut() = None;
        self.show_view(screen.view(), fade);
        self.window.setTitle(&NSString::from_str(title(&route)));
        if let Some(toolbar) = self.toolbar.get() {
            let position = self.position.get();
            toolbar.set_history(position > 0, position + 1 < self.history.borrow().len());
        }
        if let Some(sidebar) = self.sidebar.get() {
            sidebar.select(destination_of(&route).as_ref());
        }
        // Results arrive while the viewer is still typing in the search
        // field, which must keep the keyboard.
        if !matches!(route, Route::Search(_)) {
            screen.focus();
        }
    }

    /// Makes `view` the content, optionally fading from what was there.
    fn show_view(&self, view: &NSView, fade: bool) {
        if let (true, Some(layer)) = (fade, self.content.layer()) {
            let transition = CATransition::new();
            // SAFETY: the type is a constant that Core Animation provides.
            transition.setType(unsafe { kCATransitionFade });
            transition.setDuration(theme::SCREEN_FADE);
            layer.addAnimation_forKey(&transition, Some(ns_string!("screen")));
        }
        layout::fill(&self.content, view);
    }

    // The screens.

    /// Creates the screen for `route` and starts loading what it shows.
    fn build(self: &Rc<Self>, route: Route) -> Option<Entry> {
        let session = self.session()?;
        let (screen, reload) = match &route {
            Route::Home => self.home(&session),
            Route::Library(section, listing) => self.library(&session, section, *listing),
            Route::Item(item, season) => self.item_page(&session, item, season.clone()),
            Route::Search(query) => self.search_results(&session, query),
        };
        reload();
        Some(Entry { route, screen, reload })
    }

    fn home(self: &Rc<Self>, session: &Session) -> (Screen, Rc<dyn Fn()>) {
        let outlines = vec![
            Outline { title: "Continue Watching", style: Style::Shelf(Shape::Landscape), count: 6 },
            Outline { title: "Recently Added", style: Style::Shelf(Shape::Poster), count: 10 },
        ];
        let posters = self.posters(session, None, outlines, "Nothing to watch yet", || {});
        let shown = posters.clone();
        let reload = self.loader(
            session,
            |server| async move { server.home(SHELF_LENGTH).await },
            move |hubs: &Vec<Hub>| shown.show(home_shelves(hubs)),
        );
        (Screen::Posters(posters), reload)
    }

    fn library(
        self: &Rc<Self>,
        session: &Session,
        section: &Section,
        listing: Listing,
    ) -> (Screen, Rc<dyn Fn()>) {
        let (reordered, of) = (Rc::downgrade(self), section.clone());
        let noun = if section.kind == Kind::Show { "show" } else { "film" };
        let bar = LibraryBar::new(self.main_thread, listing, noun, move |listing| {
            if let Some(shell) = reordered.upgrade() {
                shell.replace(Route::Library(of.clone(), listing));
            }
        });
        let header = Header { view: bar.view().retain(), height: theme::LIBRARY_BAR_HEIGHT };
        let outlines = vec![Outline { title: "", style: Style::Grid(Shape::Poster), count: 24 }];
        let (more, of) = (Rc::downgrade(self), section.clone());
        let nothing = if listing.unwatched_only { "Nothing left to watch" } else { "Nothing here" };
        let posters = self.posters(session, Some(header), outlines, nothing, move || {
            if let Some(shell) = more.upgrade() {
                shell.load_page(&of, listing);
            }
        });

        let (shown, section) = (posters.clone(), section.clone());
        let reload = self.loader(
            session,
            move |server| {
                let section = section.clone();
                async move { server.section_items(&section, listing, 0, PAGE_SIZE).await }
            },
            // The bar lives as long as this closure, which the entry keeps.
            move |page: &Page| {
                bar.set_total(page.total);
                let (title, style) = (String::new(), Style::Grid(Shape::Poster));
                shown.show(vec![Shelf { title, style, items: page.items.clone() }]);
            },
        );
        (Screen::Posters(posters), reload)
    }

    /// Fetches the next page of a library and adds it to the grid, if that
    /// library is still the one on screen.
    fn load_page(self: &Rc<Self>, section: &Section, listing: Listing) {
        let route = Route::Library(section.clone(), listing);
        let on_screen = self.current(|entry| (entry.route == route).then(|| entry.screen.clone()));
        let (Some(session), Some(Some(Screen::Posters(posters)))) = (self.session(), on_screen)
        else {
            return;
        };
        let start = u32::try_from(posters.len()).unwrap_or(u32::MAX);
        let (server, section) = (session.server.clone(), section.clone());
        let load = async move { server.section_items(&section, listing, start, PAGE_SIZE).await };
        let (shell, moves) = (Rc::clone(self), self.moves.get());
        self.background.run(self.main_thread, load, move |page| match page {
            Ok(page) => posters.append(page.items),
            Err(error) => shell.fail(moves, &error),
        });
    }

    fn item_page(
        self: &Rc<Self>,
        session: &Session,
        item: &Item,
        season: Option<RatingKey>,
    ) -> (Screen, Rc<dyn Fn()>) {
        let detail = Detail::new(self.main_thread, &session.images, season, self.actions());
        // What the list the viewer came from knew is enough to draw the
        // page at once. The full item follows.
        detail.show(item);

        let (shown, key) = (detail.clone(), item.rating_key.clone());
        let load_item = self.loader(
            session,
            move |server| {
                let key = key.clone();
                async move { server.item(&key).await }
            },
            move |item: &Item| shown.show(item),
        );
        let load_episodes = (item.kind == Kind::Show).then(|| {
            let (shown, key) = (detail.clone(), item.rating_key.clone());
            self.loader(
                session,
                move |server| {
                    let key = key.clone();
                    async move { server.episodes(&key).await }
                },
                move |episodes: &Vec<Item>| shown.show_episodes(episodes.clone()),
            )
        });
        let reload = Rc::new(move || {
            load_item();
            if let Some(load_episodes) = &load_episodes {
                load_episodes();
            }
        });
        (Screen::Detail(detail), reload)
    }

    fn search_results(self: &Rc<Self>, session: &Session, query: &str) -> (Screen, Rc<dyn Fn()>) {
        let nothing = format!("Nothing found for “{query}”");
        let posters = self.posters(session, None, Vec::new(), &nothing, || {});
        let (shown, query) = (posters.clone(), query.to_owned());
        let reload = self.loader(
            session,
            move |server| {
                let query = query.clone();
                async move { server.search(&query, SEARCH_LIMIT).await }
            },
            move |hubs: &Vec<Hub>| shown.show(search_shelves(hubs)),
        );
        (Screen::Posters(posters), reload)
    }

    /// Searches for `query` as the viewer types it. Each new query takes
    /// the place of the last, so that Back leaves the search in one step.
    fn search(self: &Rc<Self>, query: &str) {
        let searching = self.current(|entry| matches!(entry.route, Route::Search(_))) == Some(true);
        match (query.is_empty(), searching) {
            (true, true) => self.back(),
            (true, false) => {}
            (false, true) => self.replace(Route::Search(query.to_owned())),
            (false, false) => self.go(Route::Search(query.to_owned())),
        }
    }

    fn focus_search(self: &Rc<Self>) {
        if let Some(toolbar) = self.toolbar.get() {
            toolbar.focus_search();
        }
    }

    /// A new, empty collection that reports what the viewer does in it.
    fn posters(
        self: &Rc<Self>,
        session: &Session,
        header: Option<Header>,
        outlines: Vec<Outline>,
        nothing: &str,
        on_near_end: impl Fn() + 'static,
    ) -> Retained<Posters> {
        Posters::new(
            self.main_thread,
            Rc::clone(&session.images),
            header,
            outlines,
            nothing,
            self.actions(),
            on_near_end,
        )
    }

    /// Makes a closure that asks the server something and shows the
    /// answer. The first time it runs it asks twice over: what the server
    /// answered the last time the app ran, so the screen is full at once,
    /// then what it answers now. `show` is only called with an answer that
    /// differs from the one on screen, so a screen that is already right
    /// is left alone.
    fn loader<T, F>(
        self: &Rc<Self>,
        session: &Session,
        ask: impl Fn(MediaServer) -> F + 'static,
        show: impl Fn(&T) + 'static,
    ) -> Rc<dyn Fn()>
    where
        T: PartialEq + Send + 'static,
        F: Future<Output = plex::Result<T>> + Send + 'static,
    {
        let shown: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
        let present = Rc::new(move |answer: T| {
            if shown.borrow().as_ref() != Some(&answer) {
                show(&answer);
                *shown.borrow_mut() = Some(answer);
            }
        });
        // Set once anything is on screen, so that a failure to refresh it
        // is not shown in its place.
        let has_shown = Rc::new(Cell::new(false));
        let (shell, server) = (Rc::downgrade(self), session.server.clone());
        Rc::new(move || {
            let Some(shell) = shell.upgrade() else {
                return;
            };
            // Set once the server's own answer is on screen, so that a late
            // answer from last time cannot replace it.
            let is_current = Rc::new(Cell::new(false));
            if !has_shown.get() {
                let (present, current, has_shown) =
                    (Rc::clone(&present), Rc::clone(&is_current), Rc::clone(&has_shown));
                let remembered = ask(server.remembered());
                shell.background.run(shell.main_thread, remembered, move |answer| {
                    if let (Ok(answer), false) = (answer, current.get()) {
                        present(answer);
                        has_shown.set(true);
                    }
                });
            }

            let (present, has_shown) = (Rc::clone(&present), Rc::clone(&has_shown));
            let (failed, moves) = (Rc::clone(&shell), shell.moves.get());
            let fresh = ask(server.clone());
            shell.background.run(shell.main_thread, fresh, move |answer| match answer {
                Ok(answer) => {
                    is_current.set(true);
                    present(answer);
                    has_shown.set(true);
                }
                // What is on screen from last time is better than an error.
                Err(error) if has_shown.get() => eprintln!("plex-rust: {}", with_causes(&error)),
                Err(error) => failed.fail(moves, &error),
            });
        })
    }

    // What the viewer does with an item.

    fn actions(self: &Rc<Self>) -> impl Fn(&Item, Action) + 'static {
        let shell = Rc::downgrade(self);
        move |item, action| {
            let Some(shell) = shell.upgrade() else {
                return;
            };
            match action {
                Action::Open => shell.open(item),
                Action::Play => shell.play(item, Purpose::Watching),
                Action::SetWatched(watched) => shell.set_watched(item, watched),
                Action::OpenShow => shell.open_show_of(item),
            }
        }
    }

    /// Goes to the page of `item`. A season has no page of its own: it is
    /// its show's page, opened on that season.
    fn open(self: &Rc<Self>, item: &Item) {
        match (item.kind, &item.parent_rating_key) {
            (Kind::Season, Some(show)) => {
                let season = item.rating_key.clone();
                self.fetch(show.clone(), move |shell, show| {
                    shell.go(Route::Item(Box::new(show.clone()), Some(season)));
                });
            }
            _ => self.go(Route::Item(Box::new(item.clone()), None)),
        }
    }

    /// Goes from an episode to its show, opened on the episode's season.
    fn open_show_of(self: &Rc<Self>, episode: &Item) {
        let Some(show) = episode.grandparent_rating_key.clone() else {
            return;
        };
        let season = episode.parent_rating_key.clone();
        self.fetch(show, move |shell, show| {
            shell.go(Route::Item(Box::new(show.clone()), season));
        });
    }

    /// Marks `item` as watched or not on the server, then brings the
    /// screen up to date with what that changed.
    fn set_watched(self: &Rc<Self>, item: &Item, watched: bool) {
        let Some(session) = self.session() else {
            return;
        };
        let (server, key) = (session.server.clone(), item.rating_key.clone());
        let mark = async move { server.set_watched(&key, watched).await };
        let (shell, moves) = (Rc::clone(self), self.moves.get());
        self.background.run(self.main_thread, mark, move |marked| match marked {
            Ok(()) => shell.refresh(),
            Err(error) => shell.fail(moves, &error),
        });
    }

    fn play_current(self: &Rc<Self>) {
        if let Some(Screen::Detail(detail)) = self.current(|entry| entry.screen.clone()) {
            detail.press_play();
        }
    }

    fn toggle_watched_current(self: &Rc<Self>) {
        if let Some(Screen::Detail(detail)) = self.current(|entry| entry.screen.clone()) {
            detail.press_watched();
        }
    }

    /// Fetches the item with `key` and hands it to `then`.
    fn fetch(self: &Rc<Self>, key: RatingKey, then: impl FnOnce(&Rc<Self>, &Item) + 'static) {
        let Some(session) = self.session() else {
            return;
        };
        let server = session.server.clone();
        let load = async move { server.item(&key).await };
        let (shell, moves) = (Rc::clone(self), self.moves.get());
        self.background.run(self.main_thread, load, move |item| match item {
            Ok(item) => then(&shell, &item),
            Err(error) => shell.fail(moves, &error),
        });
    }

    // Playing.

    /// Plays `item`, first fetching it in full: a listing leaves out the
    /// intro and credits markers that the player offers to skip.
    fn play(self: &Rc<Self>, item: &Item, purpose: Purpose) {
        // A second click on Play while the first is still fetching must
        // not open a second player behind the first.
        if self.playing.borrow().is_some() || self.opening_player.replace(true) {
            return;
        }
        let Some(session) = self.session() else {
            self.opening_player.set(false);
            return;
        };
        let (server, key) = (session.server.clone(), item.rating_key.clone());
        let load = async move { server.item(&key).await };
        let (shell, moves) = (Rc::clone(self), self.moves.get());
        self.background.run(self.main_thread, load, move |item| {
            shell.opening_player.set(false);
            match item {
                Ok(item) => shell.open_player(&item, purpose),
                Err(error) => shell.fail(moves, &error),
            }
        });
    }

    /// Replaces the whole window content with the player, playing `item`.
    fn open_player(self: &Rc<Self>, item: &Item, purpose: Purpose) {
        let Some(session) = self.session() else {
            return;
        };
        // Replacing a player without closing it would leave it running
        // unseen, with nothing left that could stop it.
        if self.playing.borrow().is_some() {
            return;
        }
        let closed = Rc::downgrade(self);
        let screen = PlayerScreen::open(
            self.main_thread,
            Rc::clone(&self.background),
            session.server.clone(),
            item,
            self.settings.get(),
            purpose,
            move |outcome| {
                if let Some(shell) = closed.upgrade() {
                    shell.player_closed(outcome, purpose);
                }
            },
        );
        match screen {
            Ok(screen) => {
                let controller = view_controller(self.main_thread);
                controller.setView(screen.view());
                self.set_toolbar_visible(false);
                self.set_window_content(&controller);
                screen.focus();
                *self.playing.borrow_mut() = Some(screen);
            }
            Err(error) => self.fail(self.moves.get(), &error),
        }
    }

    fn player_closed(self: &Rc<Self>, outcome: Outcome, purpose: Purpose) {
        let Some(screen) = self.playing.borrow_mut().take() else {
            return;
        };
        self.keep_settings(&screen);
        self.set_window_content(&self.split);
        self.set_toolbar_visible(true);
        // The screen behind the player still says where the item was left
        // before it was played.
        if purpose == Purpose::Watching {
            self.refresh();
        }
        match outcome {
            Outcome::Closed => {}
            Outcome::Finished if screen.item().kind == Kind::Episode => {
                self.play_episode_after(screen.item(), purpose);
            }
            Outcome::Finished => {}
            Outcome::Previous => self.play_episode_before(screen.item(), purpose),
            Outcome::Failed(reason) => {
                let dismiss = self.command(|shell| shell.display(true));
                self.show_notice(Notice::new(
                    self.main_thread,
                    "exclamationmark.triangle",
                    &format!("{} could not be played", screen.item().title),
                    &reason,
                    Some(("Go Back", Callback::new(self.main_thread, move |_| dismiss()))),
                ));
            }
        }
    }

    /// The toolbar belongs to browsing. The player has its own controls.
    fn set_toolbar_visible(&self, visible: bool) {
        if let Some(toolbar) = self.toolbar.get() {
            toolbar.set_visible(visible);
        }
    }

    /// Takes over the speed and volume the viewer set in `screen`, for
    /// whatever is played next and for the next launch.
    fn keep_settings(&self, screen: &PlayerScreen) {
        let settings = match screen.settings() {
            Ok(Some(settings)) => settings,
            Ok(None) => return,
            Err(error) => return eprintln!("plex-rust: {error}"),
        };
        self.settings.set(settings);
        if let Err(error) = settings.save() {
            eprintln!("plex-rust: {}", with_causes(&error));
        }
    }

    /// Plays the episode that follows `episode` in its season, if there is
    /// one. The end of a season is a natural place to stop.
    fn play_episode_after(self: &Rc<Self>, episode: &Item, purpose: Purpose) {
        let (Some(session), Some(season), Some(number)) =
            (self.session(), episode.parent_rating_key.clone(), episode.index)
        else {
            return;
        };
        let server = session.server.clone();
        let load = async move { server.children(&season).await };
        let (shell, moves) = (Rc::clone(self), self.moves.get());
        self.background.run(self.main_thread, load, move |episodes| match episodes {
            Ok(episodes) => {
                // The lowest number above this one, not this one plus one: a
                // season can have a gap, and one file can hold two episodes.
                let later = episodes.iter().filter(|episode| episode.index > Some(number));
                let next = later.min_by_key(|episode| episode.index);
                if let Some(next) = next {
                    shell.play(next, purpose);
                }
            }
            Err(error) => shell.fail(moves, &error),
        });
    }

    /// Plays the episode that comes before `episode` in its season, if
    /// there is one.
    fn play_episode_before(self: &Rc<Self>, episode: &Item, purpose: Purpose) {
        let (Some(session), Some(season), Some(number)) =
            (self.session(), episode.parent_rating_key.clone(), episode.index)
        else {
            return;
        };
        let server = session.server.clone();
        let load = async move { server.children(&season).await };
        let (shell, moves) = (Rc::clone(self), self.moves.get());
        self.background.run(self.main_thread, load, move |episodes| match episodes {
            Ok(episodes) => {
                // The highest number below this one, for the reasons given
                // in `play_episode_after`. An episode with no number sorts
                // as `None` and is not one of them.
                let earlier = episodes
                    .iter()
                    .filter(|episode| episode.index.is_some_and(|index| index < number));
                if let Some(previous) = earlier.max_by_key(|episode| episode.index) {
                    shell.play(previous, purpose);
                }
            }
            Err(error) => shell.fail(moves, &error),
        });
    }

    /// Presses `keys` in the player once it has had time to open its file.
    fn press_later(self: &Rc<Self>, keys: Vec<Key>) {
        // A timer can only be created on the runtime's own threads.
        let wait = async { tokio::time::sleep(SNAPSHOT_KEYS_AFTER).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, wait, move |()| {
            // Cloned so that a key that closes the player finds no borrow.
            let screen = shell.playing.borrow().clone();
            if let Some(screen) = screen {
                for key in keys {
                    screen.press(key);
                }
            }
        });
    }

    /// Swaps what the window shows without letting it change size. A
    /// window otherwise shrinks to fit a controller whose view has no size
    /// of its own.
    fn set_window_content(&self, controller: &NSViewController) {
        let frame = self.window.frame();
        self.window.setContentViewController(Some(controller));
        self.window.setFrame_display(frame, true);
    }

    // Messages in place of a screen.

    fn show_notice(&self, notice: Notice) {
        self.show_view(notice.view(), false);
        *self.notice.borrow_mut() = Some(notice);
    }

    /// Shows `error` in place of the screen under `title`, with a button
    /// that runs `retry`.
    fn show_failure(
        self: &Rc<Self>,
        title: &str,
        error: &dyn std::error::Error,
        retry: fn(&Rc<Self>),
    ) {
        let retry = self.command(retry);
        self.show_notice(Notice::new(
            self.main_thread,
            "wifi.exclamationmark",
            title,
            &with_causes(error),
            Some(("Try Again", Callback::new(self.main_thread, move |_| retry()))),
        ));
    }

    /// Shows a failure to load the current place, unless the viewer has
    /// moved on since move number `moves`, when it is only logged: an
    /// error about a screen they left would replace the one they are on.
    fn fail(self: &Rc<Self>, moves: u64, error: &dyn std::error::Error) {
        if moves == self.moves.get() {
            self.show_failure(NOT_LOADED, error, Self::reload);
        } else {
            eprintln!("plex-rust: {}", with_causes(error));
        }
    }

    // Snapshots.

    /// Waits for the screen to settle, applies the snapshot's tweaks,
    /// waits again and writes the PNG.
    fn snapshot_later(self: &Rc<Self>, snapshot: Snapshot) {
        // A timer can only be created on the runtime's own threads.
        let settle = async { tokio::time::sleep(SNAPSHOT_SETTLE).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, settle, move |()| {
            let screen = shell.current(|entry| entry.screen.clone());
            if let Some(screen) = &screen {
                if snapshot.tweaks.scroll > 0.0 {
                    screen.scroll_to(snapshot.tweaks.scroll);
                }
                if let (true, Screen::Posters(posters)) = (snapshot.tweaks.hover, screen) {
                    posters.hover_first();
                }
                if let (true, Screen::Detail(detail)) = (snapshot.tweaks.follow_link, screen) {
                    detail.follow_show_link();
                }
            }
            let settle = async { tokio::time::sleep(SNAPSHOT_TWEAKS_SETTLE).await };
            let done = Rc::clone(&shell);
            let write = move |()| done.snapshot_and_exit(&snapshot.path);
            shell.background.run(shell.main_thread, settle, write);
        });
    }

    fn snapshot_and_exit(&self, path: &Path) {
        // The content view's parent is the window's frame, which also draws
        // the background and the title bar.
        // SAFETY: the content view is in a window, so its parent is alive.
        let frame = self.window.contentView().and_then(|view| unsafe { view.superview() });
        let result = match frame {
            Some(view) => {
                // A window that was never on screen has not laid out yet.
                view.layoutSubtreeIfNeeded();
                snapshot::write_png(&view, path)
            }
            None => Err(Error::Snapshot),
        };
        if let Err(error) = result {
            eprintln!("plex-rust: {}", with_causes(&error));
            std::process::exit(1);
        }
        std::process::exit(0);
    }
}

/// The words in the window's title bar for `route`.
fn title(route: &Route) -> &str {
    match route {
        Route::Home => "Home",
        Route::Library(section, _) => &section.title,
        Route::Item(item, _) => &item.title,
        Route::Search(_) => "Search",
    }
}

/// The sidebar row that stands for `route`, if one does.
fn destination_of(route: &Route) -> Option<Destination> {
    match route {
        Route::Home => Some(Destination::Home),
        Route::Library(section, _) => Some(Destination::Section(section.clone())),
        Route::Item(..) | Route::Search(_) => None,
    }
}

/// The shelves of the home screen. What the viewer is in the middle of
/// comes as wide cards with their progress. "On Deck" is left out when
/// "Continue Watching" is there, which already holds everything in it.
fn home_shelves(hubs: &[Hub]) -> Vec<Shelf> {
    const CONTINUE: &str = "home.continue";
    const ON_DECK: &str = "home.ondeck";
    let has_continue = hubs.iter().any(|hub| hub.identifier == CONTINUE);
    hubs.iter()
        .filter(|hub| !(has_continue && hub.identifier == ON_DECK))
        .map(|hub| {
            let wide = matches!(hub.identifier.as_str(), CONTINUE | ON_DECK);
            let shape = if wide { Shape::Landscape } else { Shape::Poster };
            Shelf { title: hub.title.clone(), style: Style::Shelf(shape), items: hub.items.clone() }
        })
        .collect()
}

/// The shelves of a search: one per kind of result, episodes as frames.
fn search_shelves(hubs: &[Hub]) -> Vec<Shelf> {
    hubs.iter()
        .map(|hub| {
            let episodes = hub.items.first().is_some_and(|item| item.kind == Kind::Episode);
            let shape = if episodes { Shape::Landscape } else { Shape::Poster };
            Shelf { title: hub.title.clone(), style: Style::Shelf(shape), items: hub.items.clone() }
        })
        .collect()
}

/// Where the server's answers are kept between launches, and the server
/// used the last time the app ran, if there was one.
fn saved_server(client: &Client) -> Result<(PathBuf, Option<MediaServer>)> {
    let home = std::env::var_os("HOME").ok_or(Error::NoHome)?;
    let responses = PathBuf::from(home).join("Library/Caches/plex-rust/responses");
    std::fs::create_dir_all(&responses)
        .map_err(|source| Error::File { path: responses.clone(), source })?;
    let saved = account::saved_server(client)?.map(|server| server.with_cache(responses.clone()));
    Ok((responses, saved))
}

fn window(main_thread: MainThreadMarker) -> Retained<NSWindow> {
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::FullSizeContentView;
    // SAFETY: the arguments are valid, and `setReleasedWhenClosed(false)`
    // below gives the window the ownership rules `Retained` expects.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(main_thread),
            NSRect::new(NSPoint::ZERO, theme::WINDOW_SIZE),
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: called before the window can be closed.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(ns_string!("plex-rust"));
    window.setContentMinSize(theme::WINDOW_MIN_SIZE);
    window
}

fn view_controller(main_thread: MainThreadMarker) -> Retained<NSViewController> {
    NSViewController::initWithNibName_bundle(NSViewController::alloc(main_thread), None, None)
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `AppDelegate`
    // does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustShellDelegate"]
    #[ivars = Rc<Shell>]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last_window_closed(&self, _app: &NSApplication) -> bool {
            true
        }

        /// Quitting mid-film must still leave the server with the place
        /// the viewer got to.
        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            if let Some(screen) = self.ivars().playing.borrow_mut().take() {
                self.ivars().keep_settings(&screen);
                screen.close_for_quit();
            }
        }
    }
);

impl AppDelegate {
    fn new(main_thread: MainThreadMarker, shell: Rc<Shell>) -> Retained<Self> {
        let this = Self::alloc(main_thread).set_ivars(shell);
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub(identifier: &str) -> Hub {
        let item = serde_json::from_value(serde_json::json!({
            "ratingKey": "1", "type": "episode", "title": "Title",
        }));
        Hub {
            title: identifier.to_owned(),
            identifier: identifier.to_owned(),
            items: vec![item.unwrap()],
            more: false,
        }
    }

    #[test]
    fn home_leaves_out_on_deck_when_continue_watching_holds_it() {
        let titles = |hubs: &[Hub]| -> Vec<String> {
            home_shelves(hubs).into_iter().map(|shelf| shelf.title).collect()
        };
        let all = [hub("home.continue"), hub("home.ondeck"), hub("home.movies.recent")];
        assert_eq!(titles(&all), ["home.continue", "home.movies.recent"]);
        assert_eq!(titles(&all[1..]), ["home.ondeck", "home.movies.recent"]);
    }
}
