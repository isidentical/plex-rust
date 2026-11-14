//! What went wrong, in words for the viewer.
//!
//! An error says what the code saw: a status, an address, a cause under a
//! cause. The viewer needs to know what that means for them and what to
//! do about it, so each error is first sorted into one of a few kinds.

use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Nothing answered: the server is off, asleep or on another network.
    Unanswered,
    /// Plex no longer accepts the stored sign-in.
    SignedOut,
    /// The server answered that what was asked for is not there.
    Missing,
    /// The account has no server to show.
    NoServer,
    /// Anything else. Only the error itself says more.
    Other,
}

impl Failure {
    pub fn of(error: &(dyn std::error::Error + 'static)) -> Self {
        match error.downcast_ref::<Error>() {
            Some(Error::Plex(error)) => Self::of_plex(error),
            Some(Error::NoServer) => Self::NoServer,
            Some(Error::SignedOut) => Self::SignedOut,
            Some(_) => Self::Other,
            None => error.downcast_ref::<plex::Error>().map_or(Self::Other, Self::of_plex),
        }
    }

    fn of_plex(error: &plex::Error) -> Self {
        if error.is_unanswered() {
            Self::Unanswered
        } else if error.is_unauthorized() {
            Self::SignedOut
        } else if error.is_missing() {
            Self::Missing
        } else {
            Self::Other
        }
    }

    /// The SF Symbol that stands for this kind of failure.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Unanswered => "wifi.exclamationmark",
            Self::SignedOut => "person.crop.circle.badge.exclamationmark",
            Self::Missing => "questionmark.folder",
            Self::NoServer => "server.rack",
            Self::Other => "exclamationmark.triangle",
        }
    }

    /// What happened. `None` when only the place it happened can say.
    pub fn title(self) -> Option<&'static str> {
        match self {
            Self::Unanswered => Some("Your server is not answering"),
            Self::SignedOut => Some("Your sign-in has expired"),
            Self::Missing => Some("This is no longer on your server"),
            Self::NoServer => Some("No server on this account"),
            Self::Other => None,
        }
    }

    /// What the viewer can do about it.
    pub fn advice(self) -> &'static str {
        match self {
            Self::Unanswered => {
                "Check that the server is switched on, and that this Mac is on the same network \
                 or VPN as it."
            }
            Self::SignedOut => "Plex no longer accepts it. Sign in again to carry on.",
            Self::Missing => "It may have been removed, or moved to another library.",
            Self::NoServer => {
                "Sign in with the Plex account that owns a Plex Media Server, or that one is \
                 shared with."
            }
            Self::Other => "Try again. If it keeps happening, the line below says what went wrong.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_errors_whether_or_not_the_app_wrapped_them() {
        let unreachable =
            || plex::Error::Unreachable { server: "Attic".to_owned(), tried: Vec::new() };
        assert_eq!(Failure::of(&unreachable()), Failure::Unanswered);
        assert_eq!(Failure::of(&Error::Plex(unreachable())), Failure::Unanswered);
        assert_eq!(Failure::of(&Error::NoServer), Failure::NoServer);
        assert_eq!(Failure::of(&Error::NoHome), Failure::Other);
        assert_eq!(Failure::of(&plex::Error::PinExpired), Failure::Other);
    }
}
