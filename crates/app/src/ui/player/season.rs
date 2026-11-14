//! The episodes on either side of the one being played.

use plex::model::Item;

#[derive(Debug, Clone, PartialEq)]
pub struct Neighbours {
    pub previous: Option<Item>,
    pub next: Option<Item>,
}

impl Neighbours {
    /// Picks the neighbours of episode `number` out of `episodes`, its
    /// whole season. They are the nearest numbers below and above, not one
    /// less and one more: a season can have a gap, and one file can hold
    /// two episodes. An episode with no number is nobody's neighbour.
    pub fn of(episodes: Vec<Item>, number: u32) -> Self {
        let (mut previous, mut next): (Option<Item>, Option<Item>) = (None, None);
        for episode in episodes {
            let Some(index) = episode.index else {
                continue;
            };
            let nearer_below =
                previous.as_ref().is_none_or(|previous| previous.index < Some(index));
            let nearer_above = next.as_ref().is_none_or(|next| next.index > Some(index));
            if index < number && nearer_below {
                previous = Some(episode);
            } else if index > number && nearer_above {
                next = Some(episode);
            }
        }
        Self { previous, next }
    }
}
