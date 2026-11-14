//! The layer that shows the video.
//!
//! Frames are drawn on a thread of their own, the frame clock, which a
//! display link wakes ahead of every refresh of the display. Each frame is
//! drawn for the refresh nearest the moment mpv wants it on screen. The
//! main thread takes no part in showing a frame, so nothing the interface
//! does can delay one.

#![expect(
    deprecated,
    reason = "Apple deprecated OpenGL, but it is the only API mpv can render through when embedded"
)]

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak as WeakArc, mpsc};
use std::thread;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::{Allocated, Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, define_class, msg_send, sel};
use objc2_app_kit::NSView;
use objc2_core_foundation::{CFRetained, CFRunLoop, CFTimeInterval, kCFRunLoopDefaultMode};
use objc2_core_graphics::{
    CGColorSpace, kCGColorSpaceDisplayP3_PQ, kCGColorSpaceITUR_2100_PQ, kCGColorSpaceSRGB,
};
use objc2_core_video::CVTimeStamp;
use objc2_foundation::{NSDefaultRunLoopMode, NSObject, NSPort, NSRunLoop};
use objc2_open_gl::{CGLContextObj, CGLPixelFormatObj, CGLRetainContext, CGLRetainPixelFormat};
use objc2_quartz_core::{CADisplayLink, CALayer, CAOpenGLLayer, CATransaction};
use player::{Cadence, Colors, FrameTiming, HostTime, Pending, Player, Refresh, Renderer};

/// The display link pauses after this many refreshes in a row with no
/// frame to show, about a second, so that a paused player wakes nothing.
const IDLE_REFRESHES: u32 = 120;

pub struct Ivars {
    pace: Arc<Pace>,
    /// Empty until [`fit_to_display`] finds the layer in a window.
    clock: Arc<OnceLock<Clock>>,
}

define_class!(
    // SAFETY:
    // - CAOpenGLLayer is designed to be subclassed and the overrides below
    //   are its documented customization points.
    // - `VideoLayer` does not implement `Drop`.
    #[unsafe(super(CAOpenGLLayer, CALayer, NSObject))]
    #[name = "PlexRustVideoLayer"]
    #[ivars = Ivars]
    pub struct VideoLayer;

    impl VideoLayer {
        /// Core Animation copies layers through this initializer, so the
        /// copy needs ivars too.
        #[unsafe(method_id(initWithLayer:))]
        fn init_with_layer(this: Allocated<Self>, layer: &AnyObject) -> Retained<Self> {
            // SAFETY: Core Animation only copies a layer from one of its own
            // class, so `layer` is a `VideoLayer`.
            let original = unsafe { &*std::ptr::from_ref(layer).cast::<Self>() };
            let Ivars { pace, clock } = original.ivars();
            let this = this.set_ivars(Ivars { pace: Arc::clone(pace), clock: Arc::clone(clock) });
            // SAFETY: `initWithLayer:` is CALayer's copy initializer and
            // `layer` is a valid layer.
            unsafe { msg_send![super(this), initWithLayer: layer] }
        }

        // Core Animation draws with the renderer's own context. Both `copy`
        // methods hand over a reference that Core Animation releases.

        #[unsafe(method(copyCGLPixelFormatForDisplayMask:))]
        fn pixel_format(&self, _display_mask: u32) -> CGLPixelFormatObj {
            // SAFETY: the renderer's pixel format is valid while it lives.
            unsafe { CGLRetainPixelFormat(self.ivars().pace.renderer.gl_pixel_format()) }
        }

        #[unsafe(method(copyCGLContextForPixelFormat:))]
        fn context(&self, _format: CGLPixelFormatObj) -> CGLContextObj {
            // SAFETY: the renderer's context is valid while it lives.
            unsafe { CGLRetainContext(self.ivars().pace.renderer.gl_context()) }
        }

        /// The frame clock draws frames through this, and the main thread
        /// a resized layer. One at a time: the layer draws with a single
        /// OpenGL context.
        #[unsafe(method(display))]
        fn display_one_at_a_time(&self) {
            self.ivars().pace.renderer.exclusively(|| {
                // SAFETY: `display` is CALayer's own method, called here on
                // whichever thread asked, as Core Animation allows inside a
                // transaction.
                let () = unsafe { msg_send![super(self), display] };
            });
        }

        #[unsafe(method(drawInCGLContext:pixelFormat:forLayerTime:displayTime:))]
        fn draw(
            &self,
            _context: CGLContextObj,
            _format: CGLPixelFormatObj,
            _layer_time: CFTimeInterval,
            _display_time: *const CVTimeStamp,
        ) {
            let pace = &self.ivars().pace;
            pace.drew.store(true, Ordering::Release);
            let refresh = match pace.drawing_for.load(Ordering::Acquire) {
                0 => None,
                nanos => Some(HostTime::from_nanos(nanos)),
            };
            if let Err(error) = pace.renderer.render(refresh) {
                eprintln!("plex-rust: {error}");
            }
        }
    }
);

impl VideoLayer {
    /// Creates the layer and the renderer behind it. `player` can load
    /// video once this returns. Frames appear once the layer's view is in
    /// a window and [`fit_to_display`] has been called for it.
    pub fn new(player: &Player, _main_thread: MainThreadMarker) -> player::Result<Retained<Self>> {
        let clock = Arc::new(OnceLock::new());
        let waker =
            Arc::new(Waker { clock: Arc::downgrade(&clock), ticking: AtomicBool::new(false) });
        let renderer = Renderer::new(player, {
            let waker = Arc::clone(&waker);
            move || waker.wake()
        })?;
        let pace = Arc::new(Pace {
            renderer,
            waker,
            layer: OnceLock::new(),
            output: Mutex::new(None),
            owes_a_draw: AtomicBool::new(false),
            cadence: Mutex::new(Cadence::default()),
            idle_refreshes: AtomicU32::new(0),
            ticks: AtomicU64::new(0),
            drawing_for: AtomicU64::new(0),
            drew: AtomicBool::new(false),
        });

        let this = Self::alloc().set_ivars(Ivars { pace: Arc::clone(&pace), clock });
        // SAFETY: `init` is CALayer's designated initializer.
        let layer: Retained<Self> = unsafe { msg_send![super(this), init] };
        // Cannot fail: the slot was created empty a few lines up.
        let _ = pace.layer.set(LayerRef(Weak::new(&*layer)));

        // Core Animation draws only when asked: by the frame clock for a
        // new frame, and by itself when the layer changes size.
        layer.setAsynchronous(false);
        layer.setNeedsDisplayOnBoundsChange(true);
        Ok(layer)
    }

    /// Width and height in pixels of the last frame drawn.
    pub fn drawn_size(&self) -> (i32, i32) {
        self.ivars().pace.renderer.drawn_size()
    }

    /// Starts keeping the times of every draw.
    pub fn record_timings(&self) {
        self.ivars().pace.renderer.record_timings();
    }

    /// The draws since [`record_timings`](Self::record_timings).
    pub fn take_timings(&self) -> Vec<FrameTiming> {
        self.ivars().pace.renderer.take_timings()
    }

    /// How often the frame clock has ticked so far. It stands still while
    /// the clock rests.
    pub fn clock_ticks(&self) -> u64 {
        self.ivars().pace.ticks.load(Ordering::Relaxed)
    }

    /// Sets up the layer and the player for the colours of the video that
    /// is playing. Call it whenever the player reports a change of picture.
    ///
    /// An HDR picture is shown in HDR when `display_has_headroom`, that is
    /// when the display can go brighter than SDR white. Otherwise, and for
    /// SDR pictures, the output is sRGB, which macOS then matches to the
    /// display. The layer and the player must always agree, or colours come
    /// out wrong, so both are set here and nowhere else.
    ///
    /// This follows IINA, which drives the same mpv renderer through the
    /// same kind of layer.
    ///
    /// mpv waits for the first frame of a file to be shown before it
    /// answers anything (`vo_wait_frame` in `write_video`, mpv's
    /// player/video.c), so the colours cannot be asked for before that
    /// frame is due. The frame clock takes it without drawing it, and
    /// draws it once the output is right for it. Drawn first and matched
    /// after, it went out through shaders built for SDR, which took 26 ms
    /// to build and were built again for HDR, and into a layer that was
    /// still in the colour space it is made with, extended linear sRGB.
    /// There an sRGB picture is several times too bright: every file
    /// opened with a flash.
    pub fn match_video(&self, player: &Player, display_has_headroom: bool) -> player::Result<()> {
        let pace = &self.ivars().pace;
        let video = player.video_colors()?;
        let output = video.map(|video| if display_has_headroom { video } else { Colors::Sdr });
        if pace.output() == output {
            return Ok(());
        }
        // From here on the clock draws nothing: with no picture there is
        // nothing to draw, and what a picture is drawn as is about to
        // change.
        pace.set_output(None);
        let Some(output) = output else {
            return Ok(());
        };
        // SAFETY: the names are constants that Core Graphics provides.
        let name = unsafe {
            match output {
                Colors::Sdr => kCGColorSpaceSRGB,
                Colors::HdrBt2020 => kCGColorSpaceITUR_2100_PQ,
                Colors::HdrDisplayP3 => kCGColorSpaceDisplayP3_PQ,
            }
        };
        self.setWantsExtendedDynamicRangeContent(output != Colors::Sdr);
        self.setColorspace(CGColorSpace::with_name(Some(name)).as_deref());
        // The clock sends each frame to the screen in a transaction of its
        // own thread. Left to the end of this turn of the main run loop,
        // the layer's new colour space could get there after a frame drawn
        // for it.
        CATransaction::flush();
        player.set_output_colors(output)?;
        pace.set_output(Some(output));
        pace.waker.wake();
        Ok(())
    }

    /// Starts the frame clock on a display link from `view`, which hosts
    /// the layer and is in a window. The link follows the view from
    /// display to display by itself, and rests while the view is hidden.
    fn start_clock(&self, view: &NSView) -> player::Result<()> {
        let Ivars { pace, clock } = self.ivars();
        if clock.get().is_some() {
            return Ok(());
        }
        let ticker = Ticker::new(Arc::clone(pace));
        // SAFETY: `ticker` responds to `tick:` with the one argument a
        // display link passes, the link itself.
        let link = unsafe { view.displayLinkWithTarget_selector(&ticker, sel!(tick:)) };
        pace.waker.ticking.store(true, Ordering::Release);
        // Cannot fail: only the main thread fills the slot, and it was
        // empty a few lines up.
        let _ = clock.set(Clock::start(link)?);
        Ok(())
    }
}

/// Fits the video layer that `view` hosts to the display its window is on:
/// the display's pixel density and refreshes, and
/// [`VideoLayer::match_video`] for its brightness range. Call it when the
/// view enters a window, when the window changes display, and when the
/// player reports a change of picture. Does nothing while the view is in
/// no window, and no frame is shown before the first call that does
/// something.
pub fn fit_to_display(view: &NSView, player: &Player) -> player::Result<()> {
    let layer = view.layer().and_then(|layer| layer.downcast::<VideoLayer>().ok());
    let (Some(layer), Some(window)) = (layer, view.window()) else {
        return Ok(());
    };
    // AppKit leaves the scale of a layer that a view was handed alone. At
    // the default of 1.0 a Retina display would get a picture rendered at
    // half its resolution and stretched.
    layer.setContentsScale(window.backingScaleFactor());
    layer.start_clock(view)?;
    // Above 1.0, the display can show brighter than SDR white.
    let headroom = window.screen().is_some_and(|screen| {
        screen.maximumPotentialExtendedDynamicRangeColorComponentValue() > 1.0
    });
    layer.match_video(player, headroom)
}

/// A reference to the layer that does not keep it alive and may be held
/// by any thread.
struct LayerRef(Weak<VideoLayer>);

// SAFETY: loading an Objective-C weak reference is thread-safe. The one
// thing another thread does with the layer it loads is `display` inside a
// transaction, which Core Animation allows off the main thread.
unsafe impl Send for LayerRef {}
// SAFETY: as above.
unsafe impl Sync for LayerRef {}

/// Resumes the frame clock when mpv has a frame and the clock rests.
struct Waker {
    clock: WeakArc<OnceLock<Clock>>,
    /// Whether the display link ticks, or is about to.
    ticking: AtomicBool,
}

impl Waker {
    /// Callable from any thread, and cheap while the clock ticks.
    fn wake(&self) {
        if self.ticking.swap(true, Ordering::AcqRel) {
            return;
        }
        // Before the clock exists there is nothing to resume, and it
        // starts out ticking.
        match self.clock.upgrade().as_deref().and_then(OnceLock::get) {
            Some(clock) => clock.on_its_thread(|link| link.setPaused(false)),
            None => self.ticking.store(false, Ordering::Release),
        }
    }
}

/// Decides, at every display refresh, whether to draw.
struct Pace {
    renderer: Renderer,
    waker: Arc<Waker>,
    /// Filled in once the layer exists.
    layer: OnceLock<LayerRef>,
    /// What the layer and the player are set up to show. `None` until the
    /// colours of the picture are known, and while they are being changed.
    output: Mutex<Option<Colors>>,
    /// Whether a frame was taken from mpv without being drawn, because
    /// `output` was not known, and is still to be drawn.
    owes_a_draw: AtomicBool,
    cadence: Mutex<Cadence>,
    /// Refreshes in a row at which mpv held nothing out.
    idle_refreshes: AtomicU32,
    /// Every refresh the clock was woken for, for measuring.
    ticks: AtomicU64,
    /// The refresh a draw in progress is timed for, in host nanoseconds,
    /// or 0.
    drawing_for: AtomicU64,
    /// Whether Core Animation called the layer's draw method since this
    /// was last cleared.
    drew: AtomicBool,
}

impl Pace {
    fn output(&self) -> Option<Colors> {
        *self.output.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_output(&self, output: Option<Colors>) {
        *self.output.lock().unwrap_or_else(PoisonError::into_inner) = output;
    }

    /// Draws what is due at `refresh`, if anything. Called on the frame
    /// clock's thread ahead of every refresh. Returns whether the clock
    /// should go on ticking: not once mpv has had nothing for a while.
    fn tick(&self, refresh: Refresh) -> bool {
        self.ticks.fetch_add(1, Ordering::Relaxed);
        let pending = self.renderer.pending();
        if self.output().is_none() {
            if pending == Pending::Nothing {
                return self.idle();
            }
            // A frame that the layer is not set up for is taken at once,
            // whenever it is due: mpv answers nothing until it is rid of
            // it. It is drawn when the layer is ready.
            self.skip();
            self.owes_a_draw.store(true, Ordering::Release);
            return true;
        }
        let owed = self.owes_a_draw.load(Ordering::Acquire);
        let due = match pending {
            Pending::Nothing if !owed => return self.idle(),
            Pending::Nothing | Pending::Now => None,
            Pending::Due(due) => Some(due),
        };
        self.idle_refreshes.store(0, Ordering::Release);
        let mut cadence = self.cadence.lock().unwrap_or_else(PoisonError::into_inner);
        // A draw that is owed does not wait for the time of the frame that
        // came after: the screen has no picture yet.
        let in_time = due.is_none_or(|due| cadence.is_time(due, refresh));
        if !in_time && !owed {
            return true;
        }
        let Some(layer) = self.layer.get().and_then(|layer| layer.0.load()) else {
            return true;
        };

        self.owes_a_draw.store(false, Ordering::Release);
        self.drew.store(false, Ordering::Release);
        self.drawing_for.store(refresh.at.nanos(), Ordering::Release);
        // The transaction is what sends the drawn frame to the screen:
        // this thread's run loop does not do that for it.
        CATransaction::begin();
        CALayer::display(&layer);
        CATransaction::commit();
        self.drawing_for.store(0, Ordering::Release);

        // Core Animation does not draw a layer nobody can see. mpv still
        // has to be rid of the frame (IINA does the same, issue #5025).
        if !self.drew.load(Ordering::Acquire) {
            self.skip();
        }
        if let Some(due) = due.filter(|_| in_time) {
            cadence.shown(due, refresh);
        }
        true
    }

    /// Counts a refresh with nothing to do. Returns whether the clock
    /// should go on ticking.
    fn idle(&self) -> bool {
        self.idle_refreshes.fetch_add(1, Ordering::AcqRel) + 1 < IDLE_REFRESHES
    }

    fn skip(&self) {
        if let Err(error) = self.renderer.skip() {
            eprintln!("plex-rust: {error}");
        }
    }

    /// Whether a tick would find something to draw or to take.
    fn has_work(&self) -> bool {
        let owed = self.owes_a_draw.load(Ordering::Acquire) && self.output().is_some();
        owed || self.renderer.pending() != Pending::Nothing
    }

    /// Pauses `link`, unless a frame turned up meanwhile. Called on the
    /// frame clock's thread.
    fn rest(&self, link: &CADisplayLink) {
        link.setPaused(true);
        self.idle_refreshes.store(0, Ordering::Release);
        self.waker.ticking.store(false, Ordering::Release);
        if self.has_work() {
            self.waker.wake();
        }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Ticker` does
    // not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[name = "PlexRustFrameTicker"]
    #[ivars = Arc<Pace>]
    struct Ticker;

    impl Ticker {
        /// What the display link calls, on the frame clock's thread, ahead
        /// of every refresh.
        #[unsafe(method(tick:))]
        fn tick(&self, link: &CADisplayLink) {
            let (now, target) = (link.timestamp(), link.targetTimestamp());
            let refresh = Refresh {
                at: HostTime::from_seconds(target),
                period: Duration::try_from_secs_f64(target - now).unwrap_or_default(),
            };
            let pace = self.ivars();
            if !pace.tick(refresh) {
                pace.rest(link);
            }
        }
    }
);

impl Ticker {
    fn new(pace: Arc<Pace>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(pace);
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// The frame clock: a thread whose run loop a display link ticks on.
/// Dropping it ends the thread and lets go of the link's target.
struct Clock {
    run_loop: CFRetained<CFRunLoop>,
    link: Retained<CADisplayLink>,
}

// SAFETY: `run_loop` is only given blocks and woken, which Core Foundation
// allows from any thread. `link` is created on the main thread and from
// then on only used inside those blocks, on the clock's own thread.
unsafe impl Send for Clock {}
// SAFETY: as above.
unsafe impl Sync for Clock {}

/// A display link on its way to the one thread that will use it.
struct ForClock(Retained<CADisplayLink>);

// SAFETY: the link is moved to the frame clock's thread and only used
// there.
unsafe impl Send for ForClock {}

impl ForClock {
    fn arrive(self) -> Retained<CADisplayLink> {
        self.0
    }
}

impl Clock {
    /// Starts the thread and puts `link` on its run loop, ticking.
    fn start(link: Retained<CADisplayLink>) -> player::Result<Self> {
        let (ready, run_loop) = mpsc::channel();
        let for_clock = ForClock(link.clone());
        let run = move || {
            let link = for_clock.arrive();
            player::run_thread_at_interactive_priority();
            let run_loop = NSRunLoop::currentRunLoop();
            // SAFETY: the mode is a valid run loop mode. The port has no
            // use but to keep the run loop from returning while the link
            // is paused.
            unsafe {
                run_loop.addPort_forMode(&NSPort::port(), NSDefaultRunLoopMode);
                link.addToRunLoop_forMode(&run_loop, NSDefaultRunLoopMode);
            }
            // The receiver waits for this, so it is there.
            let _ = ready.send(SendRunLoop(run_loop.getCFRunLoop().into()));
            CFRunLoop::run();
        };
        thread::Builder::new()
            .name("frame clock".into())
            .spawn(run)
            .map_err(|_| player::Error::NoFrameClock)?;
        let SendRunLoop(run_loop) = run_loop.recv().map_err(|_| player::Error::NoFrameClock)?;
        Ok(Self { run_loop, link })
    }

    /// Queues `work` to run with the link on the clock's thread.
    fn on_its_thread(&self, work: impl Fn(&CADisplayLink) + 'static) {
        let link = self.link.clone();
        let work = RcBlock::new(move || work(&link));
        // SAFETY: the mode is Core Foundation's own constant, and the run
        // loop copies the block. The link the block holds is only used
        // where the block runs, on the clock's thread.
        unsafe {
            self.run_loop.perform_block(kCFRunLoopDefaultMode.map(|mode| &**mode), Some(&work));
        }
        self.run_loop.wake_up();
    }
}

impl Drop for Clock {
    fn drop(&mut self) {
        self.on_its_thread(|link| {
            // Lets go of the `Ticker`, and with it of the renderer.
            link.invalidate();
            if let Some(run_loop) = CFRunLoop::current() {
                run_loop.stop();
            }
        });
    }
}

/// A run loop on its way from its thread to the thread that started it.
struct SendRunLoop(CFRetained<CFRunLoop>);

// SAFETY: a `CFRunLoop` may be retained, released, given blocks and woken
// from any thread.
unsafe impl Send for SendRunLoop {}
