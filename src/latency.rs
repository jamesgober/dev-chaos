//! Latency injection: simulate slow-but-not-failing operations.
//!
//! `LatencyInjector` produces a deterministic delay per attempt
//! according to a [`LatencyProfile`]. It composes with
//! `FailureSchedule` (see [`LatencyInjector::compose_with_schedule`]):
//! inject latency on every call, inject failures on a subset.

use std::time::Duration;

/// Per-attempt latency profile.
///
/// All variants are deterministic.
#[derive(Debug, Clone)]
pub enum LatencyProfile {
    /// Constant delay on every attempt.
    Constant(Duration),
    /// Linear ramp: `start + (attempt - 1) * step`, saturating at
    /// `Duration::MAX` instead of overflowing.
    LinearRamp {
        /// Delay applied to attempt 1.
        start: Duration,
        /// Delay added to each subsequent attempt.
        step: Duration,
    },
    /// Step function: piecewise-constant by `boundaries`. Each entry
    /// `(attempt_threshold, delay)` means "use `delay` while attempt
    /// is `<= attempt_threshold`". The list MUST be sorted ascending
    /// by `attempt_threshold`. Attempts beyond the last threshold use
    /// the final entry's `delay`.
    StepSchedule(Vec<(usize, Duration)>),
}

/// `start + n * step` in exact u128 nanoseconds, clamped to
/// `Duration::MAX`. Only reached for very large attempts or delays, so
/// it is kept out of line.
#[cold]
#[inline(never)]
fn linear_ramp_wide(start: Duration, step: Duration, n: usize) -> Duration {
    let ns = step
        .as_nanos()
        .saturating_mul(n as u128)
        .saturating_add(start.as_nanos());
    duration_from_nanos_saturating(ns)
}

/// `Duration` from a nanosecond count, clamped to `Duration::MAX`.
fn duration_from_nanos_saturating(ns: u128) -> Duration {
    const NANOS_PER_SEC: u128 = 1_000_000_000;
    match u64::try_from(ns / NANOS_PER_SEC) {
        Ok(secs) => Duration::new(secs, (ns % NANOS_PER_SEC) as u32),
        Err(_) => Duration::MAX,
    }
}

/// Computes per-attempt delays from a [`LatencyProfile`].
///
/// `LatencyInjector` is intentionally side-effect-free: it returns
/// the delay it *would* sleep, leaving the actual `thread::sleep`
/// (or `tokio::time::sleep`) up to the caller. This keeps the type
/// usable in both sync and async contexts.
///
/// # Example
///
/// ```
/// use dev_chaos::latency::{LatencyInjector, LatencyProfile};
/// use std::time::Duration;
///
/// let inj = LatencyInjector::new(LatencyProfile::Constant(Duration::from_millis(5)));
/// assert_eq!(inj.delay_for(1), Duration::from_millis(5));
/// assert_eq!(inj.delay_for(100), Duration::from_millis(5));
/// ```
pub struct LatencyInjector {
    profile: LatencyProfile,
}

impl LatencyInjector {
    /// Build an injector from a profile.
    pub fn new(profile: LatencyProfile) -> Self {
        Self { profile }
    }

    /// Compute the delay that would be applied at `attempt` (1-indexed).
    pub fn delay_for(&self, attempt: usize) -> Duration {
        match &self.profile {
            LatencyProfile::Constant(d) => *d,
            LatencyProfile::LinearRamp { start, step } => {
                // The old `as u32` cast wrapped attempts above u32::MAX
                // back to small delays, and `start + ...` panicked on
                // overflow.
                let n = attempt.saturating_sub(1);
                // Fast path: plain Duration math when nothing overflows.
                match u32::try_from(n)
                    .ok()
                    .and_then(|n| step.checked_mul(n))
                    .and_then(|d| start.checked_add(d))
                {
                    Some(d) => d,
                    None => linear_ramp_wide(*start, *step, n),
                }
            }
            LatencyProfile::StepSchedule(boundaries) => {
                if boundaries.is_empty() {
                    return Duration::ZERO;
                }
                for (threshold, delay) in boundaries.iter() {
                    if attempt <= *threshold {
                        return *delay;
                    }
                }
                boundaries.last().unwrap().1
            }
        }
    }

    /// Apply the delay synchronously by sleeping the calling thread.
    ///
    /// Equivalent to `std::thread::sleep(self.delay_for(attempt))`.
    pub fn apply_blocking(&self, attempt: usize) {
        std::thread::sleep(self.delay_for(attempt));
    }

    /// Bind this injector to a [`FailureSchedule`](crate::FailureSchedule)
    /// so a single call applies latency *and* checks for failure injection.
    ///
    /// Returns a [`LatencyAndFailure`] composer:
    /// - On every attempt, the latency is applied (sync sleep).
    /// - On scheduled-failure attempts, the call returns
    ///   `Err(InjectedFailure)` after the latency has been applied
    ///   (so the test observes both the slowdown *and* the failure).
    ///
    /// # Example
    ///
    /// ```
    /// use dev_chaos::{
    ///     latency::{LatencyInjector, LatencyProfile},
    ///     FailureMode, FailureSchedule,
    /// };
    /// use std::time::Duration;
    ///
    /// let inj = LatencyInjector::new(LatencyProfile::Constant(Duration::ZERO));
    /// let schedule = FailureSchedule::on_attempts(&[2], FailureMode::Timeout);
    /// let composed = inj.compose_with_schedule(schedule);
    ///
    /// assert!(composed.apply_blocking(1).is_ok());
    /// assert!(composed.apply_blocking(2).is_err());
    /// ```
    pub fn compose_with_schedule(self, schedule: crate::FailureSchedule) -> LatencyAndFailure {
        LatencyAndFailure {
            injector: self,
            schedule,
        }
    }
}

/// A latency injector composed with a [`FailureSchedule`](crate::FailureSchedule).
///
/// Built via [`LatencyInjector::compose_with_schedule`]. Each
/// `apply_blocking` call sleeps for the latency profile's delay and
/// then either returns `Ok(())` or `Err(InjectedFailure)` based on
/// the schedule.
pub struct LatencyAndFailure {
    injector: LatencyInjector,
    schedule: crate::FailureSchedule,
}

impl LatencyAndFailure {
    /// Apply the latency for `attempt`, then check the schedule.
    ///
    /// Returns `Ok(())` if the schedule does not fire, or
    /// `Err(InjectedFailure)` if it does. The latency is applied in
    /// either case — the failure is appended *after* the slowdown.
    pub fn apply_blocking(&self, attempt: usize) -> Result<(), crate::InjectedFailure> {
        self.injector.apply_blocking(attempt);
        self.schedule.maybe_fail(attempt)
    }

    /// Compute the delay for `attempt` without sleeping or consuming
    /// schedule attempts. Useful for diagnostics.
    pub fn delay_for(&self, attempt: usize) -> Duration {
        self.injector.delay_for(attempt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_profile_returns_same_duration() {
        let inj = LatencyInjector::new(LatencyProfile::Constant(Duration::from_micros(50)));
        for attempt in 1..=10 {
            assert_eq!(inj.delay_for(attempt), Duration::from_micros(50));
        }
    }

    #[test]
    fn linear_ramp_increases() {
        let inj = LatencyInjector::new(LatencyProfile::LinearRamp {
            start: Duration::from_micros(10),
            step: Duration::from_micros(5),
        });
        assert_eq!(inj.delay_for(1), Duration::from_micros(10));
        assert_eq!(inj.delay_for(2), Duration::from_micros(15));
        assert_eq!(inj.delay_for(5), Duration::from_micros(30));
    }

    #[test]
    fn linear_ramp_saturates_instead_of_panicking() {
        // start + step * n used to panic with "overflow when adding
        // durations" once the product came close to Duration::MAX.
        let inj = LatencyInjector::new(LatencyProfile::LinearRamp {
            start: Duration::from_secs(1),
            step: Duration::MAX,
        });
        assert_eq!(inj.delay_for(1), Duration::from_secs(1));
        assert_eq!(inj.delay_for(2), Duration::MAX);
        let inj = LatencyInjector::new(LatencyProfile::LinearRamp {
            start: Duration::MAX,
            step: Duration::from_secs(1),
        });
        assert_eq!(inj.delay_for(3), Duration::MAX);
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn linear_ramp_stays_linear_past_u32_attempts() {
        // `(attempt - 1) as u32` wrapped at 2^32, so attempt 2^32 + 1
        // got the same delay as attempt 1.
        let inj = LatencyInjector::new(LatencyProfile::LinearRamp {
            start: Duration::from_nanos(7),
            step: Duration::from_nanos(1),
        });
        let attempt = (u32::MAX as u64 + 2) as usize;
        assert_eq!(
            inj.delay_for(attempt),
            Duration::from_nanos(7 + u32::MAX as u64 + 1)
        );
        assert_eq!(
            inj.delay_for(usize::MAX),
            Duration::from_nanos(u64::MAX - 1) + Duration::from_nanos(7)
        );
    }

    #[test]
    fn nanos_conversion_saturates() {
        assert_eq!(duration_from_nanos_saturating(0), Duration::ZERO);
        assert_eq!(
            duration_from_nanos_saturating(1_500_000_000),
            Duration::from_millis(1_500)
        );
        assert_eq!(
            duration_from_nanos_saturating(Duration::MAX.as_nanos()),
            Duration::MAX
        );
        assert_eq!(duration_from_nanos_saturating(u128::MAX), Duration::MAX);
    }

    #[test]
    fn step_schedule_picks_correct_band() {
        let inj = LatencyInjector::new(LatencyProfile::StepSchedule(vec![
            (10, Duration::from_micros(1)),
            (20, Duration::from_micros(5)),
            (50, Duration::from_micros(20)),
        ]));
        assert_eq!(inj.delay_for(1), Duration::from_micros(1));
        assert_eq!(inj.delay_for(10), Duration::from_micros(1));
        assert_eq!(inj.delay_for(11), Duration::from_micros(5));
        assert_eq!(inj.delay_for(20), Duration::from_micros(5));
        assert_eq!(inj.delay_for(21), Duration::from_micros(20));
        assert_eq!(inj.delay_for(100), Duration::from_micros(20));
    }

    #[test]
    fn empty_step_schedule_yields_zero() {
        let inj = LatencyInjector::new(LatencyProfile::StepSchedule(vec![]));
        assert_eq!(inj.delay_for(1), Duration::ZERO);
    }

    #[test]
    fn apply_blocking_sleeps_at_least_the_delay() {
        let inj = LatencyInjector::new(LatencyProfile::Constant(Duration::from_millis(10)));
        let start = std::time::Instant::now();
        inj.apply_blocking(1);
        assert!(start.elapsed() >= Duration::from_millis(10));
    }
}
