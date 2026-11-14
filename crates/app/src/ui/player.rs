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
    NSBezelStyle, NSButton, NSFont, NSFontWeightRegular, NSGlassEffectView, NSImage, NSSlider,
    NSStackView, NSTextField, NSView,
};
use objc2_foundation::{
    NSArray, NSEdgeInsets, NSObject, NSObjectProtocol, NSString, NSTimer, NSUUID, ns_string,
};
use player::{EndReason, Event, Player, Source};
use plex::MediaServer;
use plex::model::{Item, RatingKey};
use plex::playback::{self, PlaybackState};

use super::layout;
use super::theme;
use crate::background::Background;
use crate::error::{Error, Result, with_causes};
use crate::video_layer::VideoLayer;

/// How often the controls catch up with the player.
const TICK: Duration = Duration::from_secs(1);

/// Plex asks to hear from a running playback every 10 seconds.
const TICKS_PER_REPORT: u32 = 10;

pub struct Ivars {
    root: Retained<NSView>,
    player: Player,
    toggle: Retained<NSButton>,
    scrubber: Retained<NSSlider>,
    elapsed: Retained<NSTextField>,
    remaining: Retained<NSTextField>,
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
            self.or_log(self.refresh());
            let ticks = self.ivars().ticks.get() + 1;
            self.ivars().ticks.set(ticks);
            if ticks.is_multiple_of(TICKS_PER_REPORT) {
                self.report_current_state();
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
        let root = NSView::new(main_thread);
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
            elapsed: time_label(main_thread),
            remaining: time_label(main_thread),
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

    /// Stops playback and gives the screen up. Safe to call more than once.
    pub fn close(&self) {
        let Some(timer) = self.ivars().timer.borrow_mut().take() else {
            return;
        };
        timer.invalidate();
        // `on_close` may drop the last other reference to this screen.
        let _alive = self.retain();
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

        let bar: [&NSView; 4] = [
            &ivars.toggle,
            &ivars.elapsed,
            &ivars.scrubber,
            &ivars.remaining,
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
        layout::dock_top_leading(&ivars.root, &close, theme::PAGE_MARGIN);
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
        self.report(if pause {
            PlaybackState::Paused
        } else {
            PlaybackState::Playing
        });
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
        ivars
            .elapsed
            .setStringValue(&NSString::from_str(&clock(position)));
        let left = duration.saturating_sub(position);
        ivars
            .remaining
            .setStringValue(&NSString::from_str(&format!("-{}", clock(left))));
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
            Ok(Some((
                ivars.player.position()?.unwrap_or(duration),
                duration,
            )))
        });
        let (position, duration) = match times {
            Ok(Some(times)) => times,
            // Nothing is loaded yet, so there is no progress to speak of.
            Ok(None) => return,
            Err(error) => return eprintln!("reel: {error}"),
        };

        let (server, item, session) = (
            ivars.server.clone(),
            ivars.item.clone(),
            ivars.session.clone(),
        );
        let send = async move {
            let progress = playback::Progress {
                item: &item,
                session: &session,
                state,
                position,
                duration,
            };
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

fn time_label(main_thread: MainThreadMarker) -> Retained<NSTextField> {
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
