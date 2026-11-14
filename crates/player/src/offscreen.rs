//! Drawing video frames where nobody sees them.

use std::sync::mpsc::{self, Receiver};

use crate::core::Player;
use crate::error::{Error, Result};
use crate::render::Renderer;

/// Draws the player's video into a buffer that is never shown, each frame
/// as soon as mpv has it.
///
/// This is for measuring. mpv holds every frame back until it was drawn,
/// and gives up on it after 200 ms (`flip_page` in mpv's
/// video/out/vo_libmpv.c). macOS does not draw a window that is covered, so
/// timings taken with a window depend on what else is on screen. These do
/// not.
pub struct Offscreen {
    renderer: Renderer,
    frames: Receiver<()>,
    framebuffer: u32,
    width: i32,
    height: i32,
}

impl Offscreen {
    /// Creates a buffer of `width` by `height` pixels in the format the
    /// video layer draws in. `player` can load video once this returns.
    pub fn new(player: &Player, width: i32, height: i32) -> Result<Self> {
        let (frame_ready, frames) = mpsc::channel();
        // A failed send means `draw_frames` has returned, and then there is
        // nobody left to draw for.
        let renderer = Renderer::new(player, move || {
            let _ = frame_ready.send(());
        })?;

        let (mut framebuffer, mut texture) = (0, 0);
        // SAFETY: `Renderer::new` left its OpenGL context current on this
        // thread. Each `glGen*` call writes one name, and `glTexImage2D` is
        // given no pixels to read.
        let status = unsafe {
            glGenTextures(1, &mut texture);
            glBindTexture(GL_TEXTURE_2D, texture);
            let (format, no_pixels) = (GL_RGBA16F.cast_signed(), std::ptr::null());
            glTexImage2D(GL_TEXTURE_2D, 0, format, width, height, 0, GL_RGBA, GL_FLOAT, no_pixels);
            glGenFramebuffers(1, &mut framebuffer);
            glBindFramebuffer(GL_FRAMEBUFFER, framebuffer);
            glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, texture, 0);
            glCheckFramebufferStatus(GL_FRAMEBUFFER)
        };
        if status != GL_FRAMEBUFFER_COMPLETE {
            return Err(Error::OpenGl("the offscreen framebuffer is incomplete"));
        }
        Ok(Self { renderer, frames, framebuffer, width, height })
    }

    /// Draws frames until the player is gone. Call it on the thread that
    /// called [`new`](Self::new), which the OpenGL context belongs to.
    pub fn draw_frames(self) -> Result<()> {
        for () in &self.frames {
            // The renderer draws into whichever framebuffer is bound, and
            // nothing promises that mpv leaves this one bound.
            // SAFETY: the renderer's context is current on this thread and
            // the framebuffer is one of its own.
            unsafe {
                glBindFramebuffer(GL_FRAMEBUFFER, self.framebuffer);
                glViewport(0, 0, self.width, self.height);
            }
            self.renderer.render(None)?;
        }
        Ok(())
    }
}

const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_FLOAT: u32 = 0x1406;
const GL_RGBA: u32 = 0x1908;
const GL_RGBA16F: u32 = 0x881A;
const GL_COLOR_ATTACHMENT0: u32 = 0x8CE0;
const GL_FRAMEBUFFER: u32 = 0x8D40;
const GL_FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;

// From the OpenGL framework, which libmpv already links.
unsafe extern "C" {
    fn glGenTextures(count: i32, names: *mut u32);
    fn glBindTexture(target: u32, texture: u32);
    fn glTexImage2D(
        target: u32,
        level: i32,
        internal_format: i32,
        width: i32,
        height: i32,
        border: i32,
        format: u32,
        kind: u32,
        pixels: *const libc::c_void,
    );
    fn glGenFramebuffers(count: i32, names: *mut u32);
    fn glBindFramebuffer(target: u32, framebuffer: u32);
    fn glFramebufferTexture2D(
        target: u32,
        attachment: u32,
        texture_target: u32,
        texture: u32,
        level: i32,
    );
    fn glViewport(x: i32, y: i32, width: i32, height: i32);
    fn glCheckFramebufferStatus(target: u32) -> u32;
}
