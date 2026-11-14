//! Artwork, fetched once and kept.

use std::path::PathBuf;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadMarker, Message};
use objc2_app_kit::NSImage;
use objc2_core_foundation::{
    CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};
use objc2_core_graphics::CGImage;
use objc2_foundation::{NSCache, NSSize, NSString};
use objc2_image_io::{
    CGImageSource, kCGImageSourceCreateThumbnailFromImageAlways,
    kCGImageSourceCreateThumbnailWithTransform, kCGImageSourceShouldCacheImmediately,
    kCGImageSourceThumbnailMaxPixelSize,
};
use plex::MediaServer;

use crate::background::Background;
use crate::error::{Error, Result, with_causes};

/// How many bytes of decoded artwork are kept in memory. A poster at twice
/// its point size is about half a megabyte, so this holds several hundred.
const MEMORY_LIMIT: usize = 384 * 1024 * 1024;

/// A picture that is already decoded, so that showing it costs the main
/// thread nothing but handing it to a layer.
#[derive(Clone)]
pub struct Artwork(Retained<AnyObject>);

impl Artwork {
    /// The picture as a layer takes it, for `CALayer::setContents`.
    pub fn contents(&self) -> &AnyObject {
        &self.0
    }

    /// The picture as AppKit takes it, at one point a pixel.
    pub fn image(&self) -> Retained<NSImage> {
        // SAFETY: an `Artwork` is only ever made from a `CGImage`, in
        // `Images::load`, and Core Foundation objects are Objective-C ones.
        let image: &CGImage = unsafe { &*Retained::as_ptr(&self.0).cast() };
        NSImage::initWithCGImage_size(NSImage::alloc(), image, NSSize::ZERO)
    }
}

/// Loads posters and backdrops through three layers: decoded pictures in
/// memory, JPEG files on disk, and the server.
pub struct Images {
    main_thread: MainThreadMarker,
    background: Rc<Background>,
    server: MediaServer,
    directory: PathBuf,
    /// Decoded `CGImage`s. Evicted by the system under memory pressure.
    memory: Retained<NSCache<NSString, AnyObject>>,
}

impl Images {
    pub fn new(
        main_thread: MainThreadMarker,
        background: Rc<Background>,
        server: MediaServer,
    ) -> Result<Self> {
        let home = std::env::var_os("HOME").ok_or(Error::NoHome)?;
        let directory = PathBuf::from(home).join("Library/Caches/plex-rust/images");
        std::fs::create_dir_all(&directory)
            .map_err(|source| Error::File { path: directory.clone(), source })?;
        // SAFETY: the cache is created empty and only ever used with the key
        // and value types in its declaration.
        let memory = unsafe { NSCache::new() };
        // SAFETY: the limit is a plain number of bytes.
        unsafe { memory.setTotalCostLimit(MEMORY_LIMIT) };
        Ok(Self { main_thread, background, server, directory, memory })
    }

    /// The artwork at server path `path` for a box of `width` by `height`
    /// pixels, if it is in memory. Never touches the disk or the network.
    pub fn cached(&self, path: &str, width: u32, height: u32) -> Option<Artwork> {
        let key = NSString::from_str(&name(path, width, height));
        // SAFETY: the cache only holds images under string keys.
        unsafe { self.memory.objectForKey(&key) }.map(Artwork)
    }

    /// Calls `then` on the main thread with the artwork at server path
    /// `path`, scaled to cover `width` by `height` pixels. The file is read
    /// or downloaded and then decoded off the main thread.
    ///
    /// If the picture cannot be had, the failure is logged and `then` is
    /// never called.
    pub fn load(
        self: &Rc<Self>,
        path: &str,
        width: u32,
        height: u32,
        then: impl FnOnce(&Artwork) + 'static,
    ) {
        let name = name(path, width, height);
        let key = NSString::from_str(&name);
        let file = self.directory.join(name);
        let (server, path) = (self.server.clone(), path.to_owned());
        let fetch = async move {
            let jpeg = match tokio::fs::read(&file).await {
                Ok(jpeg) => jpeg,
                Err(_) => {
                    let jpeg = server.image(&path, width, height).await?;
                    // A failed write only costs a download next time.
                    let _ = tokio::fs::write(&file, &jpeg).await;
                    jpeg
                }
            };
            // Decoding a poster takes a few milliseconds of processor time,
            // which must not hold up the threads that serve the network.
            let decoded = tokio::task::spawn_blocking(move || decode(&jpeg, width.max(height)));
            decoded.await.map_err(|_| Error::NotAnImage)?
        };

        let images = Rc::clone(self);
        self.background.run(self.main_thread, fetch, move |image| match image {
            Ok(image) => {
                let object: &AnyObject = (*image).as_ref();
                let artwork = Artwork(object.retain());
                let cost = CGImage::bytes_per_row(Some(&image)) * CGImage::height(Some(&image));
                // SAFETY: as in `cached`.
                unsafe { images.memory.setObject_forKey_cost(artwork.contents(), &key, cost) };
                then(&artwork);
            }
            Err(error) => eprintln!("plex-rust: artwork not loaded: {}", with_causes(&error)),
        });
    }
}

/// The name of a picture in both caches.
fn name(path: &str, width: u32, height: u32) -> String {
    format!("{:016x}-{width}x{height}.jpg", plex::stable_hash(path))
}

/// Turns a JPEG into pixels, no larger than `longest_side` on its longer
/// side. The pixels are produced here and now, not when first drawn.
fn decode(jpeg: &[u8], longest_side: u32) -> Result<CFRetained<CGImage>> {
    let data = CFData::from_bytes(jpeg);
    let size = CFNumber::new_i64(i64::from(longest_side));
    let yes = CFBoolean::new(true);
    // SAFETY: the option names are constants that ImageIO provides.
    let keys: [&CFString; 4] = unsafe {
        [
            kCGImageSourceCreateThumbnailFromImageAlways,
            kCGImageSourceCreateThumbnailWithTransform,
            kCGImageSourceShouldCacheImmediately,
            kCGImageSourceThumbnailMaxPixelSize,
        ]
    };
    let values: [&CFType; 4] = [yes.as_ref(), yes.as_ref(), yes.as_ref(), size.as_ref()];
    let options = CFDictionary::from_slices(&keys, &values);
    // SAFETY: the options are the ones ImageIO documents for these calls,
    // each with a value of the type it asks for.
    let image = unsafe {
        let source = CGImageSource::with_data(&data, None).ok_or(Error::NotAnImage)?;
        source.thumbnail_at_index(0, Some(options.as_opaque()))
    };
    image.ok_or(Error::NotAnImage)
}
