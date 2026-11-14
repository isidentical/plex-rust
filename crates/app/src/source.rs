//! Turning a library item into something the player can load.

use std::path::Path;

use player::{Sidecar, Source};
use plex::MediaServer;
use plex::model::{Item, Part, Stream, StreamKind};

use crate::error::{Error, Result};
use crate::store::Download;

/// The original file of `item` on `server`, starting where the viewer left
/// off, with the audio and subtitle tracks they last chose and the
/// subtitles the server keeps in files of their own.
pub fn of(server: &MediaServer, item: &Item) -> Result<Source> {
    let part = part(item).ok_or_else(|| Error::NotPlayable(item.title.clone()))?;
    let file = server.original_file(part);
    let sidecars = part.sidecars().filter_map(|stream| sidecar(server, stream)).collect();
    Ok(Source { location: file.url, headers: file.headers, sidecars, ..as_left(item) })
}

/// The subtitle file the server keeps as `stream`, to be fetched from it
/// with the same headers as the video.
fn sidecar(server: &MediaServer, stream: &Stream) -> Option<Sidecar> {
    Some(Sidecar {
        location: format!("{}{}", server.address(), stream.key.as_deref()?),
        title: stream.display_title.clone(),
        selected: stream.selected,
    })
}

/// The same file of `item` as `download` holds it, with the subtitle files
/// that were downloaded with it, so that nothing comes over the network.
///
/// `item` says which subtitles the viewer last chose, as far as it knows:
/// it is the server's word when the server is there.
pub fn on_disk(download: &Download, item: &Item) -> Source {
    // macOS only has file names that are text, so nothing is lost here.
    let location = |file: &Path| file.to_string_lossy().into_owned();
    let kept = |stream: &Stream| {
        // A download made before subtitles were kept has none on disk.
        let kept = download.subtitles.iter().find(|kept| kept.stream.id == stream.id)?;
        kept.file.exists().then(|| Sidecar {
            location: location(&kept.file),
            title: stream.display_title.clone(),
            selected: stream.selected,
        })
    };
    let sidecars = part(item).into_iter().flat_map(Part::sidecars).filter_map(kept).collect();
    Source { location: location(&download.file), sidecars, ..as_left(item) }
}

fn part(item: &Item) -> Option<&Part> {
    item.media.first().and_then(|media| media.parts.first())
}

/// Where `item` was left and the tracks last chosen for it, with no file.
///
/// Subtitles stay off unless the viewer chose some. A file may mark a
/// subtitle track as its default, and that is not the viewer asking for it.
fn as_left(item: &Item) -> Source {
    let part = part(item);
    Source {
        start: item.view_offset.map(Into::into).unwrap_or_default(),
        audio: part.and_then(|part| part.chosen(StreamKind::Audio)),
        subtitles: part.and_then(|part| part.chosen(StreamKind::Subtitle)),
        ..Source::default()
    }
}
