//! The main window: sidebar on the left, the chosen place on the right,
//! and the way from one place to the next.

mod walk;

use std::cell::{Cell, OnceCell, RefCell};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
use plex::library::{FirstCharacter, Listing, Page, Sort};
use plex::model::{Colors, Hub, Item, Kind, RatingKey, Section};
use plex::{Cache, Client, MediaServer, Token};

pub use self::walk::Step;
use super::callback::Callback;
use super::confirm;
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
use super::player::{PlayerScreen, Purpose};
use super::poster::Shape;
pub use super::posters::Press;
use super::posters::{Action, Header, Outline, Playback, Posters, Shelf, Style};
use super::sidebar::{Destination, Sidebar};
use super::snapshot;
use super::stale::{Reason, Stale};
use super::theme;
use super::toast::Toast;
use super::toolbar::Toolbar;
use crate::account;
use crate::background::{Background, Task};
use crate::downloads::{self, Downloads, State};
use crate::error::{Error, Result, with_causes};
use crate::images::{self, Images};
use crate::reach::{Reach, Word};
use crate::settings::{Listings, Place, Settings};
use crate::store::Download;
use crate::timing;

/// How many items each shelf of the home screen starts with.
const SHELF_LENGTH: u32 = 20;

/// How many items of a library are fetched at a time.
const PAGE_SIZE: u32 = 100;

/// How many items each row related to a film or show starts with.
const RELATED_LENGTH: u32 = 20;

/// How long the pointer or the keyboard stays on a card before what its
/// page shows is asked for ahead of the click. Shorter than the time a
/// hand takes to stop and press, longer than it takes to pass over a card
/// on the way to another.
const REST_BEFORE_ASKING_AHEAD: Duration = Duration::from_millis(120);

/// How old what is on screen may be when the viewer comes back to the app
/// without it being asked about again. Coming back after a film watched
/// elsewhere should show where that film was left. Switching windows
/// back and forth should not ask every time.
const FRESH_FOR: Duration = Duration::from_secs(30);

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

/// How many bytes of the server's answers are kept between launches. A
/// screen's worth is some tens of kilobytes.
const RESPONSES_LIMIT: u64 = 64 * 1024 * 1024;

/// How long the line that says a change did not reach the server stays.
const TOAST_STAYS: Duration = Duration::from_secs(8);

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
    /// What the place on screen has asked the server and may not have
    /// heard back about. None of it is wanted once the viewer moves on.
    asked: RefCell<Vec<Task>>,
    /// When the place on screen last asked the server about itself.
    asked_at: Cell<Instant>,
    /// The asking ahead for the card the pointer or keyboard is on.
    asked_ahead: RefCell<Option<Task>>,
    /// A message shown in place of the current entry, such as an error.
    notice: RefCell<Option<Notice>>,
    /// Says, over the current entry, that it shows what was saved and not
    /// what the server has now.
    stale: Stale,
    /// Says, over the current entry, that a change the viewer asked for
    /// did not reach the server, and counts how often it has, so that an
    /// earlier one's time running out does not hide a later one.
    toast: Toast,
    toasts: Cell<u64>,
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
    /// Whether the place on screen is kept for the next launch, and the
    /// last launch's is started from. Not so for a snapshot, which goes
    /// where it is told and must leave no trace.
    keeps_place: Cell<bool>,
    /// The place on screen, for the writes that keep it.
    place: Arc<Mutex<Option<Place>>>,
    /// What Home held at the last launch, read along with the libraries
    /// so that the first Home of this one is built with it.
    saved_home: RefCell<Option<Vec<Hub>>>,
    /// Set for a snapshot, which must leave the server as it found it.
    read_only: Cell<bool>,
    /// What is kept on this Mac, and what is on its way to it.
    downloads: Rc<Downloads>,
    /// Whether the server is there, by what every request of it came to.
    /// Only [`hear`](Self::hear) changes it.
    reach: Cell<Reach>,
}

/// What exists only while a server is known.
#[derive(Clone)]
struct Session {
    server: MediaServer,
    images: Rc<Images>,
    sections: Vec<Section>,
}

/// A server with the places its answers and its artwork are kept between
/// launches: apart from those of any other server, and from those of any
/// other account on this one.
#[derive(Clone)]
struct Kept {
    /// Keeps its answers itself.
    server: MediaServer,
    /// The identifier plex.tv has for the account in use.
    account: String,
    artwork: Cache,
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
    /// The `size` items from index `start` on.
    async fn page(self, server: MediaServer, start: u32, size: u32) -> plex::Result<Page> {
        match self {
            Self::Library(section, listing) => {
                server.section_items(&section, listing, start, size).await
            }
            Self::Path(path) => server.list(&path, start, size).await,
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
    /// The screen was drawn from what the place the viewer came from
    /// knew, and stands as that, marked as saved. Unless the server says
    /// that what it shows is gone and nothing of it is `kept_here`, as a
    /// download: then there is nothing left to do on it.
    StandsAsDrawn { kept_here: bool },
}

/// Who is asked for an item that something waits for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Asked {
    /// The server, for what is true now.
    Server,
    /// What was saved from the server's last answer, which is there at
    /// once, and the server only if nothing was.
    SavedFirst,
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
    /// Home, and from there these steps, with the time of each printed.
    Walk(Vec<Step>),
}

/// Changes to the state a snapshot is taken in.
#[derive(Clone, Default)]
pub struct Tweaks {
    /// The window at its smallest size.
    pub narrow: bool,
    /// The window as large as a big display lets it be.
    pub wide: bool,
    /// The sidebar put away.
    pub collapsed: bool,
    /// Accessibility settings to take the picture under.
    pub asked: snapshot::Asked,
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
    /// The first shelf paged on from the arrow in its heading.
    pub page: bool,
    /// In a library listed by title, the entry of this number in its
    /// index of first characters pressed, counting from zero.
    pub letter: Option<usize>,
    /// For a library, which items and in what order.
    pub listing: Listing,
    /// The screen having failed to load in this way.
    pub fault: Option<Failure>,
    /// The screen as it is before the server or the cache has answered.
    pub unanswered: bool,
    /// The first launch after signing in, before any server is known.
    pub looking: bool,
    /// The server not answering while what was saved is on screen.
    pub offline: bool,
    /// The sign-in screen once the browser has been opened.
    pub approving: bool,
    /// On the page of a film or an episode, the download button pressed.
    pub download: bool,
    /// On that page, the version whose media has this id picked first.
    pub version: Option<u64>,
    /// The server not answering, whether or not it would.
    pub away: bool,
    /// A change the viewer made on the page having failed to reach the
    /// server.
    pub refused: bool,
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
/// With `snapshot`, the window is where no display shows it: once the
/// subject has loaded, the window is saved to the path as a PNG and the
/// app exits.
pub fn run(snapshot: Option<Snapshot>) -> Result<()> {
    let main_thread =
        MainThreadMarker::new().unwrap_or_else(|| panic!("main() runs on the main thread"));
    let background = Rc::new(Background::new()?);

    if let Some(snapshot) = &snapshot {
        snapshot::pretend(snapshot.tweaks.asked);
    }
    let app = NSApplication::sharedApplication(main_thread);
    // A snapshot is not an app to switch to: it gets no Dock icon.
    app.setActivationPolicy(match snapshot {
        Some(_) => NSApplicationActivationPolicy::Accessory,
        None => NSApplicationActivationPolicy::Regular,
    });
    // A film library looks right on dark, whatever the system is set to.
    // SAFETY: the name is a constant that AppKit provides.
    let dark = unsafe { NSAppearanceNameDarkAqua };
    app.setAppearance(NSAppearance::appearanceNamed(dark).as_deref());

    let downloads = Downloads::open(main_thread, Rc::clone(&background))?;
    let (settings, listings) = (Settings::load()?, Listings::load()?);
    let shell = Shell::new(
        main_thread,
        Rc::clone(&background),
        settings,
        listings,
        downloads,
        snapshot.is_some(),
    );
    timing::mark("window built");
    match &snapshot {
        Some(snapshot) if snapshot.tweaks.narrow => {
            shell.window.setContentSize(theme::WINDOW_MIN_SIZE);
        }
        Some(snapshot) if snapshot.tweaks.wide => {
            shell.window.setContentSize(theme::WINDOW_WIDE_SIZE);
        }
        _ => {}
    }
    match &snapshot {
        Some(_) => snapshot::stage(&shell.window, main_thread)?,
        None => {
            // Opens where and as large as it was last closed.
            shell.window.setFrameAutosaveName(ns_string!("main"));
            shell.window.makeKeyAndOrderFront(None);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
    }

    // A walk stands in for a viewer, whose place is kept.
    let walks = matches!(&snapshot, Some(Snapshot { subject: Subject::Walk(_), .. }));
    shell.keeps_place.set(snapshot.is_none() || walks);

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
        staged: bool,
    ) -> Rc<Self> {
        let content = NSView::new(main_thread);
        // The fade from one screen to the next is drawn by its layer.
        content.setWantsLayer(true);
        // The content and, over it, the line that says it is out of date,
        // which stays while one screen gives way to the next.
        let pane = NSView::new(main_thread);
        layout::fill(&pane, &content);
        let content_controller = view_controller(main_thread);
        content_controller.setView(&pane);
        let window = window(main_thread, staged);
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
            let content_item =
                NSSplitViewItem::splitViewItemWithViewController(&content_controller);
            // The sidebar is glass laid over the content, so that a page's
            // colours run under it to the window's edge. What a page
            // scrolls stays clear of it by the safe area.
            content_item.setAutomaticallyAdjustsSafeAreaInsets(true);
            split.addSplitViewItem(&content_item);
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
            let stale = Stale::new(main_thread, Self::command(shell, Self::retry));
            stale.float_in(&pane);
            let toast = Toast::new(main_thread);
            toast.float_in(&pane);

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
                asked: RefCell::new(Vec::new()),
                asked_at: Cell::new(Instant::now()),
                asked_ahead: RefCell::new(None),
                notice: RefCell::new(None),
                stale,
                toast,
                toasts: Cell::new(0),
                split,
                playing: RefCell::new(None),
                opening_player: Cell::new(false),
                settings: Cell::new(settings),
                listings: RefCell::new(listings),
                holds_loads: Cell::new(false),
                downloads,
                reach: Cell::new(Reach::Unknown),
                read_only: Cell::new(false),
                keeps_place: Cell::new(false),
                place: Arc::default(),
                saved_home: RefCell::new(None),
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
        let saved = match saved_server(&client) {
            Ok(saved) => saved,
            Err(error) => return self.show_failure(UNREACHABLE, &error, Self::restart),
        };
        let subject = snapshot.clone();
        let place = self.last_place();

        // First, what the last launch left: the same server and the
        // libraries it had then. This puts the sidebar and the place the
        // viewer was at on screen at once.
        if let Some(kept) = saved.clone() {
            let (shell, subject, place) = (Rc::clone(self), subject.clone(), place.clone());
            let remembered = kept.server.remembered();
            // Home is read in the same go. Asked for only once its screen
            // is built, it arrives after the screen has been drawn empty.
            let ask = async move {
                let sections = remembered.sections().await;
                (sections, remembered.home(SHELF_LENGTH).await.ok())
            };
            self.background.run(self.main_thread, ask, move |(sections, home)| {
                if let (Ok(sections), None) = (sections, shell.session()) {
                    *shell.saved_home.borrow_mut() = home;
                    shell.connected(kept, sections, subject.as_ref());
                    // The server can fail to answer before the disk has.
                    // Then what was downloaded comes before the place the
                    // viewer was at, which cannot be brought up to date.
                    if shell.reach.get().is_away() {
                        shell.fall_back_on_downloads();
                    } else {
                        shell.resume(place);
                    }
                }
            });
        } else {
            self.show_looking();
        }

        // Then the truth. The saved server is asked directly, which is one
        // request. Only if it does not answer, say on another network, is
        // plex.tv asked where the server is now.
        let away = snapshot.as_ref().is_some_and(|snapshot| snapshot.tweaks.away);
        let find = async move {
            if away {
                let (server, tried) = ("The server".to_owned(), Vec::new());
                return Err(plex::Error::Unreachable { server, tried }.into());
            }
            if let Some(kept) = saved
                && let Ok(Ok(sections)) =
                    tokio::time::timeout(SAVED_SERVER_PATIENCE, kept.server.sections()).await
            {
                return Ok((kept, sections));
            }
            let servers = plex::discovery::servers(&client, &token).await?;
            let server = servers.first().ok_or(Error::NoServer)?;
            let account = plex::auth::account(&client, &token).await?;
            let kept = keeping(MediaServer::connect(&client, server, None).await?, account)?;
            let sections = kept.server.sections().await?;
            Ok::<_, Error>((kept, sections))
        };
        let (shell, asked) = (Rc::clone(self), Instant::now());
        self.background.run(self.main_thread, find, move |found| {
            match found {
                Ok((kept, sections)) => {
                    if let Err(error) = account::save_server(&kept.server, &kept.account) {
                        eprintln!("plex-rust: {}", with_causes(&error));
                    }
                    // Showing the same server and libraries again would
                    // reload the screen the viewer is already looking at.
                    // Unless that is a notice, as after signing in again.
                    let known = shell.notice.borrow().is_none()
                        && shell.session().is_some_and(|session| {
                            session.server.address() == kept.server.address()
                                && session.sections == sections
                        });
                    if !known {
                        // The same server at another address, as on another
                        // network, is no reason to send the viewer Home.
                        let same = shell.session().is_some_and(|session| {
                            session.server.machine_identifier() == kept.server.machine_identifier()
                        });
                        let was = shell.current(|entry| entry.route.clone()).filter(|_| same);
                        let first = shell.session().is_none();
                        shell.connected(kept, sections, subject.as_ref());
                        if let Some(route) = was.filter(|route| *route != Route::Home) {
                            shell.go(route);
                        } else if first {
                            shell.resume(place);
                        }
                    }
                    // Last, so that what follows from the server being
                    // there is done with the server that was found.
                    shell.hear(Word::Answer { heard: Instant::now() });
                }
                Err(error) => shell.server_away(asked, &error),
            }
            if let Some(snapshot) = snapshot {
                shell.snapshot_later(snapshot);
            }
        });
    }

    /// Takes in what a request of the server made at `asked` came to.
    fn heard<T>(self: &Rc<Self>, asked: Instant, answer: &plex::Result<T>) {
        if let Some(word) = Word::of(answer.as_ref().err(), asked, Instant::now()) {
            self.hear(word);
        }
    }

    /// Moves what is known of the server by `word`, and does what a
    /// change of that asks for. This is the one place it changes, so the
    /// line that says the server is away, the strip on Downloads and what
    /// plays a download all go by the same state.
    fn hear(self: &Rc<Self>, word: Word) {
        let was = self.reach.get();
        let now = was.after(word);
        self.reach.set(now);
        if (was.is_answering(), was.is_away()) == (now.is_answering(), now.is_away()) {
            return;
        }
        self.downloads_changed();
        if now.is_answering() {
            self.send_unsent();
            // What is on screen was saved before the server went. The
            // answer that brought it back was to one question of maybe
            // several the screen has.
            if self.stale.is_shown() && self.playing.borrow().is_none() {
                self.refresh();
            }
        }
    }

    /// Takes note that the server was looked for from `asked` on and not
    /// found.
    ///
    /// What was downloaded plays without it, so a viewer who has gone
    /// nowhere yet is taken to Downloads. Failing that, what the last
    /// launch left is on screen and better than an error. The error is
    /// shown when nothing is on screen, and when it is that the sign-in
    /// is over: then nothing on the server can be played or brought up to
    /// date until the viewer signs in again.
    fn server_away(self: &Rc<Self>, asked: Instant, error: &Error) {
        self.hear(Word::Silence { asked });
        // The player has the window. What is behind it is seen to when
        // it closes, by the screen there asking again.
        if self.playing.borrow().is_some() {
            return eprintln!("plex-rust: {}", with_causes(error));
        }
        if Failure::of(error) == Failure::SignedOut {
            self.show_failure(UNREACHABLE, error, Self::restart);
        } else if self.fall_back_on_downloads() {
            eprintln!("plex-rust: {}", with_causes(error));
        } else if self.session().is_some() {
            eprintln!("plex-rust: {}", with_causes(error));
            self.mark_stale(error);
        } else {
            self.show_failure(UNREACHABLE, error, Self::restart);
        }
    }

    /// Puts Downloads on screen, with the sidebar it is a place of, if
    /// there are any and the viewer has gone nowhere yet. Returns whether
    /// it did. With no server known, Downloads is the only place there is.
    fn fall_back_on_downloads(self: &Rc<Self>) -> bool {
        let has_moved = self.history.borrow().len() > 1;
        if self.downloads.entries().is_empty() || has_moved {
            return false;
        }
        if self.session().is_none() {
            self.list_places(None);
        }
        self.set_browsing_shown(true);
        // A notice may be standing in front of it, as while trying again.
        match self.current(|entry| entry.route == Route::Downloads) {
            Some(true) => self.display(false),
            _ => self.go(Route::Downloads),
        }
        true
    }

    /// Tells the server where each viewing it could not hear of stopped.
    fn send_unsent(self: &Rc<Self>) {
        let Some(session) = self.session() else {
            return;
        };
        let store = self.downloads.store().clone();
        let send = async move { downloads::send_unsent(&session.server, &store).await };
        self.background.run(self.main_thread, send, |sent| {
            if let Err(error) = sent {
                eprintln!("plex-rust: progress was not sent: {}", with_causes(&error));
            }
        });
    }

    /// Where the viewer was when the app was last used, if this launch is
    /// to start there.
    fn last_place(&self) -> Option<Place> {
        if !self.keeps_place.get() {
            return None;
        }
        // A file that cannot be read costs the place, not the launch.
        Place::load().unwrap_or_else(|error| {
            eprintln!("plex-rust: {}", with_causes(&error));
            None
        })
    }

    /// Goes from Home, where every launch starts, on to `place`. Home
    /// stays one step back.
    fn resume(self: &Rc<Self>, place: Option<Place>) {
        let Some(session) = self.session() else {
            return;
        };
        match place {
            Some(Place::Library { key }) => {
                // A library that is no longer there leaves the viewer Home.
                if let Some(section) = session.sections.iter().find(|section| section.key == key) {
                    self.go_to(&Destination::Section(section.clone()));
                }
            }
            Some(Place::List { title, path }) => self.go(Route::List { title, path }),
            Some(Place::Downloads) => self.go(Route::Downloads),
            Some(Place::Item { key }) => {
                // From what was saved of it, which is there at once. An
                // item that cannot be had, deleted since or another
                // account's, leaves the viewer Home with nothing said.
                let server = session.server.clone();
                let load = async move {
                    let key = RatingKey::new(key);
                    match server.remembered().item(&key).await {
                        Ok(saved) => Ok(saved),
                        Err(_) => server.item(&key).await,
                    }
                };
                let (shell, moves) = (Rc::clone(self), self.moves.get());
                self.background.run(self.main_thread, load, move |item| match item {
                    Ok(item) if shell.moves.get() == moves => {
                        shell.go(Route::Item(Box::new(item), Opening::default()));
                    }
                    Ok(_) => {}
                    Err(error) => eprintln!("plex-rust: {}", with_causes(&error)),
                });
            }
            Some(Place::Home) | None => {}
        }
    }

    /// Keeps `route` as the place to start from next time.
    fn keep_place(&self, route: &Route) {
        let place = match route {
            Route::Home => Place::Home,
            Route::Library(section, _) => Place::Library { key: section.key.clone() },
            Route::List { title, path } => Place::List { title: title.clone(), path: path.clone() },
            Route::Item(item, _) => Place::Item { key: item.rating_key.as_str().to_owned() },
            Route::Downloads => Place::Downloads,
            Route::Search(_) => return,
        };
        if !self.keeps_place.get() {
            return;
        }
        // The file is written off the main thread, and what is written is
        // the place at that moment: of two moves in quick succession the
        // second is then the one kept, whichever write runs last.
        let latest = Arc::clone(&self.place);
        if let Ok(mut latest) = latest.lock() {
            *latest = Some(place);
        }
        let save = async move {
            match latest.lock().as_deref() {
                Ok(Some(place)) => place.save(),
                // Nothing to keep, or a write that panicked holding it.
                Ok(None) | Err(_) => Ok(()),
            }
        };
        self.background.run(self.main_thread, save, |saved| {
            if let Err(error) = saved {
                eprintln!("plex-rust: {}", with_causes(&error));
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

    /// Looks for the server again while what was saved stays on screen:
    /// first where it was, which is enough when it was off or asleep,
    /// then wherever plex.tv says it is now.
    fn retry(self: &Rc<Self>) {
        self.stale.show(Some(Reason::Asking));
        self.restart();
    }

    /// Says that the screen holds what was saved and not what the server
    /// has now, because asking it ended in `error`. This stands where an
    /// error page would take away what there still is to look at.
    fn mark_stale(&self, error: &(dyn std::error::Error + 'static)) {
        // A message in place of the screen has left nothing saved to see,
        // and what was downloaded is on this Mac, not saved from an answer.
        let on_downloads = self.current(|entry| entry.route == Route::Downloads) == Some(true);
        if self.notice.borrow().is_some() || on_downloads {
            return;
        }
        let reason = match Failure::of(error) {
            Failure::Unanswered => Reason::Offline,
            Failure::Missing => Reason::Gone,
            _ => Reason::Refused,
        };
        self.stale.show(Some(reason));
    }

    /// Starts over from the stored account, after a failure to connect.
    fn restart(self: &Rc<Self>) {
        match account::signed_in() {
            Ok((client, token)) => self.connect(client, token, None),
            Err(Error::SignedOut) => self.show_sign_in(),
            Err(error) => self.show_failure(UNREACHABLE, &error, Self::restart),
        }
    }

    fn connected(self: &Rc<Self>, kept: Kept, sections: Vec<Section>, snapshot: Option<&Snapshot>) {
        let Kept { server, artwork, .. } = kept;
        let background = Rc::clone(&self.background);
        let images = Rc::new(Images::new(self.main_thread, background, server.clone(), artwork));
        let session = Session { server, images, sections: sections.clone() };
        *self.session.borrow_mut() = Some(session);
        self.stale.show(None);
        self.history.borrow_mut().clear();
        self.set_browsing_shown(true);
        self.mark_shown("libraries known");

        self.list_places(Some(&sections));

        let tweaks = snapshot.map(|snapshot| snapshot.tweaks.clone()).unwrap_or_default();
        self.holds_loads.set(tweaks.unanswered);
        self.read_only.set(snapshot.is_some());
        match snapshot.map(|snapshot| &snapshot.subject) {
            Some(Subject::Library(index)) => match sections.get(*index) {
                Some(section) => self.go(Route::Library(section.clone(), tweaks.listing)),
                None => self.go(Route::Home),
            },
            Some(Subject::Item(key)) => {
                let key = RatingKey::new(key.as_str());
                self.fetch(key, Asked::Server, |shell, item| shell.open(item));
            }
            Some(Subject::Play(key, keys)) => {
                let (key, keys) = (RatingKey::new(key.as_str()), keys.clone());
                // A download plays as it was kept, with no server to ask.
                match self.downloads.whole(&key) {
                    Some(download) => {
                        self.play_kept(&download, Purpose::Check, Playback::default());
                        self.press_later(keys, SNAPSHOT_KEYS_AFTER);
                    }
                    None => self.fetch(key, Asked::Server, move |shell, item| {
                        shell.open_player(item, Purpose::Check);
                        shell.press_later(keys, SNAPSHOT_KEYS_AFTER);
                    }),
                }
            }
            Some(Subject::Search(query)) => {
                self.toolbar.set_search_text(query);
                self.go(Route::Search(query.clone()));
            }
            Some(Subject::List(path)) => {
                self.go(Route::List { title: "List".to_owned(), path: path.clone() });
            }
            Some(Subject::Downloads) => self.go(Route::Downloads),
            Some(Subject::Home | Subject::SignIn | Subject::Walk(_)) | None => self.go(Route::Home),
        }
    }

    /// Fills the sidebar and the Go menu: Home and the libraries, once
    /// there is a server to have them, and Downloads.
    fn list_places(self: &Rc<Self>, sections: Option<&[Section]>) {
        let mut destinations = Vec::new();
        if let Some(sections) = sections {
            destinations.push(Destination::Home);
            destinations.extend(sections.iter().cloned().map(Destination::Section));
        }
        destinations.push(Destination::Downloads);
        self.install_places(&destinations);
        self.sidebar.show(destinations);
        self.toolbar.set_search_enabled(sections.is_some());
    }

    /// Gives the places of the sidebar their entries in the Go menu.
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
        // Taken out of the split, not collapsed in it: View > Show Sidebar
        // brings a collapsed sidebar back, and this one has nothing in it.
        let held = self.split.splitViewItems().containsObject(&self.sidebar_item);
        match (shown, held) {
            (true, false) => self.split.insertSplitViewItem_atIndex(&self.sidebar_item, 0),
            (false, true) => self.split.removeSplitViewItem(&self.sidebar_item),
            _ => {}
        }
        if !shown {
            self.stale.show(None);
        }
        self.toolbar.set_visible(shown);
    }

    // Moving from place to place.

    /// Counts a move away from the place on screen and gives up what it
    /// was still asking the server. An answer for a place the viewer has
    /// left is work for nothing, and for a search it is worse: the answer
    /// to an earlier query can come in after the answer to a later one,
    /// and would replace it.
    fn leave(&self) {
        self.moves.set(self.moves.get() + 1);
        self.toast.hide();
        for task in self.asked.borrow_mut().drain(..) {
            task.cancel();
        }
    }

    /// Goes somewhere new, leaving the current place one step back.
    fn go(self: &Rc<Self>, route: Route) {
        if self.playing.borrow().is_some() {
            return;
        }
        self.leave();
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
        self.leave();
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
        self.leave();
        self.position.set(position);
        self.display(true);
        self.refresh();
    }

    /// Asks the server about the current place again and updates the
    /// screen where the answer differs.
    fn refresh(&self) {
        let reload = self.current(|entry| Rc::clone(&entry.reload));
        if let Some(reload) = reload {
            self.asked_at.set(Instant::now());
            reload();
        }
    }

    /// Brings the place on screen up to date when the viewer comes back
    /// to the app after a while: what they watched on another device in
    /// the meantime has moved things. The screen changes only where the
    /// answer differs, so nothing flashes.
    fn activated(self: &Rc<Self>) {
        let browsing = self.playing.borrow().is_none() && self.notice.borrow().is_none();
        if browsing && self.asked_at.get().elapsed() >= FRESH_FOR {
            self.refresh();
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
        self.mark_shown("screen in the window");
        self.keep_place(&route);
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
        // What is downloaded is as it is on this Mac, whatever the server
        // does. The line comes back with the next screen that asks it.
        if route == Route::Downloads {
            self.stale.show(None);
        }
        // The search field says what was searched for while the results
        // are on screen and at no other time: left as it was, it would
        // claim that Home is what a search for it found.
        let searched = match &route {
            Route::Search(query) => query.as_str(),
            _ => "",
        };
        if self.toolbar.search_text() != searched {
            self.toolbar.set_search_text(searched);
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
        commands.find.set_enabled(browsing && self.session.borrow().is_some());
        commands.play.set_enabled(browsing && on_item);
        commands.toggle_watched.set_enabled(browsing && on_item);
    }

    /// Makes `view` the content, optionally fading from what was there.
    fn show_view(&self, view: &NSView, fade: bool) {
        if fade {
            self.fade_content();
        }
        layout::fill(&self.content, view);
    }

    /// Has the next change of the content fade in over what is there now.
    fn fade_content(&self) {
        let Some(layer) = self.content.layer().filter(|_| !theme::reduces_motion()) else {
            return;
        };
        let transition = CATransition::new();
        // SAFETY: the type is a constant that Core Animation provides.
        transition.setType(unsafe { kCATransitionFade });
        transition.setDuration(theme::MOTION_ARRIVE);
        layer.addAnimation_forKey(&transition, Some(ns_string!("screen")));
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
            self.asked_at.set(Instant::now());
            reload();
        }
        Some(Entry { route, screen, reload })
    }

    fn home(self: &Rc<Self>, session: &Session) -> (Screen, Rc<dyn Fn()>) {
        let outlines = home_outlines(&session.sections);
        let nothing = self.nothing(
            "play.rectangle.on.rectangle",
            "Nothing to watch yet",
            "What you are in the middle of and what your server has just added will be here.",
        );
        let (background, server, main_thread) =
            (Rc::clone(&self.background), session.server.clone(), self.main_thread);
        let colours = move |picture: String, then: Box<dyn FnOnce(Colors)>| {
            let server = server.clone();
            // The colours of a picture never change, so those kept from
            // last time are as good as the server's.
            let ask = async move {
                match server.remembered().colors_of(&picture).await {
                    Ok(colours) => Ok(colours),
                    Err(_) => server.colors_of(&picture).await,
                }
            };
            background.run(main_thread, ask, move |colours| match colours {
                Ok(colours) => then(colours),
                Err(error) => eprintln!("plex-rust: {}", with_causes(&error)),
            });
        };
        let hero = Hero::new(self.main_thread, Rc::clone(&session.images), colours, self.actions());
        let header = Header { view: hero.view().retain(), height: theme::HOME_HERO_HEIGHT };
        let posters = self.posters(session, Some(header), outlines, nothing, || {});
        posters.fit_header_to_page(theme::HOME_HERO_HEIGHT_PER_WIDTH, theme::HOME_HERO_MOST);
        posters.set_header_keys(hero.keys());
        posters.let_header_reach(hero.reach());
        let shown = posters.clone();
        // A snapshot of the screen still waiting is built with nothing.
        let saved = self.saved_home.borrow_mut().take().filter(|_| !self.holds_loads.get());
        let reload = self.loader_from(
            saved,
            session,
            Unanswered::ShowsTheError,
            |server| async move { server.home(SHELF_LENGTH).await },
            // The hero lives as long as this closure, which the entry keeps.
            move |hubs: &Vec<Hub>| {
                let shelves = home_shelves(hubs);
                hero.show(hero::features(&shelves));
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
        let noun = if section.kind == Kind::Show { "show" } else { "movie" };
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
        let on_total = move |total| bar.set_total(total);
        let (screen, reload) =
            self.grid(session, route.clone(), source.clone(), Some(header), "", nothing, on_total);
        let (Screen::Posters(grid), Sort::Title) = (&screen, listing.sort) else {
            return (screen, reload);
        };

        // Listed by title, a library has an index of first characters.
        let (shell, beyond) = (Rc::downgrade(self), objc2::rc::Weak::new(&**grid));
        grid.keep_index(move |card| {
            if let (Some(shell), Some(grid)) = (shell.upgrade(), beyond.load()) {
                shell.load_page(&route, &grid, source.clone(), Some(card));
            }
        });
        let (shown, of) = (grid.clone(), section.clone());
        let index = self.loader(
            session,
            Unanswered::GoesWithout,
            move |server| {
                let of = of.clone();
                async move { server.first_characters(&of, listing).await }
            },
            move |characters: &Vec<FirstCharacter>| {
                let entry = |character: &FirstCharacter| {
                    (
                        character.title.clone(),
                        usize::try_from(character.start).unwrap_or(usize::MAX),
                    )
                };
                shown.set_index(characters.iter().map(entry).collect());
            },
        );
        let reload = Rc::new(move || {
            reload();
            index();
        });
        (screen, reload)
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
        // The grid asks for its next page through a closure it is made
        // with, which hears of the grid once there is one.
        let asker = Rc::new(OnceCell::new());
        let (more, more_of, asking) = (Rc::downgrade(self), source.clone(), Rc::clone(&asker));
        let posters = self.posters(session, header, outlines, nothing, move || {
            let grid = asking.get().and_then(objc2::rc::Weak::<Posters>::load);
            if let (Some(shell), Some(grid)) = (more.upgrade(), grid) {
                shell.load_page(&route, &grid, more_of.clone(), None);
            }
        });
        // Cannot fail: the cell was created empty a few lines up.
        let _ = asker.set(objc2::rc::Weak::new(&*posters));

        let (shown, asked, title) = (posters.clone(), posters.clone(), title.to_owned());
        let reload = self.loader(
            session,
            Unanswered::ShowsTheError,
            // Everything the grid holds is asked for again, not its first
            // page alone: showing one page would drop the cards after it,
            // and what changed among those would never be seen.
            move |server| source.clone().page(server, 0, whole_pages(asked.len())),
            move |page: &Page| {
                on_total(page.total);
                let style = Style::Grid(Shape::Poster);
                shown.show(vec![Shelf::of(title.clone(), style, page.items.clone())]);
            },
        );
        (Screen::Posters(posters), reload)
    }

    /// Fetches the next page of `source` and adds it to `grid`, which
    /// asked for it, if `route` is still the place on screen. With
    /// `upto`, fetches as many pages as it takes to hold the card of that
    /// number, and then goes to it.
    fn load_page(
        self: &Rc<Self>,
        route: &Route,
        grid: &Posters,
        source: Paged,
        upto: Option<usize>,
    ) {
        let on_screen = self.current(|entry| entry.route == *route) == Some(true);
        let (Some(session), true) = (self.session(), on_screen) else {
            // The grid waits for what it asked for and asks for nothing
            // more until it hears. Told that the page is not coming, it
            // asks again when its last cards are next drawn.
            return grid.more_failed();
        };
        let posters = grid.retain();
        let start = posters.len();
        let from = u32::try_from(start).unwrap_or(u32::MAX);
        let size = whole_pages(upto.map_or(0, |card| (card + 1).saturating_sub(start)));
        let load = source.clone().page(session.server.clone(), from, size);
        let (shell, moves, asked) = (Rc::clone(self), self.moves.get(), Instant::now());
        let route = route.clone();
        self.background.run(self.main_thread, load, move |page| {
            shell.heard(asked, &page);
            match page {
                Ok(page) => {
                    let came = !page.items.is_empty();
                    posters.append(start, page.items);
                    shell.mark_shown("next page added");
                    match upto {
                        Some(card) if card < posters.len() => posters.go_to_card(card),
                        // The grid took another page while this one was on
                        // its way, and dropped this one. It is asked for
                        // again from where the grid now ends.
                        Some(card) if came => {
                            shell.load_page(&route, &posters, source, Some(card));
                        }
                        _ => {}
                    }
                }
                // The cards that are there stay. Only the sign-in being
                // over takes the screen, since nothing more can come until
                // it is renewed.
                Err(error) if !error.is_unauthorized() => {
                    posters.more_failed();
                    eprintln!("plex-rust: {}", with_causes(&error));
                    shell.mark_stale(&error);
                }
                Err(error) => {
                    posters.more_failed();
                    shell.fail(moves, &error);
                }
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

        // A download has the item in full next to its file. Where
        // nothing was saved of the server's own answer, that stands in
        // for it, so the page is whole without the server.
        let download = self.downloads.find(&item.rating_key).map(|entry| entry.download);
        let kept = download.as_ref().and_then(|download| {
            let kept = self.downloads.as_left(download);
            kept.map_err(|error| eprintln!("plex-rust: {}", with_causes(&error))).ok()
        });

        let (shown, key) = (Rc::clone(&detail), item.rating_key.clone());
        let mut loads = vec![self.loader(
            session,
            Unanswered::StandsAsDrawn { kept_here: download.is_some() },
            move |server| {
                let (key, kept) = (key.clone(), kept.clone());
                async move {
                    match (server.item(&key).await, kept) {
                        (Err(plex::Error::NotRemembered), Some(kept)) => Ok(kept),
                        (answer, _) => answer,
                    }
                }
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
        let (shown, shell) = (list.clone(), Rc::downgrade(self));
        let reload = Rc::new(move || {
            if let Some(shell) = shell.upgrade() {
                let (entries, free) = (shell.downloads.entries(), shell.downloads.free_space());
                shown.show(&entries, shell.reach.get().is_away(), free);
            }
        });
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
        self.leave();
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
        self.loader_from(None, session, unanswered, ask, show)
    }

    /// A [`loader`](Self::loader) that starts with `saved`, the answer
    /// from last time, already in hand: it is shown here and now, before
    /// the screen is in the window, and only the server is asked.
    fn loader_from<T, F>(
        self: &Rc<Self>,
        saved: Option<T>,
        session: &Session,
        unanswered: Unanswered,
        ask: impl Fn(MediaServer) -> F + 'static,
        show: impl Fn(&T) + 'static,
    ) -> Rc<dyn Fn()>
    where
        T: PartialEq + Send + 'static,
        F: Future<Output = plex::Result<T>> + Send + 'static,
    {
        if let Some(saved) = &saved {
            show(saved);
            timing::mark("saved answer in the screen as built");
        }
        // The answer on screen, from whichever run of the closure.
        let shown: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(saved));
        let show = Rc::new(show);
        let (shell, server) = (Rc::downgrade(self), session.server.clone());
        Rc::new(move || {
            let Some(shell) = shell.upgrade() else {
                return;
            };
            let moves = shell.moves.get();
            // Set once the server has answered, so that a late answer from
            // last time cannot replace its answer.
            let answered = Rc::new(Cell::new(false));
            // The server's failure to answer is held back while the answer
            // from last time is still being read. A server that refuses at
            // once is quicker than the disk, and its error would take the
            // screen from what was saved for just this case.
            let remembering = Rc::new(Cell::new(false));
            let held: Rc<RefCell<Option<plex::Error>>> = Rc::default();
            if shown.borrow().is_none() {
                remembering.set(true);
                let (shown, show, answered) =
                    (Rc::clone(&shown), Rc::clone(&show), Rc::clone(&answered));
                let (remembering, held) = (Rc::clone(&remembering), Rc::clone(&held));
                let remembered = ask(server.remembered());
                let then = Rc::clone(&shell);
                let reading = shell.background.run(shell.main_thread, remembered, move |answer| {
                    if let (Ok(answer), false) = (answer, answered.get()) {
                        show(&answer);
                        then.mark_shown("remembered answer shown");
                        *shown.borrow_mut() = Some(answer);
                    }
                    remembering.set(false);
                    if let Some(error) = held.borrow_mut().take() {
                        then.not_answered(moves, shown.borrow().is_some(), unanswered, &error);
                    }
                });
                shell.asked.borrow_mut().push(reading);
            }

            let (shown, show) = (Rc::clone(&shown), Rc::clone(&show));
            let failed = Rc::clone(&shell);
            let (fresh, asked) = (ask(server.clone()), Instant::now());
            let asking = shell.background.run(shell.main_thread, fresh, move |answer| {
                failed.heard(asked, &answer);
                answered.set(answer.is_ok());
                match answer {
                    Ok(answer) if shown.borrow().as_ref() == Some(&answer) => {
                        failed.stale.show(None);
                        failed.mark_shown("server answered the same");
                    }
                    Ok(answer) => {
                        show(&answer);
                        *shown.borrow_mut() = Some(answer);
                        failed.stale.show(None);
                        failed.mark_shown("server answer shown");
                    }
                    Err(error) if remembering.get() => *held.borrow_mut() = Some(error),
                    Err(error) => {
                        failed.not_answered(moves, shown.borrow().is_some(), unanswered, &error);
                    }
                }
            });
            shell.asked.borrow_mut().push(asking);
        })
    }

    /// What follows from the server not answering a screen's question
    /// with anything but `error`. With an answer from last time on screen
    /// (`has_shown`) that stays, marked as saved: it is better than an
    /// error, but for the one that says the sign-in is over. Without one,
    /// `otherwise` says whether the screen can stand.
    fn not_answered(
        self: &Rc<Self>,
        moves: u64,
        has_shown: bool,
        otherwise: Unanswered,
        error: &plex::Error,
    ) {
        let drawn = matches!(otherwise, Unanswered::StandsAsDrawn { .. });
        let gone =
            error.is_missing() && otherwise == Unanswered::StandsAsDrawn { kept_here: false };
        let stands = has_shown || otherwise != Unanswered::ShowsTheError;
        if error.is_unauthorized() || gone || !stands {
            return self.fail(moves, error);
        }
        eprintln!("plex-rust: {}", with_causes(error));
        if has_shown || drawn {
            self.mark_stale(error);
        }
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
                Action::Back => shell.back(),
                Action::Play(item) => shell.play(&item, Purpose::Watching, Playback::default()),
                Action::PlayAs(item, playback) => shell.play(&item, Purpose::Watching, playback),
                Action::SetWatched(item, watched) => shell.set_watched(&item, watched),
                Action::ToggleDownload(item) => shell.toggle_download(&item),
                Action::ChooseStreams { part, audio, subtitles } => {
                    let choose = move |server: MediaServer| {
                        let part = part.clone();
                        async move { server.choose_streams(&part, audio, subtitles).await }
                    };
                    shell.change("The server did not take that choice", Rc::new(choose));
                }
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
                Action::Rest(item) => shell.ask_ahead(&item),
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
            (Kind::Clip, _) => self.play(item, Purpose::Watching, Playback::default()),
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

    /// Asks the server now for what the page of `item` shows, because the
    /// pointer or the keyboard is on its card. The answers are saved, so
    /// the page opens with them and not with a wait for each. Only once
    /// the card has been rested on for a moment, and no further once
    /// another card is.
    fn ask_ahead(self: &Rc<Self>, item: &Item) {
        if let Some(passed_over) = self.asked_ahead.borrow_mut().take() {
            passed_over.cancel();
        }
        // A server that is not answering is not asked on the off chance.
        let Some(session) = self.session().filter(|_| !self.stale.is_shown()) else {
            return;
        };
        let (server, images, item) = (session.server.clone(), session.images, item.clone());
        // The backdrop of the page, as `Detail::show` picks it. It is the
        // largest thing the page waits for.
        let backdrop = match item.kind {
            Kind::Episode => item.grandparent_art.clone().or(item.art.clone()),
            Kind::Clip | Kind::Season => None,
            _ => item.art.clone(),
        };
        let ask = async move {
            tokio::time::sleep(REST_BEFORE_ASKING_AHEAD).await;
            // The same questions as `open` and `item_page` ask. An answer
            // is wanted for being saved, and a failure is the page's to
            // report when it asks again itself, so neither is looked at.
            let key = &item.rating_key;
            match (item.kind, &item.parent_rating_key) {
                (Kind::Clip, _) => {}
                (Kind::Season, Some(show)) => {
                    let _ = tokio::join!(server.item(show), server.episodes(show));
                }
                (Kind::Show, _) => {
                    let related = server.related(key, RELATED_LENGTH);
                    let _ = tokio::join!(server.item(key), server.episodes(key), related);
                }
                (Kind::Episode, Some(season)) => {
                    let _ = tokio::join!(server.item(key), server.children(season));
                }
                (Kind::Movie, _) => {
                    let _ = tokio::join!(server.item(key), server.related(key, RELATED_LENGTH));
                }
                _ => {
                    let _ = server.item(key).await;
                }
            }
        };
        let asking = self.background.run(self.main_thread, ask, |()| {});
        *self.asked_ahead.borrow_mut() = Some(asking);
        let Some(backdrop) = backdrop else {
            return;
        };
        // Not given up with the rest: a picture half fetched is fetched
        // again from the start, and once here it is kept.
        let rest = async { tokio::time::sleep(REST_BEFORE_ASKING_AHEAD).await };
        let (shell, moves) = (Rc::clone(self), self.moves.get());
        self.background.run(self.main_thread, rest, move |()| {
            let (width, height) = theme::BACKDROP_REQUEST;
            if shell.moves.get() == moves && images.cached(&backdrop, width, height).is_none() {
                images.load(&backdrop, width, height, |_| {});
            }
        });
    }

    fn open_show(self: &Rc<Self>, show: RatingKey, opening: Opening) {
        // What was saved of the show opens its page at once. The page
        // then asks the server itself.
        self.fetch(show, Asked::SavedFirst, move |shell, show| {
            shell.go(Route::Item(Box::new(show.clone()), opening));
        });
    }

    /// Marks `item` as watched or not, asking first when it stands for
    /// many episodes.
    fn set_watched(self: &Rc<Self>, item: &Item, watched: bool) {
        if !matches!(item.kind, Kind::Show | Kind::Season) {
            return self.mark(item, watched);
        }
        let (shell, marked) = (Rc::clone(self), item.clone());
        confirm::marking_every_episode(&self.window, item, watched, move || {
            shell.mark(&marked, watched);
        });
    }

    /// Marks `item` as watched or not on the server.
    fn mark(self: &Rc<Self>, item: &Item, watched: bool) {
        let key = item.rating_key.clone();
        let mark = move |server: MediaServer| {
            let key = key.clone();
            async move { server.set_watched(&key, watched).await }
        };
        let failed = match watched {
            true => "Could not mark as watched",
            false => "Could not mark as unwatched",
        };
        self.change(failed, Rc::new(mark));
    }

    /// Makes a change on the server, then brings the screen up to date
    /// with what that changed. If the server does not take it the screen
    /// stays, and a line over it says that `failed`. A snapshot changes
    /// nothing.
    fn change<F>(self: &Rc<Self>, failed: &'static str, change: Rc<dyn Fn(MediaServer) -> F>)
    where
        F: Future<Output = plex::Result<()>> + Send + 'static,
    {
        let Some(session) = self.session().filter(|_| !self.read_only.get()) else {
            return;
        };
        let (shell, moves, asked) = (Rc::clone(self), self.moves.get(), Instant::now());
        let changing = change(session.server.clone());
        self.background.run(self.main_thread, changing, move |changed| {
            shell.heard(asked, &changed);
            match changed {
                Ok(()) => shell.refresh(),
                // Nothing will reach the server until the viewer has
                // signed in again, which the screen that says so leads to.
                Err(error) if error.is_unauthorized() => shell.fail(moves, &error),
                Err(error) => {
                    eprintln!("plex-rust: {}", with_causes(&error));
                    // A server that did not answer is also why the page
                    // is from an earlier time, which its own line says.
                    if error.is_unanswered() {
                        shell.mark_stale(&error);
                    }
                    if moves == shell.moves.get() {
                        let again = Rc::clone(&change);
                        shell.not_changed(failed, move |shell| {
                            shell.change(failed, Rc::clone(&again));
                        });
                    }
                }
            }
        });
    }

    /// Says on a line over the screen that `failed`, with a button that
    /// does `again`, for a while. The page goes back to showing what the
    /// server has, which is not what the viewer just set it to.
    fn not_changed(self: &Rc<Self>, failed: &str, again: impl Fn(&Rc<Self>) + 'static) {
        if let Some(Screen::Detail(detail)) = self.current(|entry| entry.screen.clone()) {
            detail.revert();
        }
        let retried = Rc::downgrade(self);
        self.toast.show(failed, move || {
            if let Some(shell) = retried.upgrade() {
                shell.toast.hide();
                again(&shell);
            }
        });
        let shown = self.toasts.get() + 1;
        self.toasts.set(shown);
        let wait = async { tokio::time::sleep(TOAST_STAYS).await };
        let shell = Rc::clone(self);
        self.background.run(self.main_thread, wait, move |()| {
            if shell.toasts.get() == shown {
                shell.toast.hide();
            }
        });
    }

    // Downloads.

    /// Does what the download button of an item's page does: downloads the
    /// item, carries on with a download of it that stopped short, or
    /// deletes one that is whole or on its way. The version of `item` to
    /// download is its first, where its page puts the one that was picked.
    fn toggle_download(self: &Rc<Self>, item: &Item) {
        let key = &item.rating_key;
        match self.downloads.find(key).map(|entry| entry.state) {
            None => self.download(item),
            Some(State::Incomplete { .. }) => self.act_on_download(Act::Resume(key.clone())),
            Some(State::Downloading(_) | State::Downloaded) => self.downloads.remove(key),
        }
    }

    /// Starts downloading the first version of `item`. The item is asked
    /// for in full first, because that is what is kept next to its file.
    ///
    /// A download that cannot begin is said in a sheet: the page it was
    /// asked for on is as good as it was.
    fn download(self: &Rc<Self>, item: &Item) {
        let Some(session) = self.session() else {
            return;
        };
        let (server, key, title) =
            (session.server.clone(), item.rating_key.clone(), item.title.clone());
        let version = item.media.first().map(|media| media.id);
        let asked = key.clone();
        let ask = async move { server.item_json(&asked).await };
        let (shell, asked) = (Rc::clone(self), Instant::now());
        self.background.run(self.main_thread, ask, move |item| {
            shell.heard(asked, &item);
            // A second press while the first was still asking.
            if shell.downloads.find(&key).is_some() {
                return;
            }
            let added = item
                .map_err(Error::from)
                .and_then(|item| shell.downloads.add(&session.server, &item, version));
            let reason = match added {
                Ok(()) => return,
                Err(Error::InParts { parts, .. }) => format!(
                    "This version is split across {parts} files, and a download is one file. \
                     Pick another version if there is one, or play it from your server."
                ),
                Err(error) => {
                    eprintln!("plex-rust: {}", with_causes(&error));
                    downloads::reason(&error)
                }
            };
            eprintln!("plex-rust: {title} was not downloaded: {reason}");
            confirm::refusal(&shell.window, &format!("{title} was not downloaded"), &reason);
        });
    }

    fn act_on_download(self: &Rc<Self>, act: Act) {
        match act {
            Act::Play(key) => {
                if let Some(entry) = self.downloads.find(&key) {
                    self.play(&entry.download.item, Purpose::Watching, Playback::default());
                }
            }
            Act::Resume(key) => {
                if let Some(session) = self.session() {
                    self.downloads.start(&session.server, &key);
                }
            }
            Act::Remove(key) => self.downloads.remove(&key),
            Act::Reconnect => self.restart(),
        }
    }

    /// Brings the screen up to date with what the downloads have done,
    /// and with whether the server is there for them.
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

    /// Fetches the item with `key` from where `asked` says and hands it
    /// to `then`.
    fn fetch(
        self: &Rc<Self>,
        key: RatingKey,
        asked: Asked,
        then: impl FnOnce(&Rc<Self>, &Item) + 'static,
    ) {
        let Some(session) = self.session() else {
            return;
        };
        let server = session.server.clone();
        let load = async move {
            if asked == Asked::SavedFirst
                && let Ok(saved) = server.remembered().item(&key).await
            {
                return Ok(saved);
            }
            server.item(&key).await
        };
        let (shell, moves) = (Rc::clone(self), self.moves.get());
        self.background.run(self.main_thread, load, move |item| match item {
            Ok(item) => then(&shell, &item),
            Err(error) => shell.fail(moves, &error),
        });
    }

    // Playing.

    /// Plays `item` as `playback` says, first fetching it in full: a
    /// listing leaves out the intro and credits markers that the player
    /// offers to skip.
    ///
    /// A download plays as it was kept when the server is not there to ask.
    fn play(self: &Rc<Self>, item: &Item, purpose: Purpose, playback: Playback) {
        let download = self.downloads.whole(&item.rating_key);
        if let (Some(download), false) = (&download, self.reach.get().is_answering()) {
            return self.play_kept(download, purpose, playback);
        }
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
        let (shell, moves, asked) = (Rc::clone(self), self.moves.get(), Instant::now());
        self.background.run(self.main_thread, load, move |item| {
            shell.opening_player.set(false);
            shell.heard(asked, &item);
            match (item, download) {
                (Ok(mut item), _) => {
                    playback.apply(&mut item);
                    shell.open_player(&item, purpose);
                }
                // The server has gone since it last answered, or the
                // item has gone from the server. The file is still here.
                (Err(error), Some(download)) if error.is_unanswered() || error.is_missing() => {
                    eprintln!("plex-rust: {}", with_causes(&error));
                    shell.play_kept(&download, purpose, playback);
                }
                (Err(error), _) => shell.fail(moves, &error),
            }
        });
    }

    /// Plays `download` as `playback` says, from where it was last left as
    /// far as this Mac knows, with no server to say.
    fn play_kept(self: &Rc<Self>, download: &Download, purpose: Purpose, playback: Playback) {
        match self.downloads.as_left(download) {
            Ok(mut item) => {
                // The version that was downloaded is the one there is.
                Playback { version: None, ..playback }.apply(&mut item);
                self.open_player(&item, purpose);
            }
            Err(error) => self.fail(self.moves.get(), &error),
        }
    }

    /// Replaces the whole window content with the player, playing `item`:
    /// from this Mac if it was downloaded, from the server if not.
    fn open_player(self: &Rc<Self>, item: &Item, purpose: Purpose) {
        // Replacing a player without closing it would leave it running
        // unseen, with nothing left that could stop it.
        if self.playing.borrow().is_some() {
            return;
        }
        let session = self.session();
        // A download does without a server that has not answered. Anything
        // else has only the server to come from, answering or not.
        let downloaded = self.downloads.holding(item).is_some();
        let server = match (&session, self.reach.get().is_answering()) {
            (None, _) if !downloaded => return,
            (Some(session), true) => Some(session.server.clone()),
            (Some(session), false) if !downloaded => Some(session.server.clone()),
            _ => None,
        };
        let images = session.as_ref().map(|session| &session.images);
        let (closed, opened_with) = (Rc::downgrade(self), item.rating_key.clone());
        let saved = Self::command(&Rc::downgrade(self), Self::refresh_behind_player);
        let screen = PlayerScreen::open(
            self.main_thread,
            Rc::clone(&self.background),
            server,
            &self.downloads,
            images,
            item,
            self.settings.get(),
            purpose,
            move || {
                if let Some(shell) = closed.upgrade() {
                    shell.player_closed(&opened_with);
                }
            },
            move || {
                // A check told the server nothing, so nothing has changed.
                if purpose == Purpose::Watching {
                    saved();
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
    fn player_closed(self: &Rc<Self>, opened_with: &RatingKey) {
        let Some(screen) = self.playing.borrow_mut().take() else {
            return;
        };
        self.keep_settings(&screen);
        self.set_window_content(&self.split);
        self.set_toolbar_visible(true);
        self.enable_commands();

        if self.reach.get().is_answering() {
            // A viewing begun before the server answered was kept here.
            self.send_unsent();
        }

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
            }
            None => {}
        }
    }

    /// Brings the screen the player was opened from up to date: it still
    /// says where the item was left before it was played. Called when the
    /// server has heard where the viewer stopped. Asked any sooner, it
    /// answers with the old place, which the screen would then keep.
    fn refresh_behind_player(self: &Rc<Self>) {
        if self.playing.borrow().is_none() {
            self.refresh();
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
        // A message arrives the way a page does.
        self.fade_content();
        // A message in place of the screen leaves nothing that was saved
        // to be seen.
        self.stale.show(None);
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
        if let Subject::Walk(steps) = &snapshot.subject {
            return self.walk(steps.clone(), snapshot);
        }
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
            if snapshot.tweaks.collapsed {
                shell.sidebar_item.setCollapsed(true);
            }
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
                if let (true, Screen::Posters(posters)) = (snapshot.tweaks.page, screen) {
                    posters.page_first_shelf();
                }
                if let (Some(letter), Screen::Posters(posters)) = (snapshot.tweaks.letter, screen) {
                    posters.jump_to(letter);
                }
                if let Screen::Posters(posters) = screen {
                    snapshot.tweaks.presses.iter().for_each(|press| posters.press(*press));
                }
                if let (Some(link), Screen::Detail(detail)) = (snapshot.tweaks.follow, screen) {
                    detail.follow_link(link);
                }
                if let (Some(version), Screen::Detail(detail)) = (snapshot.tweaks.version, screen) {
                    detail.pick_version(version);
                }
                if let (true, Screen::Detail(detail)) = (snapshot.tweaks.download, screen) {
                    detail.press_download();
                }
            }
            if let Some(failure) = snapshot.tweaks.fault {
                let small_print = "error sending request for url (https://192.168.0.2:32400/hubs)";
                shell.show_failure_of(failure, NOT_LOADED, small_print, Self::reload);
            }
            if snapshot.tweaks.refused {
                shell.not_changed("Could not mark as watched", |_| {});
            }
            if snapshot.tweaks.offline {
                shell.stale.show(Some(Reason::Offline));
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
        if let Err(error) = snapshot::write_png(&self.window, path) {
            eprintln!("plex-rust: {}", with_causes(&error));
            std::process::exit(1);
        }
        std::process::exit(0);
    }
}

/// How many items to ask for to get every page of a grid that holds
/// `cards`: at least the first page, and whole pages, so that the next
/// page still starts where this answer ends.
fn whole_pages(cards: usize) -> u32 {
    let cards = u32::try_from(cards).unwrap_or(u32::MAX);
    cards.max(1).div_ceil(PAGE_SIZE).saturating_mul(PAGE_SIZE)
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

/// The shelves the home screen is drawn with until the server names its
/// own: what is being watched, then what was added of each kind the
/// libraries hold, under the titles the server gives those shelves. With
/// the same headings before and after, nothing moves when the answer lands.
fn home_outlines(sections: &[Section]) -> Vec<Outline> {
    let holds = |kind| sections.iter().any(|section| section.kind == kind);
    let added = |title| Outline { title, style: Style::Shelf(Shape::Poster), count: 10 };
    let mut outlines = vec![Outline {
        title: "Continue Watching",
        style: Style::Shelf(Shape::Landscape),
        count: 6,
    }];
    outlines.extend(holds(Kind::Movie).then(|| added("Recently Added Movies")));
    outlines.extend(holds(Kind::Show).then(|| added("Recently Added TV")));
    outlines
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

/// The server used the last time the app ran, if there was one.
fn saved_server(client: &Client) -> Result<Option<Kept>> {
    let saved = account::saved_server(client)?;
    saved.map(|(server, account)| keeping(server, account)).transpose()
}

/// `server` as `account` uses it, with a place for its answers and one
/// for its artwork. What is kept there is found again at whatever address
/// the server is reached, and by no other account.
fn keeping(server: MediaServer, account: String) -> Result<Kept> {
    let home = std::env::var_os("HOME").ok_or(Error::NoHome)?;
    let caches = PathBuf::from(home).join("Library/Caches/plex-rust");
    let scope = format!("{account}\n{}", server.machine_identifier());
    let folder = |name: &str, limit| {
        let path = caches.join(name);
        std::fs::create_dir_all(&path)
            .map_err(|source| Error::File { path: path.clone(), source })?;
        Ok::<_, Error>(Cache::new(path, &scope, limit))
    };
    let (responses, artwork) =
        (folder("responses", RESPONSES_LIMIT)?, folder("images", images::DISK_LIMIT)?);
    Ok(Kept { server: server.with_cache(responses), account, artwork })
}

/// The main window. `staged` makes it one a snapshot can keep off every
/// display.
fn window(main_thread: MainThreadMarker, staged: bool) -> Retained<NSWindow> {
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::FullSizeContentView;
    let frame = NSRect::new(NSPoint::ZERO, theme::WINDOW_SIZE);
    let window = if staged {
        snapshot::window(frame, style, main_thread)
    } else {
        // SAFETY: the arguments are valid, and `setReleasedWhenClosed(false)`
        // below gives the window the ownership rules `Retained` expects.
        unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(main_thread),
                frame,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        }
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

        #[unsafe(method(applicationDidBecomeActive:))]
        fn did_become_active(&self, _notification: &NSNotification) {
            self.ivars().activated();
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
    fn a_grid_is_asked_for_again_in_whole_pages() {
        assert_eq!(whole_pages(0), PAGE_SIZE);
        assert_eq!(whole_pages(49), PAGE_SIZE);
        assert_eq!(whole_pages(100), PAGE_SIZE);
        assert_eq!(whole_pages(300), 3 * PAGE_SIZE);
        // The last page of a list came short.
        assert_eq!(whole_pages(250), 3 * PAGE_SIZE);
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
