//! Deterministic time-skew injection for testing time-sensitive code.
//!
//! Code that retries on timeout, expires sessions, schedules futures,
//! or compares timestamps depends on a clock. [`Clock`] is a source
//! of `Instant`-ish values that the caller controls explicitly: you
//! advance it with [`Clock::advance`] or skew it with
//! [`Clock::skew_by`] to validate that retry loops, expiry checks,
//! and TTL logic behave correctly without `std::thread::sleep`.
//!
//! ## Determinism
//!
//! `Clock` is fully deterministic: the same sequence of `advance`
//! and `now` calls produces the same sequence of values across runs
//! and machines. No system calls, no thread sleeps.
//!
//! ## Pairing with `std::time::Instant`
//!
//! `Instant` is opaque and cannot be constructed directly, so this
//! module uses an offset-from-anchor model. `Clock::now` returns a
//! [`ClockTime`] (just a `Duration` from anchor); the caller adapts
//! this to their domain. For callers that need an `Instant`, see
//! [`Clock::anchor`] and add the offset.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A virtual time value measured as a `Duration` since the clock's anchor.
///
/// Internally just a wrapper around `Duration`; cheap to copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClockTime(pub Duration);

impl ClockTime {
    /// The duration since the clock's anchor.
    pub fn since_anchor(&self) -> Duration {
        self.0
    }
}

/// Deterministic, in-process clock for chaos testing.
///
/// `Clock` does **not** advance automatically. Callers explicitly
/// move the clock forward via [`advance`](Clock::advance) and skew
/// it via [`skew_by`](Clock::skew_by). This makes time-sensitive
/// tests fully reproducible.
///
/// `Clock` is `Clone`-able and shares state with its clones via an
/// internal `Arc<AtomicI64>` of nanoseconds-since-anchor; advancing
/// or skewing one handle advances all. Safe to share across threads.
///
/// # Example
///
/// ```
/// use dev_chaos::clock::Clock;
/// use std::time::Duration;
///
/// let c = Clock::new();
/// let t0 = c.now();
/// c.advance(Duration::from_secs(5));
/// let t1 = c.now();
/// assert_eq!(t1.since_anchor() - t0.since_anchor(), Duration::from_secs(5));
///
/// // Skew negative to simulate clock going backward (e.g. NTP step).
/// c.skew_by(-(Duration::from_secs(2).as_nanos() as i64));
/// let t2 = c.now();
/// assert!(t2 < t1);
/// ```
#[derive(Debug, Clone)]
pub struct Clock {
    /// Real wall-clock anchor when this clock was created. Used by
    /// `anchor()` for callers that need to map back to `Instant`.
    anchor: Instant,
    /// Offset from the anchor in nanoseconds. Wrapped in `Arc` so
    /// clones share state.
    offset_ns: Arc<AtomicI64>,
}

impl Clock {
    /// Build a new clock anchored at the current `Instant`.
    pub fn new() -> Self {
        Self {
            anchor: Instant::now(),
            offset_ns: Arc::new(AtomicI64::new(0)),
        }
    }

    /// The real `Instant` this clock was anchored at.
    ///
    /// Callers that need to interoperate with code expecting `Instant`
    /// can compute `clock.anchor() + clock.now().since_anchor()`.
    pub fn anchor(&self) -> Instant {
        self.anchor
    }

    /// Current virtual time, as offset from the anchor.
    pub fn now(&self) -> ClockTime {
        let ns = self.offset_ns.load(Ordering::Relaxed);
        if ns >= 0 {
            ClockTime(Duration::from_nanos(ns as u64))
        } else {
            // Negative offset: clamp to zero. Tests that observe a
            // negative offset should use `now_signed` for the raw value.
            ClockTime(Duration::ZERO)
        }
    }

    /// Current virtual offset, signed (in nanoseconds from anchor).
    ///
    /// Useful when validating skew-backward scenarios where
    /// `since_anchor()` would clamp.
    pub fn now_signed_ns(&self) -> i64 {
        self.offset_ns.load(Ordering::Relaxed)
    }

    /// Advance the clock by a non-negative `delta`.
    ///
    /// Equivalent to `skew_by(delta.as_nanos() as i64)` but rejects
    /// negative durations at the type level.
    pub fn advance(&self, delta: Duration) {
        self.offset_ns
            .fetch_add(delta.as_nanos() as i64, Ordering::Relaxed);
    }

    /// Skew the clock by `delta_ns`, which may be negative.
    ///
    /// Negative skew simulates clock-step events (e.g. NTP correction,
    /// VM pause/resume backward jump). Tests can validate that retry
    /// loops with timeout-based escape don't get stuck on negative
    /// elapsed values.
    pub fn skew_by(&self, delta_ns: i64) {
        self.offset_ns.fetch_add(delta_ns, Ordering::Relaxed);
    }

    /// Reset the clock to anchor (offset zero).
    pub fn reset(&self) {
        self.offset_ns.store(0, Ordering::Relaxed);
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_clock_starts_at_zero_offset() {
        let c = Clock::new();
        assert_eq!(c.now().since_anchor(), Duration::ZERO);
        assert_eq!(c.now_signed_ns(), 0);
    }

    #[test]
    fn advance_moves_forward() {
        let c = Clock::new();
        c.advance(Duration::from_millis(100));
        c.advance(Duration::from_millis(50));
        assert_eq!(c.now().since_anchor(), Duration::from_millis(150));
    }

    #[test]
    fn skew_negative_clamps_now_to_zero() {
        let c = Clock::new();
        c.advance(Duration::from_millis(100));
        c.skew_by(-(Duration::from_millis(200).as_nanos() as i64));
        // After negative skew past zero, `now` clamps to zero.
        assert_eq!(c.now().since_anchor(), Duration::ZERO);
        // But the raw signed offset reflects the underlying value.
        assert!(c.now_signed_ns() < 0);
    }

    #[test]
    fn skew_negative_within_positive_offset_works() {
        let c = Clock::new();
        c.advance(Duration::from_millis(500));
        c.skew_by(-(Duration::from_millis(100).as_nanos() as i64));
        assert_eq!(c.now().since_anchor(), Duration::from_millis(400));
    }

    #[test]
    fn cloned_clocks_share_state() {
        let c = Clock::new();
        let d = c.clone();
        c.advance(Duration::from_secs(1));
        assert_eq!(d.now().since_anchor(), Duration::from_secs(1));
    }

    #[test]
    fn reset_returns_to_zero() {
        let c = Clock::new();
        c.advance(Duration::from_secs(10));
        c.reset();
        assert_eq!(c.now().since_anchor(), Duration::ZERO);
    }

    #[test]
    fn anchor_is_preserved_across_clones() {
        let c = Clock::new();
        let d = c.clone();
        assert_eq!(c.anchor(), d.anchor());
    }

    #[test]
    fn deterministic_sequence_across_runs() {
        // Same operations should produce same observable values, no
        // matter how often we run.
        let c = Clock::new();
        c.advance(Duration::from_millis(100));
        c.advance(Duration::from_millis(50));
        c.skew_by(-(Duration::from_millis(20).as_nanos() as i64));
        let observed = c.now().since_anchor();
        // Run again with a fresh clock.
        let c2 = Clock::new();
        c2.advance(Duration::from_millis(100));
        c2.advance(Duration::from_millis(50));
        c2.skew_by(-(Duration::from_millis(20).as_nanos() as i64));
        let observed2 = c2.now().since_anchor();
        assert_eq!(observed, observed2);
    }
}
