//! The clock that frame times are measured on.

use std::time::Duration;

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

/// The processor time this process has used so far, on all its threads.
pub fn cpu_time() -> Duration {
    // SAFETY: an all-zero `rusage` is a valid value for the call to
    // overwrite, and the pointer is valid for the call.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage
    };
    let of = |time: libc::timeval| {
        Duration::from_secs(time.tv_sec.cast_unsigned())
            + Duration::from_micros(u64::from(time.tv_usec.cast_unsigned()))
    };
    of(usage.ru_utime) + of(usage.ru_stime)
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
