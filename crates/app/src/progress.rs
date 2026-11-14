//! Telling the server how far a playback has got.

use std::sync::Mutex;
use std::time::Duration;

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
    /// The file's length, kept for when mpv no longer knows it.
    duration: Mutex<Option<Duration>>,
}

impl Progress {
    pub fn new(runtime: Runtime, server: MediaServer, item: RatingKey) -> Self {
        let session = NSUUID::UUID().UUIDString().to_string();
        Self { runtime, server, item, session, duration: Mutex::new(None) }
    }

    /// Sends the player's current position with `state`. Blocks until the
    /// server has answered.
    pub fn report(&self, player: &Player, state: PlaybackState) -> Result<()> {
        // Once a file has played to its end mpv has closed it and knows
        // neither its length nor a position in it. The length is then the
        // last one seen, and the position is the end.
        let mut last = self.duration.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(duration) = player.duration()?.or(*last) else {
            // Nothing was ever loaded, so there is no progress to speak of.
            return Ok(());
        };
        *last = Some(duration);
        drop(last);
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
