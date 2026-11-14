//! Bringing a download to disk.

use plex::model::Kind;
use plex::{MediaServer, Transfer};

use crate::error::{Error, Result, with_causes};
use crate::store::{Download, OnDisk};

/// The size the poster of a download is kept at, in pixels: the one a
/// detail page asks for, which is the largest a poster is shown.
const POSTER_SIZE: (u32, u32) = (464, 696);

/// Brings the file of `download` to disk from `server`, carrying on from
/// what an earlier attempt left, and its poster with it. `transfer` hears
/// how far the file has got.
pub async fn fetch(server: &MediaServer, download: &Download, transfer: &Transfer) -> Result<()> {
    let file_error = |path: &std::path::Path| {
        let path = path.to_owned();
        move |source| Error::File { path, source }
    };
    if download.on_disk()? == OnDisk::Whole {
        return Ok(());
    }
    // An episode's own artwork is a wide frame, so it borrows its show's.
    let item = &download.item;
    let poster = match item.kind {
        Kind::Episode => item.grandparent_thumb.as_deref().or(item.thumb.as_deref()),
        _ => item.thumb.as_deref(),
    };
    let missing =
        !tokio::fs::try_exists(&download.poster).await.map_err(file_error(&download.poster))?;
    if let (Some(poster), true) = (poster, missing) {
        let (width, height) = POSTER_SIZE;
        match server.image(poster, width, height).await {
            Ok(jpeg) => tokio::fs::write(&download.poster, jpeg)
                .await
                .map_err(file_error(&download.poster))?,
            // The film is what was asked for. The list shows it without a
            // picture.
            Err(error) => eprintln!("plex-rust: poster not kept: {}", with_causes(&error)),
        }
    }

    server.download(&download.part, &download.partial, transfer).await?;
    tokio::fs::rename(&download.partial, &download.file)
        .await
        .map_err(file_error(&download.partial))
}
