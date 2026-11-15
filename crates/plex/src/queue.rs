//! What plays after what: the items the player goes through one after
//! another, going on to the next at the end of each.

use crate::model::{Item, RatingKey};

/// The items to go to from one item.
#[derive(Debug, Clone, PartialEq)]
pub struct Neighbours {
    pub previous: Option<Item>,
    pub next: Option<Item>,
}

/// Items in the order they play in. Each item's neighbours are the items
/// beside it, so whatever made the order, going on is the same walk.
#[derive(Debug, Clone, PartialEq)]
pub struct Queue {
    items: Vec<Item>,
    shuffled: bool,
}

impl Queue {
    /// The episodes to go through from `episode`, given every episode of
    /// its show in the server's order, as
    /// [`MediaServer::episodes`](crate::MediaServer::episodes) lists them.
    ///
    /// The order is walked as it stands. The last episode of a season leads
    /// to the first of the season after it, and a season the server does
    /// not have or a number missing within a season is stepped over,
    /// because no number is ever counted up from.
    ///
    /// Specials are a row of their own: from a special the queue holds
    /// only specials, and from any other episode no special.
    pub fn of_show(episodes: Vec<Item>, episode: &RatingKey) -> Self {
        let special =
            episodes.iter().any(|other| other.rating_key == *episode && is_special(other));
        let items = episodes.into_iter().filter(|other| is_special(other) == special).collect();
        Self { items, shuffled: false }
    }

    /// `items` in a random order drawn from `seed`, each of them once. The
    /// same seed gives the same order.
    pub fn shuffled(mut items: Vec<Item>, seed: u64) -> Self {
        fastrand::Rng::with_seed(seed).shuffle(&mut items);
        Self { items, shuffled: true }
    }

    /// What plays first.
    pub fn first(&self) -> Option<&Item> {
        self.items.first()
    }

    /// Whether the order is a random one, and not the show's own.
    pub fn is_shuffled(&self) -> bool {
        self.shuffled
    }

    /// The items before and after `item`. Neither, for an item that is
    /// not in the queue.
    pub fn neighbours(&self, item: &RatingKey) -> Neighbours {
        let Some(at) = self.items.iter().position(|other| other.rating_key == *item) else {
            return Neighbours { previous: None, next: None };
        };
        let previous = at.checked_sub(1).and_then(|before| self.items.get(before));
        Neighbours { previous: previous.cloned(), next: self.items.get(at + 1).cloned() }
    }
}

/// Whether `episode` is a special, which the server keeps as season 0.
pub fn is_special(episode: &Item) -> bool {
    episode.parent_index == Some(0)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// Every episode of a show with two specials and eleven seasons, cut
    /// down to the first two and the last episode of season 1 and the first
    /// and last of seasons 2 and 4, as recorded from a real server.
    fn leaves() -> Vec<Item> {
        let json = include_str!("../tests/fixtures/leaves.json");
        let mut envelope: serde_json::Value = serde_json::from_str(json).unwrap();
        serde_json::from_value(envelope["MediaContainer"]["Metadata"].take()).unwrap()
    }

    type SeasonAndNumber = Option<(u32, u32)>;

    /// The episodes around the one with `key` in its show's queue, each as
    /// (season, number).
    fn around(key: &str) -> (SeasonAndNumber, SeasonAndNumber) {
        let key = RatingKey::new(key);
        let numbers = |episode: Option<Item>| {
            episode.map(|episode| (episode.parent_index.unwrap(), episode.index.unwrap()))
        };
        let Neighbours { previous, next } = Queue::of_show(leaves(), &key).neighbours(&key);
        (numbers(previous), numbers(next))
    }

    #[test]
    fn a_season_leads_into_the_next_and_back() {
        // Season 1 jumps from episode 2 to 13, and season 3 is missing.
        assert_eq!(around("1058"), (Some((1, 2)), Some((2, 1))));
        assert_eq!(around("1085"), (Some((1, 13)), Some((2, 13))));
        assert_eq!(around("1097"), (Some((2, 1)), Some((4, 1))));
        assert_eq!(around("1117"), (Some((2, 13)), Some((4, 13))));
    }

    #[test]
    fn a_show_ends_at_its_first_and_last_episode() {
        // The specials come before season 1 and are not gone back into.
        assert_eq!(around("1046"), (None, Some((1, 2))));
        assert_eq!(around("1129"), (Some((4, 1)), None));
        assert_eq!(around("no such episode"), (None, None));
    }

    #[test]
    fn specials_lead_only_to_specials() {
        assert_eq!(around("1199"), (None, Some((0, 2))));
        assert_eq!(around("1200"), (Some((0, 1)), None));
    }

    /// The keys of `queue` in the order it plays them, walked from its
    /// first item by way of each item's next one.
    fn walk(queue: &Queue) -> Vec<RatingKey> {
        let mut played = Vec::new();
        let mut playing = queue.first().cloned();
        while let Some(item) = playing {
            // A repeat would walk forever. Stop it at the first one.
            assert!(!played.contains(&item.rating_key), "{} played twice", item.rating_key);
            playing = queue.neighbours(&item.rating_key).next;
            played.push(item.rating_key);
        }
        played
    }

    #[test]
    fn a_shuffle_plays_every_item_once_and_then_stops() {
        let items = leaves();
        let all: HashSet<RatingKey> = items.iter().map(|item| item.rating_key.clone()).collect();
        let queue = Queue::shuffled(items, 7);

        let played = walk(&queue);
        assert_eq!(played.len(), all.len());
        assert_eq!(played.iter().cloned().collect::<HashSet<_>>(), all);
        // It starts on its first item, with nothing before it.
        let first = queue.first().unwrap();
        assert_eq!(played[0], first.rating_key);
        assert_eq!(queue.neighbours(&first.rating_key).previous, None);
        // Going back retraces the same order.
        let last = played.last().unwrap();
        let before_last = queue.neighbours(last).previous.unwrap();
        assert_eq!(before_last.rating_key, played[played.len() - 2]);
    }

    #[test]
    fn a_shuffle_takes_the_order_of_its_seed() {
        let order = |seed| walk(&Queue::shuffled(leaves(), seed));
        let shows_order: Vec<RatingKey> =
            leaves().into_iter().map(|item| item.rating_key).collect();
        assert_eq!(order(7), order(7));
        assert_ne!(order(7), order(8));
        assert_ne!(order(7), shows_order);
        assert!(Queue::shuffled(leaves(), 7).is_shuffled());
        assert!(!Queue::of_show(leaves(), &RatingKey::new("1058")).is_shuffled());
    }

    #[test]
    fn nothing_to_shuffle_plays_nothing() {
        let queue = Queue::shuffled(Vec::new(), 7);
        assert_eq!(queue.first(), None);
        assert!(walk(&queue).is_empty());
    }
}
