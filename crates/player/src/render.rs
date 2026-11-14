//! Drawing video frames with OpenGL.

#![expect(
    deprecated,
    reason = "Apple deprecated OpenGL, but it is the only API mpv can render through when embedded"
)]

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use libmpv2_sys as sys;
use objc2_open_gl::{
    CGLChoosePixelFormat, CGLContextObj, CGLCreateContext, CGLOpenGLProfile,
    CGLPixelFormatAttribute, CGLPixelFormatObj, CGLReleaseContext, CGLReleasePixelFormat,
    CGLSetCurrentContext,
};

use crate::core::Player;
use crate::error::{Error, Result, check};

/// Called by mpv, on a thread of its choosing, when a new frame is ready.
type FrameCallback = Box<dyn Fn() + Send + Sync>;

/// Draws the player's video with OpenGL.
///
/// The renderer owns the OpenGL context it draws with. A `CAOpenGLLayer`
/// shows the video by handing that context and pixel format to Core
/// Animation and calling [`render`](Self::render) from its draw method.
///
/// mpv can only start video playback once a renderer exists, so create it
/// before calling [`Player::load`].
pub struct Renderer {
    mpv: NonNull<sys::mpv_render_context>,
    gl: CGLContextObj,
    pixel_format: CGLPixelFormatObj,
    // Boxed twice so that mpv can hold a thin pointer to it.
    _on_frame: Box<FrameCallback>,
    // Keeps mpv alive for as long as the render context exists.
    _player: Player,
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
        // SAFETY: `gl` is a valid context.
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
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                &mut init,
            ),
            END,
        ];
        let mut mpv = ptr::null_mut();
        // SAFETY: the handle is valid, the OpenGL context is current,
        // `params` ends with the INVALID marker, and everything it points to
        // outlives the call.
        let status =
            unsafe { sys::mpv_render_context_create(&mut mpv, player.raw(), params.as_mut_ptr()) };
        let mpv = match (
            check(status, || "create the render context".into()),
            NonNull::new(mpv),
        ) {
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

        let on_frame: Box<FrameCallback> = Box::new(Box::new(on_frame));
        // SAFETY: the render context is valid. The callback data points into
        // a box that `Renderer` keeps until after the context is freed.
        unsafe {
            sys::mpv_render_context_set_update_callback(
                mpv.as_ptr(),
                Some(frame_ready),
                ptr::from_ref::<FrameCallback>(&on_frame).cast_mut().cast(),
            );
        }

        Ok(Self {
            mpv,
            gl,
            pixel_format,
            _on_frame: on_frame,
            _player: player.clone(),
        })
    }

    /// The OpenGL context frames are drawn with. Valid while `self` lives.
    pub fn gl_context(&self) -> CGLContextObj {
        self.gl
    }

    /// The pixel format of [`gl_context`](Self::gl_context).
    pub fn gl_pixel_format(&self) -> CGLPixelFormatObj {
        self.pixel_format
    }

    /// Draws the current frame into the framebuffer that is bound for
    /// drawing in the renderer's context, filling the viewport.
    pub fn render(&self) -> Result<()> {
        let mut framebuffer = 0;
        // x, y, width, height in pixels.
        let mut viewport = [0; 4];
        // SAFETY: the context is valid, and each query writes exactly as
        // many integers as its target holds.
        unsafe {
            CGLSetCurrentContext(self.gl);
            glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut framebuffer);
            glGetIntegerv(GL_VIEWPORT, viewport.as_mut_ptr());
        }

        // Acknowledges mpv's frame signal. The frame is drawn either way,
        // because a resized layer needs the same picture drawn again.
        // SAFETY: the render context is valid.
        unsafe { sys::mpv_render_context_update(self.mpv.as_ptr()) };

        let mut fbo = sys::mpv_opengl_fbo {
            fbo: framebuffer,
            w: viewport[2],
            h: viewport[3],
            internal_format: 0,
        };
        // OpenGL's origin is bottom-left; without the flip the picture is
        // upside down.
        let mut flip_y: libc::c_int = 1;
        let mut params = [
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                &mut fbo,
            ),
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y,
                &mut flip_y,
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
        Ok(())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: all three objects are valid and not used again. mpv frees
        // its OpenGL resources in the context it created them in, and
        // freeing the render context also stops the frame callback.
        unsafe {
            CGLSetCurrentContext(self.gl);
            sys::mpv_render_context_free(self.mpv.as_ptr());
            CGLSetCurrentContext(ptr::null_mut());
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
        return Err(Error::OpenGl(
            "no hardware accelerated OpenGL 3.2 pixel format",
        ));
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
    sys::mpv_render_param {
        type_,
        data: data.cast(),
    }
}

/// Looks up an OpenGL function by name for mpv.
unsafe extern "C" fn gl_symbol(_: *mut c_void, name: *const libc::c_char) -> *mut c_void {
    // SAFETY: mpv passes a NUL-terminated name. The OpenGL framework is
    // linked into this process, so its symbols are globally visible.
    unsafe { libc::dlsym(libc::RTLD_DEFAULT, name) }
}

unsafe extern "C" fn frame_ready(data: *mut c_void) {
    // SAFETY: `data` is the pointer to the boxed callback registered in
    // `Renderer::new`, which stays alive until the render context is freed.
    let on_frame = unsafe { &*data.cast::<FrameCallback>() };
    on_frame();
}
