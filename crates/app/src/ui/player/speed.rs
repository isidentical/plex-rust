//! How fast the film plays.

use std::fmt;

/// A playback speed in hundredths of normal speed: 125 is one and a
/// quarter. Whole hundredths keep steps of 0.1 exact and still hold 0.75
/// and 1.25, which tenths could not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Speed(u32);

impl Speed {
    pub const NORMAL: Self = Self(100);
    /// Below this nothing moves, and above it mpv's pitch correction stops
    /// sounding like speech.
    const SLOWEST: u32 = 10;
    const FASTEST: u32 = 400;
    /// What one press of a speed key changes.
    const STEP: u32 = 10;

    /// The speeds the menu always offers. The keys reach the rest.
    const PRESETS: [Self; 9] = [
        Self(50),
        Self(75),
        Self(100),
        Self(125),
        Self(150),
        Self(175),
        Self(200),
        Self(250),
        Self(300),
    ];

    /// The speed nearest to `percent` that is watchable.
    pub fn from_percent(percent: u32) -> Self {
        Self(percent.clamp(Self::SLOWEST, Self::FASTEST))
    }

    pub fn percent(self) -> u32 {
        self.0
    }

    /// As mpv wants it, where 1.0 is normal speed.
    pub fn factor(self) -> f64 {
        f64::from(self.0) / 100.0
    }

    /// Where the speed lies between the slowest and the fastest, from 0
    /// to 1, counted in doublings: twice as fast is as far from normal
    /// as half as fast.
    pub fn level(self) -> f64 {
        let doublings = |percent: u32| f64::from(percent).log2();
        (doublings(self.0) - doublings(Self::SLOWEST))
            / (doublings(Self::FASTEST) - doublings(Self::SLOWEST))
    }

    pub fn slower(self) -> Self {
        Self::from_percent(self.0.saturating_sub(Self::STEP))
    }

    pub fn faster(self) -> Self {
        Self::from_percent(self.0 + Self::STEP)
    }

    /// What the speed menu lists while this is the speed: the presets, and
    /// this speed in its place among them if the keys took it off them.
    pub fn choices(self) -> Vec<Self> {
        let mut choices = Self::PRESETS.to_vec();
        if let Err(place) = choices.binary_search(&self) {
            choices.insert(place, self);
        }
        choices
    }
}

/// `1×`, `1.5×`, `1.25×`: no more digits than the speed needs.
impl fmt::Display for Speed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (whole, hundredths) = (self.0 / 100, self.0 % 100);
        match (hundredths, hundredths % 10) {
            (0, _) => write!(formatter, "{whole}\u{d7}"),
            (_, 0) => write!(formatter, "{whole}.{}\u{d7}", hundredths / 10),
            _ => write!(formatter, "{whole}.{hundredths:02}\u{d7}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Speed;

    #[test]
    fn a_speed_is_written_with_the_digits_it_needs() {
        let written = |percent| Speed::from_percent(percent).to_string();
        assert_eq!(written(100), "1×");
        assert_eq!(written(160), "1.6×");
        assert_eq!(written(75), "0.75×");
        assert_eq!(written(105), "1.05×");
    }

    #[test]
    fn the_menu_lists_a_speed_between_presets_in_its_place() {
        let percents = |speed: Speed| -> Vec<u32> {
            speed.choices().into_iter().map(Speed::percent).collect()
        };
        assert_eq!(percents(Speed::NORMAL), [50, 75, 100, 125, 150, 175, 200, 250, 300]);
        assert_eq!(
            percents(Speed::from_percent(160)),
            [50, 75, 100, 125, 150, 160, 175, 200, 250, 300]
        );
    }

    #[test]
    fn steps_of_a_tenth_stop_at_the_ends() {
        assert_eq!(Speed::from_percent(75).faster().percent(), 85);
        assert_eq!(Speed::from_percent(10).slower().percent(), 10);
        assert_eq!(Speed::from_percent(400).faster().percent(), 400);
    }
}
