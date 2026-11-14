//! How large subtitles are drawn and how far they are moved in time.

use std::fmt;

use super::panel::Step;

/// The size of subtitles in hundredths of the size they come at: 120 is a
/// fifth larger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size(u32);

impl Size {
    /// Half is as small as stays readable from a sofa, and at three times
    /// two lines of a film fill a third of the picture.
    const SMALLEST: u32 = 50;
    const LARGEST: u32 = 300;
    const STEP: u32 = 10;

    /// The size nearest to `scale`, which is 1.0 for the size subtitles
    /// come at.
    pub fn from_scale(scale: f64) -> Self {
        // A scale that is no number at all is the size they come at.
        let percent = if scale.is_finite() { (scale * 100.0).round() } else { 100.0 };
        Self((percent.max(0.0) as u32).clamp(Self::SMALLEST, Self::LARGEST))
    }

    pub fn scale(self) -> f64 {
        f64::from(self.0) / 100.0
    }

    /// Where the size lies between the smallest and the largest, from 0
    /// to 1.
    pub fn level(self) -> f64 {
        f64::from(self.0 - Self::SMALLEST) / f64::from(Self::LARGEST - Self::SMALLEST)
    }

    pub fn stepped(self, step: Step) -> Self {
        let percent = match step {
            Step::Down => self.0.saturating_sub(Self::STEP),
            Step::Up => self.0 + Self::STEP,
        };
        Self(percent.clamp(Self::SMALLEST, Self::LARGEST))
    }
}

impl fmt::Display for Size {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}%", self.0)
    }
}

/// How much later than their file says subtitles are shown, in tenths of
/// a second. Negative is earlier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shift(i32);

impl Shift {
    /// Subtitles that are a minute out belong to another cut of the film,
    /// which no shift puts right.
    const MOST: i32 = 600;

    /// The shift nearest to `seconds`.
    pub fn from_seconds(seconds: f64) -> Self {
        let tenths = if seconds.is_finite() { (seconds * 10.0).round() } else { 0.0 };
        Self((tenths as i32).clamp(-Self::MOST, Self::MOST))
    }

    pub fn seconds(self) -> f64 {
        f64::from(self.0) / 10.0
    }

    pub fn stepped(self, step: Step) -> Self {
        let tenths = match step {
            Step::Down => self.0 - 1,
            Step::Up => self.0 + 1,
        };
        Self(tenths.clamp(-Self::MOST, Self::MOST))
    }
}

/// `0 s`, `+0.3 s`, `−1.2 s`: the sign says which way, and U+2212 is the
/// minus that is as wide as the plus.
impl fmt::Display for Shift {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = match self.0 {
            0 => return write!(formatter, "0 s"),
            tenths if tenths < 0 => '\u{2212}',
            _ => '+',
        };
        let tenths = self.0.unsigned_abs();
        write!(formatter, "{sign}{}.{} s", tenths / 10, tenths % 10)
    }
}

#[cfg(test)]
mod tests {
    use super::{Shift, Size, Step};

    #[test]
    fn a_size_is_read_from_what_the_player_reports_and_stops_at_its_ends() {
        // mpv hands the scale back as a float that is not quite a tenth.
        assert_eq!(Size::from_scale(1.100_000_023_841_858).to_string(), "110%");
        assert_eq!(Size::from_scale(1.0).stepped(Step::Up).scale(), 1.1);
        assert_eq!(Size::from_scale(0.5).stepped(Step::Down).to_string(), "50%");
        assert_eq!(Size::from_scale(3.0).stepped(Step::Up).to_string(), "300%");
        assert_eq!(Size::from_scale(f64::NAN).to_string(), "100%");
    }

    #[test]
    fn a_shift_says_which_way_it_goes() {
        let written = |seconds: f64| Shift::from_seconds(seconds).to_string();
        assert_eq!(written(0.0), "0 s");
        assert_eq!(written(0.3), "+0.3 s");
        assert_eq!(written(-1.2), "\u{2212}1.2 s");
        // Ten steps of a tenth are a second, with no drift on the way.
        let mut shift = Shift::from_seconds(0.0);
        for _ in 0..10 {
            shift = shift.stepped(Step::Down);
        }
        assert_eq!(shift.seconds(), -1.0);
        assert_eq!(Shift::from_seconds(60.0).stepped(Step::Up).seconds(), 60.0);
    }
}
