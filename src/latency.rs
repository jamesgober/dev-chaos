//! Latency injection: simulate slow-but-not-failing operations.
//!
//! `LatencyInjector` produces a deterministic delay per attempt
//! according to a [`LatencyProfile`]. It composes with
//! [`FailureSchedule`](crate::FailureSchedule): inject latency on
//! every call, inject failures on a subset.

use std::time::Duration;

/// Per-attempt latency profile.
///
/// All variants are deterministic.
#[derive(Debug, Clone)]
pub enum LatencyProfile {
    /// Constant delay on every attempt.
    Constant(Duration),
    /// Linear ramp: `start + (attempt - 1) * step`.
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
                let n = attempt.saturating_sub(1) as u32;
                *start + step.saturating_mul(n)
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
