//! Drawing video frames with OpenGL.

#![expect(
    deprecated,
    reason = "Apple deprecated OpenGL, but it is the only API mpv can render through when embedded"
)]

use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use libmpv2_sys as sys;
use objc2_open_gl::{
    CGLChoosePixelFormat, CGLContextObj, CGLCreateContext, CGLLockContext, CGLOpenGLProfile,
    CGLPixelFormatAttribute, CGLPixelFormatObj, CGLReleaseContext, CGLReleasePixelFormat,
    CGLSetCurrentContext, CGLUnlockContext,
};

use crate::clock::HostTime;
use crate::core::Player;
use crate::error::{Error, Result, check};

/// What mpv's frame signal reaches, on a thread of mpv's choosing.
struct Signal {
    on_frame: Box<dyn Fn() + Send + Sync>,
    /// When the signal last fired, in host nanoseconds.
    at: AtomicU64,
}

/// The times of one draw, for measuring how evenly frames are shown.
#[derive(Debug, Clone, Copy)]
pub struct FrameTiming {
    /// When mpv last asked for a draw.
    pub signalled: HostTime,
    /// When mpv wants the frame on screen. `None` when the draw repeated a
    /// frame that was already shown.
    pub due: Option<HostTime>,
    /// The display refresh the draw was made for. `None` when it was not
    /// timed to one, as when a resized window is redrawn.
    pub refresh: Option<HostTime>,
    pub began: HostTime,
    pub ended: HostTime,
}

/// What mpv is holding out to be drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    Nothing,
    /// A picture to draw without delay: the last frame again, as after a
    /// change of settings while paused, or a frame mpv gave no time for.
    Now,
    /// A new frame, and when mpv wants it on screen.
    Due(HostTime),
}

/// Draws the player's video with OpenGL.
///
/// The renderer owns the OpenGL context it draws with. A `CAOpenGLLayer`
/// shows the video by handing that context and pixel format to Core
/// Animation and calling [`render`](Self::render) from its draw method.
///
/// Any thread may call any method. Each one locks the OpenGL context and
/// makes it current for the length of the call, which is also what keeps
/// the calls into mpv's renderer one at a time, as mpv requires
/// (mpv/render.h, "Threading").
///
/// mpv can only start video playback once a renderer exists, so create it
/// before calling [`Player::load`].
pub struct Renderer {
    mpv: NonNull<sys::mpv_render_context>,
    gl: CGLContextObj,
    pixel_format: CGLPixelFormatObj,
    /// Width and height in pixels of the last frame drawn.
    drawn_size: Mutex<(i32, i32)>,
    /// Host time minus mpv's clock, in nanoseconds. Both count the same
    /// ticks, mpv from when it started (osdep/timer.c).
    clock_offset: i64,
    /// The draws since [`record_timings`](Self::record_timings), if any.
    timings: Mutex<Option<Vec<FrameTiming>>>,
    // Boxed so that mpv can hold a pointer to it that stays valid.
    signal: Box<Signal>,
    // Keeps mpv alive for as long as the render context exists.
    _player: Player,
}

// SAFETY: `mpv` and `gl` are only used through `current`, which holds the
// OpenGL context's own lock, so one thread at a time uses them, with the
// context current on that thread. mpv allows its render functions on any
// thread under exactly those two conditions (mpv/render.h, "Threading").
// `gl` and `pixel_format` are also handed out to be retained, which CGL
// allows from any thread. Every other field is `Send` and `Sync`.
unsafe impl Send for Renderer {}
// SAFETY: as above.
unsafe impl Sync for Renderer {}

/// The renderer's OpenGL context, locked and current on this thread.
struct Current<'a>(&'a Renderer);

impl Drop for Current<'_> {
    fn drop(&mut self) {
        // SAFETY: the context is valid and this thread locked it in
        // `Renderer::current`. It is left current: Core Animation, which
        // calls in with it current, expects to find it so afterwards.
        unsafe { CGLUnlockContext(self.0.gl) };
    }
}

impl Renderer {
    /// `on_frame` is called from an mpv thread each time a new frame should
    /// be drawn. It must return quickly and must not call into mpv.
    pub fn new(player: &Player, on_frame: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        let pixel_format = choose_pixel_format()?;
        let gl = match create_context(pixel_format) {
            Ok(gl) => gl,
            Err(error) => {
                // SAFETY: the pixel format is valid and not used again.
                unsafe { CGLReleasePixelFormat(pixel_format) };
                return Err(error);
            }
        };
        // SAFETY: `gl` is a valid context that no other thread knows yet.
        unsafe { CGLSetCurrentContext(gl) };

        let mut init = sys::mpv_opengl_init_params {
            get_proc_address: Some(gl_symbol),
            get_proc_address_ctx: ptr::null_mut(),
        };
        let mut params = [
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                sys::MPV_RENDER_API_TYPE_OPENGL.as_ptr().cast_mut(),
            ),
            param(sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, &mut init),
            END,
        ];
        let mut mpv = ptr::null_mut();
        // SAFETY: the handle is valid, the OpenGL context is current,
        // `params` ends with the INVALID marker, and everything it points to
        // outlives the call.
        let status =
            unsafe { sys::mpv_render_context_create(&mut mpv, player.raw(), params.as_mut_ptr()) };
        let mpv = match (check(status, || "create the render context".into()), NonNull::new(mpv)) {
            (Ok(()), Some(mpv)) => mpv,
            (result, _) => {
                // SAFETY: both objects are valid and not used again.
                unsafe {
                    CGLReleaseContext(gl);
                    CGLReleasePixelFormat(pixel_format);
                }
                return Err(result.err().unwrap_or(Error::Create));
            }
        };

        let signal = Box::new(Signal { on_frame: Box::new(on_frame), at: AtomicU64::new(0) });
        // SAFETY: the render context is valid. The callback data points into
        // a box that `Renderer` keeps until after the context is freed.
        unsafe {
            sys::mpv_render_context_set_update_callback(
                mpv.as_ptr(),
                Some(frame_ready),
                ptr::from_ref::<Signal>(&signal).cast_mut().cast(),
            );
        }
        // SAFETY: the handle is valid.
        let mpv_now = unsafe { sys::mpv_get_time_ns(player.raw()) };
        let clock_offset = HostTime::now().nanos().cast_signed() - mpv_now;

        Ok(Self {
            mpv,
            gl,
            pixel_format,
            drawn_size: Mutex::new((0, 0)),
            clock_offset,
            timings: Mutex::new(None),
            signal,
            _player: player.clone(),
        })
    }

    /// Width and height in pixels of the last frame drawn, which is the
    /// resolution the picture is really rendered at.
    pub fn drawn_size(&self) -> (i32, i32) {
        *self.drawn_size.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The OpenGL context frames are drawn with. Valid while `self` lives.
    pub fn gl_context(&self) -> CGLContextObj {
        self.gl
    }

    /// The pixel format of [`gl_context`](Self::gl_context).
    pub fn gl_pixel_format(&self) -> CGLPixelFormatObj {
        self.pixel_format
    }

    /// Starts keeping the times of every draw, dropping any kept so far.
    pub fn record_timings(&self) {
        *self.timings.lock().unwrap_or_else(PoisonError::into_inner) = Some(Vec::new());
    }

    /// The draws since [`record_timings`](Self::record_timings), oldest
    /// first. Recording stops.
    pub fn take_timings(&self) -> Vec<FrameTiming> {
        self.timings.lock().unwrap_or_else(PoisonError::into_inner).take().unwrap_or_default()
    }

    /// Locks the OpenGL context and makes it current on this thread.
    fn current(&self) -> Current<'_> {
        // SAFETY: the context is valid. Its lock is recursive ("You can use
        // CGLLockContext recursively", OpenGL Programming Guide for Mac,
        // "Concurrency and OpenGL"), so this also works inside Core
        // Animation's draw call, whether or not that holds the lock.
        unsafe {
            CGLLockContext(self.gl);
            CGLSetCurrentContext(self.gl);
        }
        Current(self)
    }

    /// Runs `work` while no other thread can draw, with the OpenGL context
    /// current.
    pub fn exclusively<T>(&self, work: impl FnOnce() -> T) -> T {
        let _current = self.current();
        work()
    }

    /// What mpv is holding out to be drawn.
    pub fn pending(&self) -> Pending {
        let _current = self.current();
        self.pending_while_current()
    }

    fn pending_while_current(&self) -> Pending {
        let mut info = sys::mpv_render_frame_info { flags: 0, target_time: 0 };
        // SAFETY: the render context is valid, the OpenGL context is
        // current, and `info` is the type this parameter writes to.
        let status = unsafe {
            sys::mpv_render_context_get_info(
                self.mpv.as_ptr(),
                param(sys::mpv_render_param_type_MPV_RENDER_PARAM_NEXT_FRAME_INFO, &mut info),
            )
        };
        if status < 0 || info.flags & FRAME_PRESENT == 0 {
            return Pending::Nothing;
        }
        if info.flags & FRAME_REDRAW != 0 || info.target_time <= 0 {
            return Pending::Now;
        }
        // render.h says microseconds, but mpv 0.42 stores the frame's time
        // in nanoseconds on the clock of `mpv_get_time_ns` (vo_libmpv.c,
        // `info->target_time = frame->pts`).
        let nanos = info.target_time.saturating_add(self.clock_offset);
        Pending::Due(HostTime::from_nanos(u64::try_from(nanos).unwrap_or(0)))
    }

    /// Draws the pending frame, or the last one again if none is pending,
    /// into the framebuffer that is bound for drawing in the renderer's
    /// context, filling the viewport.
    ///
    /// `refresh` is the display refresh the caller timed the draw for. mpv
    /// is told not to wait for the frame's due time inside the call (it
    /// would, for up to `video-timing-offset`): render.h asks callers who
    /// turn that off to time frames themselves from their due times, which
    /// is what [`pending`](Self::pending) and `Cadence` are for.
    pub fn render(&self, refresh: Option<HostTime>) -> Result<()> {
        let began = HostTime::now();
        let _current = self.current();
        let pending = self.pending_while_current();
        let mut framebuffer = 0;
        // x, y, width, height in pixels.
        let mut viewport = [0; 4];
        // SAFETY: the context is current, and each query writes exactly as
        // many integers as its target holds.
        unsafe {
            glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut framebuffer);
            glGetIntegerv(GL_VIEWPORT, viewport.as_mut_ptr());
        }

        // Acknowledges mpv's frame signal. The frame is drawn either way,
        // because a resized layer needs the same picture drawn again.
        // SAFETY: the render context is valid and the context current.
        unsafe { sys::mpv_render_context_update(self.mpv.as_ptr()) };

        *self.drawn_size.lock().unwrap_or_else(PoisonError::into_inner) =
            (viewport[2], viewport[3]);
        let mut fbo = sys::mpv_opengl_fbo {
            fbo: framebuffer,
            w: viewport[2],
            h: viewport[3],
            internal_format: 0,
        };
        // OpenGL's origin is bottom-left; without the flip the picture is
        // upside down.
        let mut flip_y: libc::c_int = 1;
        let mut wait_until_due: libc::c_int = 0;
        let mut params = [
            param(sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO, &mut fbo),
            param(sys::mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y, &mut flip_y),
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
                &mut wait_until_due,
            ),
            END,
        ];
        // SAFETY: the render context is valid, its OpenGL context is
        // current, and `params` is terminated and outlives the call.
        let status =
            unsafe { sys::mpv_render_context_render(self.mpv.as_ptr(), params.as_mut_ptr()) };
        check(status, || "render a frame".into())?;

        // SAFETY: the OpenGL context is current and the render context valid.
        unsafe {
            glFlush();
            sys::mpv_render_context_report_swap(self.mpv.as_ptr());
        }
        if let Some(timings) = &mut *self.timings.lock().unwrap_or_else(PoisonError::into_inner) {
            let signalled = HostTime::from_nanos(self.signal.at.load(Ordering::Relaxed));
            let due = match pending {
                Pending::Due(due) => Some(due),
                Pending::Nothing | Pending::Now => None,
            };
            timings.push(FrameTiming { signalled, due, refresh, began, ended: HostTime::now() });
        }
        Ok(())
    }

    /// Takes the pending frame from mpv without drawing it, for when there
    /// is nowhere to draw: macOS does not draw a window that is covered or
    /// on another Space. Left pending, the frame would hold mpv's video
    /// thread for 200 ms and be counted as dropped (vo_libmpv.c,
    /// `flip_page`).
    pub fn skip(&self) -> Result<()> {
        let _current = self.current();
        let mut skip: libc::c_int = 1;
        let mut wait_until_due: libc::c_int = 0;
        let mut params = [
            param(sys::mpv_render_param_type_MPV_RENDER_PARAM_SKIP_RENDERING, &mut skip),
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME,
                &mut wait_until_due,
            ),
            END,
        ];
        // SAFETY: the render context is valid, its OpenGL context is
        // current, and `params` is terminated and outlives the call.
        let status =
            unsafe { sys::mpv_render_context_render(self.mpv.as_ptr(), params.as_mut_ptr()) };
        // SAFETY: the render context is valid.
        unsafe { sys::mpv_render_context_report_swap(self.mpv.as_ptr()) };
        check(status, || "skip a frame".into())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: all three objects are valid and not used again. mpv frees
        // its OpenGL resources in the context it created them in, which is
        // locked and current on this thread, whichever thread that is.
        // Freeing the render context also stops the frame callback.
        unsafe {
            CGLLockContext(self.gl);
            CGLSetCurrentContext(self.gl);
            sys::mpv_render_context_free(self.mpv.as_ptr());
            CGLSetCurrentContext(ptr::null_mut());
            CGLUnlockContext(self.gl);
            CGLReleaseContext(self.gl);
            CGLReleasePixelFormat(self.pixel_format);
        }
    }
}

fn choose_pixel_format() -> Result<CGLPixelFormatObj> {
    let mut attributes = [
        CGLPixelFormatAttribute::CGLPFAOpenGLProfile,
        CGLPixelFormatAttribute(CGLOpenGLProfile::CGLOGLPVersion_3_2_Core.0),
        CGLPixelFormatAttribute::CGLPFAAccelerated,
        CGLPixelFormatAttribute::CGLPFADoubleBuffer,
        // Floating point colour, 16 bits a channel. HDR needs values above
        // 1.0, which an 8 bit integer surface cannot hold.
        CGLPixelFormatAttribute::CGLPFAColorSize,
        CGLPixelFormatAttribute(64),
        CGLPixelFormatAttribute::CGLPFAColorFloat,
        CGLPixelFormatAttribute::CGLPFAAllowOfflineRenderers,
        CGLPixelFormatAttribute(0),
    ];
    let mut format: CGLPixelFormatObj = ptr::null_mut();
    let mut matches = 0;
    // SAFETY: the attribute list is zero-terminated and all three pointers
    // are valid for the duration of the call.
    unsafe {
        CGLChoosePixelFormat(
            NonNull::from(&mut attributes).cast(),
            NonNull::from(&mut format),
            NonNull::from(&mut matches),
        );
    }
    if format.is_null() {
        return Err(Error::OpenGl("no hardware accelerated OpenGL 3.2 pixel format"));
    }
    Ok(format)
}

fn create_context(pixel_format: CGLPixelFormatObj) -> Result<CGLContextObj> {
    let mut context: CGLContextObj = ptr::null_mut();
    // SAFETY: the pixel format is valid, no context is shared with, and the
    // out pointer is valid for the duration of the call.
    unsafe { CGLCreateContext(pixel_format, ptr::null_mut(), NonNull::from(&mut context)) };
    if context.is_null() {
        return Err(Error::OpenGl("the OpenGL context could not be created"));
    }
    Ok(context)
}

const FRAME_PRESENT: u64 = sys::mpv_render_frame_info_flag_MPV_RENDER_FRAME_INFO_PRESENT as u64;
const FRAME_REDRAW: u64 = sys::mpv_render_frame_info_flag_MPV_RENDER_FRAME_INFO_REDRAW as u64;

const GL_VIEWPORT: u32 = 0x0BA2;
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;

// From the OpenGL framework, which libmpv already links.
unsafe extern "C" {
    fn glGetIntegerv(name: u32, values: *mut i32);
    fn glFlush();
}

const END: sys::mpv_render_param = sys::mpv_render_param {
    type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
    data: ptr::null_mut(),
};

fn param<T>(type_: sys::mpv_render_param_type, data: *mut T) -> sys::mpv_render_param {
    sys::mpv_render_param { type_, data: data.cast() }
}

/// Looks up an OpenGL function by name for mpv.
unsafe extern "C" fn gl_symbol(_: *mut c_void, name: *const libc::c_char) -> *mut c_void {
    // SAFETY: mpv passes a NUL-terminated name. The OpenGL framework is
    // linked into this process, so its symbols are globally visible.
    unsafe { libc::dlsym(libc::RTLD_DEFAULT, name) }
}

unsafe extern "C" fn frame_ready(data: *mut c_void) {
    // SAFETY: `data` is the pointer to the boxed signal registered in
    // `Renderer::new`, which stays alive until the render context is freed.
    let signal = unsafe { &*data.cast::<Signal>() };
    signal.at.store(HostTime::now().nanos(), Ordering::Relaxed);
    (signal.on_frame)();
}
