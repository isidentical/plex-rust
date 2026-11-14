//! The screen that plays one item: video with controls on top.

pub mod input;
mod layout;
mod theme;

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSBezelStyle, NSButton, NSControlStateValueOn, NSCursor, NSFont,
    NSFontWeightRegular, NSFontWeightSemibold, NSGlassEffectView, NSImage, NSMenu, NSMenuItem,
    NSProgressIndicator, NSProgressIndicatorStyle, NSSlider, NSStackView, NSTextField, NSView,
    NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSEdgeInsets, NSObject, NSObjectProtocol, NSPoint, NSString, NSTimer, NSUUID,
    ns_string,
};
use player::{EndReason, Event, Player, Track, TrackKind};
use plex::MediaServer;
use plex::model::{Item, Kind, Marker, MarkerKind, StreamKind};
use plex::playback::{self, PlaybackState};

use self::input::{Clicks, InputView, Key};
use crate::background::Background;
use crate::error::{Result, with_causes};
use crate::settings::Settings;
use crate::source;
use crate::video_layer::{self, VideoLayer};

/// How often the screen catches up with the player.
const TICK: Duration = Duration::from_secs(1);

/// Plex asks to hear from a running playback every 10 seconds.
const TICKS_PER_REPORT: u32 = 10;

/// The controls go away after this many ticks without input.
const TICKS_UNTIL_HIDDEN: u32 = 3;

/// A notice, such as the new volume, stays up for this many ticks.
const TICKS_OF_NOTICE: u32 = 2;

/// How far the left and right arrow keys jump.
const SHORT_JUMP: f64 = 3.0;

/// How far they jump with Shift held.
const LONG_JUMP: f64 = 10.0;

/// How much the up and down arrow keys change the volume, out of 100.
const VOLUME_STEP: f64 = 5.0;

/// Playback speed is kept in tenths so that steps of 0.1 stay exact.
const NORMAL_SPEED: u32 = 10;
const SLOWEST_SPEED: u32 = 1;
const FASTEST_SPEED: u32 = 40;

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
}

pub struct Ivars {
    root: Retained<InputView>,
    /// The view that hosts the video layer.
    video: Retained<NSView>,
    player: Player,
    item: Item,
    toggle: Retained<NSButton>,
    scrubber: Retained<NSSlider>,
    elapsed: Retained<NSTextField>,
    remaining: Retained<NSTextField>,
    speed_readout: Retained<NSTextField>,
    audio: Retained<NSButton>,
    subtitles: Retained<NSButton>,
    /// Offers to jump past the intro or the credits while they play.
    skip: Retained<NSButton>,
    /// Turns while the picture is on its way: at the start, and whenever
    /// playback stalls waiting for the network.
    spinner: Retained<NSProgressIndicator>,
    /// Briefly says what a key press did.
    notice: Retained<NSTextField>,
    /// The pane of glass the notice sits on.
    notice_pane: OnceCell<Retained<NSView>>,
    ticks_of_notice_left: Cell<u32>,
    /// In tenths: 15 is one and a half times normal speed.
    speed: Cell<u32>,
    /// The views that sit on top of the video and hide when idle.
    controls: RefCell<Vec<Retained<NSView>>>,
    controls_shown: Cell<bool>,
    ticks_idle: Cell<u32>,
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
        #[unsafe(method(toggle:))]
        fn toggle_clicked(&self, _sender: Option<&AnyObject>) {
            self.or_log(self.toggle_pause());
        }

        #[unsafe(method(scrub:))]
        fn scrubbed(&self, _sender: Option<&AnyObject>) {
            let position = Duration::from_secs_f64(self.ivars().scrubber.doubleValue().max(0.0));
            self.or_log(self.ivars().player.seek(position));
        }

        #[unsafe(method(close:))]
        fn close_clicked(&self, _sender: Option<&AnyObject>) {
            self.close(Outcome::Closed);
        }

        #[unsafe(method(skip:))]
        fn skip_clicked(&self, _sender: Option<&AnyObject>) {
            self.or_log(self.skip_marker());
        }

        #[unsafe(method(fullScreen:))]
        fn full_screen_clicked(&self, _sender: Option<&AnyObject>) {
            self.toggle_full_screen();
        }

        #[unsafe(method(chooseAudio:))]
        fn choose_audio(&self, sender: &NSButton) {
            self.or_log(self.offer_tracks(TrackKind::Audio, sender));
        }

        #[unsafe(method(chooseSubtitles:))]
        fn choose_subtitles(&self, sender: &NSButton) {
            self.or_log(self.offer_tracks(TrackKind::Subtitles, sender));
        }

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
        player.set_volume(settings.volume.clamp(0.0, 100.0))?;

        let root = InputView::new(main_thread);
        let video = NSView::new(main_thread);
        let layer = VideoLayer::new(&player, main_thread)?;
        video.setLayer(Some(&layer));
        video.setWantsLayer(true);
        crate::ui::layout::cover(&root, &video);

        let this = Self::alloc(main_thread).set_ivars(Ivars {
            root,
            video,
            player,
            item: item.clone(),
            toggle: NSButton::new(main_thread),
            scrubber: NSSlider::new(main_thread),
            elapsed: readout(main_thread),
            remaining: readout(main_thread),
            speed_readout: readout(main_thread),
            audio: NSButton::new(main_thread),
            subtitles: NSButton::new(main_thread),
            skip: NSButton::new(main_thread),
            spinner: NSProgressIndicator::new(main_thread),
            notice: readout(main_thread),
            notice_pane: OnceCell::new(),
            ticks_of_notice_left: Cell::new(0),
            speed: Cell::new(speed),
            controls: RefCell::new(Vec::new()),
            controls_shown: Cell::new(true),
            ticks_idle: Cell::new(0),
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
        screen.build_controls(main_thread);
        screen.listen();
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

    /// Directs the keyboard at the player. Call once the view is in a
    /// window.
    pub fn focus(&self) {
        let root = &self.ivars().root;
        if let Some(window) = root.window() {
            window.makeFirstResponder(Some(root));
        }
        self.fit_to_display();
    }

    /// Shows or hides the spinner that says the picture is on its way.
    fn set_waiting(&self, waiting: bool) {
        let spinner = &self.ivars().spinner;
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

    /// Stops the timer. Returns whether the screen was still open.
    fn stop_ticking(&self) -> bool {
        let Some(timer) = self.ivars().timer.borrow_mut().take() else {
            return false;
        };
        timer.invalidate();
        NSCursor::setHiddenUntilMouseMoves(false);
        true
    }

    fn build_controls(&self, main_thread: MainThreadMarker) {
        let ivars = self.ivars();
        self.wire(&ivars.toggle, "pause.fill", sel!(toggle:));
        self.wire(&ivars.audio, "waveform", sel!(chooseAudio:));
        self.wire(&ivars.subtitles, "captions.bubble", sel!(chooseSubtitles:));
        let full_screen = NSButton::new(main_thread);
        self.wire(&full_screen, "arrow.up.left.and.arrow.down.right", sel!(fullScreen:));
        // SAFETY: the scrubber does not retain its target. The screen owns
        // the view it is in, so it outlives every action.
        unsafe {
            ivars.scrubber.setTarget(Some(self));
            ivars.scrubber.setAction(Some(sel!(scrub:)));
        }
        // The keyboard belongs to the screen, not to the control that was
        // clicked last.
        ivars.scrubber.setRefusesFirstResponder(true);
        self.show_speed();

        let bar: [&NSView; 8] = [
            &ivars.toggle,
            &ivars.elapsed,
            &ivars.scrubber,
            &ivars.remaining,
            &ivars.speed_readout,
            &ivars.audio,
            &ivars.subtitles,
            &full_screen,
        ];
        let bar = glass(&row(&bar, main_thread), main_thread);
        layout::dock_bottom(&ivars.root, &bar, theme::MARGIN);

        let close = NSButton::new(main_thread);
        self.wire(&close, "chevron.left", sel!(close:));
        let title =
            NSTextField::labelWithString(&NSString::from_str(&title(&ivars.item)), main_thread);
        // SAFETY: the weight is a constant that AppKit provides.
        let semibold = unsafe { NSFontWeightSemibold };
        title.setFont(Some(&NSFont::systemFontOfSize_weight(theme::FONT_SIZE, semibold)));
        let heading: [&NSView; 2] = [&close, &title];
        let heading = glass(&row(&heading, main_thread), main_thread);
        layout::dock_top_leading(&ivars.root, &heading, theme::MARGIN);

        // SAFETY: as for the scrubber.
        unsafe {
            ivars.skip.setTarget(Some(self));
            ivars.skip.setAction(Some(sel!(skip:)));
        }
        ivars.skip.setBezelStyle(NSBezelStyle::Glass);
        ivars.skip.setRefusesFirstResponder(true);
        ivars.skip.setHidden(true);
        layout::dock_above(&ivars.root, &ivars.skip, &bar, theme::CONTROL_SPACING);

        let notice = glass(&row(&[&*ivars.notice as &NSView], main_thread), main_thread);
        notice.setAlphaValue(0.0);
        layout::dock_top_trailing(&ivars.root, &notice, theme::MARGIN);
        // Cannot fail: the controls are built once.
        let _ = ivars.notice_pane.set(notice);

        ivars.spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        ivars.spinner.setIndeterminate(true);
        ivars.spinner.setDisplayedWhenStopped(false);
        layout::dock_center(&ivars.root, &ivars.spinner);
        self.set_waiting(true);

        // The skip button, the notice and the spinner come and go by
        // themselves.
        *ivars.controls.borrow_mut() = vec![bar, heading];
    }

    /// Makes `button` a borderless symbol that calls `action` on the screen.
    fn wire(&self, button: &NSButton, symbol_name: &str, action: Sel) {
        // SAFETY: the button does not retain its target. The screen owns
        // the view the button is in, so it outlives every click.
        unsafe {
            button.setTarget(Some(self));
            button.setAction(Some(action));
        }
        button.setBordered(false);
        button.setImage(symbol(symbol_name).as_deref());
        button.setTitle(ns_string!(""));
        button.setRefusesFirstResponder(true);
    }

    /// Connects the keyboard and the mouse to the screen.
    fn listen(&self) {
        let (keys, mouse, clicks, display) =
            (Weak::new(self), Weak::new(self), Weak::new(self), Weak::new(self));
        self.ivars().root.listen(
            move |key| keys.load().is_some_and(|screen| screen.press(key)),
            move || {
                if let Some(screen) = mouse.load() {
                    screen.show_controls(true);
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

    /// Acts on a key. Returns whether the key means anything here.
    pub fn press(&self, key: Key) -> bool {
        let result = match key {
            Key::Space | Key::Character('k') => self.toggle_pause(),
            Key::Left => self.jump(-SHORT_JUMP),
            Key::Right => self.jump(SHORT_JUMP),
            Key::ShiftLeft => self.jump(-LONG_JUMP),
            Key::ShiftRight => self.jump(LONG_JUMP),
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
            Key::Character('n') => {
                self.close(Outcome::Finished);
                return true;
            }
            Key::Escape => {
                // Escape leaves full screen first, as it does everywhere.
                if self.is_full_screen() {
                    self.toggle_full_screen();
                } else {
                    self.close(Outcome::Closed);
                }
                return true;
            }
            Key::Character(_) => return false,
        };
        self.or_log(result);
        self.show_controls(true);
        true
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

        // The position is only read when something on screen depends on it:
        // the controls, or an intro or credits that could be skipped.
        let shown = ivars.controls_shown.get();
        if shown || !ivars.item.markers.is_empty() {
            let position = ivars.player.position()?;
            self.offer_skip(position);
        }
        if !shown {
            return Ok(());
        }
        self.refresh()?;
        let idle = ivars.ticks_idle.get() + 1;
        ivars.ticks_idle.set(idle);
        // A paused picture keeps its controls.
        if idle >= TICKS_UNTIL_HIDDEN && !ivars.player.is_paused()? {
            self.show_controls(false);
        }
        Ok(())
    }

    /// Jumps `seconds` forward, or back if negative.
    fn jump(&self, seconds: f64) -> player::Result<()> {
        self.ivars().player.seek_by(seconds)?;
        // U+2212 is the minus sign, as wide as the plus.
        let sign = if seconds < 0.0 { '\u{2212}' } else { '+' };
        self.notify(&format!("{sign}{} s", seconds.abs()));
        Ok(())
    }

    fn change_volume(&self, by: f64) -> player::Result<()> {
        let player = &self.ivars().player;
        let volume = (player.volume()? + by).clamp(0.0, 100.0);
        player.set_volume(volume)?;
        self.notify(&format!("Volume {volume:.0}%"));
        Ok(())
    }

    fn toggle_mute(&self) -> player::Result<()> {
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
        let text = speed_text(self.speed());
        self.ivars().speed_readout.setStringValue(&NSString::from_str(&text));
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
        let name = next.or(tracks.first().filter(|_| kind == TrackKind::Audio)).map(track_name);
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
        let part = ivars.item.media.first().and_then(|media| media.parts.first());
        let (Some(part), Purpose::Watching) = (part, ivars.purpose) else {
            return;
        };
        let tracks = match ivars.player.tracks() {
            Ok(tracks) => tracks,
            Err(error) => return eprintln!("plex-rust: {error}"),
        };
        let stream_kind = match kind {
            TrackKind::Audio => StreamKind::Audio,
            TrackKind::Subtitles => StreamKind::Subtitle,
            TrackKind::Video => return,
        };
        // The player's id for a track is its position among the tracks of
        // its kind, which is also the order the server lists them in.
        let selected = tracks.iter().find(|track| track.kind == kind && track.selected);
        let stream = selected.and_then(|track| {
            let position = usize::try_from(track.id).ok()?.checked_sub(1)?;
            part.embedded(stream_kind).nth(position).map(|stream| stream.id)
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

    /// Pops up a menu of the tracks of `kind` under `button`.
    fn offer_tracks(&self, kind: TrackKind, button: &NSButton) -> player::Result<()> {
        // The menu runs its own event loop. The file can end inside it,
        // which closes the screen and would otherwise free it under us.
        let _alive = self.retain();
        let main_thread = self.mtm();
        let tracks = self.ivars().player.tracks()?;
        let tracks: Vec<&Track> = tracks.iter().filter(|track| track.kind == kind).collect();
        let action = match kind {
            TrackKind::Audio => sel!(audioChosen:),
            _ => sel!(subtitlesChosen:),
        };

        let menu = NSMenu::new(main_thread);
        let add = |title: &str, tag: isize, selected: bool| {
            // SAFETY: both selectors are defined on this class.
            let entry = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(main_thread),
                    &NSString::from_str(title),
                    Some(action),
                    ns_string!(""),
                )
            };
            // SAFETY: a menu item does not retain its target, and the
            // screen is kept alive above for as long as the menu is up.
            unsafe { entry.setTarget(Some(self)) };
            entry.setTag(tag);
            if selected {
                entry.setState(NSControlStateValueOn);
            }
            menu.addItem(&entry);
        };
        if kind == TrackKind::Subtitles {
            add("Off", 0, tracks.iter().all(|track| !track.selected));
        }
        for track in tracks {
            add(&track_name(track), track.id as isize, track.selected);
        }

        // Blocks until the menu is dismissed.
        menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::ZERO, Some(button));
        if self.ivars().timer.borrow().is_some() {
            self.show_controls(true);
        }
        Ok(())
    }

    /// Shows or hides the skip button to match `position`.
    fn offer_skip(&self, position: Option<Duration>) {
        let skip = &self.ivars().skip;
        let Some(marker) = position.and_then(|position| self.ivars().item.marker_at(position))
        else {
            skip.setHidden(true);
            return;
        };
        let title = match marker.kind {
            MarkerKind::Intro => "Skip Intro",
            MarkerKind::Credits => "Skip Credits",
            MarkerKind::Other => "Skip",
        };
        skip.setTitle(&NSString::from_str(title));
        skip.setHidden(false);
    }

    /// Jumps to the end of the intro or credits that is playing, if any.
    fn skip_marker(&self) -> player::Result<()> {
        let player = &self.ivars().player;
        let marker: Option<&Marker> =
            player.position()?.and_then(|position| self.ivars().item.marker_at(position));
        if let Some(marker) = marker {
            player.seek(marker.end_time_offset.into())?;
            self.ivars().skip.setHidden(true);
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
        ivars.notice.setStringValue(&NSString::from_str(text));
        ivars.ticks_of_notice_left.set(TICKS_OF_NOTICE);
        self.set_notice_shown(true);
    }

    fn set_notice_shown(&self, shown: bool) {
        if let Some(pane) = self.ivars().notice_pane.get() {
            pane.animator().setAlphaValue(if shown { 1.0 } else { 0.0 });
        }
    }

    /// Fades the controls in or out. Showing them also restarts the wait
    /// before they hide again.
    fn show_controls(&self, shown: bool) {
        let ivars = self.ivars();
        if shown {
            ivars.ticks_idle.set(0);
        }
        if ivars.controls_shown.replace(shown) == shown {
            return;
        }
        for control in ivars.controls.borrow().iter() {
            control.animator().setAlphaValue(if shown { 1.0 } else { 0.0 });
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
            Event::Loaded => self.report(PlaybackState::Playing),
            Event::VideoChanged => {
                // The first picture is here: the wait since opening is over.
                self.set_waiting(false);
                let ivars = self.ivars();
                self.or_log(video_layer::fit_to_display(&ivars.video, &ivars.player));
            }
            Event::Ended(EndReason::Failed(error)) => self.close(Outcome::Failed(error.clone())),
            Event::Buffering(stalled) => self.set_waiting(*stalled),
            Event::Ended(EndReason::Finished) => self.close(Outcome::Finished),
            // Stopped is our own doing, and shutdown follows `close`.
            Event::Ended(EndReason::Stopped) | Event::Shutdown => {}
        }
    }

    fn toggle_pause(&self) -> player::Result<()> {
        let player = &self.ivars().player;
        let pause = !player.is_paused()?;
        player.set_paused(pause)?;
        let image = symbol(if pause { "play.fill" } else { "pause.fill" });
        self.ivars().toggle.setImage(image.as_deref());
        self.report(if pause { PlaybackState::Paused } else { PlaybackState::Playing });
        self.show_controls(true);
        Ok(())
    }

    /// Brings the scrubber and the two times up to date.
    fn refresh(&self) -> player::Result<()> {
        let ivars = self.ivars();
        let (Some(position), Some(duration)) = (ivars.player.position()?, ivars.player.duration()?)
        else {
            return Ok(());
        };
        ivars.scrubber.setMaxValue(duration.as_secs_f64());
        ivars.scrubber.setDoubleValue(position.as_secs_f64());
        ivars.elapsed.setStringValue(&NSString::from_str(&clock(position)));
        let left = duration.saturating_sub(position);
        ivars.remaining.setStringValue(&NSString::from_str(&format!("-{}", clock(left))));
        Ok(())
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

/// What the player calls the item: an episode by its show, season and
/// episode numbers and name, anything else by its title.
fn title(item: &Item) -> String {
    match (item.kind, &item.grandparent_title, item.parent_index, item.index) {
        (Kind::Episode, Some(show), Some(season), Some(episode)) => {
            format!("{show} · S{season} E{episode} · {}", item.title)
        }
        _ => item.title.clone(),
    }
}

/// What a track is called in a menu: its name, else its language, else its
/// number.
fn track_name(track: &Track) -> String {
    match (&track.title, &track.language) {
        (Some(title), Some(language)) => format!("{title} ({language})"),
        (Some(name), None) | (None, Some(name)) => name.clone(),
        (None, None) => format!("Track {}", track.id),
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

/// A label for text that changes while it is on screen.
fn readout(main_thread: MainThreadMarker) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(ns_string!("0:00"), main_thread);
    // SAFETY: the weight is a constant that AppKit provides.
    let regular = unsafe { NSFontWeightRegular };
    // Digits of equal width keep the scrubber from shifting every second.
    let font = NSFont::monospacedDigitSystemFontOfSize_weight(theme::FONT_SIZE, regular);
    label.setFont(Some(&font));
    label
}

/// Views side by side, with room around them.
fn row(views: &[&NSView], main_thread: MainThreadMarker) -> Retained<NSStackView> {
    let row = NSStackView::stackViewWithViews(&NSArray::from_slice(views), main_thread);
    row.setSpacing(theme::CONTROL_SPACING);
    let padding = theme::CONTROL_SPACING;
    row.setEdgeInsets(NSEdgeInsets {
        top: padding,
        left: padding,
        bottom: padding,
        right: padding,
    });
    row
}

/// `content` on a rounded pane of glass.
fn glass(content: &NSView, main_thread: MainThreadMarker) -> Retained<NSView> {
    let glass = NSGlassEffectView::new(main_thread);
    glass.setContentView(Some(content));
    glass.setCornerRadius(theme::CONTROL_BAR_RADIUS);
    Retained::into_super(glass)
}

fn symbol(name: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
}
