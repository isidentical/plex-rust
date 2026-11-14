//! The main window: sidebar on the left, the chosen place on the right,
//! and the way from one place to the next.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameDarkAqua, NSApplication, NSApplicationActivationPolicy,
    NSApplicationDelegate, NSBackingStoreType, NSMenu, NSMenuItem, NSSplitViewController,
    NSSplitViewItem, NSView, NSViewController, NSWindow, NSWindowStyleMask, NSWindowTabbingMode,
};
use objc2_foundation::{
    NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSString, ns_string,
};
use objc2_quartz_core::{CAMediaTiming, CATransition, kCATransitionFade};
use plex::library::{Listing, Page};
use plex::model::{Hub, Item, Kind, RatingKey, Section};
use plex::{Client, MediaServer, Token};

use super::callback::Callback;
pub use super::detail::Link;
use super::detail::{Detail, Opening};
use super::downloads::{Act, DownloadList};
pub use super::failure::Failure;
use super::hero::{self, Hero};
use super::layout;
use super::library_bar::LibraryBar;
use super::menus::{self, Commands};
use super::notice::{Mark, Notice};
use super::player::input::Key;
use super::player::{Outcome, PlayerScreen, Purpose};
use super::poster::Shape;
pub use super::posters::Press;
use super::posters::{Action, Header, Outline, Posters, Shelf, Style};
use super::sidebar::{Destination, Sidebar};
use super::snapshot;
use super::theme;
use super::toolbar::Toolbar;
use crate::account;
use crate::background::Background;
use crate::downloads::{Downloads, State};
use crate::error::{Error, Result, with_causes};
use crate::images::Images;
use crate::settings::{Listings, Settings};

/// How many items each shelf of the home screen starts with.
const SHELF_LENGTH: u32 = 20;

/// How many items of a library are fetched at a time.
const PAGE_SIZE: u32 = 100;

/// How many items each row related to a film or show starts with.
const RELATED_LENGTH: u32 = 20;

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

/// How long `Key::Wait` holds up the keys after it, and the snapshot.
const SNAPSHOT_KEY_WAIT: Duration = Duration::from_secs(1);

/// How long the server of the last launch gets to answer at its old
/// address before plex.tv is asked where it is now. On another network
/// that address can take the request and never answer, and a request
/// gives up by itself only after half a minute.
const SAVED_SERVER_PATIENCE: Duration = Duration::from_secs(3);

const UNREACHABLE: &str = "Could not reach your server";
const NOT_LOADED: &str = "This could not be loaded";

struct Shell {
    main_thread: MainThreadMarker,
    background: Rc<Background>,
    window: Retained<NSWindow>,
    /// The area right of the sidebar. Its one subview is the current place.
    content: Retained<NSView>,
    sidebar: Retained<Sidebar>,
    /// The sidebar's place in the split, which it gives up while there is
    /// nothing to list in it.
    sidebar_item: Retained<NSSplitViewItem>,
    toolbar: Retained<Toolbar>,
    /// What the menu bar's own entries do.
    commands: Commands,
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
    /// Speed, volume and low power, kept from one item to the next and
    /// across launches.
    settings: Cell<Settings>,
    /// How each library was last listed, kept across launches.
    listings: RefCell<Listings>,
    /// Set for a snapshot of a screen that is still waiting: screens are
    /// then built and never loaded.
    holds_loads: Cell<bool>,
    /// What is kept on this Mac, and what is on its way to it.
    downloads: Rc<Downloads>,
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
    /// The whole of a list the server names by a path: all of a shelf, or
    /// everything a member of a cast is in.
    List {
        title: String,
        path: String,
    },
    /// The page of an item. For a show, with how it opens.
    Item(Box<Item>, Opening),
    Search(String),
    Downloads,
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
    Detail(Rc<Detail>),
    Downloads(Retained<DownloadList>),
}

impl Screen {
    fn view(&self) -> &NSView {
        match self {
            Self::Posters(posters) => posters.view(),
            Self::Detail(detail) => detail.view(),
            Self::Downloads(list) => list.view(),
        }
    }

    fn focus(&self) {
        match self {
            Self::Posters(posters) => posters.focus(),
            Self::Detail(detail) => detail.focus(),
            Self::Downloads(list) => list.focus(),
        }
    }

    fn scroll_to(&self, y: f64) {
        match self {
            Self::Posters(posters) => posters.scroll_to(y),
            Self::Detail(detail) => detail.scroll_to(y),
            Self::Downloads(list) => list.scroll_to(y),
        }
    }
}

/// A list that is fetched a page at a time as the viewer scrolls.
#[derive(Clone)]
enum Paged {
    Library(Section, Listing),
    /// The list at this server path.
    Path(String),
}

impl Paged {
    /// The page that starts at index `start`.
    async fn page(self, server: MediaServer, start: u32) -> plex::Result<Page> {
        match self {
            Self::Library(section, listing) => {
                server.section_items(&section, listing, start, PAGE_SIZE).await
            }
            Self::Path(path) => server.list(&path, start, PAGE_SIZE).await,
        }
    }
}

/// What a screen does when the server does not answer one of its
/// questions and nothing remembered stands in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Unanswered {
    /// The screen is nothing without the answer, and gives way to the
    /// error.
    ShowsTheError,
    /// The screen does without. The error is logged.
    GoesWithout,
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
    /// The whole list at this server path, as a shelf's heading leads to.
    List(String),
    /// What is kept on this Mac.
    Downloads,
    /// The screen a signed-out viewer starts on.
    SignIn,
}

/// Changes to the state a snapshot is taken in.
#[derive(Clone, Default)]
pub struct Tweaks {
    /// The window at its smallest size.
    pub narrow: bool,
    /// The window as large as a big display lets it be.
    pub wide: bool,
    /// These keys pressed among the cards, in order.
    pub presses: Vec<Press>,
    /// The pointer over the first item.
    pub hover: bool,
    /// On an episode's page, this link followed.
    pub follow: Option<Link>,
    /// Scrolled this many points down.
    pub scroll: f64,
    /// The first shelf scrolled this many points sideways.
    pub across: f64,
    /// For a library, which items and in what order.
    pub listing: Listing,
    /// The screen having failed to load in this way.
    pub fault: Option<Failure>,
    /// The screen as it is before the server or the cache has answered.
    pub unanswered: bool,
    /// The first launch after signing in, before any server is known.
    pub looking: bool,
    /// The sign-in screen once the browser has been opened.
    pub approving: bool,
    /// On the page of a film or an episode, the download button pressed.
    pub download: bool,
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

    let downloads = Downloads::open(main_thread, Rc::clone(&background))?;
    let (settings, listings) = (Settings::load()?, Listings::load()?);
    let shell = Shell::new(main_thread, Rc::clone(&background), settings, listings, downloads);
    match &snapshot {
        Some(snapshot) if snapshot.tweaks.narrow => {
            shell.window.setContentSize(theme::WINDOW_MIN_SIZE);
        }
        Some(snapshot) if snapshot.tweaks.wide => {
            shell.window.setContentSize(theme::WINDOW_WIDE_SIZE);
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
        listings: Listings,
        downloads: Rc<Downloads>,
    ) -> Rc<Self> {
        let content = NSView::new(main_thread);
        // The fade from one screen to the next is drawn by its layer.
        content.setWantsLayer(true);
        let content_controller = view_controller(main_thread);
        content_controller.setView(&content);
        let window = window(main_thread);
        let split = NSSplitViewController::new(main_thread);

        // The sidebar, the toolbar and the menus call back into the shell,
        // which is made of them: they get it before it exists.
        Rc::new_cyclic(|shell: &Weak<Self>| {
            let selected = Weak::clone(shell);
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
            split.addSplitViewItem(&sidebar_item);
            split.addSplitViewItem(&NSSplitViewItem::splitViewItemWithViewController(
                &content_controller,
            ));
            window.setContentViewController(Some(&split));
            window.setContentSize(theme::WINDOW_SIZE);
            window.center();

            let searched = Weak::clone(shell);
            let toolbar = Toolbar::new(
                main_thread,
                Self::command(shell, Self::back),
                Self::command(shell, Self::forward),
                move |query: &str| {
                    if let Some(shell) = searched.upgrade() {
                        shell.search(query);
                    }
                },
                Self::command(shell, Self::focus_content),
            );
            toolbar.install(&window);

            let menu = |act: fn(&Rc<Self>)| {
                let act = Self::command(shell, act);
                Callback::new(main_thread, move |_| act())
            };
            let commands = Commands {
                back: menu(Self::back),
                forward: menu(Self::forward),
                reload: menu(Self::reload),
                find: menu(Self::focus_search),
                play: menu(Self::play_current),
                toggle_watched: menu(Self::toggle_watched_current),
                toggle_low_power: {
                    let shell = Weak::clone(shell);
                    Callback::new(main_thread, move |entry| {
                        if let Some(shell) = shell.upgrade() {
                            shell.toggle_low_power(entry);
                        }
                    })
                },
            };
            // Until a screen is up there is nowhere to go back to and
            // nothing to play.
            for command in
                [&commands.back, &commands.forward, &commands.play, &commands.toggle_watched]
            {
                command.set_enabled(false);
            }
            menus::install(main_thread, &commands, &[], settings.low_power);
            downloads.listen(Self::command(shell, Self::downloads_changed));

            Self {
                main_thread,
                background,
                window,
                content,
                sidebar,
                sidebar_item,
                toolbar,
                commands,
                places: RefCell::new(Vec::new()),
                session: RefCell::new(None),
                history: RefCell::new(Vec::new()),
                position: Cell::new(0),
                moves: Cell::new(0),
                notice: RefCell::new(None),
                split,
                playing: RefCell::new(None),
                opening_player: Cell::new(false),
                settings: Cell::new(settings),
                listings: RefCell::new(listings),
                holds_loads: Cell::new(false),
                downloads,
            }
        })
    }

    /// A closure that runs `act` on `shell` for as long as there is one.
    fn command(shell: &Weak<Self>, act: fn(&Rc<Self>)) -> impl Fn() + 'static {
        let shell = Weak::clone(shell);
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
            self.show_looking();
        }

        // Then the truth. The saved server is asked directly, which is one
        // request. Only if it does not answer, say on another network, is
        // plex.tv asked where the server is now.
        let find = async move {
            if let Some(server) = saved
                && let Ok(Ok(sections)) =
                    tokio::time::timeout(SAVED_SERVER_PATIENCE, server.sections()).await
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
                    // Unless that is a notice, as after signing in again.
                    let known = shell.notice.borrow().is_none()
                        && shell.session().is_some_and(|session| {
                            session.server.address() == server.address()
                                && session.sections == sections
                        });
                    if !known {
                        shell.connected(server, sections, subject.as_ref());
                    }
                }
                // What the last launch left is on screen and better than
                // an error, unless the error is that the sign-in is over:
                // then nothing on screen can be played or brought up to
                // date until the viewer signs in again.
                Err(error)
                    if shell.session().is_some() && Failure::of(&error) != Failure::SignedOut =>
                {
                    eprintln!("plex-rust: {}", with_causes(&error));
                }
                Err(error) => shell.show_failure(UNREACHABLE, &error, Self::restart),
            }
            if let Some(snapshot) = snapshot {
                shell.snapshot_later(snapshot);
            }
        });
    }

    /// What the first launch shows until plex.tv has said where the
    /// server is. Later launches start from the server they remember.
    fn show_looking(&self) {
        self.show_notice(Notice::new(
            self.main_thread,
            Mark::Spinner,
            "Looking for your server",
            "Asking plex.tv where it is.",
            None,
        ));
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
        self.set_browsing_shown(true);

        let mut destinations = vec![Destination::Home];
        destinations.extend(sections.iter().cloned().map(Destination::Section));
        destinations.push(Destination::Downloads);
        self.install_places(&destinations);
        self.sidebar.show(destinations);

        let tweaks = snapshot.map(|snapshot| snapshot.tweaks.clone()).unwrap_or_default();
        self.holds_loads.set(tweaks.unanswered);
        match snapshot.map(|snapshot| &snapshot.subject) {
            Some(Subject::Library(index)) => match sections.get(*index) {
                Some(section) => self.go(Route::Library(section.clone(), tweaks.listing)),
                None => self.go(Route::Home),
            },
            Some(Subject::Item(key)) => {
                self.fetch(RatingKey::new(key.as_str()), |shell, item| shell.open(item));
            }
            Some(Subject::Play(key, keys)) => {
                let keys = keys.clone();
                self.fetch(RatingKey::new(key.as_str()), move |shell, item| {
                    shell.open_player(item, Purpose::Check);
                    shell.press_later(keys, SNAPSHOT_KEYS_AFTER);
                });
            }
            Some(Subject::Search(query)) => {
                self.toolbar.set_search_text(query);
                self.go(Route::Search(query.clone()));
            }
            Some(Subject::List(path)) => {
                self.go(Route::List { title: "List".to_owned(), path: path.clone() });
            }
            Some(Subject::Downloads) => self.go(Route::Downloads),
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
        menus::install(self.main_thread, &self.commands, &places, self.settings.get().low_power);
        *self.places.borrow_mut() = places;
    }

    fn session(&self) -> Option<Session> {
        self.session.borrow().clone()
    }

    // Signing in.

    fn show_sign_in(self: &Rc<Self>) {
        self.set_browsing_shown(false);
        let pressed = Self::command(&Rc::downgrade(self), Self::sign_in);
        self.show_notice(Notice::new(
            self.main_thread,
            Mark::Symbol("person.crop.circle"),
            "Sign in to Plex",
            "plex-rust opens plex.tv in your browser. Approve the sign-in there and come back.",
            Some(("Sign In", Box::new(pressed))),
        ));
    }

    /// Runs the sign-in: asks plex.tv for a link, opens it in the browser,
    /// and waits for the viewer to approve it there.
    fn sign_in(self: &Rc<Self>) {
        const FAILED: &str = "The sign-in did not go through";
        if let Some(notice) = &*self.notice.borrow() {
            notice.set_busy("Opening Your Browser…");
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
            let page = pin.approval_url(&client);
            account::open_in_browser(&page);
            shell.show_awaiting_approval(page);
            let waiting = client.clone();
            let wait = async move { plex::auth::wait_for_token(&waiting, &pin).await };
            let signed_in = Rc::clone(&shell);
            shell.background.run(shell.main_thread, wait, move |token| {
                let stored = token
                    .map_err(Error::from)
                    .and_then(|token| account::store_token(&token).map(|()| token));
                match stored {
                    Ok(token) => {
                        // The viewer is in the browser, with nothing more
                        // to do there.
                        #[allow(deprecated)]
                        NSApplication::sharedApplication(signed_in.main_thread)
                            .activateIgnoringOtherApps(true);
                        signed_in.connect(client, token, None);
                    }
                    Err(error) => signed_in.show_failure(FAILED, &error, Self::show_sign_in),
                }
            });
        });
    }

    /// What the window shows while the viewer is in the browser, at
    /// `page`. The button is for a page that was closed or never opened.
    fn show_awaiting_approval(&self, page: String) {
        self.show_notice(Notice::new(
            self.main_thread,
            Mark::Spinner,
            "Waiting for you to approve",
            "Approve the sign-in on the plex.tv page that opened in your browser. This window \
             carries on by itself once you have.",
            Some(("Open the Page Again", Box::new(move || account::open_in_browser(&page)))),
        ));
    }

    /// Shows or hides what browsing is done with: the sidebar and the
    /// toolbar. Without a library they lead nowhere, and the sign-in has
    /// the window to itself. They come back together with the first
    /// screen of the library, and in one step: a sidebar that slid in
    /// would push that screen sideways as it arrived.
    fn set_browsing_shown(&self, shown: bool) {
        self.sidebar_item.setCollapsed(!shown);
        self.toolbar.set_visible(shown);
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
            Destination::Section(section) => {
                Route::Library(section.clone(), self.listings.borrow().of(&section.key))
            }
            Destination::Downloads => Route::Downloads,
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
        let position = self.position.get();
        self.toolbar.set_history(position > 0, position + 1 < self.history.borrow().len());
        self.enable_commands();
        self.sidebar.select(destination_of(&route).as_ref());
        // A page left in the history has not heard what became of its
        // item's download since.
        if let Screen::Detail(detail) = &screen {
            detail.show_download();
        }
        // Results arrive while the viewer is still typing in the search
        // field, which must keep the keyboard.
        if !matches!(route, Route::Search(_)) {
            screen.focus();
        }
    }

    /// Greys out the menu's commands that have nothing to do where the
    /// viewer is. While the player has the window that is every one of these: the
    /// menu bar stays up, and what is behind the player is not there to be
    /// reloaded or marked as watched unseen.
    fn enable_commands(&self) {
        let browsing = self.playing.borrow().is_none();
        let position = self.position.get();
        // Only an item's page has something to play or to mark.
        let on_item = self.current(|entry| matches!(entry.screen, Screen::Detail(_))) == Some(true);
        let commands = &self.commands;
        commands.back.set_enabled(browsing && position > 0);
        commands.forward.set_enabled(browsing && position + 1 < self.history.borrow().len());
        commands.reload.set_enabled(browsing);
        commands.find.set_enabled(browsing);
        commands.play.set_enabled(browsing && on_item);
        commands.toggle_watched.set_enabled(browsing && on_item);
    }

    /// Makes `view` the content, optionally fading from what was there.
    fn show_view(&self, view: &NSView, fade: bool) {
        let fade = fade && !theme::reduces_motion();
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
        let session = self.session();
        let (screen, reload) = match (&route, &session) {
            // The one place that is there without a server.
            (Route::Downloads, _) => self.download_list(),
            (_, None) => return None,
            (Route::Home, Some(session)) => self.home(session),
            (Route::Library(section, listing), Some(session)) => {
                self.library(session, section, *listing)
            }
            (Route::List { title, path }, Some(session)) => self.list(session, &route, title, path),
            (Route::Item(item, opening), Some(session)) => self.item_page(session, item, opening),
            (Route::Search(query), Some(session)) => self.search_results(session, query),
        };
        if !self.holds_loads.get() {
            reload();
        }
        Some(Entry { route, screen, reload })
    }

    fn home(self: &Rc<Self>, session: &Session) -> (Screen, Rc<dyn Fn()>) {
        let outlines = vec![
            Outline { title: "Continue Watching", style: Style::Shelf(Shape::Landscape), count: 6 },
            Outline { title: "Recently Added", style: Style::Shelf(Shape::Poster), count: 10 },
        ];
        let nothing = self.nothing(
            "play.rectangle.on.rectangle",
            "Nothing to watch yet",
            "What you are in the middle of and what your server has just added will be here.",
        );
        let hero = Hero::new(self.main_thread, Rc::clone(&session.images), self.actions());
        let header = Header { view: hero.view().retain(), height: theme::HOME_HERO_HEIGHT };
        let posters = self.posters(session, Some(header), outlines, nothing, || {});
        posters.fit_header_to_page(theme::HOME_HERO_HEIGHT_PER_WIDTH, theme::HOME_HERO_MOST);
        let shown = posters.clone();
        let reload = self.loader(
            session,
            Unanswered::ShowsTheError,
            |server| async move { server.home(SHELF_LENGTH).await },
            // The hero lives as long as this closure, which the entry keeps.
            move |hubs: &Vec<Hub>| {
                let mut shelves = home_shelves(hubs);
                hero.show(hero::feature(&mut shelves));
                shown.show(shelves);
            },
        );
        (Screen::Posters(posters), reload)
    }

    fn library(
        self: &Rc<Self>,
        session: &Session,
        section: &Section,
        listing: Listing,
    ) -> (Screen, Rc<dyn Fn()>) {
        let (relisted, of) = (Rc::downgrade(self), section.clone());
        let noun = if section.kind == Kind::Show { "show" } else { "film" };
        let bar = LibraryBar::new(self.main_thread, listing, noun, move |listing| {
            if let Some(shell) = relisted.upgrade() {
                shell.relist(&of, listing);
            }
        });
        let header = Header { view: bar.view().retain(), height: theme::LIBRARY_BAR_HEIGHT };
        let nothing = if listing.unwatched_only {
            self.nothing(
                "checkmark.circle",
                "Nothing left to watch",
                &format!("You have watched every {noun} in this library."),
            )
        } else {
            self.nothing(
                "square.stack",
                "This library is empty",
                &format!("Once your server has a {noun} in it, it will be here."),
            )
        };
        let route = Route::Library(section.clone(), listing);
        let source = Paged::Library(section.clone(), listing);
        // The bar lives as long as this closure, which the entry keeps.
        self.grid(session, route, source, Some(header), "", nothing, move |total| {
            bar.set_total(total);
        })
    }

    /// Lists a library another way from now on, and shows it that way.
    fn relist(self: &Rc<Self>, section: &Section, listing: Listing) {
        self.listings.borrow_mut().set(&section.key, listing);
        if let Err(error) = self.listings.borrow().save() {
            eprintln!("plex-rust: {}", with_causes(&error));
        }
        self.replace(Route::Library(section.clone(), listing));
    }

    fn list(
        self: &Rc<Self>,
        session: &Session,
        route: &Route,
        title: &str,
        path: &str,
    ) -> (Screen, Rc<dyn Fn()>) {
        let source = Paged::Path(path.to_owned());
        let nothing = self.nothing("square.stack", "Nothing here", "This list is empty.");
        self.grid(session, route.clone(), source, None, title, nothing, |_| {})
    }

    /// A grid of posters under the heading `title` that shows `source` a
    /// page at a time, as the screen for `route`. `on_total` hears how
    /// many items the whole list has.
    #[allow(clippy::too_many_arguments)]
    fn grid(
        self: &Rc<Self>,
        session: &Session,
        route: Route,
        source: Paged,
        header: Option<Header>,
        title: &str,
        nothing: Notice,
        on_total: impl Fn(u32) + 'static,
    ) -> (Screen, Rc<dyn Fn()>) {
        let outlines = vec![Outline { title: "", style: Style::Grid(Shape::Poster), count: 24 }];
        let (more, more_of) = (Rc::downgrade(self), source.clone());
        let posters = self.posters(session, header, outlines, nothing, move || {
            if let Some(shell) = more.upgrade() {
                shell.load_page(&route, more_of.clone());
            }
        });

        let (shown, title) = (posters.clone(), title.to_owned());
        let reload = self.loader(
            session,
            Unanswered::ShowsTheError,
            move |server| source.clone().page(server, 0),
            move |page: &Page| {
                on_total(page.total);
                let style = Style::Grid(Shape::Poster);
                shown.show(vec![Shelf::of(title.clone(), style, page.items.clone())]);
            },
        );
        (Screen::Posters(posters), reload)
    }

    /// Fetches the next page of `source` and adds it to the grid, if the
    /// grid for `route` is still the one on screen.
    fn load_page(self: &Rc<Self>, route: &Route, source: Paged) {
        let on_screen = self.current(|entry| (entry.route == *route).then(|| entry.screen.clone()));
        let (Some(session), Some(Some(Screen::Posters(posters)))) = (self.session(), on_screen)
        else {
            return;
        };
        let start = posters.len();
        let load = source.page(session.server.clone(), u32::try_from(start).unwrap_or(u32::MAX));
        let (shell, moves) = (Rc::clone(self), self.moves.get());
        self.background.run(self.main_thread, load, move |page| match page {
            Ok(page) => posters.append(start, page.items),
            Err(error) => {
                posters.more_failed();
                shell.fail(moves, &error);
            }
        });
    }

    fn item_page(
        self: &Rc<Self>,
        session: &Session,
        item: &Item,
        opening: &Opening,
    ) -> (Screen, Rc<dyn Fn()>) {
        let detail = Detail::new(
            self.main_thread,
            &session.images,
            &self.downloads,
            item.kind,
            opening.clone(),
            self.actions(),
        );
        // What the list the viewer came from knew is enough to draw the
        // page at once. The full item follows.
        detail.show(item);

        let (shown, key) = (Rc::clone(&detail), item.rating_key.clone());
        let mut loads = vec![self.loader(
            session,
            Unanswered::ShowsTheError,
            move |server| {
                let key = key.clone();
                async move { server.item(&key).await }
            },
            move |item: &Item| shown.show(item),
        )];

        // What goes under the header. The page stands without any of it.
        let shown = Rc::clone(&detail);
        let show_episodes = move |episodes: &Vec<Item>| shown.show_episodes(episodes.clone());
        match (item.kind, item.parent_rating_key.clone()) {
            (Kind::Show, _) => {
                let show = item.rating_key.clone();
                let ask = move |server: MediaServer| {
                    let show = show.clone();
                    async move { server.episodes(&show).await }
                };
                loads.push(self.loader(session, Unanswered::GoesWithout, ask, show_episodes));
            }
            (Kind::Episode, Some(season)) => {
                let ask = move |server: MediaServer| {
                    let season = season.clone();
                    async move { server.children(&season).await }
                };
                loads.push(self.loader(session, Unanswered::GoesWithout, ask, show_episodes));
            }
            _ => {}
        }
        if matches!(item.kind, Kind::Movie | Kind::Show) {
            let (shown, key) = (Rc::clone(&detail), item.rating_key.clone());
            loads.push(self.loader(
                session,
                Unanswered::GoesWithout,
                move |server| {
                    let key = key.clone();
                    async move { server.related(&key, RELATED_LENGTH).await }
                },
                move |related: &Vec<Hub>| shown.show_related(related.clone()),
            ));
        }
        let reload = Rc::new(move || {
            for load in &loads {
                load();
            }
        });
        (Screen::Detail(detail), reload)
    }

    fn download_list(self: &Rc<Self>) -> (Screen, Rc<dyn Fn()>) {
        let shell = Rc::downgrade(self);
        let list = DownloadList::new(self.main_thread, move |act| {
            if let Some(shell) = shell.upgrade() {
                shell.act_on_download(act);
            }
        });
        let (shown, downloads) = (list.clone(), Rc::clone(&self.downloads));
        let reload = Rc::new(move || shown.show(&downloads.entries()));
        (Screen::Downloads(list), reload)
    }

    fn search_results(self: &Rc<Self>, session: &Session, query: &str) -> (Screen, Rc<dyn Fn()>) {
        let nothing = self.nothing(
            "magnifyingglass",
            "No results",
            "Check the spelling, or try fewer words.",
        );
        // The two kinds a search most often finds, in the server's order,
        // so that the wait for the first answer is not a blank pane.
        let outlines = vec![
            Outline { title: "Shows", style: Style::Shelf(Shape::Poster), count: 8 },
            Outline { title: "Movies", style: Style::Shelf(Shape::Poster), count: 8 },
        ];
        let posters = self.posters(session, None, outlines, nothing, || {});
        let reload = self.search_loader(session, &posters, query);
        (Screen::Posters(posters), reload)
    }

    /// What asks for the results of `query` and shows them in `posters`.
    fn search_loader(
        self: &Rc<Self>,
        session: &Session,
        posters: &Retained<Posters>,
        query: &str,
    ) -> Rc<dyn Fn()> {
        let (shown, query) = (posters.clone(), query.to_owned());
        self.loader(
            session,
            Unanswered::ShowsTheError,
            move |server| {
                let query = query.clone();
                async move { server.search(&query, SEARCH_LIMIT).await }
            },
            move |hubs: &Vec<Hub>| shown.show(search_shelves(hubs)),
        )
    }

    /// Searches for `query` as the viewer types it. Each new query takes
    /// the place of the last, so that Back leaves the search in one step.
    fn search(self: &Rc<Self>, query: &str) {
        let searching = self.current(|entry| matches!(entry.route, Route::Search(_))) == Some(true);
        match (query.is_empty(), searching) {
            (true, true) => self.back(),
            (true, false) => {}
            (false, true) => self.search_again(query),
            (false, false) => self.go(Route::Search(query.to_owned())),
        }
    }

    /// Asks the screen of results that is on screen for the results of
    /// `query` instead. The results it shows stay until the new ones are
    /// there, so typing does not blank the screen at every letter.
    fn search_again(self: &Rc<Self>, query: &str) {
        let screen = self.current(|entry| entry.screen.clone());
        let (Some(session), Some(Screen::Posters(posters))) = (self.session(), screen) else {
            return;
        };
        self.moves.set(self.moves.get() + 1);
        let reload = self.search_loader(&session, &posters, query);
        if let Some(entry) = self.history.borrow_mut().get_mut(self.position.get()) {
            entry.route = Route::Search(query.to_owned());
            entry.reload = Rc::clone(&reload);
        }
        reload();
    }

    fn focus_search(self: &Rc<Self>) {
        self.toolbar.focus_search();
    }

    /// Gives the keyboard to the screen, from the search field.
    fn focus_content(self: &Rc<Self>) {
        if let Some(screen) = self.current(|entry| entry.screen.clone()) {
            screen.focus();
        }
    }

    /// A new, empty collection that reports what the viewer does in it.
    fn posters(
        self: &Rc<Self>,
        session: &Session,
        header: Option<Header>,
        outlines: Vec<Outline>,
        nothing: Notice,
        on_near_end: impl Fn() + 'static,
    ) -> Retained<Posters> {
        Posters::new(
            self.main_thread,
            Rc::clone(&session.images),
            header,
            outlines,
            Some(nothing),
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
        unanswered: Unanswered,
        ask: impl Fn(MediaServer) -> F + 'static,
        show: impl Fn(&T) + 'static,
    ) -> Rc<dyn Fn()>
    where
        T: PartialEq + Send + 'static,
        F: Future<Output = plex::Result<T>> + Send + 'static,
    {
        // The answer on screen, from whichever run of the closure.
        let shown: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
        let show = Rc::new(show);
        let (shell, server) = (Rc::downgrade(self), session.server.clone());
        Rc::new(move || {
            let Some(shell) = shell.upgrade() else {
                return;
            };
            // Set once the server has answered, so that a late answer from
            // last time cannot replace its answer.
            let answered = Rc::new(Cell::new(false));
            if shown.borrow().is_none() {
                let (shown, show, answered) =
                    (Rc::clone(&shown), Rc::clone(&show), Rc::clone(&answered));
                let remembered = ask(server.remembered());
                shell.background.run(shell.main_thread, remembered, move |answer| {
                    if let (Ok(answer), false) = (answer, answered.get()) {
                        show(&answer);
                        *shown.borrow_mut() = Some(answer);
                    }
                });
            }

            let (shown, show) = (Rc::clone(&shown), Rc::clone(&show));
            let (failed, moves) = (Rc::clone(&shell), shell.moves.get());
            let fresh = ask(server.clone());
            shell.background.run(shell.main_thread, fresh, move |answer| {
                answered.set(true);
                let has_shown = shown.borrow().is_some();
                match answer {
                    Ok(answer) if shown.borrow().as_ref() == Some(&answer) => {}
                    Ok(answer) => {
                        show(&answer);
                        *shown.borrow_mut() = Some(answer);
                    }
                    // What is on screen from last time is better than an
                    // error, but for the one that says the sign-in is over.
                    Err(error)
                        if (has_shown || unanswered == Unanswered::GoesWithout)
                            && !error.is_unauthorized() =>
                    {
                        eprintln!("plex-rust: {}", with_causes(&error));
                    }
                    Err(error) => failed.fail(moves, &error),
                }
            });
        })
    }

    // What the viewer does with an item.

    fn actions(self: &Rc<Self>) -> impl Fn(Action) + 'static {
        let shell = Rc::downgrade(self);
        move |action| {
            let Some(shell) = shell.upgrade() else {
                return;
            };
            match action {
                Action::Open(item) => shell.open(&item),
                Action::Play(item) => shell.play(&item, Purpose::Watching),
                Action::SetWatched(item, watched) => shell.set_watched(&item, watched),
                Action::ToggleDownload(item) => shell.toggle_download(&item.rating_key),
                Action::OpenShow(episode) => {
                    let season = episode.parent_rating_key.clone();
                    shell.open_show_of(&episode, Opening { season, ..Opening::default() });
                }
                Action::OpenSeason(episode) => {
                    let opening = Opening {
                        season: episode.parent_rating_key.clone(),
                        at_episodes: true,
                        episode: Some(episode.rating_key.clone()),
                    };
                    shell.open_show_of(&episode, opening);
                }
                Action::OpenList { title, path } => shell.go(Route::List { title, path }),
                // Only a detail page lists people, and it turns this into
                // the list of what the person is in before it gets here.
                Action::OpenPerson(_) => {}
            }
        }
    }

    /// Goes to the page of `item`. A season's page is its show's page,
    /// scrolled down to that season's episodes. A trailer has no page and
    /// plays.
    fn open(self: &Rc<Self>, item: &Item) {
        match (item.kind, &item.parent_rating_key) {
            (Kind::Clip, _) => self.play(item, Purpose::Watching),
            (Kind::Season, Some(show)) => {
                let season = Some(item.rating_key.clone());
                let opening = Opening { season, at_episodes: true, episode: None };
                self.open_show(show.clone(), opening);
            }
            _ => self.go(Route::Item(Box::new(item.clone()), Opening::default())),
        }
    }

    /// Goes from an episode to its show, opened as `opening` says.
    fn open_show_of(self: &Rc<Self>, episode: &Item, opening: Opening) {
        if let Some(show) = episode.grandparent_rating_key.clone() {
            self.open_show(show, opening);
        }
    }

    fn open_show(self: &Rc<Self>, show: RatingKey, opening: Opening) {
        self.fetch(show, move |shell, show| {
            shell.go(Route::Item(Box::new(show.clone()), opening));
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

    // Downloads.

    /// Does what the download button of an item's page does: downloads the
    /// item, carries on with a download of it that stopped short, or
    /// deletes one that is whole or on its way.
    fn toggle_download(self: &Rc<Self>, key: &RatingKey) {
        match self.downloads.find(key).map(|entry| entry.state) {
            None => self.download(key),
            Some(State::Incomplete { .. }) => self.act_on_download(Act::Resume(key.clone())),
            Some(State::Downloading(_) | State::Downloaded) => self.downloads.remove(key),
        }
    }

    /// Starts downloading the item with `key`. It is asked for in full
    /// first, because that is what is kept next to its file.
    fn download(self: &Rc<Self>, key: &RatingKey) {
        let Some(session) = self.session() else {
            return;
        };
        let (server, key) = (session.server.clone(), key.clone());
        let asked = key.clone();
        let ask = async move { server.item_json(&asked).await };
        let (shell, moves) = (Rc::clone(self), self.moves.get());
        self.background.run(self.main_thread, ask, move |item| {
            // A second press while the first was still asking.
            if shell.downloads.find(&key).is_some() {
                return;
            }
            let added = item
                .map_err(Error::from)
                .and_then(|item| shell.downloads.add(&session.server, &item));
            if let Err(error) = added {
                shell.fail(moves, &error);
            }
        });
    }

    fn act_on_download(self: &Rc<Self>, act: Act) {
        match act {
            Act::Play(key) => {
                if let Some(entry) = self.downloads.find(&key) {
                    self.play(&entry.download.item, Purpose::Watching);
                }
            }
            Act::Resume(key) => {
                if let Some(session) = self.session() {
                    self.downloads.start(&session.server, &key);
                }
            }
            Act::Remove(key) => self.downloads.remove(&key),
        }
    }

    /// Brings the screen up to date with what the downloads have done.
    fn downloads_changed(self: &Rc<Self>) {
        match self.current(|entry| entry.screen.clone()) {
            Some(Screen::Downloads(_)) => self.refresh(),
            Some(Screen::Detail(detail)) => detail.show_download(),
            Some(Screen::Posters(_)) | None => {}
        }
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
        let (closed, opened_with) = (Rc::downgrade(self), item.rating_key.clone());
        let screen = PlayerScreen::open(
            self.main_thread,
            Rc::clone(&self.background),
            session.server.clone(),
            &session.images,
            item,
            self.settings.get(),
            purpose,
            move |outcome| {
                if let Some(shell) = closed.upgrade() {
                    shell.player_closed(outcome, purpose, &opened_with);
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
                self.enable_commands();
            }
            Err(error) => self.fail(self.moves.get(), &error),
        }
    }

    /// Puts the browsing screens back in the window. `opened_with` is the
    /// item the player started on, which an episode may have moved on from.
    fn player_closed(self: &Rc<Self>, outcome: Outcome, purpose: Purpose, opened_with: &RatingKey) {
        let Some(screen) = self.playing.borrow_mut().take() else {
            return;
        };
        self.keep_settings(&screen);
        self.set_window_content(&self.split);
        self.set_toolbar_visible(true);
        self.enable_commands();

        let played = screen.item();
        let behind = self.current(|entry| (entry.route.clone(), entry.screen.clone()));
        match behind {
            // The page of the episode the player started on would be a
            // step behind the viewer, who went on to another in the player.
            Some((Route::Item(page, _), _))
                if page.rating_key == *opened_with && played.rating_key != *opened_with =>
            {
                self.replace(Route::Item(Box::new((*played).clone()), Opening::default()));
            }
            Some((_, behind)) => {
                // The keyboard was the player's, and the window does not
                // hand it back to the screen that had it before.
                behind.focus();
                // That screen still says where the item was left before it
                // was played.
                if purpose == Purpose::Watching {
                    self.refresh();
                }
            }
            None => {}
        }
        if let Outcome::Failed(reason) = outcome {
            let dismiss = Self::command(&Rc::downgrade(self), |shell| shell.display(true));
            self.show_notice(Notice::new(
                self.main_thread,
                Mark::Symbol("exclamationmark.triangle"),
                &format!("{} could not be played", screen.item().title),
                &reason,
                Some(("Go Back", Box::new(dismiss))),
            ));
        }
    }

    /// The toolbar belongs to browsing. The player has its own controls.
    fn set_toolbar_visible(&self, visible: bool) {
        self.toolbar.set_visible(visible);
    }

    /// Takes over the speed and volume the viewer set in `screen`, for
    /// whatever is played next and for the next launch.
    fn keep_settings(&self, screen: &PlayerScreen) {
        match screen.settings() {
            Ok(Some(settings)) => self.save_settings(settings),
            Ok(None) => {}
            Err(error) => eprintln!("plex-rust: {error}"),
        }
    }

    fn save_settings(&self, settings: Settings) {
        self.settings.set(settings);
        if let Err(error) = settings.save() {
            eprintln!("plex-rust: {}", with_causes(&error));
        }
    }

    /// Turns low power on if it is off and off if it is on, for what is
    /// playing now and everything played after it. `entry` is the menu
    /// item that asked for it, which shows the new state.
    fn toggle_low_power(&self, entry: Option<&AnyObject>) {
        let low_power = !self.settings.get().low_power;
        self.save_settings(Settings { low_power, ..self.settings.get() });
        if let Some(screen) = self.playing.borrow().as_ref() {
            screen.set_low_power(low_power);
        }
        if let Some(entry) = entry.and_then(|entry| entry.downcast_ref::<NSMenuItem>()) {
            menus::check(entry, low_power);
        }
    }

    /// Presses `keys` in the player once `delay` has passed: at first the
    /// time the player needs to open its file, then each wait among the
    /// keys.
    fn press_later(self: &Rc<Self>, keys: Vec<Key>, delay: Duration) {
        // A timer can only be created on the runtime's own threads.
        let wait = async move { tokio::time::sleep(delay).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, wait, move |()| {
            // Cloned so that a key that closes the player finds no borrow.
            let Some(screen) = shell.playing.borrow().clone() else {
                return;
            };
            let mut keys = keys.into_iter();
            for key in keys.by_ref() {
                if key == Key::Wait {
                    return shell.press_later(keys.collect(), SNAPSHOT_KEY_WAIT);
                }
                screen.press(key);
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

    /// What a list says in its middle when it turns out to hold nothing.
    fn nothing(&self, symbol: &str, title: &str, detail: &str) -> Notice {
        Notice::new(self.main_thread, Mark::Symbol(symbol), title, detail, None)
    }

    fn show_notice(&self, notice: Notice) {
        layout::centre(&self.content, notice.view());
        notice.announce(&self.window);
        *self.notice.borrow_mut() = Some(notice);
    }

    /// Shows `error` in place of the screen, in words for the viewer, with
    /// a button for what to do about it: sign in again when that is what
    /// it takes, else `retry`. `title` says what failed when the error is
    /// of no kind that says it better.
    fn show_failure(
        self: &Rc<Self>,
        title: &str,
        error: &(dyn std::error::Error + 'static),
        retry: fn(&Rc<Self>),
    ) {
        self.show_failure_of(Failure::of(error), title, &with_causes(error), retry);
    }

    fn show_failure_of(
        self: &Rc<Self>,
        failure: Failure,
        title: &str,
        small_print: &str,
        retry: fn(&Rc<Self>),
    ) {
        let (label, act) = match failure {
            Failure::SignedOut => ("Sign In", Self::sign_in as fn(&Rc<Self>)),
            _ => ("Try Again", retry),
        };
        // Being asked to sign in again is the sign-in screen once more.
        if failure == Failure::SignedOut {
            self.set_browsing_shown(false);
        }
        let act = Self::command(&Rc::downgrade(self), act);
        let notice = Notice::new(
            self.main_thread,
            Mark::Symbol(failure.symbol()),
            failure.title().unwrap_or(title),
            failure.advice(),
            Some((label, Box::new(act))),
        );
        notice.add_small_print(small_print, self.main_thread);
        self.show_notice(notice);
    }

    /// Shows a failure to load the current place, unless the viewer has
    /// moved on since move number `moves`, when it is only logged: an
    /// error about a screen they left would replace the one they are on.
    fn fail(self: &Rc<Self>, moves: u64, error: &(dyn std::error::Error + 'static)) {
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
        // The picture is taken once the keys pressed in a player are done.
        let waits = match &snapshot.subject {
            Subject::Play(_, keys) => keys.iter().filter(|key| **key == Key::Wait).count(),
            _ => 0,
        };
        let settle = SNAPSHOT_SETTLE + SNAPSHOT_KEY_WAIT * u32::try_from(waits).unwrap_or(u32::MAX);
        // A timer can only be created on the runtime's own threads.
        let settle = async move { tokio::time::sleep(settle).await };
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
                if let (true, Screen::Posters(posters)) = (snapshot.tweaks.across > 0.0, screen) {
                    posters.scroll_first_shelf_to(snapshot.tweaks.across);
                }
                if let Screen::Posters(posters) = screen {
                    snapshot.tweaks.presses.iter().for_each(|press| posters.press(*press));
                }
                if let (Some(link), Screen::Detail(detail)) = (snapshot.tweaks.follow, screen) {
                    detail.follow_link(link);
                }
                if let (true, Screen::Detail(detail)) = (snapshot.tweaks.download, screen) {
                    detail.press_download();
                }
            }
            if let Some(failure) = snapshot.tweaks.fault {
                let small_print = "error sending request for url (https://192.168.0.2:32400/hubs)";
                shell.show_failure_of(failure, NOT_LOADED, small_print, Self::reload);
            }
            if snapshot.tweaks.looking {
                shell.show_looking();
            }
            if snapshot.tweaks.approving {
                shell.show_awaiting_approval("https://app.plex.tv/auth".to_owned());
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
        Route::List { title, .. } => title,
        Route::Item(item, _) => &item.title,
        Route::Search(_) => "Search",
        Route::Downloads => "Downloads",
    }
}

/// The sidebar row that stands for `route`, if one does.
fn destination_of(route: &Route) -> Option<Destination> {
    match route {
        Route::Home => Some(Destination::Home),
        Route::Library(section, _) => Some(Destination::Section(section.clone())),
        Route::Downloads => Some(Destination::Downloads),
        Route::List { .. } | Route::Item(..) | Route::Search(_) => None,
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
            let mut shelf = Shelf::of(hub.title.clone(), Style::Shelf(shape), hub.items.clone());
            // The heading leads to all of what the shelf starts.
            shelf.more = hub.more.clone();
            shelf
        })
        .collect()
}

/// The shelves of a search: one per kind of result, episodes as frames.
fn search_shelves(hubs: &[Hub]) -> Vec<Shelf> {
    hubs.iter()
        .map(|hub| {
            let episodes = hub.items.first().is_some_and(|item| item.kind == Kind::Episode);
            let shape = if episodes { Shape::Landscape } else { Shape::Poster };
            Shelf::of(hub.title.clone(), Style::Shelf(shape), hub.items.clone())
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
    // One window is all there is. Left to itself the system offers a tab
    // bar and "Merge All Windows" for it in the menus.
    window.setTabbingMode(NSWindowTabbingMode::Disallowed);
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

        #[unsafe(method_id(applicationDockMenu:))]
        fn dock_menu(&self, _app: &NSApplication) -> Option<Retained<NSMenu>> {
            let shell = self.ivars();
            Some(menus::dock(shell.main_thread, &shell.places.borrow()))
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
            more: None,
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
