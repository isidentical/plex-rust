//! Whether the server is there, going by what was last heard from it.
//!
//! Nothing here asks the server anything. Every request the app makes of
//! it anyway ends in an answer or in silence, and each of those is a
//! [`Word`] that moves the [`Reach`]. What is drawn and what is played
//! go by that one state, so they agree.

use std::time::Instant;

/// What is known of the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Nothing was heard yet, either way.
    Unknown,
    /// It answered, the last time at `heard`.
    Answering { heard: Instant },
    /// It did not answer. Downloads play without it.
    Away,
}

/// What came of asking the server something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Word {
    /// It answered at `heard`, if only to refuse.
    Answer { heard: Instant },
    /// It never answered what was asked at `asked`.
    Silence { asked: Instant },
}

impl Word {
    /// What a request made at `asked` says of the server, having ended at
    /// `now` in `error` or in none. `None` when it says nothing: an error
    /// that is the app's own, such as nothing having been kept.
    pub fn of(error: Option<&plex::Error>, asked: Instant, now: Instant) -> Option<Self> {
        match error {
            None => Some(Self::Answer { heard: now }),
            Some(error) if error.is_unanswered() => Some(Self::Silence { asked }),
            Some(error) if error.is_from_server() => Some(Self::Answer { heard: now }),
            Some(_) => None,
        }
    }
}

impl Reach {
    /// What is known once `word` has come in.
    ///
    /// An answer always counts. Silence counts unless the request it is
    /// about was made before the last answer: a request can wait half a
    /// minute for a reply that never comes while later ones are answered,
    /// and its giving up then says nothing about now.
    pub fn after(self, word: Word) -> Self {
        match (self, word) {
            (_, Word::Answer { heard }) => Self::Answering { heard },
            (Self::Answering { heard }, Word::Silence { asked }) if asked < heard => self,
            (_, Word::Silence { .. }) => Self::Away,
        }
    }

    pub fn is_answering(self) -> bool {
        matches!(self, Self::Answering { .. })
    }

    pub fn is_away(self) -> bool {
        self == Self::Away
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use plex::model::RatingKey;

    use super::*;

    /// A moment `seconds` after `start`.
    fn at(start: Instant, seconds: u64) -> Instant {
        start + Duration::from_secs(seconds)
    }

    #[test]
    fn the_first_word_settles_what_was_unknown() {
        let start = Instant::now();
        let answer = Word::Answer { heard: start };
        assert_eq!(Reach::Unknown.after(answer), Reach::Answering { heard: start });
        assert_eq!(Reach::Unknown.after(Word::Silence { asked: start }), Reach::Away);
    }

    #[test]
    fn a_server_that_stops_answering_is_away_until_it_answers_again() {
        let start = Instant::now();
        let answering = Reach::Unknown.after(Word::Answer { heard: at(start, 1) });
        let away = answering.after(Word::Silence { asked: at(start, 2) });
        assert!(away.is_away() && !away.is_answering());
        // Still nothing: it stays away.
        assert_eq!(away.after(Word::Silence { asked: at(start, 3) }), Reach::Away);
        let back = away.after(Word::Answer { heard: at(start, 9) });
        assert_eq!(back, Reach::Answering { heard: at(start, 9) });
    }

    #[test]
    fn a_request_that_gives_up_late_does_not_undo_a_later_answer() {
        let start = Instant::now();
        // Asked at 0 and never answered, it gives up at 30. Meanwhile
        // another, asked at 20, was answered at 21.
        let answering = Reach::Unknown.after(Word::Answer { heard: at(start, 21) });
        assert_eq!(answering.after(Word::Silence { asked: start }), answering);
        // Asked after that answer, silence is news.
        assert_eq!(answering.after(Word::Silence { asked: at(start, 22) }), Reach::Away);
        // And once away, old silence is no reason to think otherwise.
        assert_eq!(Reach::Away.after(Word::Silence { asked: start }), Reach::Away);
    }

    #[test]
    fn a_refusal_is_an_answer_and_an_error_of_our_own_is_no_word() {
        let (asked, now) = (Instant::now(), at(Instant::now(), 5));
        let word = |error: Option<plex::Error>| Word::of(error.as_ref(), asked, now);
        assert_eq!(word(None), Some(Word::Answer { heard: now }));
        // The server said the item is not there, so the server is.
        let missing = plex::Error::Missing(RatingKey::new("1"));
        assert_eq!(word(Some(missing)), Some(Word::Answer { heard: now }));
        let unreachable = plex::Error::Unreachable { server: "Attic".into(), tried: Vec::new() };
        assert_eq!(word(Some(unreachable)), Some(Word::Silence { asked }));
        assert_eq!(word(Some(plex::Error::NotRemembered)), None);
        assert_eq!(word(Some(plex::Error::Cancelled)), None);
    }
}
