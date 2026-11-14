//! Words for the numbers the server sends.

use std::time::Duration;

use plex::model::Item;

/// A running time as people say it: "41 min", "2 h 18 min".
pub fn length(time: Duration) -> String {
    let minutes = (time.as_secs() + 30) / 60;
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

/// How much of `item` is left to watch, such as "23 min left". `None` when
/// it was not started.
pub fn remaining(item: &Item) -> Option<String> {
    let (offset, duration) = (item.view_offset?, item.duration?);
    let left = Duration::from_millis(duration.0.saturating_sub(offset.0));
    Some(format!("{} left", length(left)))
}

/// "S2 · E15" for an episode that knows both numbers.
pub fn episode_code(item: &Item) -> Option<String> {
    Some(format!("S{} · E{}", item.parent_index?, item.index?))
}

/// "1 season", "13 episodes".
pub fn count(number: u32, one: &str) -> String {
    if number == 1 { format!("1 {one}") } else { format!("{number} {one}s") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_round_to_the_minute_and_drop_empty_parts() {
        let seconds = |seconds| length(Duration::from_secs(seconds));
        assert_eq!(seconds(41 * 60 + 16), "41 min");
        assert_eq!(seconds(41 * 60 + 45), "42 min");
        assert_eq!(seconds(2 * 3600), "2 h");
        assert_eq!(seconds(8286), "2 h 18 min");
        assert_eq!(seconds(3599), "1 h");
    }
}
