//! Words for the numbers the server sends.

use std::time::Duration;

use plex::model::{Date, Item};

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

/// A day as it is written out in English: "Apr 24, 2011".
pub fn date(date: Date) -> String {
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    // A `Date` holds a month from 1 to 12.
    let month = MONTHS[usize::from(date.month - 1)];
    format!("{month} {}, {}", date.day, date.year)
}

/// The size of a file as the Finder writes it, in units of a thousand:
/// "6.8 GB", "734 MB".
pub fn size(bytes: u64) -> String {
    const MB: u64 = 1_000_000;
    const GB: u64 = 1_000 * MB;
    // Tenths of a gigabyte, rounded, without going through a float.
    let tenths = (bytes + GB / 20) / (GB / 10);
    match bytes {
        0..GB => format!("{} MB", (bytes + MB / 2) / MB),
        _ => format!("{}.{} GB", tenths / 10, tenths % 10),
    }
}

/// A whole number with its thousands set apart: "590,295".
pub fn thousands(number: u64) -> String {
    let digits = number.to_string();
    let mut grouped = String::new();
    for (place, digit) in digits.chars().enumerate() {
        if place > 0 && (digits.len() - place).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// How many hundredths of `whole` `part` is, rounded down, so that only
/// all of it is 100. All of nothing is there too.
pub fn percent(part: u64, whole: u64) -> u64 {
    part.saturating_mul(100).checked_div(whole).unwrap_or(100)
}

/// What kind of extra the server's `subtype` names.
pub fn extra(subtype: Option<&str>) -> &'static str {
    match subtype {
        Some("trailer") => "Trailer",
        Some("behindTheScenes") => "Behind the Scenes",
        Some("deletedScene") => "Deleted Scene",
        Some("featurette") => "Featurette",
        Some("interview") => "Interview",
        Some("sceneOrSample") => "Scene",
        _ => "Extra",
    }
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

    #[test]
    fn sizes_are_in_thousands_and_change_unit_at_a_gigabyte() {
        assert_eq!(size(733_900_000), "734 MB");
        assert_eq!(size(999_999_999), "1000 MB");
        assert_eq!(size(1_000_000_000), "1.0 GB");
        assert_eq!(size(6_828_818_931), "6.8 GB");
        assert_eq!(size(58_960_000_000), "59.0 GB");
    }

    #[test]
    fn thousands_are_set_apart() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1232), "1,232");
        assert_eq!(thousands(590_295), "590,295");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }

    #[test]
    fn a_download_is_at_a_hundred_percent_only_when_all_of_it_is_there() {
        assert_eq!(percent(0, 1000), 0);
        assert_eq!(percent(999, 1000), 99);
        assert_eq!(percent(1000, 1000), 100);
    }
}
