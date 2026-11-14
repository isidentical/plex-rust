//! Telling the server how far a playback has got.

use objc2_foundation::NSUUID;
use player::Player;
use plex::MediaServer;
use plex::model::RatingKey;
use plex::playback::{self, PlaybackState};
use tokio::runtime::Runtime;

use crate::error::Result;

/// Reports the progress of one playback of one item.
pub struct Progress {
    runtime: Runtime,
    server: MediaServer,
    item: RatingKey,
    session: String,
}

impl Progress {
    pub fn new(runtime: Runtime, server: MediaServer, item: RatingKey) -> Self {
        let session = NSUUID::UUID().UUIDString().to_string();
        Self {
            runtime,
            server,
            item,
            session,
        }
    }

    /// Sends the player's current position with `state`. Blocks until the
    /// server has answered.
    pub fn report(&self, player: &Player, state: PlaybackState) -> Result<()> {
        let Some(duration) = player.duration()? else {
            // Nothing is loaded yet, so there is no progress to speak of.
            return Ok(());
        };
        // mpv has no position once a file has played to its end.
        let position = player.position()?.unwrap_or(duration);

        let progress = playback::Progress {
            item: &self.item,
            session: &self.session,
            state,
            position,
            duration,
        };
        Ok(self.runtime.block_on(self.server.report(&progress))?)
    }
}
