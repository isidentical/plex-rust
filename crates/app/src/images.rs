//! Artwork, fetched once and kept.

use std::path::PathBuf;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::NSImage;
use objc2_foundation::{NSCache, NSData, NSString};
use plex::MediaServer;

use crate::background::Background;
use crate::error::{Error, Result, with_causes};

/// Loads posters and backdrops through three layers: decoded images in
/// memory, JPEG files on disk, and the server.
pub struct Images {
    main_thread: MainThreadMarker,
    background: Rc<Background>,
    server: MediaServer,
    directory: PathBuf,
    /// Evicted by the system under memory pressure.
    memory: Retained<NSCache<NSString, NSImage>>,
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
        Ok(Self { main_thread, background, server, directory, memory })
    }

    /// Calls `then` with the artwork at server path `path`, scaled by the
    /// server to cover `width` by `height` pixels.
    ///
    /// `then` runs at once when the image is in memory and later otherwise.
    /// If the image cannot be had, the failure is logged and `then` is
    /// never called.
    pub fn load(
        self: &Rc<Self>,
        path: &str,
        width: u32,
        height: u32,
        then: impl FnOnce(&NSImage) + 'static,
    ) {
        let name = format!("{:016x}-{width}x{height}.jpg", plex::stable_hash(path));
        let key = NSString::from_str(&name);
        // SAFETY: the cache only holds images under string keys.
        if let Some(image) = unsafe { self.memory.objectForKey(&key) } {
            then(&image);
            return;
        }

        let file = self.directory.join(name);
        let (server, path) = (self.server.clone(), path.to_owned());
        let fetch = async move {
            if let Ok(jpeg) = tokio::fs::read(&file).await {
                return Ok(jpeg);
            }
            let jpeg = server.image(&path, width, height).await?;
            // A failed write only costs a download next time.
            let _ = tokio::fs::write(&file, &jpeg).await;
            Ok::<_, Error>(jpeg)
        };

        let images = Rc::clone(self);
        self.background.run(self.main_thread, fetch, move |jpeg| {
            let image = jpeg.and_then(|jpeg| {
                NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&jpeg))
                    .ok_or(Error::NotAnImage)
            });
            match image {
                Ok(image) => {
                    // SAFETY: as above.
                    unsafe { images.memory.setObject_forKey(&image, &key) };
                    then(&image);
                }
                Err(error) => eprintln!("plex-rust: artwork not loaded: {}", with_causes(&error)),
            }
        });
    }
}
