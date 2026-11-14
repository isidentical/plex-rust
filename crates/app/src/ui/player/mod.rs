//! The screen that plays one item: video with controls on top.

mod chip;
mod chrome;
mod controls;
pub mod input;
mod layout;
mod pane;
mod season;
mod slider;
mod theme;
mod tracks;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSControlStateValueOn, NSCursor, NSMenu, NSMenuItem, NSView,
    NSWindowStyleMask,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSString, NSTimer, NSUUID, ns_string};
use player::{Chapter, EndReason, Event, Player, Track, TrackKind};
use plex::MediaServer;
use plex::model::{Item, Kind, Marker, MarkerKind, Part};
use plex::playback::{self, PlaybackState};

use self::chip::Chip;
use self::chrome::Chrome;
use self::controls::Controls;
use self::input::{Clicks, InputView, Key, Press};
use self::season::Neighbours;
use self::slider::{Grip, Pointed};
use crate::background::Background;
use crate::error::{Result, with_causes};
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

/// For this many ticks after the scrubber's knob is let go, the knob and
/// the times stay where the viewer put them. The player needs a moment to
/// start on the jump, and until then still reports where it was before.
const TICKS_OF_SCRUB_SETTLING: u32 = 2;

/// How far the left and right arrow keys jump.
const SHORT_JUMP: f64 = 3.0;

/// How far they jump with Shift held, and how far the buttons jump.
const LONG_JUMP: f64 = 10.0;

/// This far into an episode, going back means going back to its start.
/// Before that it means the episode before, as in every player.
const RESTART_AFTER: Duration = Duration::from_secs(5);

/// How much the up and down arrow keys change the volume, out of 100.
const VOLUME_STEP: f64 = 5.0;

/// Playback speed is kept in tenths so that steps of 0.1 stay exact.
const NORMAL_SPEED: u32 = 10;
const SLOWEST_SPEED: u32 = 1;
const FASTEST_SPEED: u32 = 40;

/// The speeds the speed menu offers, in tenths. The keys reach the rest.
const SPEED_CHOICES: [u32; 9] = [5, 8, 10, 12, 15, 17, 20, 25, 30];

/// Why the player is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Someone is watching.
    Watching,
    /// An unattended check. It is silent and tells the server nothing, so
    /// that checking the app never moves anyone's place in a film.
    Check,
}

/// How a playback came to its end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The viewer left.
    Closed,
    /// Playback failed. Holds the reason.
    Failed(String),
    /// It played to the end, or the viewer asked for what comes next.
    Finished,
    /// The viewer asked for the episode before this one.
    Previous,
}

pub struct Ivars {
    root: Retained<InputView>,
    /// The view that hosts the video layer.
    video: Retained<NSView>,
    player: Player,
    item: Item,
    controls: Controls,
    /// How the window's title bar was before the player cleared it. Empty
    /// until the screen is in a window, and again once it has closed.
    chrome: RefCell<Option<Chrome>>,
    ticks_of_notice_left: Cell<u32>,
    notice_shown: Cell<bool>,
    /// In tenths: 15 is one and a half times normal speed.
    speed: Cell<u32>,
    controls_shown: Cell<bool>,
    /// Whether the pointer was last seen on the controls. They stay for as
    /// long as it rests there.
    pointer_on_controls: Cell<bool>,
    ticks_idle: Cell<u32>,
    ticks_of_scrub_settling_left: Cell<u32>,
    /// Whether the time to the right of the scrubber is the file's full
    /// length, and not what is left of it.
    shows_length: Cell<bool>,
    chapters: RefCell<Vec<Chapter>>,
    /// The episodes before and after this one in its season. `None` for a
    /// film, and for an episode until the server has listed its season.
    neighbours: RefCell<Option<Neighbours>>,
    /// Running from open to close. Its absence means the screen is closed.
    timer: RefCell<Option<Retained<NSTimer>>>,
    ticks: Cell<u32>,
    /// The file's length, kept for when mpv no longer knows it.
    duration: Cell<Option<Duration>>,
    background: Rc<Background>,
    server: MediaServer,
    /// Lets the server tell this playback from another of the same item.
    session: String,
    purpose: Purpose,
    on_close: Box<dyn Fn(Outcome)>,
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
        /// A menu item's tag is the id of its track, or 0 for no subtitles.
        #[unsafe(method(audioChosen:))]
        fn audio_chosen(&self, item: &NSMenuItem) {
            self.or_log(self.ivars().player.select_audio(item.tag() as i64));
            self.remember_track(TrackKind::Audio);
        }

        #[unsafe(method(subtitlesChosen:))]
        fn subtitles_chosen(&self, item: &NSMenuItem) {
            let id = item.tag() as i64;
            self.or_log(self.ivars().player.select_subtitles((id != 0).then_some(id)));
            self.remember_track(TrackKind::Subtitles);
        }

        /// Here the tag is the speed in tenths.
        #[unsafe(method(speedChosen:))]
        fn speed_chosen(&self, item: &NSMenuItem) {
            let speed = u32::try_from(item.tag()).unwrap_or(NORMAL_SPEED);
            self.or_log(self.set_speed(speed));
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: Option<&AnyObject>) {
            self.or_log(self.on_tick());
        }
    }
);

impl PlayerScreen {
    /// Starts playing `item` from where it was left, at the speed and
    /// volume in `settings`. `on_close` is called once, when the screen is done.
    ///
    /// `item` should be the full item from [`MediaServer::item`], which is
    /// the one that knows its intro and credits.
    pub fn open(
        main_thread: MainThreadMarker,
        background: Rc<Background>,
        server: MediaServer,
        item: &Item,
        settings: Settings,
        purpose: Purpose,
        on_close: impl Fn(Outcome) + 'static,
    ) -> Result<Retained<Self>> {
        let source = source::of(&server, item)?;

        let player = Player::new()?;
        player.set_muted(purpose == Purpose::Check)?;
        let speed = settings.speed.clamp(SLOWEST_SPEED, FASTEST_SPEED);
        player.set_speed(f64::from(speed) / 10.0)?;
        player.set_volume(settings.volume.clamp(0.0, controls::FULL_VOLUME))?;

        let root = InputView::new(main_thread);
        let video = NSView::new(main_thread);
        let layer = VideoLayer::new(&player, main_thread)?;
        video.setLayer(Some(&layer));
        video.setWantsLayer(true);
        crate::ui::layout::cover(&root, &video);
        let controls = Controls::new(main_thread, &root, item);

        let this = Self::alloc(main_thread).set_ivars(Ivars {
            root,
            video,
            player,
            item: item.clone(),
            controls,
            chrome: RefCell::new(None),
            ticks_of_notice_left: Cell::new(0),
            notice_shown: Cell::new(false),
            speed: Cell::new(speed),
            controls_shown: Cell::new(true),
            pointer_on_controls: Cell::new(false),
            ticks_idle: Cell::new(0),
            ticks_of_scrub_settling_left: Cell::new(0),
            shows_length: Cell::new(false),
            chapters: RefCell::new(Vec::new()),
            neighbours: RefCell::new(None),
            timer: RefCell::new(None),
            ticks: Cell::new(0),
            duration: Cell::new(None),
            background,
            server,
            session: NSUUID::UUID().UUIDString().to_string(),
            purpose,
            on_close: Box::new(on_close),
        });
        // SAFETY: `init` is NSObject's designated initializer.
        let screen: Retained<Self> = unsafe { msg_send![super(this), init] };
        screen.connect_controls();
        screen.listen();
        screen.show_speed();
        screen.set_waiting(true);
        // The player knows neither time until the file is open, which can
        // take a while. The item knows both now, so the controls never show
        // a film that is being resumed as if it started over.
        if let Some(duration) = item.duration {
            screen.show_progress(source.start, duration.into());
        }
        // Loaded before the timer and the event thread exist: both keep the
        // screen and its player alive, so a failure after them would leak.
        screen.ivars().player.load(&source)?;

        // SAFETY: `tick:` is defined above. The timer retains the screen
        // until `close` invalidates it.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                TICK.as_secs_f64(),
                &screen,
                sel!(tick:),
                None,
                true,
            )
        };
        *screen.ivars().timer.borrow_mut() = Some(timer);
        screen.follow_events(main_thread);
        screen.find_neighbours();
        Ok(screen)
    }

    pub fn view(&self) -> &NSView {
        &self.ivars().root
    }

    /// The item being played.
    pub fn item(&self) -> &Item {
        &self.ivars().item
    }

    /// The speed and volume the viewer has set, to be used again next time.
    /// `None` for a check, which must leave no trace.
    pub fn settings(&self) -> player::Result<Option<Settings>> {
        if self.ivars().purpose == Purpose::Check {
            return Ok(None);
        }
        Ok(Some(Settings { speed: self.speed(), volume: self.ivars().player.volume()? }))
    }

    /// The playback speed in tenths of normal speed.
    fn speed(&self) -> u32 {
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

    /// Shows or hides the spinner that says the picture is on its way.
    fn set_waiting(&self, waiting: bool) {
        let spinner = &self.ivars().controls.spinner;
        // SAFETY: the sender is optional and not kept.
        unsafe {
            if waiting {
                spinner.startAnimation(None);
            } else {
                spinner.stopAnimation(None);
            }
        }
    }

    fn fit_to_display(&self) {
        let ivars = self.ivars();
        self.or_log(video_layer::fit_to_display(&ivars.video, &ivars.player));
    }

    /// Stops playback and gives the screen up. Safe to call more than once.
    pub fn close(&self, outcome: Outcome) {
        if !self.stop_ticking() {
            return;
        }
        // `on_close` may drop the last other reference to this screen.
        let _alive = self.retain();
        self.report(PlaybackState::Stopped);
        self.or_log(self.ivars().player.quit());
        (self.ivars().on_close)(outcome);
    }

    /// Like [`close`](Self::close), for when the app is quitting: it waits
    /// until the server has the final position, because nothing that is
    /// still on its way when the process exits will arrive.
    pub fn close_for_quit(&self) {
        if !self.stop_ticking() {
            return;
        }
        if let Some(progress) = self.progress(PlaybackState::Stopped) {
            let sent = self.ivars().background.wait(progress.send(&self.ivars().server));
            log_unsaved(sent);
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
        NSCursor::setHiddenUntilMouseMoves(false);
        if let (Some(chrome), Some(window)) =
            (ivars.chrome.borrow_mut().take(), ivars.root.window())
        {
            chrome.restore(&window);
        }
        true
    }

    /// Says what each control does.
    fn connect_controls(&self) {
        let controls = &self.ivars().controls;
        self.on_click(&controls.back, |screen| screen.close(Outcome::Closed));
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
        self.on_click(&controls.remaining, |screen| {
            let shows_length = &screen.ivars().shows_length;
            shows_length.set(!shows_length.get());
            screen.or_log(screen.refresh());
        });
        self.on_click(&controls.speed, |screen| screen.offer_speeds());
        self.on_click(&controls.audio, |screen| {
            screen.or_log(screen.offer_tracks(TrackKind::Audio));
        });
        self.on_click(&controls.subtitles, |screen| {
            screen.or_log(screen.offer_tracks(TrackKind::Subtitles));
        });
        self.on_click(&controls.full_screen, |screen| screen.toggle_full_screen());
        self.on_click(&controls.skip, |screen| screen.or_log(screen.skip_marker()));

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
        let (keys, pointer, clicks, display) =
            (Weak::new(self), Weak::new(self), Weak::new(self), Weak::new(self));
        self.ivars().root.listen(
            move |key, press| keys.load().is_some_and(|screen| screen.act_on(key, press)),
            move |place| {
                if let Some(screen) = pointer.load() {
                    screen.pointer_moved(place);
                }
            },
            move |count| {
                let Some(screen) = clicks.load() else {
                    return;
                };
                match count {
                    Clicks::Single => screen.or_log(screen.toggle_pause()),
                    // The first click of the pair already paused or resumed.
                    // Undo it, so that a double click only changes the size.
                    Clicks::Double => {
                        screen.or_log(screen.toggle_pause());
                        screen.toggle_full_screen();
                    }
                }
            },
            move || {
                if let Some(screen) = display.load() {
                    screen.fit_to_display();
                }
            },
        );
    }

    /// Acts on a key going down. Returns whether the key means anything
    /// here.
    pub fn press(&self, key: Key) -> bool {
        self.act_on(key, Press::First)
    }

    fn act_on(&self, key: Key, press: Press) -> bool {
        // Only the arrows mean something again and again while held. A
        // held Space must not pause and resume thirty times a second.
        let repeats = matches!(
            key,
            Key::Left | Key::Right | Key::ShiftLeft | Key::ShiftRight | Key::Up | Key::Down
        );
        if press == Press::Held && !repeats {
            return true;
        }
        let is_episode = self.ivars().item.kind == Kind::Episode;
        let result = match key {
            Key::Space | Key::Character('k') => self.toggle_pause(),
            Key::Left => self.jump(-SHORT_JUMP, press),
            Key::Right => self.jump(SHORT_JUMP, press),
            Key::ShiftLeft => self.jump(-LONG_JUMP, press),
            Key::ShiftRight => self.jump(LONG_JUMP, press),
            Key::Up => self.change_volume(VOLUME_STEP),
            Key::Down => self.change_volume(-VOLUME_STEP),
            Key::Character('m') => self.toggle_mute(),
            Key::Character('s') => self.set_speed(self.speed().saturating_sub(1)),
            Key::Character('d') => self.set_speed(self.speed() + 1),
            Key::Character('r') => self.set_speed(NORMAL_SPEED),
            Key::Character('a') => self.next_track(TrackKind::Audio),
            Key::Character('c') => self.next_track(TrackKind::Subtitles),
            Key::Return => self.skip_marker(),
            Key::Character('f') => {
                self.toggle_full_screen();
                Ok(())
            }
            Key::Character('n') if is_episode => {
                self.go_to_next();
                return true;
            }
            Key::Character('p') if is_episode => self.go_to_previous(),
            Key::Escape => {
                // Escape leaves full screen first, as it does everywhere.
                if self.is_full_screen() {
                    self.toggle_full_screen();
                } else {
                    self.close(Outcome::Closed);
                }
                return true;
            }
            Key::Idle => {
                self.show_controls(false);
                return true;
            }
            Key::Hover => {
                let scrubber = &self.ivars().controls.scrubber;
                scrubber.point_at(scrubber.bounds().size.width / 2.0);
                Ok(())
            }
            Key::Drag => {
                let scrubber = &self.ivars().controls.scrubber;
                scrubber.hold_at(scrubber.bounds().size.width * 0.75);
                Ok(())
            }
            Key::Character(_) => return false,
        };
        self.or_log(result);
        // A hand on the keyboard is not on the mouse: wherever the pointer
        // was left, it is no longer holding the controls up.
        self.ivars().pointer_on_controls.set(false);
        self.show_controls(true);
        true
    }

    /// Hears where the pointer has moved to in the screen's view, or that
    /// it has left the view.
    fn pointer_moved(&self, place: Option<NSPoint>) {
        let controls = &self.ivars().controls;
        let on_controls =
            place.is_some_and(|place| controls.top.holds(place) || controls.bottom.holds(place));
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
        }

        let notice_left = ivars.ticks_of_notice_left.get();
        if notice_left > 0 {
            ivars.ticks_of_notice_left.set(notice_left - 1);
            if notice_left == 1 {
                self.set_notice_shown(false);
            }
        }

        // While the controls are hidden the position is read only for an
        // intro or credits that could be skipped, and then once a second.
        let shown = ivars.controls_shown.get();
        let has_markers = !ivars.item.markers.is_empty();
        if shown || (has_markers && ticks.is_multiple_of(TICKS_PER_SECOND)) {
            let position = ivars.player.position()?;
            self.offer_skip(position);
        }
        if !shown {
            return Ok(());
        }
        self.refresh()?;
        let idle = ivars.ticks_idle.get() + 1;
        ivars.ticks_idle.set(idle);
        // A paused picture keeps its controls, and so does a pointer that
        // is on them.
        let in_use = ivars.pointer_on_controls.get() || ivars.controls.scrubber.is_dragging();
        if idle >= TICKS_UNTIL_HIDDEN && !in_use && !ivars.player.is_paused()? {
            self.show_controls(false);
        }
        Ok(())
    }

    /// Asks the server for the season of the episode being played, to know
    /// what comes before and after it.
    fn find_neighbours(&self) {
        let ivars = self.ivars();
        let (Kind::Episode, Some(season), Some(number)) =
            (ivars.item.kind, ivars.item.parent_rating_key.clone(), ivars.item.index)
        else {
            return;
        };
        let server = ivars.server.clone();
        let load = async move { server.children(&season).await };
        let screen = Weak::new(self);
        ivars.background.run(self.mtm(), load, move |episodes| {
            let Some(screen) = screen.load() else {
                return;
            };
            match episodes {
                Ok(episodes) => screen.set_neighbours(Neighbours::of(episodes, number)),
                // Next and Previous then work as they do before the answer
                // has come: the shell looks the season up when asked.
                Err(error) => eprintln!("plex-rust: season not listed: {}", with_causes(&error)),
            }
        });
    }

    fn set_neighbours(&self, neighbours: Neighbours) {
        self.ivars().controls.next.set_enabled(neighbours.next.is_some());
        *self.ivars().neighbours.borrow_mut() = Some(neighbours);
    }

    /// Goes back to the start of the episode, or to the episode before it
    /// when this one has hardly begun. With no episode before it, the
    /// start is the only place to go back to.
    fn go_to_previous(&self) -> player::Result<()> {
        let ivars = self.ivars();
        let position = ivars.player.position()?.unwrap_or_default();
        let is_first =
            ivars.neighbours.borrow().as_ref().is_some_and(|known| known.previous.is_none());
        if position <= RESTART_AFTER && !is_first {
            self.close(Outcome::Previous);
            return Ok(());
        }
        ivars.player.seek(Duration::ZERO)?;
        self.notify("From the start");
        Ok(())
    }

    /// Goes to the next episode, unless the season is known to end here.
    fn go_to_next(&self) {
        let is_last =
            self.ivars().neighbours.borrow().as_ref().is_some_and(|known| known.next.is_none());
        if is_last {
            self.notify("Last episode of the season");
            self.show_controls(true);
            return;
        }
        self.close(Outcome::Finished);
    }

    /// Jumps `seconds` forward, or back if negative.
    fn jump(&self, seconds: f64, press: Press) -> player::Result<()> {
        let player = &self.ivars().player;
        match press {
            Press::First => {
                player.seek_by(seconds)?;
                // U+2212 is the minus sign, as wide as the plus.
                let sign = if seconds < 0.0 { '\u{2212}' } else { '+' };
                self.notify(&format!("{sign}{} s", seconds.abs()));
            }
            // An exact jump takes longer than the gap between repeats, so a
            // held key would stand still. Keyframe to keyframe keeps up.
            Press::Held => {
                player.scan_by(seconds)?;
                // Where a held key has got to says more than the size of
                // one step, repeated for as long as the key is down.
                if let Some(position) = player.position()? {
                    self.notify(&clock(position));
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
            controls.bubble.setHidden(true);
            return;
        };
        let time = Duration::from_secs_f64(place.value.max(0.0));
        let chapters = ivars.chapters.borrow();
        let chapter = chapters.iter().rev().find(|chapter| chapter.start <= time);
        let text = match chapter.and_then(|chapter| chapter.title.as_deref()) {
            Some(title) => format!("{}  {title}", clock(time)),
            None => clock(time),
        };
        controls.bubble.set_title(&text);
        controls.bubble_x.setConstant(place.x);
        controls.bubble.setHidden(false);
    }

    fn change_volume(&self, by: f64) -> player::Result<()> {
        let player = &self.ivars().player;
        let volume = (player.volume()? + by).clamp(0.0, controls::FULL_VOLUME);
        player.set_volume(volume)?;
        self.notify(&format!("Volume {volume:.0}%"));
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
        self.notify(if muted { "Muted" } else { "Sound on" });
        Ok(())
    }

    /// Sets the speed to `speed` tenths, kept within what stays watchable.
    fn set_speed(&self, speed: u32) -> player::Result<()> {
        let speed = speed.clamp(SLOWEST_SPEED, FASTEST_SPEED);
        self.ivars().player.set_speed(f64::from(speed) / 10.0)?;
        self.ivars().speed.set(speed);
        self.show_speed();
        self.notify(&speed_text(speed));
        Ok(())
    }

    fn show_speed(&self) {
        self.ivars().controls.speed.set_title(&speed_text(self.speed()));
    }

    /// The file being played, as the server describes it.
    fn part(&self) -> Option<&Part> {
        self.ivars().item.media.first().and_then(|media| media.parts.first())
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
        let name = chosen.map(|track| tracks::name(track, self.part()));
        self.remember_track(kind);
        let label = if kind == TrackKind::Audio { "Audio" } else { "Subtitles" };
        self.notify(&format!("{label}: {}", name.as_deref().unwrap_or("Off")));
        Ok(())
    }

    /// Saves the track of `kind` now in use to the server, so the same
    /// choice is made the next time this file is played, here or in any
    /// other Plex app. Only that kind is touched: changing the audio must
    /// not disturb the subtitles the server remembers.
    fn remember_track(&self, kind: TrackKind) {
        let ivars = self.ivars();
        let (Some(part), Purpose::Watching) = (self.part(), ivars.purpose) else {
            return;
        };
        let tracks = match ivars.player.tracks() {
            Ok(tracks) => tracks,
            Err(error) => return eprintln!("plex-rust: {error}"),
        };
        // The player's id for a track is its position among the tracks of
        // its kind, which is also the order the server lists them in.
        let selected = tracks.iter().find(|track| track.kind == kind && track.selected);
        let stream = selected.and_then(|track| {
            let position = usize::try_from(track.id).ok()?.checked_sub(1)?;
            part.embedded(tracks::stream_kind(kind)).nth(position).map(|stream| stream.id)
        });
        let (audio, subtitles) = match (kind, selected, stream) {
            (TrackKind::Audio, _, Some(stream)) => (Some(stream), None),
            (TrackKind::Subtitles, _, Some(stream)) => (None, Some(Some(stream))),
            // No subtitle track is selected: the viewer turned them off.
            (TrackKind::Subtitles, None, None) => (None, Some(None)),
            // A track the server does not list at that position. Saving a
            // guess would be worse than saving nothing.
            _ => return,
        };

        let (server, part) = (ivars.server.clone(), part.clone());
        let save = async move { server.choose_streams(&part, audio, subtitles).await };
        ivars.background.run(self.mtm(), save, |saved| {
            if let Err(error) = saved {
                eprintln!("plex-rust: track choice was not saved: {}", with_causes(&error));
            }
        });
    }

    /// Pops up a menu of the tracks of `kind` over its button.
    fn offer_tracks(&self, kind: TrackKind) -> player::Result<()> {
        let controls = &self.ivars().controls;
        let tracks = self.ivars().player.tracks()?;
        let tracks: Vec<&Track> = tracks.iter().filter(|track| track.kind == kind).collect();
        let mut choices = Vec::new();
        if kind == TrackKind::Subtitles {
            choices.push(Choice {
                title: "Off".to_owned(),
                tag: 0,
                chosen: tracks.iter().all(|track| !track.selected),
            });
        }
        choices.extend(tracks.iter().map(|track| Choice {
            title: tracks::name(track, self.part()),
            tag: track.id as isize,
            chosen: track.selected,
        }));
        match kind {
            TrackKind::Audio => self.offer(&choices, sel!(audioChosen:), &controls.audio),
            _ => self.offer(&choices, sel!(subtitlesChosen:), &controls.subtitles),
        }
        Ok(())
    }

    /// Pops up a menu of playback speeds over the speed button.
    fn offer_speeds(&self) {
        let choices: Vec<Choice> = SPEED_CHOICES
            .iter()
            .map(|speed| Choice {
                title: speed_text(*speed),
                tag: *speed as isize,
                chosen: *speed == self.speed(),
            })
            .collect();
        self.offer(&choices, sel!(speedChosen:), &self.ivars().controls.speed);
    }

    /// Pops up a menu of `choices` just above `chip`. Picking one sends
    /// `action`, a method of this class, with the menu item.
    fn offer(&self, choices: &[Choice], action: Sel, chip: &Chip) {
        // The menu runs its own event loop. The file can end inside it,
        // which closes the screen and would otherwise free it under us.
        let _alive = self.retain();
        let main_thread = self.mtm();
        let menu = NSMenu::new(main_thread);
        for choice in choices {
            // SAFETY: the caller passes a selector this class defines.
            let entry = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(main_thread),
                    &NSString::from_str(&choice.title),
                    Some(action),
                    ns_string!(""),
                )
            };
            // SAFETY: a menu item does not retain its target, and the
            // screen is kept alive above for as long as the menu is up.
            unsafe { entry.setTarget(Some(self)) };
            entry.setTag(choice.tag);
            if choice.chosen {
                entry.setState(NSControlStateValueOn);
            }
            menu.addItem(&entry);
        }

        // A menu hangs down from the point it is given, so to sit on top
        // of the chip it starts its own height above it.
        let above = NSPoint::new(0.0, chip.bounds().size.height + menu.size().height);
        // Blocks until the menu is dismissed.
        menu.popUpMenuPositioningItem_atLocation_inView(None, above, Some(chip));
        if self.ivars().timer.borrow().is_some() {
            self.show_controls(true);
        }
    }

    /// Shows or hides the skip button to match `position`.
    fn offer_skip(&self, position: Option<Duration>) {
        let skip = &self.ivars().controls.skip;
        let Some(marker) = position.and_then(|position| self.ivars().item.marker_at(position))
        else {
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

    /// Jumps to the end of the intro or credits that is playing, if any.
    fn skip_marker(&self) -> player::Result<()> {
        let player = &self.ivars().player;
        let marker: Option<&Marker> =
            player.position()?.and_then(|position| self.ivars().item.marker_at(position));
        if let Some(marker) = marker {
            player.seek(marker.end_time_offset.into())?;
            self.ivars().controls.skip.setHidden(true);
        }
        Ok(())
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

    /// Says `text` in the corner for a moment.
    fn notify(&self, text: &str) {
        let ivars = self.ivars();
        ivars.controls.notice.set_title(text);
        ivars.ticks_of_notice_left.set(TICKS_OF_NOTICE);
        self.set_notice_shown(true);
    }

    fn set_notice_shown(&self, shown: bool) {
        // A notice that is renewed while it is up is not faded in again.
        if self.ivars().notice_shown.replace(shown) == shown {
            return;
        }
        let notice = &self.ivars().controls.notice;
        notice.animator().setAlphaValue(if shown { 1.0 } else { 0.0 });
    }

    /// Fades the controls, the window's own buttons and the cursor in or
    /// out. Showing them also restarts the wait before they hide again.
    fn show_controls(&self, shown: bool) {
        let ivars = self.ivars();
        if shown {
            ivars.ticks_idle.set(0);
        }
        if ivars.controls_shown.replace(shown) == shown {
            return;
        }
        ivars.controls.top.set_shown(shown);
        ivars.controls.bottom.set_shown(shown);
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
        match event {
            Event::Loaded => {
                self.report(PlaybackState::Playing);
                self.or_log(self.show_chapters());
                self.or_log(self.refresh());
            }
            Event::VideoChanged => {
                // The first picture is here: the wait since opening is over.
                self.set_waiting(false);
                self.fit_to_display();
            }
            Event::Ended(EndReason::Failed(error)) => self.close(Outcome::Failed(error.clone())),
            Event::Buffering(stalled) => self.set_waiting(*stalled),
            Event::Ended(EndReason::Finished) => self.close(Outcome::Finished),
            // Stopped is our own doing, and shutdown follows `close`.
            Event::Ended(EndReason::Stopped) | Event::Shutdown => {}
        }
    }

    /// Notches the scrubber where the file's chapters start.
    fn show_chapters(&self) -> player::Result<()> {
        let ivars = self.ivars();
        let chapters = ivars.player.chapters()?;
        // The first chapter starts with the file, where a notch says nothing.
        let starts = chapters.iter().map(|chapter| chapter.start.as_secs_f64());
        ivars.controls.scrubber.set_marks(starts.filter(|start| *start > 0.0).collect());
        *ivars.chapters.borrow_mut() = chapters;
        Ok(())
    }

    fn toggle_pause(&self) -> player::Result<()> {
        let player = &self.ivars().player;
        let pause = !player.is_paused()?;
        player.set_paused(pause)?;
        let symbol = if pause { "play.fill" } else { "pause.fill" };
        self.ivars().controls.toggle.set_symbol(symbol, theme::MAIN_SYMBOL_SIZE);
        self.report(if pause { PlaybackState::Paused } else { PlaybackState::Playing });
        self.show_controls(true);
        Ok(())
    }

    /// Brings the controls up to date with the player.
    fn refresh(&self) -> player::Result<()> {
        let ivars = self.ivars();
        let (player, controls) = (&ivars.player, &ivars.controls);

        let volume = player.volume()?;
        let silent = player.is_muted()? || volume == 0.0;
        controls.volume.set_value(volume);
        let speaker = if silent { "speaker.slash.fill" } else { "speaker.wave.2.fill" };
        controls.mute.set_symbol(speaker, theme::SYMBOL_SIZE);
        let arrows = if self.is_full_screen() {
            "arrow.down.right.and.arrow.up.left"
        } else {
            "arrow.up.left.and.arrow.down.right"
        };
        controls.full_screen.set_symbol(arrows, theme::SYMBOL_SIZE);

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

    /// Tells the server where this playback is, without waiting for it.
    fn report(&self, state: PlaybackState) {
        let Some(progress) = self.progress(state) else {
            return;
        };
        let server = self.ivars().server.clone();
        let send = async move { progress.send(&server).await };
        self.ivars().background.run(self.mtm(), send, log_unsaved);
    }

    /// What to tell the server right now, or `None` if there is nothing to
    /// tell: a check, or nothing loaded yet.
    fn progress(&self, state: PlaybackState) -> Option<Progress> {
        let ivars = self.ivars();
        if ivars.purpose == Purpose::Check {
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
            Ok(Some((ivars.player.position()?.unwrap_or(duration), duration)))
        });
        match times {
            Ok(times) => times.map(|(position, duration)| Progress {
                item: ivars.item.rating_key.clone(),
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

/// One line of a pop-up menu.
struct Choice {
    title: String,
    /// What the menu item carries to the method that handles it.
    tag: isize,
    /// Whether it is the one in effect, which gets the check mark.
    chosen: bool,
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
        };
        server.report(&progress).await
    }
}

fn log_unsaved(sent: plex::Result<()>) {
    if let Err(error) = sent {
        eprintln!("plex-rust: progress was not saved: {}", with_causes(&error));
    }
}

/// `1.5×` for 15 tenths.
fn speed_text(speed: u32) -> String {
    format!("{}.{}×", speed / 10, speed % 10)
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
