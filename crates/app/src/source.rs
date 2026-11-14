//! Turning a library item into something the player can load.

use std::path::Path;

use player::Source;
use plex::MediaServer;
use plex::model::{Item, Part, StreamKind};

use crate::error::{Error, Result};

/// The original file of `item` on `server`, starting where the viewer left
/// off, with the audio and subtitle tracks they last chose.
pub fn of(server: &MediaServer, item: &Item) -> Result<Source> {
    let part = part(item).ok_or_else(|| Error::NotPlayable(item.title.clone()))?;
    let file = server.original_file(part);
    Ok(Source { location: file.url, headers: file.headers, ..as_left(item) })
}

/// The same file of `item` as it was downloaded to `file`, so that nothing
/// of it comes over the network.
pub fn on_disk(file: &Path, item: &Item) -> Source {
    // macOS only has file names that are text, so nothing is lost here.
    Source { location: file.to_string_lossy().into_owned(), ..as_left(item) }
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
