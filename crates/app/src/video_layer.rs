//! The layer that shows the video.

#![expect(
    deprecated,
    reason = "Apple deprecated OpenGL, but it is the only API mpv can render through when embedded"
)]

use std::cell::OnceCell;
use std::rc::Rc;
use std::sync::Arc;

use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::rc::{Allocated, Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, define_class, msg_send};
use objc2_core_foundation::CFTimeInterval;
use objc2_core_video::CVTimeStamp;
use objc2_foundation::NSObject;
use objc2_open_gl::{CGLContextObj, CGLPixelFormatObj, CGLRetainContext, CGLRetainPixelFormat};
use objc2_quartz_core::{CALayer, CAOpenGLLayer};
use player::{Player, Renderer};

pub struct Ivars {
    renderer: Rc<Renderer>,
}

define_class!(
    // SAFETY:
    // - CAOpenGLLayer is designed to be subclassed and the overrides below
    //   are its documented customization points.
    // - `VideoLayer` does not implement `Drop`.
    #[unsafe(super(CAOpenGLLayer, CALayer, NSObject))]
    #[name = "ReelVideoLayer"]
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
            let renderer = Rc::clone(&original.ivars().renderer);
            let this = this.set_ivars(Ivars { renderer });
            // SAFETY: `initWithLayer:` is CALayer's copy initializer and
            // `layer` is a valid layer.
            unsafe { msg_send![super(this), initWithLayer: layer] }
        }

        // Core Animation draws with the renderer's own context. Both `copy`
        // methods hand over a reference that Core Animation releases.

        #[unsafe(method(copyCGLPixelFormatForDisplayMask:))]
        fn pixel_format(&self, _display_mask: u32) -> CGLPixelFormatObj {
            // SAFETY: the renderer's pixel format is valid while it lives.
            unsafe { CGLRetainPixelFormat(self.ivars().renderer.gl_pixel_format()) }
        }

        #[unsafe(method(copyCGLContextForPixelFormat:))]
        fn context(&self, _format: CGLPixelFormatObj) -> CGLContextObj {
            // SAFETY: the renderer's context is valid while it lives.
            unsafe { CGLRetainContext(self.ivars().renderer.gl_context()) }
        }

        #[unsafe(method(drawInCGLContext:pixelFormat:forLayerTime:displayTime:))]
        fn draw(
            &self,
            _context: CGLContextObj,
            _format: CGLPixelFormatObj,
            _layer_time: CFTimeInterval,
            _display_time: *const CVTimeStamp,
        ) {
            if let Err(error) = self.ivars().renderer.render() {
                eprintln!("reel: {error}");
            }
        }
    }
);

impl VideoLayer {
    /// Creates the layer and the renderer behind it. `player` can load
    /// video once this returns.
    pub fn new(player: &Player, main_thread: MainThreadMarker) -> player::Result<Retained<Self>> {
        // The renderer needs a way to reach the layer, and the layer needs
        // the renderer to exist. The slot is filled in once both do.
        let slot = Arc::new(MainThreadBound::new(OnceCell::new(), main_thread));
        let renderer = Renderer::new(player, redraw_request(Arc::clone(&slot)))?;

        let this = Self::alloc().set_ivars(Ivars {
            renderer: Rc::new(renderer),
        });
        // SAFETY: `init` is CALayer's designated initializer.
        let layer: Retained<Self> = unsafe { msg_send![super(this), init] };
        // Cannot fail: the slot was created empty a few lines up.
        let _ = slot.get(main_thread).set(Weak::new(&*layer));

        // Draw when mpv has a frame, not on every display refresh.
        layer.setAsynchronous(false);
        layer.setNeedsDisplayOnBoundsChange(true);
        Ok(layer)
    }
}

type LayerSlot = Arc<MainThreadBound<OnceCell<Weak<VideoLayer>>>>;

/// A closure, callable from any thread, that asks the main thread to redraw
/// the layer in `slot`.
fn redraw_request(slot: LayerSlot) -> impl Fn() + Send + Sync + 'static {
    move || {
        let slot = Arc::clone(&slot);
        DispatchQueue::main().exec_async(move || {
            slot.get_on_main(|slot| {
                if let Some(layer) = slot.get().and_then(Weak::load) {
                    layer.setNeedsDisplay();
                }
            });
        });
    }
}
