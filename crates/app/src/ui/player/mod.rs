//! The screen that plays one item: video with controls on top.

mod badge;
mod bubble;
mod captions;
mod chip;
mod chrome;
mod controls;
mod failure;
mod info;
pub mod input;
mod layout;
mod mini;
mod pane;
mod panel;
mod remote;
mod slider;
mod speed;
mod stage;
mod surface;
mod theme;
mod tracks;
mod upnext;
mod waiting;
mod zoom;

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSCursor, NSEvent, NSView, NSWindow, NSWindowDidExitFullScreenNotification,
    NSWindowStyleMask,
};
use objc2_foundation::{
    NSActivityOptions, NSDate, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol,
    NSOperationQueue, NSPoint, NSProcessInfo, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer,
    NSUUID, ns_string,
};
use player::{Chapter, DolbyVision, EndReason, Event, Player, Sidecar, Track, TrackKind};
use plex::MediaServer;
use plex::model::{Item, Kind, Marker, MarkerKind, Part};
use plex::playback::{self, PlaybackState};
use plex::queue::{Neighbours, Queue};

use self::badge::Notice;
use self::captions::{Shift, Size};
use self::chip::Chip;
use self::chrome::Chrome;
use self::controls::Controls;
use self::input::{Clicks, Gesture, InputView, Key, Press, Scene};
use self::mini::Mini;
use self::panel::{Choice, Step, Stepper};
use self::remote::{Command, Playing, Remote};
use self::slider::{Grip, Pointed};
use self::speed::Speed;
use self::surface::Material;
use self::zoom::Zoom;
use crate::background::Background;
use crate::downloads::Downloads;
use crate::error::{Error, Result, with_causes};
use crate::images::Images;
use crate::settings::Settings;
use crate::source;
use crate::video_layer::{self, VideoLayer};

/// How often the screen catches up with the player. Four times a second
/// is enough for a clock that counts seconds to never look late.
const TICK: Duration = Duration::from_millis(250);
const TICKS_PER_SECOND: u32 = 4;

/// Plex asks to hear from a running playback every 10 seconds.
const TICKS_PER_REPORT: u32 = 10 * TICKS_PER_SECOND;

/// The controls go away after this many ticks without input.
const TICKS_UNTIL_HIDDEN: u32 = 3 * TICKS_PER_SECOND;

/// A notice, such as the new volume, stays up for this many ticks.
const TICKS_OF_NOTICE: u32 = 6;

/// A warning is a sentence to read, and stays up for this many ticks.
const TICKS_OF_WARNING: u32 = 8 * TICKS_PER_SECOND;

/// What the viewer is told about a file that
/// [`player::DolbyVision::is_shown_in_wrong_colors`].
const WRONG_COLORS: &str = "Dolby Vision profile 5 plays in the wrong colours here";

/// For this many ticks after the scrubber's knob is let go, the knob and
/// the times stay where the viewer put them. The player needs a moment to
/// start on the jump, and until then still reports where it was before.
const TICKS_OF_SCRUB_SETTLING: u32 = 2;

/// How far the left and right arrow keys jump.
const SHORT_JUMP: f64 = 5.0;

/// How far they jump with Shift held, and how far the buttons jump.
const LONG_JUMP: f64 = 10.0;

/// With no credits marked, the next episode is offered this long before
/// the end, and starts when this one has run out.
const UP_NEXT_LEAD: Duration = Duration::from_secs(30);

/// With the closing credits marked, the next episode is offered when they
/// start and takes over this far into them.
const UP_NEXT_COUNTDOWN: Duration = Duration::from_secs(10);

/// Credits are the closing credits if they end within this much of the
/// end of the file. Credits with a scene after them are not: counting
/// down over them would cut that scene off.
const CLOSING_CREDITS_END_WITHIN: Duration = Duration::from_secs(10);

/// This far into an episode, going back means going back to its start.
/// Before that it means the episode before, as in every player.
const RESTART_AFTER: Duration = Duration::from_secs(5);

/// How much the up and down arrow keys change the volume, out of 100.
const VOLUME_STEP: f64 = theme::VOLUME_STEP;

/// Why the player is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Someone is watching.
    Watching,
    /// An unattended check. It is silent and tells the server nothing, so
    /// that checking the app never moves anyone's place in a film.
    Check,
}

/// Where the offer of the next episode stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpNext {
    /// The episode is not near its end, or no next episode is known.
    Waiting,
    /// The card is up and counting down.
    Counting,
    /// The viewer turned the offer down. Nothing follows this episode.
    Cancelled,
}

/// What the panel above the bar is open for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Offer {
    Chapters,
    Speed,
    Audio,
    Subtitles,
}

/// What has come of the last click on the picture.
enum Click {
    /// Nothing that a second click would have to take back.
    Settled,
    /// It is waiting, on this timer, to be known for a single click.
    Waiting(Retained<NSTimer>),
    /// It paused or resumed.
    Acted,
}

/// How far the item on screen has got towards playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The server is being asked for it in full. What the player holds is
    /// still the episode before it, paused on its last picture.
    Fetching,
    /// The player has been given its file and has not opened it yet.
    Opening,
    /// Its file is open: positions and the end of the file are its own.
    Playing,
    /// It stopped because something went wrong, and the screen says so
    /// until the viewer tries again or leaves.
    Failed,
}

pub struct Ivars {
    root: Retained<InputView>,
    /// The view that hosts the video layer.
    video: Retained<NSView>,
    player: Player,
    /// What is playing. Another episode takes its place when the viewer
    /// moves on to it, or when this one ends.
    item: RefCell<Rc<Item>>,
    phase: Cell<Phase>,
    controls: Controls,
    material: Material,
    /// The small floating window the video is in, while it is in one.
    mini: RefCell<Option<Mini>>,
    /// Waits for the window to leave full screen, to open the mini player
    /// after it. Only a window that is not full screen can be put away.
    after_full_screen: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
    /// How the window's title bar was before the player cleared it. Empty
    /// until the screen is in a window, and again once it has closed.
    chrome: RefCell<Option<Chrome>>,
    ticks_of_notice_left: Cell<u32>,
    speed: Cell<Speed>,
    low_power: Cell<bool>,
    controls_shown: Cell<bool>,
    /// Whether the pointer was last seen on the controls. They stay for as
    /// long as it rests there.
    pointer_on_controls: Cell<bool>,
    ticks_idle: Cell<u32>,
    click: RefCell<Click>,
    /// What the panel above the bar is open for, if it is. The controls
    /// it stands on stay for as long as it is.
    offer: Cell<Option<Offer>>,
    ticks_of_scrub_settling_left: Cell<u32>,
    /// Whether the time to the right of the scrubber is the file's full
    /// length, and not what is left of it.
    shows_length: Cell<bool>,
    chapters: RefCell<Vec<Chapter>>,
    /// The subtitle files to add to the file being opened, once it is.
    sidecars: RefCell<Vec<Sidecar>>,
    /// What plays after what: a shuffle given on opening, or the
    /// episodes of the show being played once the server has listed them.
    /// `None` for a film played on its own.
    queue: RefCell<Option<Queue>>,
    /// The items before and after the one playing in the queue. `None`
    /// while there is no queue.
    neighbours: RefCell<Option<Neighbours>>,
    up_next: Cell<UpNext>,
    /// Held while playing, so that the display does not go to sleep in the
    /// middle of a film. Given up while paused and on closing.
    awake: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
    /// The system's media controls, held from open to close while someone
    /// is watching. A check never holds them: it must not take the media
    /// keys away from whatever the owner of the Mac is listening to.
    remote: RefCell<Option<Remote>>,
    /// Held from open to close. Its absence means the screen is closed.
    timer: RefCell<Option<Retained<NSTimer>>>,
    /// Whether the timer is put off until something happens: nothing on
    /// screen changes while a film is paused and its controls are away,
    /// and a timer that fires all the same keeps the processor awake.
    resting: Cell<bool>,
    ticks: Cell<u32>,
    /// The file's length, kept for when mpv no longer knows it.
    duration: Cell<Option<Duration>>,
    /// How far the picture is enlarged and which part of it is in view.
    /// It is the viewer's for one file, across pauses, jumps and changes
    /// of the window's size, and starts whole with the next.
    zoom: Cell<Zoom>,
    /// Where the player last said it was in the file.
    last_position: Cell<Option<Duration>>,
    background: Rc<Background>,
    /// `None` while the server is away, as the server is.
    images: Option<Rc<Images>>,
    /// `None` while the server is away. Then only downloads play, their
    /// progress is kept on this Mac, and an episode leads on to the next
    /// one that was downloaded.
    server: Option<MediaServer>,
    downloads: Rc<Downloads>,
    /// Lets the server tell this playback from another of the same item.
    session: String,
    purpose: Purpose,
    on_close: Box<dyn Fn()>,
    on_saved: Box<dyn Fn()>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `PlayerScreen`
    // does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustPlayerScreen"]
    #[ivars = Ivars]
    pub struct PlayerScreen;

    unsafe impl NSObjectProtocol for PlayerScreen {}

    impl PlayerScreen {
        /// No second click came: the first was a click and not half of a
        /// double click.
        #[unsafe(method(clickWaited:))]
        fn click_waited(&self, _timer: Option<&AnyObject>) {
            self.act_on_click();
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: Option<&AnyObject>) {
            self.or_log(self.on_tick());
            self.or_log(self.rest_if_still());
        }
    }
);

impl PlayerScreen {
    /// Starts playing `item` from where it was left, at the speed and
    /// volume in `settings`. `on_close` is called once, when the screen is
    /// done, and `on_saved` once after it, when the server has been told
    /// where the viewer stopped or could not be. Only from then on does the
    /// server answer with the new place.
    ///
    /// `item` should be the full item from [`MediaServer::item`], which is
    /// the one that knows its intro and credits. An episode gives way to
    /// the one after it without the screen closing, so what
    /// [`item`](Self::item) returns changes. With `queue`, `item` is its
    /// first and the screen goes through the queue in its order instead.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        main_thread: MainThreadMarker,
        background: Rc<Background>,
        server: Option<MediaServer>,
        downloads: &Rc<Downloads>,
        images: Option<&Rc<Images>>,
        item: &Item,
        queue: Option<Queue>,
        settings: Settings,
        purpose: Purpose,
        on_close: impl Fn() + 'static,
        on_saved: impl Fn() + 'static,
    ) -> Result<Retained<Self>> {
        let player = Player::new()?;
        player.set_muted(purpose == Purpose::Check)?;
        let speed = Speed::from_percent(settings.speed_percent);
        player.set_speed(speed.factor())?;
        player.set_volume(settings.volume.clamp(0.0, controls::FULL_VOLUME))?;
        player.set_low_power(settings.low_power)?;

        let root = InputView::new(main_thread);
        let video = NSView::new(main_thread);
        let layer = VideoLayer::new(&player, main_thread)?;
        video.setLayer(Some(&layer));
        video.setWantsLayer(true);
        crate::ui::layout::cover(&root, &video);
        root.hold_picture(&video);
        let material = Material::choose();
        let steps = steps_through(item, queue.as_ref());
        let controls = Controls::new(main_thread, &root, item, steps, material);

        let this = Self::alloc(main_thread).set_ivars(Ivars {
            root,
            video,
            player,
            item: RefCell::new(Rc::new(item.clone())),
            phase: Cell::new(Phase::Opening),
            controls,
            material,
            mini: RefCell::new(None),
            after_full_screen: RefCell::new(None),
            chrome: RefCell::new(None),
            ticks_of_notice_left: Cell::new(0),
            speed: Cell::new(speed),
            low_power: Cell::new(settings.low_power),
            controls_shown: Cell::new(true),
            pointer_on_controls: Cell::new(false),
            ticks_idle: Cell::new(0),
            click: RefCell::new(Click::Settled),
            offer: Cell::new(None),
            ticks_of_scrub_settling_left: Cell::new(0),
            shows_length: Cell::new(false),
            chapters: RefCell::new(Vec::new()),
            sidecars: RefCell::new(Vec::new()),
            queue: RefCell::new(queue),
            neighbours: RefCell::new(None),
            up_next: Cell::new(UpNext::Waiting),
            awake: RefCell::new(None),
            remote: RefCell::new(None),
            timer: RefCell::new(None),
            resting: Cell::new(false),
            ticks: Cell::new(0),
            duration: Cell::new(None),
            zoom: Cell::new(Zoom::FIT),
            last_position: Cell::new(None),
            background,
            images: images.cloned(),
            server,
            downloads: Rc::clone(downloads),
            session: NSUUID::UUID().UUIDString().to_string(),
            purpose,
            on_close: Box::new(on_close),
            on_saved: Box::new(on_saved),
        });
        // SAFETY: `init` is NSObject's designated initializer.
        let screen: Retained<Self> = unsafe { msg_send![super(this), init] };
        screen.connect_controls();
        screen.listen();
        screen.show_speed();
        screen.find_neighbours();
        // Loaded before the timer and the event thread exist: both keep the
        // screen and its player alive, so a failure after them would leak.
        screen.load(None)?;

        // SAFETY: `tick:` is defined above. The timer retains the screen
        // until `close` invalidates it.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                TICK.as_secs_f64(),
                &screen,
                sel!(tick:),
                None,
                true,
            )
        };
        // In the common modes, so that it also runs while a menu is open
        // and while the window is being resized. In the default mode alone
        // the times stood still through both, and the server heard nothing.
        // SAFETY: the mode is a constant that Foundation provides.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
        *screen.ivars().timer.borrow_mut() = Some(timer);
        if purpose == Purpose::Watching {
            screen.take_media_controls(steps);
        }
        screen.follow_events(main_thread);
        screen.list_episodes();
        Ok(screen)
    }

    pub fn view(&self) -> &NSView {
        &self.ivars().root
    }

    /// The item being played.
    pub fn item(&self) -> Rc<Item> {
        Rc::clone(&self.ivars().item.borrow())
    }

    /// Hands the item's file to the player: the file on this Mac if the
    /// item was downloaded, and the one on the server if not. It starts
    /// at `from`, or with `None` where the viewer left it the last time.
    fn load(&self, from: Option<Duration>) -> Result<()> {
        let (ivars, item) = (self.ivars(), self.item());
        let mut source = match (ivars.downloads.holding(&item), &ivars.server) {
            (Some(download), _) => source::on_disk(&download, &item),
            (None, Some(server)) => source::of(server, &item)?,
            (None, None) => return Err(Error::NotPlayable(item.title.clone())),
        };
        if let Some(from) = from {
            source.start = from;
        }
        ivars.controls.waiting.show(None);
        // The player knows neither time until the file is open, which can
        // take a while. The item knows both now, so the controls never show
        // a film that is being resumed as if it started over.
        if let Some(duration) = item.duration {
            self.show_progress(source.start, duration.into());
        }
        ivars.player.load(&source)?;
        *ivars.sidecars.borrow_mut() = source.sidecars;
        ivars.phase.set(Phase::Opening);
        self.set_zoom(Zoom::FIT)?;
        // An episode that takes over from a paused one plays.
        self.set_paused(false)?;
        Ok(())
    }

    /// What the viewer has set, to be used again next time. `None` for a
    /// check, which must leave no trace.
    pub fn settings(&self) -> player::Result<Option<Settings>> {
        if self.ivars().purpose == Purpose::Check {
            return Ok(None);
        }
        Ok(Some(Settings {
            speed_percent: self.speed().percent(),
            volume: self.ivars().player.volume()?,
            low_power: self.ivars().low_power.get(),
        }))
    }

    /// Turns low power on or off for what is playing, from its next frame.
    pub fn set_low_power(&self, low_power: bool) {
        let ivars = self.ivars();
        self.or_log(ivars.player.set_low_power(low_power));
        ivars.low_power.set(low_power);
        let text = if low_power { "Low power on" } else { "Low power off" };
        self.notify(&Notice { symbol: "leaf.fill", text, level: None });
    }

    fn speed(&self) -> Speed {
        self.ivars().speed.get()
    }

    /// Directs the keyboard at the player and gives the video the whole
    /// window. Call once the view is in a window.
    pub fn focus(&self) {
        let ivars = self.ivars();
        if let Some(window) = ivars.root.window() {
            window.makeFirstResponder(Some(&ivars.root));
            let mut chrome = ivars.chrome.borrow_mut();
            if chrome.is_none() {
                *chrome = Some(Chrome::clear(&window));
            }
        }
        self.fit_to_display();
    }

    fn fit_to_display(&self) {
        let ivars = self.ivars();
        self.or_log(video_layer::fit_to_display(&ivars.video, &ivars.player));
    }

    /// Stops playback and gives the screen up. Safe to call more than once.
    pub fn close(&self) {
        if !self.stop_ticking() {
            return;
        }
        // `on_close` may drop the last other reference to this screen.
        let _alive = self.retain();
        // Nothing is reported after a failure: mpv has no position then,
        // and a report without one gives the end of the file, which the
        // server would mark as watched.
        let stopped = self.progress(PlaybackState::Stopped);
        self.or_log(self.ivars().player.quit());
        (self.ivars().on_close)();

        let (stopped, server) = match (stopped, self.ivars().server.clone()) {
            (Some(stopped), Some(server)) => (stopped, server),
            // With no server there, the place is kept for it on this Mac.
            (Some(stopped), None) => {
                self.keep(&stopped);
                return (self.ivars().on_saved)();
            }
            (None, _) => return (self.ivars().on_saved)(),
        };
        // The window is back with the browsing screens while this is on
        // its way. The screen is kept until it has arrived, for the sake
        // of `on_saved`.
        let screen = self.retain();
        self.send(stopped, server, move || (screen.ivars().on_saved)());
    }

    /// Stops on a failure and says so, with `reason` in the player's own
    /// words. The screen stays: a film that fails an hour in, because the
    /// network dropped for a moment, is one press away from playing on.
    fn fail(&self, reason: &str) {
        // The failure is said, and the way on offered, in the full window.
        self.leave_mini();
        let ivars = self.ivars();
        ivars.phase.set(Phase::Failed);
        self.keep_display_awake(false);
        ivars.up_next.set(UpNext::Waiting);
        let controls = &ivars.controls;
        controls.waiting.hide();
        controls.skip.setHidden(true);
        controls.up_next.hide();
        self.close_offer();
        controls.failure.show(&self.item().title, reason);
        // The title and the way out stay. The bar steers a file that is
        // no longer there.
        self.show_controls(true);
        controls.bottom.set_shown(false);
        controls.bubble.hide();
        self.show_neighbours();
    }

    /// Plays the file again from where it stopped.
    fn retry(&self) {
        let ivars = self.ivars();
        ivars.controls.failure.hide();
        ivars.controls.bottom.set_shown(ivars.controls_shown.get());
        if let Err(error) = self.load(ivars.last_position.get()) {
            self.fail(&with_causes(&error));
        }
    }

    /// Puts away what is on top: the panel above the bar, the keyboard's
    /// place among the buttons, the panel of playback information, the
    /// offer of the next episode, then full screen, as Escape does
    /// everywhere, and then the player.
    fn back_out(&self) {
        if self.ivars().offer.get().is_some() {
            self.close_offer();
        } else if self.focused().is_some() {
            self.drop_focus();
        } else if self.ivars().controls.info.is_shown() {
            self.ivars().controls.info.set_shown(false);
        } else if self.ivars().up_next.get() == UpNext::Counting {
            self.turn_down_next_episode();
        } else if self.is_full_screen() {
            self.toggle_full_screen();
        } else {
            self.close();
        }
    }

    /// The buttons the keyboard can be put on right now, in the order
    /// they are read in. While the panel above the bar is open they are
    /// the panel's: what is behind it waits.
    fn focusable(&self) -> Vec<Retained<Chip>> {
        let controls = &self.ivars().controls;
        if self.ivars().offer.get().is_some() {
            return controls.panel.chips();
        }
        let chips: [&Retained<Chip>; 19] = [
            &controls.back,
            &controls.failure.retry,
            &controls.failure.leave,
            &controls.skip,
            &controls.up_next.play,
            &controls.up_next.cancel,
            &controls.remaining,
            &controls.mute,
            &controls.previous,
            &controls.rewind,
            &controls.toggle,
            &controls.forward,
            &controls.next,
            &controls.chapters,
            &controls.speed,
            &controls.audio,
            &controls.subtitles,
            &controls.mini,
            &controls.full_screen,
        ];
        chips.into_iter().filter(|chip| chip.can_be_pressed()).cloned().collect()
    }

    /// The button the keyboard is on, if it is on one.
    fn focused(&self) -> Option<Retained<Chip>> {
        self.focusable().into_iter().find(|chip| chip.is_focused())
    }

    /// Moves the keyboard to the next button, or the one before. Past the
    /// last, and before the first, it is back on the video, where every
    /// key means what it always does.
    ///
    /// The controls come up for it and stay while the keyboard is on one
    /// of them, so it is never on a button that cannot be seen.
    fn move_focus(&self, forward: bool) {
        self.show_controls(true);
        let chips = self.focusable();
        let current = chips.iter().position(|chip| chip.is_focused());
        let next = match (current, forward) {
            (None, true) => chips.first(),
            (None, false) => chips.last(),
            (Some(index), true) => chips.get(index + 1),
            (Some(index), false) => index.checked_sub(1).and_then(|index| chips.get(index)),
        };
        match next {
            Some(chip) => chip.take_keys(),
            None => self.drop_focus(),
        }
    }

    /// Puts the keyboard back on the video.
    fn drop_focus(&self) {
        let root = &self.ivars().root;
        if let Some(window) = root.window() {
            window.makeFirstResponder(Some(root));
        }
    }

    /// Does to the picture what fingers asked for, and says how large it
    /// now is.
    fn handle_gesture(&self, gesture: Gesture) -> player::Result<()> {
        let point = |place: NSPoint| zoom::Point { x: place.x, y: place.y };
        self.zoom_by(|zoom, view, picture| match gesture {
            Gesture::Pinch { factor, from, to } => {
                zoom.pinched(factor, point(from), point(to), view, picture)
            }
            Gesture::Pan(by) => {
                zoom.panned(zoom::Point { x: by.width, y: by.height }, view, picture)
            }
            Gesture::Toggle if zoom.is_fit() => Zoom::fill(view, picture),
            Gesture::Toggle => Zoom::FIT,
        })?;
        // A drag changes where, not how large, which is all the badge
        // has to say.
        if !matches!(gesture, Gesture::Pan(_)) {
            self.notify_of_zoom();
        }
        Ok(())
    }

    /// Makes the picture `factor` times the size it is, about the middle
    /// of the view.
    fn step_zoom(&self, factor: f64) -> player::Result<()> {
        let size = self.ivars().root.bounds().size;
        let middle = NSPoint::new(size.width / 2.0, size.height / 2.0);
        self.handle_gesture(Gesture::Pinch { factor, from: middle, to: middle })
    }

    /// Changes the zoom to what `change` makes of it, given the sizes of
    /// the view and of the picture. Without a picture there is nothing to
    /// change.
    fn zoom_by(
        &self,
        change: impl FnOnce(Zoom, zoom::Size, zoom::Size) -> Zoom,
    ) -> player::Result<()> {
        let ivars = self.ivars();
        let Some((width, height)) = ivars.player.picture_size()? else {
            return Ok(());
        };
        if ivars.phase.get() != Phase::Playing {
            return Ok(());
        }
        let bounds = ivars.root.bounds().size;
        let view = zoom::Size { width: bounds.width, height: bounds.height };
        let zoom = change(ivars.zoom.get(), view, zoom::Size { width, height });
        if zoom == ivars.zoom.get() {
            return Ok(());
        }
        self.set_zoom(zoom)
    }

    fn set_zoom(&self, zoom: Zoom) -> player::Result<()> {
        let ivars = self.ivars();
        ivars.zoom.set(zoom);
        ivars.root.set_pannable(!zoom.is_fit());
        let (doublings, pan) = zoom.for_player();
        ivars.player.place_picture(doublings, pan, !zoom.is_fit())
    }

    /// Says how large the picture is.
    fn notify_of_zoom(&self) {
        let zoom = self.ivars().zoom.get();
        let (symbol, text) = if zoom.is_fit() {
            ("minus.magnifyingglass", "Fit".to_owned())
        } else {
            // Two digits: filling a 16 to 9 window with a 1.85 to 1 film
            // is 1.04 times, which one digit would call no change.
            ("plus.magnifyingglass", format!("{:.2}\u{d7}", zoom.scale()))
        };
        self.notify(&Notice { symbol, text: &text, level: Some(zoom.level()) });
    }

    /// Shows the panel of playback information, or puts it away.
    fn toggle_info(&self) -> player::Result<()> {
        let ivars = self.ivars();
        let info = &ivars.controls.info;
        info.set_shown(!info.is_shown());
        info.refresh(&ivars.player)
    }

    /// Switches the time right of the scrubber between what is left and
    /// the full length.
    fn switch_time_shown(&self) -> player::Result<()> {
        let shows_length = &self.ivars().shows_length;
        shows_length.set(!shows_length.get());
        self.refresh()
    }

    /// Where the player is in the file, which is also kept for when it no
    /// longer knows: after a failure.
    fn position(&self) -> player::Result<Option<Duration>> {
        let position = self.ivars().player.position()?;
        if position.is_some() {
            self.ivars().last_position.set(position);
        }
        Ok(position)
    }

    /// Like [`close`](Self::close), for when the app is quitting: it waits
    /// until the server has the final position, because nothing that is
    /// still on its way when the process exits will arrive.
    pub fn close_for_quit(&self) {
        if !self.stop_ticking() {
            return;
        }
        let ivars = self.ivars();
        match (self.progress(PlaybackState::Stopped), &ivars.server) {
            (Some(progress), Some(server)) => {
                log_unsaved(ivars.background.wait(progress.send(server)));
            }
            (Some(progress), None) => self.keep(&progress),
            (None, _) => {}
        }
        self.or_log(self.ivars().player.quit());
    }

    /// Stops the timer and hands the window back as it was found. Returns
    /// whether the screen was still open.
    fn stop_ticking(&self) -> bool {
        let ivars = self.ivars();
        let Some(timer) = ivars.timer.borrow_mut().take() else {
            return false;
        };
        timer.invalidate();
        if let Click::Waiting(wait) = ivars.click.replace(Click::Settled) {
            wait.invalidate();
        }
        if let Some(remote) = ivars.remote.borrow_mut().take() {
            remote.release();
        }
        self.keep_display_awake(false);
        NSCursor::setHiddenUntilMouseMoves(false);
        self.forget_full_screen();
        self.put_back();
        if let (Some(chrome), Some(window)) =
            (ivars.chrome.borrow_mut().take(), ivars.root.window())
        {
            chrome.restore(&window);
        }
        true
    }

    /// Holds off the display's sleep, or stops holding it off. Nobody
    /// touches the keyboard during a film, which to the system looks like
    /// nobody being there. A check holds nothing: it must leave no trace.
    fn keep_display_awake(&self, awake: bool) {
        let mut held = self.ivars().awake.borrow_mut();
        let process = NSProcessInfo::processInfo();
        match (held.take(), awake && self.ivars().purpose == Purpose::Watching) {
            (None, true) => {
                *held = Some(process.beginActivityWithOptions_reason(
                    NSActivityOptions::UserInitiated | NSActivityOptions::IdleDisplaySleepDisabled,
                    ns_string!("Playing video"),
                ));
            }
            (Some(activity), true) => *held = Some(activity),
            // SAFETY: the activity is the one `beginActivity` returned.
            (Some(activity), false) => unsafe { process.endActivity(&activity) },
            (None, false) => {}
        }
    }

    /// Takes the system's media controls for this playback, with a next
    /// and a previous item if it `steps` through a queue, and fetches the
    /// poster they show next to its title.
    fn take_media_controls(&self, steps: bool) {
        let screen = Weak::new(self);
        let remote = Remote::register(self.mtm(), steps, LONG_JUMP, move |command| {
            if let Some(screen) = screen.load() {
                screen.obey(command);
            }
        });
        *self.ivars().remote.borrow_mut() = Some(remote);
        self.show_poster();
    }

    /// Fetches the poster the system's media controls show next to the
    /// title of what plays: its show's, for an episode. A shuffle moves
    /// from show to show and film to film, so each item fetches its own.
    fn show_poster(&self) {
        let item = self.item();
        let poster = item.grandparent_thumb.as_deref().or(item.thumb.as_deref());
        let (Some(poster), Some(images)) = (poster, self.ivars().images.as_ref()) else {
            return;
        };
        // The size the item's own page shows it at, so it is rarely fetched.
        let (width, height) = crate::ui::theme::HERO_POSTER_REQUEST;
        let scale = crate::ui::theme::RETINA_SCALE;
        let screen = Weak::new(self);
        let wanted = item.rating_key.clone();
        images.load(poster, width * scale, height * scale, move |artwork| {
            let Some(screen) = screen.load() else {
                return;
            };
            // The viewer may have gone on to another item meanwhile.
            if screen.item().rating_key != wanted {
                return;
            }
            if let Some(remote) = screen.ivars().remote.borrow().as_ref() {
                remote.set_artwork(artwork);
            }
            screen.or_log(screen.show_now_playing());
        });
    }

    /// Does what the viewer asked for through the media controls.
    fn obey(&self, command: Command) {
        // What the player holds for now is the episode before this one,
        // which is not there to be steered.
        if matches!(self.ivars().phase.get(), Phase::Fetching | Phase::Failed) {
            return;
        }
        let result = match command {
            Command::Play => self.set_paused(false),
            Command::Pause => self.set_paused(true),
            Command::TogglePause => self.toggle_pause(),
            Command::Next => {
                self.go_to_next();
                Ok(())
            }
            Command::Previous => self.go_to_previous(),
            Command::Skip(seconds) => self.jump(seconds, Press::First),
            Command::Seek(position) => self.ivars().player.seek(position),
        };
        self.or_log(result);
    }

    /// Tells the system's media controls where playback stands. They count
    /// the position on by themselves, so they are told only when it jumps
    /// or changes pace, and every ten seconds to correct what has drifted.
    fn show_now_playing(&self) -> player::Result<()> {
        let ivars = self.ivars();
        let remote = ivars.remote.borrow();
        let Some(remote) = remote.as_ref() else {
            return Ok(());
        };
        // Until the file is open the player's times are not this item's.
        if ivars.phase.get() != Phase::Playing {
            return Ok(());
        }
        let player = &ivars.player;
        let (Some(position), Some(duration)) = (player.position()?, player.duration()?) else {
            return Ok(());
        };
        let rate = if player.is_paused()? { 0.0 } else { self.speed().factor() };
        remote.show(&Playing { item: &self.item(), position, duration, rate });
        Ok(())
    }

    /// Says what each control does.
    fn connect_controls(&self) {
        let controls = &self.ivars().controls;
        self.on_click(&controls.back, Self::close);
        self.on_click(&controls.failure.retry, Self::retry);
        self.on_click(&controls.failure.leave, Self::close);
        self.on_click(&controls.toggle, |screen| screen.or_log(screen.toggle_pause()));
        self.on_click(&controls.rewind, |screen| {
            screen.or_log(screen.jump(-LONG_JUMP, Press::First))
        });
        self.on_click(&controls.forward, |screen| {
            screen.or_log(screen.jump(LONG_JUMP, Press::First))
        });
        self.on_click(&controls.previous, |screen| screen.or_log(screen.go_to_previous()));
        self.on_click(&controls.next, |screen| screen.go_to_next());
        self.on_click(&controls.mute, |screen| screen.or_log(screen.toggle_mute()));
        self.on_click(&controls.remaining, |screen| screen.or_log(screen.switch_time_shown()));
        // Pressed with the keyboard on it, a button hands the keyboard on
        // to what it opens.
        let offers = [
            (&controls.chapters, Offer::Chapters),
            (&controls.speed, Offer::Speed),
            (&controls.audio, Offer::Audio),
            (&controls.subtitles, Offer::Subtitles),
        ];
        for (chip, offer) in offers {
            self.on_click(chip, move |screen| {
                screen.toggle_offer(offer, screen.focused().is_some());
            });
        }
        self.on_click(&controls.mini, Self::enter_mini);
        self.on_click(&controls.full_screen, |screen| screen.toggle_full_screen());
        self.on_click(&controls.skip, |screen| screen.or_log(screen.skip_marker()));
        self.on_click(&controls.up_next.play, |screen| screen.go_to_next());
        self.on_click(&controls.up_next.cancel, Self::turn_down_next_episode);

        let (scrubbed, pointed, volume) = (Weak::new(self), Weak::new(self), Weak::new(self));
        controls.scrubber.listen(
            move |seconds, grip| {
                if let Some(screen) = scrubbed.load() {
                    screen.or_log(screen.scrub_to(seconds, grip));
                }
            },
            move |place| {
                if let Some(screen) = pointed.load() {
                    screen.show_bubble(place);
                }
            },
        );
        controls.volume.listen(
            move |level, _| {
                if let Some(screen) = volume.load() {
                    screen.or_log(screen.ivars().player.set_volume(level));
                }
            },
            |_| {},
        );
    }

    /// Makes a click on `chip` call `act` with the screen.
    fn on_click(&self, chip: &Chip, act: impl Fn(&Self) + 'static) {
        let screen = Weak::new(self);
        chip.on_click(move || {
            if let Some(screen) = screen.load() {
                act(&screen);
            }
        });
    }

    /// Connects the keyboard and the mouse to the screen.
    fn listen(&self) {
        let (keys, pointer, clicks, gestures, fit) =
            (Weak::new(self), Weak::new(self), Weak::new(self), Weak::new(self), Weak::new(self));
        self.ivars().root.listen(
            move |key, press| keys.load().is_some_and(|screen| screen.act_on(key, press)),
            move |place| {
                if let Some(screen) = pointer.load() {
                    screen.pointer_moved(place);
                }
            },
            move |count| {
                if let Some(screen) = clicks.load() {
                    screen.clicked(count);
                }
            },
            move |gesture| {
                if let Some(screen) = gestures.load() {
                    screen.or_log(screen.handle_gesture(gesture));
                }
            },
            move || {
                if let Some(screen) = fit.load() {
                    screen.fit_to_display();
                    // In a view of another size the same zoom can leave
                    // an edge of the picture inside it.
                    screen
                        .or_log(screen.zoom_by(|zoom, view, picture| zoom.kept_in(view, picture)));
                }
            },
        );
    }

    /// Acts on a click or a tap on the picture: one pauses or resumes, two
    /// change the size of the window or of the picture.
    ///
    /// A click waits to see whether a second follows, so that a double
    /// click does not pause the film for the moment between its two
    /// clicks. A tap acts at once, because a finger on the glass expects
    /// the picture to answer it, and is taken back if a second follows.
    fn clicked(&self, clicks: Clicks) {
        // With the panel open, a click beside it is how it is put away.
        if self.ivars().offer.get().is_some() {
            return self.close_offer();
        }
        match clicks {
            Clicks::Single => self.wait_for_second_click(),
            Clicks::Tap => self.act_on_click(),
            Clicks::Double => {
                self.take_back_click();
                self.toggle_full_screen();
            }
            // A finger has no second pair to tap with, as a trackpad has
            // for this, and a screen is where a picture gets pinched:
            // there two taps fill the view.
            Clicks::DoubleTap => {
                self.take_back_click();
                self.or_log(self.handle_gesture(Gesture::Toggle));
            }
        }
    }

    /// Puts a click's pause off until [`theme::CLICK_WAIT`] has passed
    /// without another click, or the whole of the system's double click
    /// interval if the viewer has set that shorter. A second click that
    /// comes later than the wait and still inside the interval finds the
    /// pause made, and takes it back.
    fn wait_for_second_click(&self) {
        // A click that was still waiting when this one came was not half
        // of a double click: the system counted this one as a first.
        if matches!(*self.ivars().click.borrow(), Click::Waiting(_)) {
            self.act_on_click();
        }
        let wait = theme::CLICK_WAIT.min(NSEvent::doubleClickInterval());
        // SAFETY: `clickWaited:` is defined above. The timer retains the
        // screen until it fires or is invalidated.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                wait,
                self,
                sel!(clickWaited:),
                None,
                false,
            )
        };
        // In the common modes, as the screen's own timer is and for the
        // same reason.
        // SAFETY: the mode is a constant that Foundation provides.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
        self.ivars().click.replace(Click::Waiting(timer));
        // The controls answer the click at once, which the pause cannot.
        self.show_controls(true);
    }

    /// Pauses or resumes for a click that no second one followed.
    fn act_on_click(&self) {
        let ivars = self.ivars();
        if let Click::Waiting(wait) = ivars.click.replace(Click::Acted) {
            wait.invalidate();
        }
        // The file may have ended, and the screen with it, during the wait.
        if ivars.timer.borrow().is_some() {
            self.or_log(self.toggle_pause());
        }
    }

    /// Makes a first click one that changed nothing, now that a second
    /// has followed it.
    fn take_back_click(&self) {
        match self.ivars().click.replace(Click::Settled) {
            Click::Waiting(wait) => wait.invalidate(),
            Click::Acted => self.or_log(self.toggle_pause()),
            Click::Settled => {}
        }
    }

    /// Acts on a key going down. Returns whether the key means anything
    /// here.
    ///
    /// With the keyboard on a button, Space and Return go to that button
    /// as key events, as they do from a real keyboard.
    pub fn press(&self, key: Key) -> bool {
        let code = match key {
            Key::Space => Some(input::code::SPACE),
            Key::Return => Some(input::code::RETURN),
            _ => None,
        };
        match self.focused().zip(code) {
            Some((chip, code)) => stage::press_key(&chip, code),
            None => return self.act_on(key, Press::First),
        }
        true
    }

    fn act_on(&self, key: Key, press: Press) -> bool {
        self.wake();
        let in_mini = self.ivars().mini.borrow().is_some();
        if in_mini {
            return self.act_in_mini(key, press);
        }
        // A file that has failed has two ways on, and the size of the
        // window. Every other key is for a file that plays.
        if self.ivars().phase.get() == Phase::Failed {
            match (key, press) {
                (Key::Return, Press::First) => self.retry(),
                (Key::Escape, Press::First) => self.back_out(),
                (Key::Tab, _) => self.move_focus(true),
                (Key::BackTab, _) => self.move_focus(false),
                (Key::Character('f'), Press::First) => self.toggle_full_screen(),
                _ => {}
            }
            return true;
        }
        if press == Press::Held && !input::repeats(key) {
            return true;
        }
        // Changing the volume, the speed or a track says what it did in a
        // notice. Bringing every control up for it as well would cover
        // the picture each time the viewer nudges the speed.
        let reveals = !matches!(
            key,
            Key::Up
                | Key::Down
                | Key::ZoomIn
                | Key::ZoomOut
                | Key::ZoomFit
                | Key::Character(
                    'm' | 's'
                        | 'd'
                        | 'r'
                        | 'a'
                        | 'c'
                        | 'f'
                        | 'o'
                        | 'z'
                        | 'i'
                        | '-'
                        | '='
                        | '['
                        | ']'
                )
        );
        let steps = steps_through(&self.item(), self.ivars().queue.borrow().as_ref());
        let result = match key {
            // In an open list the arrows go from row to row.
            Key::Up | Key::Down if self.ivars().offer.get().is_some() => {
                self.move_focus(key == Key::Down);
                return true;
            }
            Key::Space | Key::Character('k') => self.toggle_pause(),
            Key::Left => self.jump(-SHORT_JUMP, press),
            Key::Right => self.jump(SHORT_JUMP, press),
            Key::ShiftLeft => self.jump(-LONG_JUMP, press),
            Key::ShiftRight => self.jump(LONG_JUMP, press),
            Key::Up => self.change_volume(VOLUME_STEP),
            Key::Down => self.change_volume(-VOLUME_STEP),
            Key::Character('m') => self.toggle_mute(),
            Key::Character('s') => self.set_speed(self.speed().slower()),
            Key::Character('d') => self.set_speed(self.speed().faster()),
            Key::Character('r') => self.set_speed(Speed::NORMAL),
            Key::Character('a') => self.next_track(TrackKind::Audio),
            Key::Character('c') => self.next_track(TrackKind::Subtitles),
            Key::Return => self.skip_marker(),
            Key::Character('t') => self.switch_time_shown(),
            Key::Character('i') => self.toggle_info(),
            Key::Character('z') => self.handle_gesture(Gesture::Toggle),
            Key::Character('l') => {
                self.toggle_offer(Offer::Chapters, true);
                return true;
            }
            Key::Character(',') => self.step_chapter(Step::Down),
            Key::Character('.') => self.step_chapter(Step::Up),
            Key::Character('-') => self.step_subtitle_size(Step::Down),
            Key::Character('=') => self.step_subtitle_size(Step::Up),
            Key::Character('[') => self.step_subtitle_delay(Step::Down),
            Key::Character(']') => self.step_subtitle_delay(Step::Up),
            Key::ZoomIn => self.step_zoom(theme::ZOOM_STEP),
            Key::ZoomOut => self.step_zoom(1.0 / theme::ZOOM_STEP),
            Key::ZoomFit => self.set_zoom(Zoom::FIT).map(|()| self.notify_of_zoom()),
            Key::Scene(Scene::Pinch) => {
                let size = self.ivars().root.bounds().size;
                let at = NSPoint::new(size.width * 0.75, size.height * 0.75);
                self.handle_gesture(Gesture::Pinch { factor: 2.0, from: at, to: at })
            }
            Key::Tab | Key::BackTab => {
                self.move_focus(key == Key::Tab);
                return true;
            }
            Key::Character('f') => {
                self.toggle_full_screen();
                Ok(())
            }
            Key::Character('o') => {
                self.enter_mini();
                Ok(())
            }
            Key::Character('n') if steps => {
                self.go_to_next();
                return true;
            }
            Key::Character('p') if steps => self.go_to_previous(),
            Key::Escape => {
                self.back_out();
                return true;
            }
            // What only the player itself brings about, sent the way the
            // player sends it.
            Key::Scene(Scene::Stalled) => {
                self.handle(&Event::Buffering(true));
                return true;
            }
            Key::Scene(Scene::Failed) => {
                self.handle(&Event::Ended(EndReason::Failed("loading failed".to_owned())));
                return true;
            }
            Key::Idle => {
                self.close_offer();
                self.show_controls(false);
                return true;
            }
            Key::NearEnd => {
                let player = &self.ivars().player;
                player.seek(self.length_shown().saturating_sub(UP_NEXT_LEAD / 2))
            }
            Key::Scene(scene) => {
                let ivars = self.ivars();
                stage::set(scene, &ivars.root, &ivars.controls, &ivars.player, &self.item())
            }
            // The script that presses the keys does the waiting.
            Key::Wait => return true,
            Key::Character(_) => return false,
        };
        self.or_log(result);
        // The key may have closed the screen, which then has no controls
        // left to bring up.
        if self.ivars().timer.borrow().is_none() {
            return true;
        }
        // A hand on the keyboard is not on the mouse: wherever the pointer
        // was left, it is no longer holding the controls up.
        self.ivars().pointer_on_controls.set(false);
        if reveals {
            self.show_controls(true);
        }
        true
    }

    /// Hears where the pointer has moved to in the screen's view, or that
    /// it has left the view.
    fn pointer_moved(&self, place: Option<NSPoint>) {
        self.wake();
        let controls = &self.ivars().controls;
        let on_controls = place.is_some_and(|place| {
            controls.top.holds(place) || controls.bottom.holds(place) || controls.panel.holds(place)
        });
        self.ivars().pointer_on_controls.set(on_controls);
        if place.is_some() {
            self.show_controls(true);
        }
    }

    fn on_tick(&self) -> player::Result<()> {
        let ivars = self.ivars();
        let ticks = ivars.ticks.get() + 1;
        ivars.ticks.set(ticks);
        if ticks.is_multiple_of(TICKS_PER_REPORT) {
            self.report_current_state();
            self.or_log(self.show_now_playing());
        }

        // Once a second is as often as its numbers are worth reading.
        if ivars.controls.info.is_shown() && ticks.is_multiple_of(TICKS_PER_SECOND) {
            ivars.controls.info.refresh(&ivars.player)?;
        }

        let notice_left = ivars.ticks_of_notice_left.get();
        if notice_left > 0 {
            ivars.ticks_of_notice_left.set(notice_left - 1);
            if notice_left == 1 {
                ivars.controls.badge.hide();
            }
        }

        // While the controls are hidden the position is read only for an
        // intro or credits that could be skipped and for the end of an
        // episode that another follows, and then once a second. A running
        // countdown is kept up with on every tick.
        let shown = ivars.controls_shown.get();
        let counting = ivars.up_next.get() == UpNext::Counting;
        let watches_position = !self.item().markers.is_empty() || self.next_episode_is_known();
        let reads_position =
            shown || counting || (watches_position && ticks.is_multiple_of(TICKS_PER_SECOND));
        // Until the file is open the player's position is not this item's.
        if reads_position && ivars.phase.get() == Phase::Playing {
            let position = self.position()?;
            self.offer_next_episode(position);
            // The countdown running out moves on to the next episode, and
            // this position is then no longer the one to act on.
            if ivars.phase.get() == Phase::Playing {
                self.offer_skip(position);
            }
        }
        if !shown {
            return Ok(());
        }
        self.refresh()?;
        let idle = ivars.ticks_idle.get() + 1;
        ivars.ticks_idle.set(idle);
        // A pointer that is on the controls keeps them, and so does the
        // keyboard.
        let in_use = ivars.pointer_on_controls.get()
            || self.focused().is_some()
            || ivars.controls.scrubber.is_dragging()
            || ivars.offer.get().is_some();
        if idle >= TICKS_UNTIL_HIDDEN && !in_use {
            self.show_controls(false);
        }
        Ok(())
    }

    /// Puts the timer off for as long as nothing can change: the film has
    /// failed, or is paused with the bar away, and no notice is waiting to
    /// be taken down.
    /// Anything that happens next wakes it.
    fn rest_if_still(&self) -> player::Result<()> {
        let ivars = self.ivars();
        let paused_and_bare = ivars.phase.get() == Phase::Playing
            && !ivars.controls_shown.get()
            && ivars.player.is_paused()?;
        let still = ivars.ticks_of_notice_left.get() == 0
            && (paused_and_bare || ivars.phase.get() == Phase::Failed);
        if let (true, Some(timer)) = (still, ivars.timer.borrow().as_ref()) {
            timer.setFireDate(&NSDate::distantFuture());
            ivars.resting.set(true);
        }
        Ok(())
    }

    /// Sets the timer going again after a rest.
    fn wake(&self) {
        let ivars = self.ivars();
        if !ivars.resting.replace(false) {
            return;
        }
        if let Some(timer) = ivars.timer.borrow().as_ref() {
            timer.setFireDate(&NSDate::dateWithTimeIntervalSinceNow(TICK.as_secs_f64()));
        }
    }

    /// Asks the server for the episodes of the show being played, to know
    /// what comes before and after each. All of them, and not the season
    /// alone: the last episode of a season is followed by the first of
    /// the next. A shuffle given on opening has its own order already.
    fn list_episodes(&self) {
        let (ivars, item) = (self.ivars(), self.item());
        let (Kind::Episode, Some(show)) = (item.kind, item.grandparent_rating_key.clone()) else {
            return;
        };
        if ivars.queue.borrow().is_some() {
            return;
        }
        let Some(server) = ivars.server.clone() else {
            match ivars.downloads.episodes(&show) {
                Ok(episodes) => {
                    *ivars.queue.borrow_mut() = Some(Queue::of_show(episodes, &item.rating_key));
                    self.find_neighbours();
                }
                Err(error) => eprintln!("plex-rust: show not listed: {}", with_causes(&error)),
            }
            return;
        };
        let load = async move { server.episodes(&show).await };
        let screen = Weak::new(self);
        ivars.background.run(self.mtm(), load, move |episodes| {
            let Some(screen) = screen.load() else {
                return;
            };
            match episodes {
                Ok(episodes) => {
                    let queue = Queue::of_show(episodes, &item.rating_key);
                    *screen.ivars().queue.borrow_mut() = Some(queue);
                    screen.find_neighbours();
                }
                // Next and Previous then stay as they are before the answer
                // has come: with nowhere to go.
                Err(error) => eprintln!("plex-rust: show not listed: {}", with_causes(&error)),
            }
        });
    }

    /// Works out which items of the queue are on either side of the one
    /// playing, and offers the next one on the card that comes up as this
    /// one ends.
    fn find_neighbours(&self) {
        let (ivars, item) = (self.ivars(), self.item());
        let neighbours =
            ivars.queue.borrow().as_ref().map(|queue| queue.neighbours(&item.rating_key));
        if let Some(next) = neighbours.as_ref().and_then(|known| known.next.as_ref()) {
            ivars.controls.up_next.offer(next, &item, ivars.images.as_ref());
        }
        *ivars.neighbours.borrow_mut() = neighbours;
        self.show_neighbours();
    }

    /// Lights the buttons for the previous and the next episode when there
    /// is one to go to and the episode playing can be left.
    fn show_neighbours(&self) {
        let ivars = self.ivars();
        let neighbours = ivars.neighbours.borrow();
        let leads_to = |pick: fn(&Neighbours) -> &Option<Item>| {
            ivars.phase.get() == Phase::Playing
                && neighbours.as_ref().is_some_and(|known| pick(known).is_some())
        };
        ivars.controls.previous.set_enabled(leads_to(|known| &known.previous));
        ivars.controls.next.set_enabled(leads_to(|known| &known.next));
    }

    /// Goes back to the start of the episode, or to the episode before it
    /// when this one has hardly begun. With no episode before it, the
    /// start is the only place to go back to.
    fn go_to_previous(&self) -> player::Result<()> {
        let ivars = self.ivars();
        let position = ivars.player.position()?.unwrap_or_default();
        let previous = ivars.neighbours.borrow().as_ref().and_then(|known| known.previous.clone());
        match previous {
            Some(previous) if position <= RESTART_AFTER => self.switch_to(previous),
            _ => {
                ivars.player.seek(Duration::ZERO)?;
                self.notify(&Notice {
                    symbol: "backward.end.fill",
                    text: "From the start",
                    level: None,
                });
            }
        }
        Ok(())
    }

    /// Goes to the next item of the queue, if there is a queue and it has
    /// one.
    fn go_to_next(&self) {
        let next = match &*self.ivars().neighbours.borrow() {
            Some(known) => known.next.clone(),
            None => return,
        };
        match next {
            Some(next) => self.switch_to(next),
            None => {
                let shuffled = self.ivars().queue.borrow().as_ref().is_some_and(Queue::is_shuffled);
                self.notify(&Notice {
                    symbol: "forward.end.fill",
                    text: if shuffled { "End of the shuffle" } else { "Last episode" },
                    level: None,
                });
                self.show_controls(true);
            }
        }
    }

    /// Plays `episode` in place of the one playing. The screen stays as it
    /// is, with its window, speed, volume and controls: only what plays
    /// changes.
    ///
    /// `episode` comes from a listing, which is enough to name it at once.
    /// Its file follows when the server has sent the episode in full: a
    /// listing leaves out the markers and the tracks. Without the server
    /// `episode` is a download, which was kept in full.
    fn switch_to(&self, episode: Item) {
        let ivars = self.ivars();
        // An episode that is itself still on its way cannot be left yet:
        // what the player would say next could be about either of the two.
        if ivars.phase.get() != Phase::Playing {
            return;
        }
        self.report(PlaybackState::Stopped);
        ivars.phase.set(Phase::Fetching);
        // Holds the last picture still under the new name.
        self.or_log(ivars.player.set_paused(true));

        let key = episode.rating_key.clone();
        self.show_item(episode);
        let Some(server) = ivars.server.clone() else {
            if let Err(error) = self.load(None) {
                self.fail(&with_causes(&error));
            }
            return;
        };
        let fetch = async move { server.item(&key).await };
        let screen = Weak::new(self);
        ivars.background.run(self.mtm(), fetch, move |item| {
            let Some(screen) = screen.load() else {
                return;
            };
            // The viewer may have left while the server was answering.
            if screen.ivars().timer.borrow().is_none() {
                return;
            }
            let loaded = item.map_err(Into::into).and_then(|item| {
                *screen.ivars().item.borrow_mut() = Rc::new(item);
                screen.load(None)
            });
            if let Err(error) = loaded {
                screen.fail(&with_causes(&error));
            }
        });
    }

    /// Makes the controls, and the system's, those of `item`, with nothing
    /// left on them of what played before it.
    fn show_item(&self, item: Item) {
        let ivars = self.ivars();
        let controls = &ivars.controls;
        controls.set_heading(&item, None);
        let start = item.view_offset.map(Into::into).unwrap_or_default();
        let duration = item.duration.map(Into::into).unwrap_or_default();
        self.show_progress(start, duration);
        controls.scrubber.set_loaded(0.0);
        controls.scrubber.set_marks(Vec::new());
        controls.chapters.setHidden(true);
        self.close_offer();
        controls.bubble.hide();
        controls.skip.setHidden(true);
        controls.up_next.hide();
        ivars.chapters.borrow_mut().clear();
        ivars.up_next.set(UpNext::Waiting);
        ivars.duration.set(None);
        ivars.last_position.set(None);
        ivars.ticks_of_scrub_settling_left.set(0);
        // The media controls name the episode now too, standing still at
        // its start until its file plays.
        if let Some(remote) = ivars.remote.borrow().as_ref() {
            remote.show(&Playing { item: &item, position: start, duration, rate: 0.0 });
        }
        *ivars.item.borrow_mut() = Rc::new(item);
        if ivars.remote.borrow().is_some() {
            self.show_poster();
        }
        ivars.controls.waiting.show(None);
        self.find_neighbours();
        self.show_controls(true);
    }

    /// Jumps `seconds` forward, or back if negative.
    fn jump(&self, seconds: f64, press: Press) -> player::Result<()> {
        let player = &self.ivars().player;
        match press {
            Press::First => {
                player.seek_by(seconds)?;
                let symbol = if seconds < 0.0 { "gobackward" } else { "goforward" };
                let text = format!("{} s", seconds.abs());
                self.notify(&Notice { symbol, text: &text, level: None });
            }
            // An exact jump takes longer than the gap between repeats, so a
            // held key would stand still. Keyframe to keyframe keeps up.
            Press::Held => {
                player.scan_by(seconds)?;
                // Where a held key has got to says more than the size of
                // one step, repeated for as long as the key is down.
                if let Some(position) = player.position()? {
                    let symbol = if seconds < 0.0 { "backward.fill" } else { "forward.fill" };
                    self.notify(&Notice { symbol, text: &clock(position), level: None });
                    self.show_progress(position, self.length_shown());
                }
            }
        }
        Ok(())
    }

    /// Goes to `seconds` into the file, as the viewer drags the scrubber
    /// or clicks on it. While the knob is held the picture follows it from
    /// keyframe to keyframe, which is fast enough to keep up with a hand.
    /// Letting go lands on the exact place.
    fn scrub_to(&self, seconds: f64, grip: Grip) -> player::Result<()> {
        let ivars = self.ivars();
        let position = Duration::from_secs_f64(seconds.max(0.0));
        // The times follow the knob at once, not the player once it has
        // caught up.
        self.show_progress(position, self.length_shown());
        match grip {
            Grip::Held => ivars.player.scan_to(position),
            Grip::Released => {
                ivars.ticks_of_scrub_settling_left.set(TICKS_OF_SCRUB_SETTLING);
                ivars.player.seek(position)
            }
        }
    }

    /// The length of the file as the scrubber has it, which saves asking
    /// the player on every move of the pointer.
    fn length_shown(&self) -> Duration {
        Duration::from_secs_f64(self.ivars().controls.scrubber.limit())
    }

    /// Shows the time at `place` on the scrubber, and the chapter it is
    /// in, in a bubble above that place. `None` puts the bubble away.
    fn show_bubble(&self, place: Option<Pointed>) {
        let ivars = self.ivars();
        let controls = &ivars.controls;
        let Some(place) = place else {
            controls.bubble.hide();
            return;
        };
        let time = Duration::from_secs_f64(place.value.max(0.0));
        let chapters = ivars.chapters.borrow();
        let chapter = chapters.iter().rev().find(|chapter| chapter.start <= time);
        let title = chapter.and_then(|chapter| chapter.title.as_deref());
        controls.bubble.show(&clock(time), title.filter(|title| !is_timestamp(title)), place.x);
    }

    fn change_volume(&self, by: f64) -> player::Result<()> {
        let player = &self.ivars().player;
        let volume = (player.volume()? + by).clamp(0.0, controls::FULL_VOLUME);
        player.set_volume(volume)?;
        self.notify_of_volume(volume, player.is_muted()?);
        Ok(())
    }

    fn toggle_mute(&self) -> player::Result<()> {
        // A check stays silent whatever is pressed.
        if self.ivars().purpose == Purpose::Check {
            return Ok(());
        }
        let player = &self.ivars().player;
        let muted = !player.is_muted()?;
        player.set_muted(muted)?;
        self.notify_of_volume(player.volume()?, muted);
        Ok(())
    }

    /// Says how loud the sound now is, in a number and as a level.
    fn notify_of_volume(&self, volume: f64, muted: bool) {
        let level = volume / controls::FULL_VOLUME;
        if muted {
            let symbol = "speaker.slash.fill";
            return self.notify(&Notice { symbol, text: "Muted", level: Some(0.0) });
        }
        let symbol = match level {
            level if level <= 0.0 => "speaker.fill",
            level if level < 0.34 => "speaker.wave.1.fill",
            level if level < 0.67 => "speaker.wave.2.fill",
            _ => "speaker.wave.3.fill",
        };
        self.notify(&Notice { symbol, text: &format!("{volume:.0}%"), level: Some(level) });
    }

    fn set_speed(&self, speed: Speed) -> player::Result<()> {
        self.ivars().player.set_speed(speed.factor())?;
        self.ivars().speed.set(speed);
        self.show_speed();
        self.notify(&Notice {
            symbol: "gauge.with.needle",
            text: &speed.to_string(),
            level: Some(speed.level()),
        });
        self.show_now_playing()
    }

    fn show_speed(&self) {
        self.ivars().controls.speed.set_title(&self.speed().to_string());
    }

    /// Switches to the track after the current one. For subtitles, "off"
    /// comes after the last track.
    fn next_track(&self, kind: TrackKind) -> player::Result<()> {
        let player = &self.ivars().player;
        let tracks: Vec<Track> =
            player.tracks()?.into_iter().filter(|track| track.kind == kind).collect();
        let current = tracks.iter().position(|track| track.selected);
        let next = match current {
            Some(index) => tracks.get(index + 1),
            None => tracks.first(),
        };
        match (kind, next) {
            (TrackKind::Audio, Some(track)) => player.select_audio(track.id)?,
            // Past the last audio track, start again from the first.
            (TrackKind::Audio, None) => match tracks.first() {
                Some(track) => player.select_audio(track.id)?,
                None => return Ok(()),
            },
            (_, track) => player.select_subtitles(track.map(|track| track.id))?,
        }
        let chosen = next.or(tracks.first().filter(|_| kind == TrackKind::Audio));
        let item = self.item();
        let name = chosen.map(|track| tracks::described(track, part(&item)));
        self.remember_track(kind);
        let symbol = if kind == TrackKind::Audio { "waveform" } else { "captions.bubble" };
        self.notify(&Notice { symbol, text: name.as_deref().unwrap_or("Off"), level: None });
        Ok(())
    }

    /// Saves the track of `kind` now in use to the server, so the same
    /// choice is made the next time this file is played, here or in any
    /// other Plex app. Only that kind is touched: changing the audio must
    /// not disturb the subtitles the server remembers.
    fn remember_track(&self, kind: TrackKind) {
        let (ivars, item) = (self.ivars(), self.item());
        let (Some(part), Purpose::Watching, Some(server)) =
            (part(&item), ivars.purpose, &ivars.server)
        else {
            return;
        };
        let tracks = match ivars.player.tracks() {
            Ok(tracks) => tracks,
            Err(error) => return eprintln!("plex-rust: {error}"),
        };
        let selected = tracks.iter().find(|track| track.kind == kind && track.selected);
        let stream = selected.and_then(|track| tracks::stream(track, part)).map(|stream| stream.id);
        let (audio, subtitles) = match (kind, selected, stream) {
            (TrackKind::Audio, _, Some(stream)) => (Some(stream), None),
            (TrackKind::Subtitles, _, Some(stream)) => (None, Some(Some(stream))),
            // No subtitle track is selected: the viewer turned them off.
            (TrackKind::Subtitles, None, None) => (None, Some(None)),
            // A track the server does not list. Saving a guess would be
            // worse than saving nothing.
            _ => return,
        };

        let (server, part) = (server.clone(), part.clone());
        let save = async move { server.choose_streams(&part, audio, subtitles).await };
        ivars.background.run(self.mtm(), save, |saved| {
            if let Err(error) = saved {
                eprintln!("plex-rust: track choice was not saved: {}", with_causes(&error));
            }
        });
    }

    /// Opens the panel above the bar for `offer`, or puts it away if that
    /// is what it is open for. Opened from the `keyboard`, it has the
    /// keyboard on the row in effect, for the arrows to move on from.
    fn toggle_offer(&self, offer: Offer, keyboard: bool) {
        if self.ivars().offer.get() == Some(offer) {
            return self.close_offer();
        }
        self.or_log(self.show_offer(offer));
        if let Some(row) = self.ivars().controls.panel.row_in_effect().filter(|_| keyboard) {
            row.take_keys();
        }
    }

    fn close_offer(&self) {
        let ivars = self.ivars();
        if ivars.offer.take().is_none() {
            return;
        }
        // The keyboard is not left on a button that is no longer there.
        if self.focused().is_none() {
            self.drop_focus();
        }
        ivars.controls.panel.hide();
    }

    /// Fills the panel with what `offer` has to choose from as things
    /// stand, and opens it over the button it belongs to.
    fn show_offer(&self, offer: Offer) -> player::Result<()> {
        let ivars = self.ivars();
        let controls = &ivars.controls;
        // An offer of chapters to a film that has none says nothing.
        if offer == Offer::Chapters && ivars.chapters.borrow().is_empty() {
            return Ok(());
        }
        let (heading, steppers, choices, button) = match offer {
            Offer::Chapters => {
                ("Chapters", Vec::new(), self.chapter_choices()?, &controls.chapters)
            }
            Offer::Speed => (
                "Playback Speed",
                vec![self.speed_stepper()],
                self.speed_choices(),
                &controls.speed,
            ),
            Offer::Audio => {
                ("Audio", Vec::new(), self.track_choices(TrackKind::Audio)?, &controls.audio)
            }
            Offer::Subtitles => (
                "Subtitles",
                self.subtitle_steppers()?,
                self.track_choices(TrackKind::Subtitles)?,
                &controls.subtitles,
            ),
        };
        ivars.offer.set(Some(offer));
        controls.panel.show(heading, steppers, choices, button);
        self.show_controls(true);
        Ok(())
    }

    /// Brings the open panel up to date after one of its buttons changed
    /// what it shows, with the keyboard on the button it was on.
    fn refresh_offer(&self) {
        let Some(offer) = self.ivars().offer.get() else {
            return;
        };
        let panel = &self.ivars().controls.panel;
        let focused = panel.chips().iter().position(|chip| chip.is_focused());
        self.or_log(self.show_offer(offer));
        if let Some(chip) = focused.and_then(|index| panel.chips().get(index).cloned()) {
            chip.take_keys();
        }
    }

    /// A line of the panel that does `act` and puts the panel away.
    fn choice(
        &self,
        title: String,
        chosen: bool,
        act: impl Fn(&Self) -> player::Result<()> + 'static,
    ) -> Choice {
        let screen = Weak::new(self);
        let on_choose = move || {
            if let Some(screen) = screen.load() {
                screen.or_log(act(&screen));
                screen.close_offer();
            }
        };
        Choice { title, chosen, on_choose: Box::new(on_choose) }
    }

    /// A value of the panel that `act` moves a step, after which the
    /// panel shows what it has become.
    fn stepper(
        &self,
        (name, lower, raise): (&'static str, &'static str, &'static str),
        value: String,
        act: impl Fn(&Self, Step) -> player::Result<()> + 'static,
    ) -> Stepper {
        let screen = Weak::new(self);
        let on_step = move |step| {
            if let Some(screen) = screen.load() {
                screen.or_log(act(&screen, step));
                screen.refresh_offer();
                // The panel says what the value now is, where the notice
                // of it would only lie behind the panel.
                screen.ivars().controls.badge.hide();
            }
        };
        Stepper { name, value, lower, raise, on_step: Box::new(on_step) }
    }

    /// The chapters of the file, each with the time it starts at, and
    /// the one playing ticked.
    fn chapter_choices(&self) -> player::Result<Vec<Choice>> {
        let position = self.position()?.unwrap_or_default();
        let chapters = self.ivars().chapters.borrow();
        let playing = chapters.iter().rposition(|chapter| chapter.start <= position);
        let choices = chapters.iter().enumerate().map(|(index, chapter)| {
            let start = chapter.start;
            let title = format!("{}  {}", clock(start), chapter_name(chapter, index));
            self.choice(title, playing == Some(index), move |screen| {
                screen.ivars().player.seek(start)
            })
        });
        Ok(choices.collect())
    }

    /// Goes to the start of the next chapter, or back to the start of
    /// this one, and from there to the one before.
    fn step_chapter(&self, step: Step) -> player::Result<()> {
        let ivars = self.ivars();
        let position = self.position()?.unwrap_or_default();
        let chapters = ivars.chapters.borrow();
        let found = match step {
            Step::Up => chapters.iter().position(|chapter| chapter.start > position),
            // Just past a chapter's start, back means the chapter before.
            Step::Down => chapters
                .iter()
                .rposition(|chapter| chapter.start + RESTART_AFTER <= position)
                .or((!chapters.is_empty()).then_some(0)),
        };
        let Some((index, chapter)) = found.and_then(|index| Some((index, chapters.get(index)?)))
        else {
            return Ok(());
        };
        ivars.player.seek(chapter.start)?;
        let symbol = if step == Step::Up { "forward.end.fill" } else { "backward.end.fill" };
        self.notify(&Notice { symbol, text: &chapter_name(chapter, index), level: None });
        Ok(())
    }

    fn speed_stepper(&self) -> Stepper {
        let names = ("Speed", "Slower", "Faster");
        self.stepper(names, self.speed().to_string(), |screen, step| {
            screen.set_speed(match step {
                Step::Down => screen.speed().slower(),
                Step::Up => screen.speed().faster(),
            })
        })
    }

    /// The speeds the panel always offers, and the one in effect among
    /// them if the keys or the stepper took it off them.
    fn speed_choices(&self) -> Vec<Choice> {
        let current = self.speed();
        let choice = |speed: Speed| {
            self.choice(speed.to_string(), speed == current, move |screen| screen.set_speed(speed))
        };
        current.choices().into_iter().map(choice).collect()
    }

    /// The tracks of `kind`, the one in use ticked, with "Off" before the
    /// subtitles.
    fn track_choices(&self, kind: TrackKind) -> player::Result<Vec<Choice>> {
        let item = self.item();
        let tracks = self.ivars().player.tracks()?;
        let tracks: Vec<&Track> = tracks.iter().filter(|track| track.kind == kind).collect();
        let mut choices = Vec::new();
        if kind == TrackKind::Subtitles {
            let off = tracks.iter().all(|track| !track.selected);
            choices.push(self.choice("Off".to_owned(), off, |screen| {
                screen.choose_track(TrackKind::Subtitles, None)
            }));
        }
        choices.extend(tracks.iter().map(|track| {
            let (id, name) = (track.id, tracks::name(track, part(&item)));
            let title = match tracks::detail(track, part(&item)) {
                Some(detail) => format!("{name} \u{b7} {detail}"),
                None => name,
            };
            self.choice(title, track.selected, move |screen| screen.choose_track(kind, Some(id)))
        }));
        Ok(choices)
    }

    /// Plays or shows the track of `kind` with this id, and has the server
    /// remember it. No id is no subtitles.
    fn choose_track(&self, kind: TrackKind, id: Option<i64>) -> player::Result<()> {
        let player = &self.ivars().player;
        match (kind, id) {
            (TrackKind::Audio, Some(id)) => player.select_audio(id)?,
            (TrackKind::Subtitles, id) => player.select_subtitles(id)?,
            (TrackKind::Audio, None) | (TrackKind::Video, _) => return Ok(()),
        }
        self.remember_track(kind);
        Ok(())
    }

    fn subtitle_steppers(&self) -> player::Result<Vec<Stepper>> {
        let player = &self.ivars().player;
        let size = Size::from_scale(player.subtitle_scale()?);
        let shift = Shift::from_seconds(player.subtitle_delay()?);
        Ok(vec![
            self.stepper(
                ("Size", "Smaller subtitles", "Larger subtitles"),
                size.to_string(),
                Self::step_subtitle_size,
            ),
            self.stepper(
                ("Delay", "Subtitles earlier", "Subtitles later"),
                shift.to_string(),
                Self::step_subtitle_delay,
            ),
        ])
    }

    fn step_subtitle_size(&self, step: Step) -> player::Result<()> {
        let player = &self.ivars().player;
        let size = Size::from_scale(player.subtitle_scale()?).stepped(step);
        player.set_subtitle_scale(size.scale())?;
        let text = format!("Subtitles {size}");
        self.notify(&Notice { symbol: "textformat.size", text: &text, level: Some(size.level()) });
        Ok(())
    }

    fn step_subtitle_delay(&self, step: Step) -> player::Result<()> {
        let player = &self.ivars().player;
        let shift = Shift::from_seconds(player.subtitle_delay()?).stepped(step);
        player.set_subtitle_delay(shift.seconds())?;
        let text = format!("Subtitles {shift}");
        self.notify(&Notice { symbol: "captions.bubble", text: &text, level: None });
        Ok(())
    }

    fn next_episode_is_known(&self) -> bool {
        self.ivars().neighbours.borrow().as_ref().is_some_and(|known| known.next.is_some())
    }

    /// The stretch of the episode in which the next one is offered: from
    /// where the card comes up to where the next episode takes over.
    fn up_next_stretch(&self) -> (Duration, Duration) {
        let (length, item) = (self.length_shown(), self.item());
        let closing_credits = item.markers.iter().find(|marker| {
            let ends_before_end = length.saturating_sub(marker.end_time_offset.into());
            marker.kind == MarkerKind::Credits && ends_before_end <= CLOSING_CREDITS_END_WITHIN
        });
        match closing_credits {
            Some(credits) => {
                let start: Duration = credits.start_time_offset.into();
                (start, start + UP_NEXT_COUNTDOWN)
            }
            None => (length.saturating_sub(UP_NEXT_LEAD), length),
        }
    }

    /// Shows, counts down or hides the offer of the next episode to match
    /// `position`, and goes to that episode when the countdown runs out.
    /// The countdown is a stretch of the episode, so pausing holds it and
    /// jumping back out of that stretch takes the offer away again.
    fn offer_next_episode(&self, position: Option<Duration>) {
        let ivars = self.ivars();
        let state = ivars.up_next.get();
        let Some(position) = position.filter(|_| self.next_episode_is_known()) else {
            return;
        };
        if state == UpNext::Cancelled {
            return;
        }
        let (start, takeover) = self.up_next_stretch();
        // A jump from before the stretch to past its end is the viewer
        // going there on purpose, not a countdown running out.
        let jumped_past = state == UpNext::Waiting && position >= takeover;
        if position < start || jumped_past {
            ivars.up_next.set(UpNext::Waiting);
            ivars.controls.up_next.hide();
            return;
        }
        if position >= takeover {
            self.go_to_next();
            return;
        }
        ivars.up_next.set(UpNext::Counting);
        let left = (takeover - position).as_secs_f64() / self.speed().factor();
        // Shown in whole seconds, counted so that 0 is never seen.
        ivars.controls.up_next.show(left.ceil() as u64);
    }

    /// Takes the offer of the next episode away for the rest of this one.
    fn turn_down_next_episode(&self) {
        self.ivars().up_next.set(UpNext::Cancelled);
        self.ivars().controls.up_next.hide();
    }

    /// Shows or hides the skip button to match `position`.
    fn offer_skip(&self, position: Option<Duration>) {
        let skip = &self.ivars().controls.skip;
        // The offer of the next episode takes the same corner, and its
        // Play Now does what skipping the closing credits would.
        let counting = self.ivars().up_next.get() == UpNext::Counting;
        let item = self.item();
        let marker = position.and_then(|position| item.marker_at(position));
        let Some(marker) = marker.filter(|_| !counting) else {
            skip.setHidden(true);
            return;
        };
        skip.set_title(match marker.kind {
            MarkerKind::Intro => "Skip Intro",
            MarkerKind::Credits => "Skip Credits",
            MarkerKind::Other => "Skip",
        });
        skip.setHidden(false);
    }

    /// Jumps to the end of the intro or credits that is playing, if any,
    /// or to the next episode while it is being offered.
    fn skip_marker(&self) -> player::Result<()> {
        if self.ivars().up_next.get() == UpNext::Counting {
            self.go_to_next();
            return Ok(());
        }
        let (player, item) = (&self.ivars().player, self.item());
        let marker: Option<&Marker> =
            player.position()?.and_then(|position| item.marker_at(position));
        if let Some(marker) = marker {
            player.seek(marker.end_time_offset.into())?;
            self.ivars().controls.skip.setHidden(true);
        }
        Ok(())
    }

    /// Moves the video into the mini player and puts the window away. A
    /// window in full screen leaves it first, and the mini player opens
    /// when it has.
    fn enter_mini(&self) {
        let ivars = self.ivars();
        let busy = ivars.mini.borrow().is_some() || ivars.after_full_screen.borrow().is_some();
        // A failed file has no picture to float.
        if busy || ivars.phase.get() == Phase::Failed {
            return;
        }
        let Some(window) = ivars.root.window() else {
            return;
        };
        if self.is_full_screen() {
            self.wait_for_full_screen_to_end(&window);
            window.toggleFullScreen(None);
            return;
        }
        let picture = ivars.player.picture_size().unwrap_or_else(|error| {
            eprintln!("plex-rust: {error}");
            None
        });
        self.show_controls(false);
        NSCursor::setHiddenUntilMouseMoves(false);
        let staged = ivars.purpose == Purpose::Check;
        let mini =
            match Mini::open(self.mtm(), &ivars.video, picture, &window, ivars.material, staged) {
                Ok(mini) => mini,
                Err(error) => return eprintln!("plex-rust: {}", with_causes(&error)),
            };
        self.connect_mini(&mini);
        mini.show_paused(ivars.player.is_paused().unwrap_or(false));
        *ivars.mini.borrow_mut() = Some(mini);
        self.fit_to_display();
        // A check's window is where no display shows it, and stays there.
        if ivars.purpose == Purpose::Watching {
            window.orderOut(None);
        }
    }

    /// Says what the mini player's keys and buttons do.
    fn connect_mini(&self, mini: &Mini) {
        self.on_click(&mini.toggle, |screen| screen.or_log(screen.toggle_pause()));
        self.on_click(&mini.expand, Self::leave_mini);
        self.on_click(&mini.close, Self::close);
        let (keys, fit) = (Weak::new(self), Weak::new(self));
        mini.listen(
            move |key, press| keys.load().is_some_and(|screen| screen.act_on(key, press)),
            move || {
                if let Some(screen) = fit.load() {
                    screen.fit_to_display();
                }
            },
        );
    }

    /// Acts on a key in the mini player, which has a few: pause, jumps,
    /// mute, and back to the full window. Returns whether the key means
    /// anything there.
    fn act_in_mini(&self, key: Key, press: Press) -> bool {
        if press == Press::Held && !input::repeats(key) {
            return true;
        }
        let result = match key {
            Key::Space | Key::Character('k') => self.toggle_pause(),
            Key::Left => self.jump(-SHORT_JUMP, press),
            Key::Right => self.jump(SHORT_JUMP, press),
            Key::ShiftLeft => self.jump(-LONG_JUMP, press),
            Key::ShiftRight => self.jump(LONG_JUMP, press),
            Key::Character('m') => self.toggle_mute(),
            Key::Character('o') | Key::Escape => {
                self.leave_mini();
                Ok(())
            }
            // The pointer over the picture, for a snapshot.
            Key::Scene(Scene::Hover) => {
                if let Some(mini) = self.ivars().mini.borrow().as_ref() {
                    mini.show_controls(true);
                }
                Ok(())
            }
            Key::Wait => return true,
            _ => return false,
        };
        self.or_log(result);
        true
    }

    /// Brings the video back from the mini player to the full window, and
    /// the window back on screen.
    pub fn leave_mini(&self) {
        if !self.put_back() {
            return;
        }
        self.focus();
        self.show_controls(true);
    }

    /// Moves the video back into the full window, shows the window again
    /// and closes the mini player. Returns whether there was one.
    fn put_back(&self) -> bool {
        let ivars = self.ivars();
        let Some(mini) = ivars.mini.borrow_mut().take() else {
            return false;
        };
        mini::put_under(&ivars.root, &ivars.video);
        if let (Purpose::Watching, Some(window)) = (ivars.purpose, ivars.root.window()) {
            window.makeKeyAndOrderFront(None);
            NSApplication::sharedApplication(self.mtm()).activate();
        }
        mini.close();
        true
    }

    /// The mini player's window, while there is one.
    pub fn mini_window(&self) -> Option<Retained<NSWindow>> {
        self.ivars().mini.borrow().as_ref().map(|mini| mini.window().retain())
    }

    /// Gives the mini player the shape of the picture, which can change
    /// from one episode to the next.
    fn shape_mini(&self) -> player::Result<()> {
        let ivars = self.ivars();
        let mini = ivars.mini.borrow();
        let (Some(mini), Some(picture)) = (mini.as_ref(), ivars.player.picture_size()?) else {
            return Ok(());
        };
        mini.keep_shape(picture);
        Ok(())
    }

    /// Opens the mini player once `window` has left full screen.
    fn wait_for_full_screen_to_end(&self, window: &NSWindow) {
        let screen = Weak::new(self);
        let block = RcBlock::new(move |_: NonNull<NSNotification>| {
            if let Some(screen) = screen.load() {
                screen.forget_full_screen();
                screen.enter_mini();
            }
        });
        // SAFETY: the name is a constant that AppKit provides, and the
        // block takes the one argument a notification block is given. The
        // observer is removed by `forget_full_screen`.
        let observer = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSWindowDidExitFullScreenNotification),
                Some(window),
                Some(&NSOperationQueue::mainQueue()),
                &block,
            )
        };
        *self.ivars().after_full_screen.borrow_mut() = Some(observer);
    }

    /// Stops waiting for the window to leave full screen.
    fn forget_full_screen(&self) {
        if let Some(observer) = self.ivars().after_full_screen.borrow_mut().take() {
            // SAFETY: the observer is the one the center returned.
            unsafe { NSNotificationCenter::defaultCenter().removeObserver(observer.as_ref()) };
        }
    }

    fn is_full_screen(&self) -> bool {
        let window = self.ivars().root.window();
        window.is_some_and(|window| window.styleMask().contains(NSWindowStyleMask::FullScreen))
    }

    fn toggle_full_screen(&self) {
        if let Some(window) = self.ivars().root.window() {
            window.toggleFullScreen(None);
        }
    }

    /// Says what just changed in the badge for a moment.
    fn notify(&self, notice: &Notice) {
        self.notify_for(notice, TICKS_OF_NOTICE);
    }

    fn notify_for(&self, notice: &Notice, ticks: u32) {
        self.wake();
        let ivars = self.ivars();
        ivars.controls.badge.show(notice);
        ivars.ticks_of_notice_left.set(ticks);
    }

    /// Tells the viewer if the file is one this player cannot show in its
    /// own colours. Such a file plays like any other, so nothing else
    /// would say so.
    fn warn_of_wrong_colors(&self) -> player::Result<()> {
        let dolby_vision = self.ivars().player.dolby_vision()?;
        if dolby_vision.is_some_and(DolbyVision::is_shown_in_wrong_colors) {
            let symbol = "exclamationmark.triangle.fill";
            self.notify_for(&Notice { symbol, text: WRONG_COLORS, level: None }, TICKS_OF_WARNING);
        }
        Ok(())
    }

    /// Fades the controls, the window's own buttons and the cursor in or
    /// out. Showing them also restarts the wait before they hide again.
    ///
    /// Over a paused picture the title stays when the rest goes: it and
    /// the time left are what someone coming back to the screen wants to
    /// know, and nothing else is in the way of the picture.
    fn show_controls(&self, shown: bool) {
        let ivars = self.ivars();
        if shown {
            self.wake();
            ivars.ticks_idle.set(0);
        }
        // The window they are in is put away, and the cursor is the mini
        // player's to show.
        if ivars.mini.borrow().is_some() {
            return;
        }
        if ivars.controls_shown.replace(shown) == shown {
            return;
        }
        let failed = ivars.phase.get() == Phase::Failed;
        let paused = ivars.player.is_paused().unwrap_or(false);
        ivars.controls.top.set_shown(shown || paused || failed);
        ivars.controls.bottom.set_shown(shown && !failed);
        if !shown {
            self.close_offer();
            ivars.controls.bubble.hide();
            if self.focused().is_some() {
                self.drop_focus();
            }
        }
        if let Some(window) = ivars.root.window() {
            chrome::show_buttons(&window, shown);
        }
        NSCursor::setHiddenUntilMouseMoves(!shown);
        if shown {
            self.or_log(self.refresh());
        }
    }

    /// Watches the player from a thread of its own and brings what happens
    /// back to the main thread.
    fn follow_events(&self, main_thread: MainThreadMarker) {
        let screen = Arc::new(MainThreadBound::new(Weak::new(self), main_thread));
        let player = self.ivars().player.clone();
        thread::spawn(move || {
            loop {
                let event = player.wait_event();
                let done = event == Event::Shutdown;
                let screen = Arc::clone(&screen);
                DispatchQueue::main().exec_async(move || {
                    screen.get_on_main(|screen| {
                        if let Some(screen) = screen.load() {
                            screen.handle(&event);
                        }
                    });
                });
                if done {
                    return;
                }
            }
        });
    }

    fn handle(&self, event: &Event) {
        self.wake();
        let ivars = self.ivars();
        // What a failed file still has to say changes nothing.
        if ivars.phase.get() == Phase::Failed {
            return;
        }
        // Until its file is open, what the player says is about the file
        // this item took over from, which is on its way out.
        let playing = ivars.phase.get() == Phase::Playing;
        match event {
            Event::Loaded => {
                ivars.phase.set(Phase::Playing);
                self.report(PlaybackState::Playing);
                self.add_sidecars();
                self.or_log(self.show_chapters());
                self.or_log(self.refresh());
                self.or_log(self.warn_of_wrong_colors());
                self.or_log(self.show_now_playing());
                self.show_neighbours();
            }
            Event::Restarted => self.or_log(self.show_now_playing()),
            Event::VideoChanged => {
                // The first picture is here: the wait since opening is over.
                if playing {
                    ivars.controls.waiting.hide();
                }
                self.fit_to_display();
                self.or_log(self.shape_mini());
            }
            Event::Ended(EndReason::Failed(error)) => self.fail(error),
            Event::Buffering(true) if playing => ivars.controls.waiting.show(Some("Buffering")),
            Event::Buffering(false) if playing => ivars.controls.waiting.hide(),
            // A viewer who turned the next episode down gets none.
            Event::Ended(EndReason::Finished) if playing => {
                let cancelled = ivars.up_next.get() == UpNext::Cancelled;
                if cancelled || !self.next_episode_is_known() {
                    self.close();
                } else {
                    self.go_to_next();
                }
            }
            // Stopped is our own doing, and shutdown follows `close`.
            Event::Buffering(_) | Event::Ended(_) | Event::Shutdown => {}
        }
    }

    /// Adds the subtitles the server keeps in files of their own to the
    /// tracks of the file that has just opened, so that they are in the
    /// list of subtitles with the rest, and the one last chosen is shown.
    fn add_sidecars(&self) {
        let ivars = self.ivars();
        let sidecars = ivars.sidecars.take();
        if sidecars.is_empty() {
            return;
        }
        // The player fetches each file before it answers.
        let player = ivars.player.clone();
        let add = async move {
            tokio::task::spawn_blocking(move || player.add_sidecars(&sidecars)).await
        };
        ivars.background.run(self.mtm(), add, |added| match added {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("plex-rust: subtitles not added: {error}"),
            Err(error) => eprintln!("plex-rust: subtitles not added: {error}"),
        });
    }

    /// Notches the scrubber where the file's chapters start.
    fn show_chapters(&self) -> player::Result<()> {
        let ivars = self.ivars();
        let chapters = ivars.player.chapters()?;
        // The first chapter starts with the file, where a notch says nothing.
        let starts = chapters.iter().map(|chapter| chapter.start.as_secs_f64());
        ivars.controls.scrubber.set_marks(starts.filter(|start| *start > 0.0).collect());
        ivars.controls.chapters.setHidden(chapters.is_empty());
        *ivars.chapters.borrow_mut() = chapters;
        Ok(())
    }

    fn toggle_pause(&self) -> player::Result<()> {
        self.set_paused(!self.ivars().player.is_paused()?)
    }

    fn set_paused(&self, pause: bool) -> player::Result<()> {
        // The pause that holds the last picture of the episode before
        // this one is not the viewer's to lift.
        if self.ivars().phase.get() == Phase::Fetching {
            return Ok(());
        }
        self.ivars().player.set_paused(pause)?;
        self.show_time_left(pause);
        let (symbol, name) = if pause { ("play.fill", "Play") } else { ("pause.fill", "Pause") };
        self.ivars().controls.toggle.set_symbol(symbol, theme::MAIN_SYMBOL_SIZE);
        self.ivars().controls.toggle.set_name(name);
        if let Some(mini) = self.ivars().mini.borrow().as_ref() {
            mini.show_paused(pause);
        }
        self.keep_display_awake(!pause);
        self.report(if pause { PlaybackState::Paused } else { PlaybackState::Playing });
        self.show_controls(true);
        self.show_now_playing()
    }

    /// Adds what is left of the film to the line under its title, or with
    /// `shown` false takes it off again. It is there while paused.
    fn show_time_left(&self, shown: bool) {
        let ivars = self.ivars();
        let left = match (shown, ivars.player.position(), ivars.player.duration()) {
            (true, Ok(Some(position)), Ok(Some(duration))) => {
                Some(time_left(duration.saturating_sub(position)))
            }
            _ => None,
        };
        ivars.controls.set_heading(&self.item(), left.as_deref());
    }

    /// Brings the controls up to date with the player.
    fn refresh(&self) -> player::Result<()> {
        let ivars = self.ivars();
        let (player, controls) = (&ivars.player, &ivars.controls);

        let volume = player.volume()?;
        let silent = player.is_muted()? || volume == 0.0;
        controls.volume.set_value(volume);
        let (speaker, name) =
            if silent { ("speaker.slash.fill", "Unmute") } else { ("speaker.wave.2.fill", "Mute") };
        controls.mute.set_symbol(speaker, theme::SYMBOL_SIZE);
        controls.mute.set_name(name);
        let (arrows, name) = if self.is_full_screen() {
            ("arrow.down.right.and.arrow.up.left", "Leave full screen")
        } else {
            ("arrow.up.left.and.arrow.down.right", "Full screen")
        };
        controls.full_screen.set_symbol(arrows, theme::SYMBOL_SIZE);
        controls.full_screen.set_name(name);

        // Until the file is open the times are the item's own, as `load`
        // showed them.
        if ivars.phase.get() != Phase::Playing {
            return Ok(());
        }
        let (Some(position), Some(duration)) = (player.position()?, player.duration()?) else {
            return Ok(());
        };
        let loaded = player.buffered_until()?.unwrap_or_default();
        controls.scrubber.set_loaded(loaded.as_secs_f64());
        // While the knob is held, and just after, it and the times belong
        // to the viewer.
        let settling = ivars.ticks_of_scrub_settling_left.get();
        ivars.ticks_of_scrub_settling_left.set(settling.saturating_sub(1));
        if !controls.scrubber.is_dragging() && settling == 0 {
            self.show_progress(position, duration);
        }
        // Paused, the place can still change: by a key or the scrubber.
        if player.is_paused()? {
            self.show_time_left(true);
        }
        Ok(())
    }

    /// Shows `position` in a file of length `duration` on the scrubber and
    /// in the two times.
    fn show_progress(&self, position: Duration, duration: Duration) {
        let ivars = self.ivars();
        let controls = &ivars.controls;
        controls.scrubber.set_limit(duration.as_secs_f64());
        controls.scrubber.set_value(position.as_secs_f64());
        controls.elapsed.setStringValue(&NSString::from_str(&clock(position)));
        let right = if ivars.shows_length.get() {
            clock(duration)
        } else {
            // U+2212 is the minus sign, which sits level with the digits.
            format!("\u{2212}{}", clock(duration.saturating_sub(position)))
        };
        controls.remaining.set_title(&right);
    }

    fn report_current_state(&self) {
        match self.ivars().player.is_paused() {
            Ok(true) => self.report(PlaybackState::Paused),
            Ok(false) => self.report(PlaybackState::Playing),
            Err(error) => eprintln!("plex-rust: {error}"),
        }
    }

    /// Tells the server where this playback is, without waiting for it,
    /// or keeps that for later if the server is not there.
    fn report(&self, state: PlaybackState) {
        let Some(progress) = self.progress(state) else {
            return;
        };
        let Some(server) = self.ivars().server.clone() else {
            return self.keep(&progress);
        };
        self.send(progress, server, || {});
    }

    /// Sends `progress` to `server` and calls `then` once that has come
    /// to something. A report the server did not take is kept next to
    /// the item's download, if it has one, as is done with no server.
    fn send(&self, progress: Progress, server: MediaServer, then: impl FnOnce() + 'static) {
        let (downloads, made) = (Rc::clone(&self.ivars().downloads), Instant::now());
        let send = async move { (progress.send(&server).await, progress) };
        self.ivars().background.run(self.mtm(), send, move |(sent, progress)| {
            let Progress { item, position, duration, .. } = &progress;
            downloads.reported(item, *position, *duration, made, sent.is_ok());
            log_unsaved(sent);
            then();
        });
    }

    /// Keeps `progress` next to the download it is of, for the server to
    /// hear of when it is next there.
    fn keep(&self, progress: &Progress) {
        let downloads = &self.ivars().downloads;
        downloads.keep_position(&progress.item, progress.position, progress.duration);
    }

    /// What to tell the server right now, or `None` if there is nothing to
    /// tell: a check, or nothing loaded yet.
    fn progress(&self, state: PlaybackState) -> Option<Progress> {
        let ivars = self.ivars();
        if ivars.purpose == Purpose::Check || ivars.phase.get() != Phase::Playing {
            return None;
        }
        let times = ivars.player.duration().and_then(|duration| {
            // Once a file has played to its end mpv has closed it and knows
            // neither its length nor a position in it. The length is then
            // the last one seen, and the position is the end.
            let Some(duration) = duration.or(ivars.duration.get()) else {
                return Ok(None);
            };
            ivars.duration.set(Some(duration));
            Ok(Some((self.position()?.unwrap_or(duration), duration)))
        });
        match times {
            Ok(times) => times.map(|(position, duration)| Progress {
                item: self.item().rating_key.clone(),
                session: ivars.session.clone(),
                state,
                position,
                duration,
            }),
            Err(error) => {
                eprintln!("plex-rust: {error}");
                None
            }
        }
    }

    fn or_log(&self, result: player::Result<()>) {
        if let Err(error) = result {
            eprintln!("plex-rust: {error}");
        }
    }
}

/// Whether a player that opened on `item` goes from item to item, and so
/// has a next and a previous one to offer: an episode goes through its
/// show, and anything goes through a queue it was given.
fn steps_through(item: &Item, queue: Option<&Queue>) -> bool {
    item.kind == Kind::Episode || queue.is_some()
}

/// The file of `item`, as the server describes it.
fn part(item: &Item) -> Option<&Part> {
    item.media.first().and_then(|media| media.parts.first())
}

/// One progress report, owning what it needs so it can cross threads.
struct Progress {
    item: plex::model::RatingKey,
    session: String,
    state: PlaybackState,
    position: Duration,
    duration: Duration,
}

impl Progress {
    async fn send(&self, server: &MediaServer) -> plex::Result<()> {
        let progress = playback::Progress {
            item: &self.item,
            session: &self.session,
            state: self.state,
            position: self.position,
            duration: self.duration,
            offline: false,
        };
        server.report(&progress).await
    }
}

fn log_unsaved(sent: plex::Result<()>) {
    if let Err(error) = sent {
        eprintln!("plex-rust: progress was not saved: {}", with_causes(&error));
    }
}

/// What to call `chapter`, which is the one at `index` in its file: its
/// title, or its number if it has none that says anything.
fn chapter_name(chapter: &Chapter, index: usize) -> String {
    match chapter.title.as_deref().filter(|title| !is_timestamp(title)) {
        Some(title) => title.to_owned(),
        None => format!("Chapter {}", index + 1),
    }
}

/// Whether a chapter's title is only the time it starts at, such as
/// `00:17:47.316`. Some files name their chapters so, and next to the time
/// under the pointer that is the same thing said twice.
fn is_timestamp(title: &str) -> bool {
    title.chars().all(|character| character.is_ascii_digit() || matches!(character, ':' | '.'))
}

/// What is left of a film, as it is said: "1 h 2 min left", "6 min left",
/// and in the last minute "40 s left".
fn time_left(left: Duration) -> String {
    let seconds = left.as_secs();
    match (seconds / 3600, seconds / 60 % 60) {
        (0, 0) => format!("{seconds} s left"),
        (0, minutes) => format!("{minutes} min left"),
        (hours, minutes) => format!("{hours} h {minutes} min left"),
    }
}

/// `1:02:03`, or `2:03` under an hour.
pub fn clock(time: Duration) -> String {
    let seconds = time.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}
