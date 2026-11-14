//! The screen that plays one item: video with controls on top.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSBezelStyle, NSButton, NSCursor, NSFont, NSFontWeightRegular,
    NSGlassEffectView, NSImage, NSSlider, NSStackView, NSTextField, NSView,
};
use objc2_foundation::{
    NSArray, NSEdgeInsets, NSObject, NSObjectProtocol, NSString, NSTimer, NSUUID, ns_string,
};
use player::{EndReason, Event, Player, Source};
use plex::MediaServer;
use plex::model::{Item, RatingKey};
use plex::playback::{self, PlaybackState};

use super::input::{InputView, Key};
use super::layout;
use super::theme;
use crate::background::Background;
use crate::error::{Error, Result, with_causes};
use crate::video_layer::VideoLayer;

/// How often the screen catches up with the player.
const TICK: Duration = Duration::from_secs(1);

/// Plex asks to hear from a running playback every 10 seconds.
const TICKS_PER_REPORT: u32 = 10;

/// The controls go away after this many ticks without input.
const TICKS_UNTIL_HIDDEN: u32 = 3;

/// How far the arrow keys jump.
const SKIP: Duration = Duration::from_secs(10);

/// The playback rates that `[` and `]` step through.
const SPEEDS: [f64; 10] = [0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0];
const NORMAL_SPEED: usize = 2;

pub struct Ivars {
    root: Retained<InputView>,
    player: Player,
    toggle: Retained<NSButton>,
    scrubber: Retained<NSSlider>,
    elapsed: Retained<NSTextField>,
    remaining: Retained<NSTextField>,
    speed_readout: Retained<NSTextField>,
    /// Index into `SPEEDS`.
    speed: Cell<usize>,
    /// The views that sit on top of the video and hide when idle.
    controls: RefCell<Vec<Retained<NSView>>>,
    controls_shown: Cell<bool>,
    ticks_idle: Cell<u32>,
    /// Running from open to close. Its absence means the screen is closed.
    timer: RefCell<Option<Retained<NSTimer>>>,
    ticks: Cell<u32>,
    background: Rc<Background>,
    server: MediaServer,
    item: RatingKey,
    /// Lets the server tell this playback from another of the same item.
    session: String,
    on_close: Box<dyn Fn()>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `PlayerScreen`
    // does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ReelPlayerScreen"]
    #[ivars = Ivars]
    pub struct PlayerScreen;

    unsafe impl NSObjectProtocol for PlayerScreen {}

    impl PlayerScreen {
        #[unsafe(method(toggle:))]
        fn toggle(&self, _sender: Option<&AnyObject>) {
            self.or_log(self.toggle_pause());
        }

        #[unsafe(method(scrub:))]
        fn scrub(&self, _sender: Option<&AnyObject>) {
            let position = Duration::from_secs_f64(self.ivars().scrubber.doubleValue().max(0.0));
            self.or_log(self.ivars().player.seek(position));
        }

        #[unsafe(method(close:))]
        fn close_clicked(&self, _sender: Option<&AnyObject>) {
            self.close();
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: Option<&AnyObject>) {
            let ivars = self.ivars();
            let ticks = ivars.ticks.get() + 1;
            ivars.ticks.set(ticks);
            if ticks.is_multiple_of(TICKS_PER_REPORT) {
                self.report_current_state();
            }

            if !ivars.controls_shown.get() {
                // Nothing on screen needs the player's state.
                return;
            }
            self.or_log(self.refresh());
            let idle = ivars.ticks_idle.get() + 1;
            ivars.ticks_idle.set(idle);
            // A paused picture keeps its controls.
            if idle >= TICKS_UNTIL_HIDDEN && ivars.player.is_paused().is_ok_and(|paused| !paused) {
                self.show_controls(false);
            }
        }
    }
);

impl PlayerScreen {
    /// Starts playing `item` from where it was left. `on_close` is called
    /// once, when the screen is done, whether closed or played to the end.
    pub fn open(
        main_thread: MainThreadMarker,
        background: Rc<Background>,
        server: MediaServer,
        item: &Item,
        muted: bool,
        on_close: impl Fn() + 'static,
    ) -> Result<Retained<Self>> {
        let part = item.media.first().and_then(|media| media.parts.first());
        let part = part.ok_or_else(|| Error::NotPlayable(item.title.clone()))?;
        let file = server.original_file(part);
        let source = Source {
            location: file.url,
            headers: file.headers,
            start: item.view_offset.map(Into::into).unwrap_or_default(),
        };

        let player = Player::new()?;
        player.set_muted(muted)?;
        let root = InputView::new(main_thread);
        let video = NSView::new(main_thread);
        let layer = VideoLayer::new(&player, main_thread)?;
        video.setLayer(Some(&layer));
        video.setWantsLayer(true);
        layout::cover(&root, &video);

        let this = Self::alloc(main_thread).set_ivars(Ivars {
            root,
            player,
            toggle: NSButton::new(main_thread),
            scrubber: NSSlider::new(main_thread),
            elapsed: readout(main_thread),
            remaining: readout(main_thread),
            speed_readout: readout(main_thread),
            speed: Cell::new(NORMAL_SPEED),
            controls: RefCell::new(Vec::new()),
            controls_shown: Cell::new(true),
            ticks_idle: Cell::new(0),
            timer: RefCell::new(None),
            ticks: Cell::new(0),
            background,
            server,
            item: item.rating_key.clone(),
            session: NSUUID::UUID().UUIDString().to_string(),
            on_close: Box::new(on_close),
        });
        // SAFETY: `init` is NSObject's designated initializer.
        let screen: Retained<Self> = unsafe { msg_send![super(this), init] };
        screen.build_controls(main_thread);
        screen.listen();

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
        screen.ivars().player.load(&source)?;
        Ok(screen)
    }

    pub fn view(&self) -> &NSView {
        &self.ivars().root
    }

    /// Directs the keyboard at the player. Call once the view is in a
    /// window.
    pub fn focus(&self) {
        let root = &self.ivars().root;
        if let Some(window) = root.window() {
            window.makeFirstResponder(Some(root));
        }
    }

    /// Stops playback and gives the screen up. Safe to call more than once.
    pub fn close(&self) {
        let Some(timer) = self.ivars().timer.borrow_mut().take() else {
            return;
        };
        timer.invalidate();
        // `on_close` may drop the last other reference to this screen.
        let _alive = self.retain();
        NSCursor::setHiddenUntilMouseMoves(false);
        self.report(PlaybackState::Stopped);
        self.or_log(self.ivars().player.quit());
        (self.ivars().on_close)();
    }

    fn build_controls(&self, main_thread: MainThreadMarker) {
        let ivars = self.ivars();
        // SAFETY: none of these controls retains its target. The screen
        // owns the views they are in, so it outlives every action.
        unsafe {
            ivars.toggle.setTarget(Some(self));
            ivars.toggle.setAction(Some(sel!(toggle:)));
            ivars.scrubber.setTarget(Some(self));
            ivars.scrubber.setAction(Some(sel!(scrub:)));
        }
        ivars.toggle.setBordered(false);
        ivars.toggle.setImage(symbol("pause.fill").as_deref());
        // The keyboard belongs to the screen, not to whichever control was
        // clicked last.
        ivars.toggle.setRefusesFirstResponder(true);
        ivars.scrubber.setRefusesFirstResponder(true);
        self.show_speed();

        let bar: [&NSView; 5] = [
            &ivars.toggle,
            &ivars.elapsed,
            &ivars.scrubber,
            &ivars.remaining,
            &ivars.speed_readout,
        ];
        let bar = NSStackView::stackViewWithViews(&NSArray::from_slice(&bar), main_thread);
        bar.setSpacing(theme::CONTROL_SPACING);
        let padding = theme::CONTROL_SPACING;
        bar.setEdgeInsets(NSEdgeInsets {
            top: padding,
            left: padding,
            bottom: padding,
            right: padding,
        });
        let glass = NSGlassEffectView::new(main_thread);
        glass.setContentView(Some(&bar));
        glass.setCornerRadius(theme::CONTROL_BAR_RADIUS);
        layout::dock_bottom(&ivars.root, &glass, theme::PAGE_MARGIN);

        let close = NSButton::new(main_thread);
        // SAFETY: as above.
        unsafe {
            close.setTarget(Some(self));
            close.setAction(Some(sel!(close:)));
        }
        close.setBezelStyle(NSBezelStyle::Glass);
        close.setImage(symbol("chevron.left").as_deref());
        close.setTitle(ns_string!(""));
        close.setRefusesFirstResponder(true);
        layout::dock_top_leading(&ivars.root, &close, theme::PAGE_MARGIN);

        *ivars.controls.borrow_mut() =
            vec![Retained::into_super(glass), Retained::into_super(Retained::into_super(close))];
    }

    /// Connects the keyboard and the mouse to the screen.
    fn listen(&self) {
        let (keys, mouse) = (Weak::new(self), Weak::new(self));
        self.ivars().root.listen(
            move |key| keys.load().is_some_and(|screen| screen.press(key)),
            move || {
                if let Some(screen) = mouse.load() {
                    screen.show_controls(true);
                }
            },
        );
    }

    /// Acts on a key. Returns whether the key means anything here.
    fn press(&self, key: Key) -> bool {
        let result = match key {
            Key::Space => self.toggle_pause(),
            Key::Left => self.skip(|position| position.saturating_sub(SKIP)),
            Key::Right => self.skip(|position| position + SKIP),
            Key::Character('[') => self.step_speed(|speed| speed.saturating_sub(1)),
            Key::Character(']') => self.step_speed(|speed| (speed + 1).min(SPEEDS.len() - 1)),
            Key::Character('f') => {
                if let Some(window) = self.ivars().root.window() {
                    window.toggleFullScreen(None);
                }
                Ok(())
            }
            Key::Escape => {
                self.close();
                return true;
            }
            Key::Character(_) => return false,
        };
        self.or_log(result);
        self.show_controls(true);
        true
    }

    fn skip(&self, to: impl FnOnce(Duration) -> Duration) -> player::Result<()> {
        let player = &self.ivars().player;
        match player.position()? {
            Some(position) => player.seek(to(position)),
            None => Ok(()),
        }
    }

    fn step_speed(&self, to: impl FnOnce(usize) -> usize) -> player::Result<()> {
        let speed = to(self.ivars().speed.get());
        self.ivars().player.set_speed(SPEEDS[speed])?;
        self.ivars().speed.set(speed);
        self.show_speed();
        Ok(())
    }

    fn show_speed(&self) {
        let speed = SPEEDS[self.ivars().speed.get()];
        self.ivars().speed_readout.setStringValue(&NSString::from_str(&format!("{speed}×")));
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
            Event::Ended(EndReason::Failed(error)) => {
                eprintln!("reel: playback failed: {error}");
                self.close();
            }
            Event::Ended(EndReason::Finished) => self.close(),
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
            Err(error) => eprintln!("reel: {error}"),
        }
    }

    /// Tells the server where this playback is, without waiting for it.
    fn report(&self, state: PlaybackState) {
        let ivars = self.ivars();
        let times = ivars.player.duration().and_then(|duration| {
            let Some(duration) = duration else {
                return Ok(None);
            };
            // mpv has no position once a file has played to its end.
            Ok(Some((ivars.player.position()?.unwrap_or(duration), duration)))
        });
        let (position, duration) = match times {
            Ok(Some(times)) => times,
            // Nothing is loaded yet, so there is no progress to speak of.
            Ok(None) => return,
            Err(error) => return eprintln!("reel: {error}"),
        };

        let (server, item, session) =
            (ivars.server.clone(), ivars.item.clone(), ivars.session.clone());
        let send = async move {
            let progress =
                playback::Progress { item: &item, session: &session, state, position, duration };
            server.report(&progress).await
        };
        ivars.background.run(self.mtm(), send, |sent| {
            if let Err(error) = sent {
                eprintln!("reel: progress was not saved: {}", with_causes(&error));
            }
        });
    }

    fn or_log(&self, result: player::Result<()>) {
        if let Err(error) = result {
            eprintln!("reel: {error}");
        }
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

/// A label for a number that changes while it is on screen.
fn readout(main_thread: MainThreadMarker) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(ns_string!("0:00"), main_thread);
    // SAFETY: the weight is a constant that AppKit provides.
    let regular = unsafe { NSFontWeightRegular };
    // Digits of equal width keep the scrubber from shifting every second.
    label.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
        theme::TITLE_FONT_SIZE,
        regular,
    )));
    label
}

fn symbol(name: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
}
