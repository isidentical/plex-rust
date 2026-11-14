//! The system's media controls: the keyboard's media keys, the buttons on
//! headphones, and Control Center. They send the player commands and show
//! what it is playing.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::MainThreadBound;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadMarker, Message};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSSize, NSString};
use objc2_media_player::{
    MPChangePlaybackPositionCommandEvent, MPMediaItemArtwork, MPMediaItemPropertyAlbumTitle,
    MPMediaItemPropertyArtist, MPMediaItemPropertyArtwork, MPMediaItemPropertyPlaybackDuration,
    MPMediaItemPropertyTitle, MPNowPlayingInfoCenter, MPNowPlayingInfoMediaType,
    MPNowPlayingInfoPropertyElapsedPlaybackTime, MPNowPlayingInfoPropertyMediaType,
    MPNowPlayingInfoPropertyPlaybackRate, MPNowPlayingPlaybackState, MPRemoteCommand,
    MPRemoteCommandCenter, MPRemoteCommandEvent, MPRemoteCommandHandlerStatus,
    MPSkipIntervalCommandEvent,
};
use plex::model::{Item, Kind};

use crate::images::Artwork;

/// What the viewer asked for from outside the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Play,
    Pause,
    TogglePause,
    /// The next episode.
    Next,
    /// The start of this episode, or the one before it.
    Previous,
    /// Jump this many seconds, backwards if negative.
    Skip(f64),
    /// Go to this position in the file.
    Seek(Duration),
}

/// Where a playback stands, for the system to show.
pub struct Playing<'a> {
    pub item: &'a Item,
    pub position: Duration,
    pub duration: Duration,
    /// How fast the position moves: the playback speed, or 0 while paused.
    /// The system counts the position on from this by itself.
    pub rate: f64,
}

/// The player's hold on the system's media controls, from [`register`]
/// to [`release`]. There is one set of controls for the whole app, so
/// there must be one of these at a time.
///
/// [`register`]: Self::register
/// [`release`]: Self::release
pub struct Remote {
    /// Every command that was given a handler, with what takes the handler
    /// away again.
    targets: Vec<(Retained<MPRemoteCommand>, Retained<AnyObject>)>,
    artwork: RefCell<Option<Retained<MPMediaItemArtwork>>>,
}

impl Remote {
    /// Has `act` called on the main thread with every command the viewer
    /// gives through the media controls. `item` is what will play: only
    /// an episode has a next and a previous one to offer. `skip` is how
    /// many seconds the controls should offer to jump by.
    pub fn register(
        main_thread: MainThreadMarker,
        item: &Item,
        skip: f64,
        act: impl Fn(Command) + 'static,
    ) -> Self {
        // MediaPlayer does not say which thread it calls a handler on.
        let act: Box<dyn Fn(Command)> = Box::new(act);
        let act = Arc::new(MainThreadBound::new(act, main_thread));
        let mut targets = Vec::new();
        let mut add = |command: &MPRemoteCommand,
                       read: fn(&MPRemoteCommandEvent) -> Option<Command>| {
            let act = Arc::clone(&act);
            let handler = RcBlock::new(move |event: NonNull<MPRemoteCommandEvent>| {
                // SAFETY: MediaPlayer passes an event that is valid for the
                // length of the call.
                let Some(command) = read(unsafe { event.as_ref() }) else {
                    return MPRemoteCommandHandlerStatus::CommandFailed;
                };
                // The command may close the player, which takes this
                // handler away while it is still running.
                let act = Arc::clone(&act);
                act.get_on_main(|act| act(command));
                MPRemoteCommandHandlerStatus::Success
            });
            // SAFETY: the handler takes and returns what the method
            // declares, and can be called from any thread.
            let target = unsafe {
                command.setEnabled(true);
                command.addTargetWithHandler(&handler)
            };
            targets.push((command.retain(), target));
        };

        let is_episode = item.kind == Kind::Episode;
        let intervals = NSArray::from_retained_slice(&[NSNumber::new_f64(skip)]);
        // SAFETY: these read the app's one command center and its commands,
        // and set a property to a value of its declared type.
        unsafe {
            let center = MPRemoteCommandCenter::sharedCommandCenter();
            add(&center.playCommand(), |_| Some(Command::Play));
            add(&center.pauseCommand(), |_| Some(Command::Pause));
            add(&center.togglePlayPauseCommand(), |_| Some(Command::TogglePause));
            if is_episode {
                add(&center.nextTrackCommand(), |_| Some(Command::Next));
                add(&center.previousTrackCommand(), |_| Some(Command::Previous));
            }
            let (forward, back) = (center.skipForwardCommand(), center.skipBackwardCommand());
            forward.setPreferredIntervals(&intervals);
            back.setPreferredIntervals(&intervals);
            add(&forward, |event| interval(event).map(Command::Skip));
            add(&back, |event| interval(event).map(|seconds| Command::Skip(-seconds)));
            add(&center.changePlaybackPositionCommand(), |event| {
                position(event).map(Command::Seek)
            });
        }
        Self { targets, artwork: RefCell::new(None) }
    }

    /// The picture shown next to the title from the next [`show`] on.
    ///
    /// [`show`]: Self::show
    pub fn set_artwork(&self, picture: &Artwork) {
        let image = picture.image();
        let size = image.size();
        let provide = RcBlock::new(move |_: NSSize| NonNull::from(&*image));
        // SAFETY: the block returns an image that it keeps for as long as
        // the artwork keeps the block.
        let artwork = unsafe {
            MPMediaItemArtwork::initWithBoundsSize_requestHandler(
                MPMediaItemArtwork::alloc(),
                size,
                &provide,
            )
        };
        *self.artwork.borrow_mut() = Some(artwork);
    }

    /// Tells the system what is playing and where it has got to.
    pub fn show(&self, playing: &Playing) {
        let item = playing.item;
        let text = |text: &str| object(&*NSString::from_str(text));
        let number = |number: f64| object(&*NSNumber::new_f64(number));
        // SAFETY: the keys are constants that MediaPlayer provides.
        let mut info = unsafe {
            vec![
                (MPMediaItemPropertyTitle, text(&item.title)),
                (MPMediaItemPropertyPlaybackDuration, number(playing.duration.as_secs_f64())),
                (
                    MPNowPlayingInfoPropertyElapsedPlaybackTime,
                    number(playing.position.as_secs_f64()),
                ),
                (MPNowPlayingInfoPropertyPlaybackRate, number(playing.rate)),
                (
                    MPNowPlayingInfoPropertyMediaType,
                    object(&*NSNumber::new_usize(MPNowPlayingInfoMediaType::Video.0)),
                ),
            ]
        };
        // SAFETY: as above.
        unsafe {
            if let Some(show) = &item.grandparent_title {
                info.push((MPMediaItemPropertyArtist, text(show)));
            }
            if let Some(season) = &item.parent_title {
                info.push((MPMediaItemPropertyAlbumTitle, text(season)));
            }
            if let Some(artwork) = self.artwork.borrow().as_deref() {
                info.push((MPMediaItemPropertyArtwork, object(artwork)));
            }
        }
        let (keys, values): (Vec<&NSString>, Vec<&AnyObject>) =
            info.iter().map(|(key, value)| (*key, &**value)).unzip();
        let info = NSDictionary::from_slices(&keys, &values);
        let state = if playing.rate > 0.0 {
            MPNowPlayingPlaybackState::Playing
        } else {
            MPNowPlayingPlaybackState::Paused
        };
        // SAFETY: the dictionary holds the type of value MediaPlayer
        // documents for each key.
        unsafe {
            let center = MPNowPlayingInfoCenter::defaultCenter();
            center.setNowPlayingInfo(Some(&info));
            center.setPlaybackState(state);
        }
    }

    /// Gives the media controls back: no command reaches the player after
    /// this, and the system shows nothing as playing.
    pub fn release(self) {
        // SAFETY: each target is the one its command returned in
        // `register`.
        unsafe {
            for (command, target) in &self.targets {
                command.removeTarget(Some(target));
            }
            let center = MPNowPlayingInfoCenter::defaultCenter();
            center.setNowPlayingInfo(None);
            center.setPlaybackState(MPNowPlayingPlaybackState::Stopped);
        }
    }
}

/// The seconds a skip command asks to jump by.
fn interval(event: &MPRemoteCommandEvent) -> Option<f64> {
    let event = event.downcast_ref::<MPSkipIntervalCommandEvent>()?;
    // SAFETY: reading a property of a valid event.
    Some(unsafe { event.interval() })
}

/// The position a command asks to go to, as dragging the scrubber in
/// Control Center does.
fn position(event: &MPRemoteCommandEvent) -> Option<Duration> {
    let event = event.downcast_ref::<MPChangePlaybackPositionCommandEvent>()?;
    // SAFETY: reading a property of a valid event.
    Duration::try_from_secs_f64(unsafe { event.positionTime() }).ok()
}

/// `value` as a dictionary holds it.
fn object<T: Message + AsRef<AnyObject>>(value: &T) -> Retained<AnyObject> {
    value.as_ref().retain()
}
