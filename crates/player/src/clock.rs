//! The clock that frame times are measured on.

/// A moment on the host clock: nanoseconds the machine has been awake
/// since boot. Core Video and Core Animation stamp display refreshes on
/// the same clock, and mpv's own clock is this one with a fixed offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct HostTime(u64);

impl HostTime {
    pub fn now() -> Self {
        // SAFETY: the call has no preconditions.
        Self::from_ticks(unsafe { mach_absolute_time() })
    }

    /// From a count of `mach_absolute_time` ticks, which is what a
    /// `CVTimeStamp` holds in `hostTime`.
    pub fn from_ticks(ticks: u64) -> Self {
        let mut ratio = Timebase { numerator: 0, denominator: 0 };
        // SAFETY: the pointer is valid for the call, which only writes the
        // two fields.
        unsafe { mach_timebase_info(&mut ratio) };
        let nanos =
            u128::from(ticks) * u128::from(ratio.numerator) / u128::from(ratio.denominator.max(1));
        Self(u64::try_from(nanos).unwrap_or(u64::MAX))
    }

    pub fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    pub fn nanos(self) -> u64 {
        self.0
    }
}

/// `mach_timebase_info_data_t`: one tick lasts `numerator / denominator`
/// nanoseconds.
#[repr(C)]
struct Timebase {
    numerator: u32,
    denominator: u32,
}

// From libSystem.
unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut Timebase) -> libc::c_int;
}
