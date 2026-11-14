//! Choosing the display refresh each video frame appears at.
//!
//! A display shows pictures only at its refreshes, and video frames are
//! due at times that fall between them. Each frame goes to the refresh
//! nearest its due time. Done naively that judders: Matroska rounds frame
//! times to whole milliseconds, so while a due time sits near the midpoint
//! between two refreshes, the rounding alone tosses frames to one side or
//! the other. [`Cadence`] therefore keeps the pace of the frame before as
//! long as that stays close to the nearest refresh.

use std::time::Duration;

use crate::clock::HostTime;

/// One refresh of the display.
#[derive(Debug, Clone, Copy)]
pub struct Refresh {
    /// When the picture prepared for this refresh reaches the screen.
    pub at: HostTime,
    /// The time from one refresh to the next.
    pub period: Duration,
}

/// Places frames on display refreshes, evenly.
#[derive(Debug, Default)]
pub struct Cadence {
    /// When the frame before was due, and the refresh it was given.
    last: Option<(HostTime, HostTime)>,
}

impl Cadence {
    /// Whether a frame due at `due` should be on screen at `refresh`, as
    /// opposed to at a later one. Ask at every refresh until it is.
    pub fn is_time(&self, due: HostTime, refresh: Refresh) -> bool {
        let period = i64::try_from(refresh.period.as_nanos()).unwrap_or(i64::MAX).max(1);
        let (due, now) = (due.nanos().cast_signed(), refresh.at.nanos().cast_signed());
        let nearest_has_come = due <= now + period / 2;
        let Some((last_due, last_shown)) = self.last else {
            return nearest_has_come;
        };

        // The refresh as many whole periods after the last frame's as the
        // two frames are apart.
        let apart = due - last_due.nanos().cast_signed();
        let periods = (apart + period / 2).div_euclid(period);
        let steady = last_shown.nanos().cast_signed() + periods * period;
        // The nearest refresh is at most half a period from the due time.
        // The eighth on top is what keeps frames from flipping sides: it
        // is 1 ms at 120 Hz, more than millisecond rounding can move a
        // due time.
        if (steady - due).abs() > period / 2 + period / 8 {
            return nearest_has_come;
        }
        now + period / 2 > steady
    }

    /// Records that the frame due at `due` was given `refresh`.
    pub fn shown(&mut self, due: HostTime, refresh: Refresh) {
        self.last = Some((due, refresh.at));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERIOD: Duration = Duration::from_nanos(8_333_333);

    /// Shows frames due at `due` (nanoseconds) on a 120 Hz display, and
    /// returns how many refreshes each stayed up and how late each was
    /// in nanoseconds.
    fn play(due: &[i64]) -> (Vec<i64>, Vec<i64>) {
        let period = PERIOD.as_nanos().cast_signed() as i64;
        let mut cadence = Cadence::default();
        let (mut shown, mut late) = (Vec::new(), Vec::new());
        let mut frames = due.iter().peekable();
        let mut index = 0_i64;
        while let Some(due) = frames.peek().copied() {
            let at = HostTime::from_nanos((index * period).cast_unsigned());
            let refresh = Refresh { at, period: PERIOD };
            let due_at = HostTime::from_nanos(due.cast_unsigned());
            if cadence.is_time(due_at, refresh) {
                cadence.shown(due_at, refresh);
                shown.push(index);
                late.push(index * period - due);
                frames.next();
            }
            index += 1;
        }
        (shown.windows(2).map(|pair| pair[1] - pair[0]).collect(), late)
    }

    /// Due times of frames `interval` seconds apart, starting a second in,
    /// rounded to milliseconds as a Matroska file stores them.
    fn matroska(interval: f64, count: usize) -> Vec<i64> {
        (0..count)
            .map(|frame| (1e3 + frame as f64 * interval * 1e3).round() as i64 * 1_000_000)
            .collect()
    }

    #[test]
    fn film_holds_five_refreshes_a_frame() {
        // 23.976 fps is 5.005 refreshes a frame: one frame in two hundred
        // has to stay for six, and no frame may stay for four.
        let (held, late) = play(&matroska(1001.0 / 24000.0, 3000));
        assert!(held.iter().all(|count| *count == 5 || *count == 6), "{held:?}");
        let sixes = held.iter().filter(|count| **count == 6).count();
        assert!((14..=16).contains(&sixes), "{sixes}");
        let limit = PERIOD.as_nanos().cast_signed() as i64 * 5 / 8;
        assert!(late.iter().all(|late| late.abs() <= limit + 1_000_000));
    }

    #[test]
    fn exact_24_fps_never_changes_pace() {
        let (held, _) = play(&matroska(1.0 / 24.0, 3000));
        assert!(held.iter().all(|count| *count == 5), "{held:?}");
    }

    #[test]
    fn faster_playback_uses_the_two_nearest_counts() {
        // 1.5x, 2x and 3x of 24 fps: 3.33, 2.5 and 1.67 refreshes a frame.
        for (speed, counts) in [(1.5, [3, 4]), (2.0, [2, 3]), (3.0, [1, 2])] {
            let (held, late) = play(&matroska(1.0 / 24.0 / speed, 3000));
            assert!(held.iter().all(|count| counts.contains(count)), "{speed}: {held:?}");
            let limit = PERIOD.as_nanos().cast_signed() as i64 * 5 / 8;
            assert!(late.iter().all(|late| late.abs() <= limit + 1_000_000));
        }
    }

    #[test]
    fn an_overdue_frame_is_shown_at_once() {
        let mut cadence = Cadence::default();
        let refresh = |at| Refresh { at: HostTime::from_nanos(at), period: PERIOD };
        cadence.shown(HostTime::from_nanos(1_000_000_000), refresh(1_000_000_000));
        // Due 20 ms after the last one, but the display has moved on by a
        // whole second, as after a stall.
        assert!(cadence.is_time(HostTime::from_nanos(1_020_000_000), refresh(2_000_000_000)));
    }
}
