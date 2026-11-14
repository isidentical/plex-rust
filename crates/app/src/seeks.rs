//! Timing jumps in a playing file: the measurement behind `play --seeks`.
//!
//! A script is a comma-separated list of steps. `3x20` presses "3 seconds
//! forward" 20 times, letting playback pick up again after each press, and
//! times every one. `-3x30@80` holds "3 seconds back" down: 30 presses, 80 ms
//! apart, whether or not the one before has finished. A separate press is
//! [`Player::seek_by`] and a press of a held key is [`Player::scan_by`], as
//! in the player screen.

use std::time::{Duration, Instant};

use player::{Moment, Player, Probe};
use serde_json::{Value, json};

/// How long playback runs between two timed presses, so that each one
/// interrupts a picture that is moving and a sound that is playing.
const SETTLE: Duration = Duration::from_millis(700);

/// Frames that mpv hands over within this long of a restart were waiting
/// for it and say nothing about the picture moving: one refresh at 60 Hz.
const HANDED_AT_ONCE: Duration = Duration::from_millis(17);

/// When a press that has not finished counts as failed.
const GIVE_UP: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// What the script called it.
    name: String,
    /// The jump, backwards if negative.
    seconds: f64,
    presses: u32,
    /// The time between presses of a held key. `None` waits for each press
    /// to finish instead.
    every: Option<Duration>,
}

/// Reads a script, or returns `None` if it is not one.
pub fn parse(script: &str) -> Option<Vec<Step>> {
    script.split(',').map(step).collect()
}

fn step(name: &str) -> Option<Step> {
    let (jump, every) = match name.split_once('@') {
        Some((jump, ms)) => (jump, Some(Duration::from_millis(ms.parse().ok()?))),
        None => (name, None),
    };
    let (seconds, presses) = jump.split_once('x')?;
    Some(Step {
        name: name.to_owned(),
        seconds: seconds.parse().ok()?,
        presses: presses.parse().ok()?,
        every,
    })
}

/// How a playback began.
pub struct Start {
    /// From asking for the file to its first picture being ready.
    pub took: Duration,
    /// How long sound takes from the audio device to the ear, if the device
    /// said. Sound is heard this much later than every `audio_ms` below.
    pub audio_latency: Option<Duration>,
}

/// How one jump went, as times since the key press.
#[derive(Default)]
struct Stall {
    /// Until mpv had the first picture at the new position.
    restarted: Option<Duration>,
    /// Until sound was handed to the audio device.
    audio: Option<Duration>,
    /// Until the picture moved on from the frame the jump landed on.
    moving: Option<Duration>,
    /// Where the jump landed.
    landed: Option<f64>,
    /// How far the sound was ahead of the picture, in seconds, at every
    /// frame from the restart until the next press.
    av_offsets: Vec<f64>,
}

impl Stall {
    fn is_over(&self) -> bool {
        self.restarted.is_some() && self.audio.is_some() && self.moving.is_some()
    }

    /// The time from mpv having the first picture until the picture moved.
    fn hold(&self) -> Option<Duration> {
        Some(self.moving?.saturating_sub(self.restarted?))
    }

    fn note(&mut self, moment: Moment, pressed: Instant, player: &Player) -> player::Result<()> {
        let now = pressed.elapsed();
        match moment {
            Moment::Restarted => {
                self.restarted = Some(now);
                self.landed = player.position()?.map(|position| position.as_secs_f64());
            }
            // A press that interrupts an unfinished jump restarts the sound
            // it had not started yet. Only the last start counts.
            Moment::AudioStarted => self.audio = Some(now),
            Moment::Position(position) => {
                let (Some(restarted), Some(landed)) = (self.restarted, self.landed) else {
                    return Ok(());
                };
                // mpv moves the position on when it hands a frame over for
                // display. It hands over one or two at once when it
                // restarts, and from then on each one as the frame before
                // it is due, so the first that comes later than that is the
                // picture moving.
                if self.moving.is_none() && position > landed && now > restarted + HANDED_AT_ONCE {
                    self.moving = Some(now);
                }
            }
            // Whatever finished before mpv took up this press belongs to
            // an earlier one.
            Moment::Seeking => *self = Self::default(),
            Moment::AvOffset(offset) => {
                if self.restarted.is_some() {
                    self.av_offsets.push(offset);
                }
            }
            Moment::PictureShown | Moment::AudioLatency(_) | Moment::Shutdown => {}
        }
        Ok(())
    }
}

/// Waits for the playback that `player` was just told to load, which began
/// at `loading`, and returns how it started.
pub fn started(probe: &mut Probe, loading: Instant) -> Option<Start> {
    let mut audio_latency = None;
    loop {
        match probe.wait(GIVE_UP.saturating_sub(loading.elapsed()))? {
            Moment::Restarted => return Some(Start { took: loading.elapsed(), audio_latency }),
            Moment::AudioLatency(latency) => audio_latency = Some(latency),
            Moment::Shutdown => return None,
            _ => {}
        }
    }
}

/// Lets playback run for `time`, returning how many jumps got as far as
/// showing their picture in it.
pub fn idle(probe: &mut Probe, time: Duration) -> u32 {
    let until = Instant::now() + time;
    let mut pictures = 0;
    while let Some(moment) = probe.wait(until.saturating_duration_since(Instant::now())) {
        pictures += u32::from(moment == Moment::PictureShown);
    }
    pictures
}

/// Runs the script and reports each step.
pub fn run(player: &Player, probe: &mut Probe, steps: &[Step]) -> player::Result<Value> {
    idle(probe, SETTLE);
    steps
        .iter()
        .map(|step| match step.every {
            Some(every) => held(player, probe, step, every),
            None => pressed(player, probe, step),
        })
        .collect()
}

/// Separate presses: every one is timed.
fn pressed(player: &Player, probe: &mut Probe, step: &Step) -> player::Result<Value> {
    let mut stalls = Vec::new();
    let mut off_target: f64 = 0.0;
    for _ in 0..step.presses {
        let from = player.position()?.map(|position| position.as_secs_f64());
        let mut stall = jump(player, probe, step.seconds, Player::seek_by)?;
        if let (Some(from), Some(landed)) = (from, stall.landed) {
            off_target = off_target.max((landed - from - step.seconds).abs());
        }
        settle(probe, &mut stall);
        stalls.push(stall);
    }
    let ms = |offset: f64| offset.abs() * 1000.0;
    let worst = stalls.iter().flat_map(|stall| &stall.av_offsets).copied().map(ms);
    let settled = stalls.iter().filter_map(|stall| stall.av_offsets.last()).copied().map(ms);
    Ok(json!({
        "step": step.name,
        "restarted_ms": spread(stalls.iter().map(|stall| stall.restarted)),
        "audio_ms": spread(stalls.iter().map(|stall| stall.audio)),
        "moving_ms": spread(stalls.iter().map(|stall| stall.moving)),
        // How long the picture a jump landed on stood still. One frame's
        // duration means it did not wait for anything.
        "hold_ms": spread(stalls.iter().map(Stall::hold)),
        // The furthest apart that sound and picture were, in milliseconds:
        // at any frame after a jump, and when the next jump was due.
        "av_offset_ms": { "worst": worst.reduce(f64::max), "settled": settled.reduce(f64::max) },
        // The furthest a jump landed from where it was aimed, in seconds.
        "off_target": off_target,
        "unfinished": stalls.iter().filter(|stall| !stall.is_over()).count(),
    }))
}

/// Lets playback run on after a jump as [`idle`] does, noting how far apart
/// sound and picture are meanwhile.
fn settle(probe: &mut Probe, stall: &mut Stall) {
    let until = Instant::now() + SETTLE;
    while let Some(moment) = probe.wait(until.saturating_duration_since(Instant::now())) {
        if let Moment::AvOffset(offset) = moment {
            stall.av_offsets.push(offset);
        }
    }
}

/// A held key: what matters is how many pictures show while it is down and
/// how long playback takes to pick up after the last press.
fn held(player: &Player, probe: &mut Probe, step: &Step, every: Duration) -> player::Result<Value> {
    let from = player.position()?.map(|position| position.as_secs_f64());
    let mut pictures = 0;
    for _ in 1..step.presses {
        player.scan_by(step.seconds)?;
        pictures += idle(probe, every);
    }
    let release = jump(player, probe, step.seconds, Player::scan_by)?;
    idle(probe, SETTLE);
    let ms = |time: Option<Duration>| time.map(|time| time.as_secs_f64() * 1000.0);
    Ok(json!({
        "step": step.name,
        "pictures_while_held": pictures,
        "restarted_ms": ms(release.restarted),
        "audio_ms": ms(release.audio),
        "moving_ms": ms(release.moving),
        "travelled": from.zip(release.landed).map(|(from, landed)| landed - from),
    }))
}

/// Presses once and watches until playback has fully picked up again.
/// `press` is what the key does: [`Player::seek_by`] or [`Player::scan_by`].
fn jump(
    player: &Player,
    probe: &mut Probe,
    seconds: f64,
    press: fn(&Player, f64) -> player::Result<()>,
) -> player::Result<Stall> {
    let mut stall = Stall::default();
    let pressed = Instant::now();
    press(player, seconds)?;
    while !stall.is_over() {
        let Some(moment) = probe.wait(GIVE_UP.saturating_sub(pressed.elapsed())) else {
            break;
        };
        stall.note(moment, pressed, player)?;
    }
    Ok(stall)
}

/// The median, 90th percentile and maximum of `times` in milliseconds,
/// leaving out the ones that never happened.
fn spread(times: impl Iterator<Item = Option<Duration>>) -> Value {
    let mut ms: Vec<f64> = times.flatten().map(|time| time.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    let at = |percent: usize| ms.get((ms.len() * percent).div_ceil(100).saturating_sub(1));
    json!({ "p50": at(50), "p90": at(90), "max": ms.last(), "n": ms.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_has_pressed_and_held_steps() {
        let steps = parse("3x20,-10x5@80").unwrap();
        assert_eq!(steps[0], Step { name: "3x20".into(), seconds: 3.0, presses: 20, every: None });
        assert_eq!(steps[1].seconds, -10.0);
        assert_eq!(steps[1].every, Some(Duration::from_millis(80)));
        assert_eq!(parse("3"), None);
        assert_eq!(parse("3x2,"), None);
    }
}
