//! # dev-chaos
//!
//! Failure injection and recovery testing for Rust. Part of the
//! `dev-*` verification suite.
//!
//! Most code is tested only on the happy path. Real systems fail
//! through partial writes, crashes, timeouts, corrupt data, and
//! broken connections. `dev-chaos` provides primitives for injecting
//! those failures on purpose, then verifying that recovery logic does
//! its job.
//!
//! ## Quick example
//!
//! ```no_run
//! use dev_chaos::{FailureSchedule, FailureMode};
//!
//! // Fail on the 3rd, 7th, and 10th attempt.
//! let schedule = FailureSchedule::on_attempts(&[3, 7, 10], FailureMode::IoError);
//!
//! for attempt in 1..=12 {
//!     match schedule.maybe_fail(attempt) {
//!         Ok(()) => { /* operation proceeds */ }
//!         Err(_e) => { /* recovery path */ }
//!     }
//! }
//! ```
//!
//! ## Modules
//!
//! - [`io`] — sync IO wrappers (`ChaosReader`, `ChaosWriter`, `ChaosFile`).
//! - [`latency`] — non-failing slowdowns via `LatencyInjector`,
//!   composable with `FailureSchedule` via `LatencyAndFailure`.
//! - [`crash`] — write-truncation via `CrashPoint`.
//! - [`clock`] — deterministic `Clock` for time-skew injection.
//! - [`memory_pressure`] — `MemoryPressure` guards for memory-bound chaos.
//! - `async_io` (feature `async-io`) — `tokio::io` equivalents
//!   (visible in rustdoc when the feature is enabled).
//!
//! ## Determinism
//!
//! All schedules are deterministic by default: the same sequence of
//! attempts MUST produce the same sequence of failures across runs
//! and machines. Probabilistic schedules
//! ([`FailureSchedule::seeded_random`]) are opt-in, seeded, and
//! reproducible from the seed.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![warn(rust_2018_idioms)]

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use dev_report::{CheckResult, Evidence, Producer, Report, Severity};

pub mod clock;
pub mod crash;
pub mod io;
pub mod latency;
pub mod memory_pressure;

#[cfg(feature = "async-io")]
#[cfg_attr(docsrs, doc(cfg(feature = "async-io")))]
pub mod async_io;

/// A type of failure that can be injected.
///
/// # Example
///
/// ```
/// use dev_chaos::FailureMode;
/// assert_eq!(FailureMode::IoError.as_str(), "io_error");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureMode {
    /// Generic I/O error.
    IoError,
    /// Partial write: returns an error after writing some bytes.
    PartialWrite,
    /// Connection reset.
    ConnectionReset,
    /// Operation timeout.
    Timeout,
    /// Corrupted data: returns success but with corrupted bytes.
    Corruption,
    /// Permission denied.
    PermissionDenied,
}

impl FailureMode {
    /// Human-readable name for this failure mode.
    pub fn as_str(&self) -> &'static str {
        match self {
            FailureMode::IoError => "io_error",
            FailureMode::PartialWrite => "partial_write",
            FailureMode::ConnectionReset => "connection_reset",
            FailureMode::Timeout => "timeout",
            FailureMode::Corruption => "corruption",
            FailureMode::PermissionDenied => "permission_denied",
        }
    }

    /// Map this mode to an `std::io::ErrorKind`.
    pub fn to_io_kind(&self) -> std::io::ErrorKind {
        match self {
            FailureMode::IoError => std::io::ErrorKind::Other,
            FailureMode::PartialWrite => std::io::ErrorKind::WriteZero,
            FailureMode::ConnectionReset => std::io::ErrorKind::ConnectionReset,
            FailureMode::Timeout => std::io::ErrorKind::TimedOut,
            FailureMode::Corruption => std::io::ErrorKind::InvalidData,
            FailureMode::PermissionDenied => std::io::ErrorKind::PermissionDenied,
        }
    }
}

/// An error returned by injected failures.
///
/// # Example
///
/// ```
/// use dev_chaos::{FailureMode, InjectedFailure};
/// let f = InjectedFailure { mode: FailureMode::Timeout, attempt: 3 };
/// assert_eq!(f.mode.as_str(), "timeout");
/// ```
#[derive(Debug, Clone)]
pub struct InjectedFailure {
    /// The mode of failure that was injected.
    pub mode: FailureMode,
    /// The attempt number at which the failure was injected.
    pub attempt: usize,
}

impl std::fmt::Display for InjectedFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "injected failure {} at attempt {}",
            self.mode.as_str(),
            self.attempt
        )
    }
}

impl std::error::Error for InjectedFailure {}

impl From<InjectedFailure> for std::io::Error {
    fn from(f: InjectedFailure) -> Self {
        std::io::Error::new(f.mode.to_io_kind(), f.to_string())
    }
}

/// A schedule that decides whether a given attempt fails.
///
/// Schedules are deterministic by default. The same `(schedule, attempt)`
/// pair produces the same outcome across runs and machines.
///
/// # Example
///
/// ```
/// use dev_chaos::{FailureMode, FailureSchedule};
///
/// let s = FailureSchedule::on_attempts(&[2, 4], FailureMode::IoError);
/// assert!(s.maybe_fail(1).is_ok());
/// assert!(s.maybe_fail(2).is_err());
/// ```
pub struct FailureSchedule {
    inner: ScheduleKind,
    mode: FailureMode,
    invocations: AtomicUsize,
    failures: AtomicUsize,
    /// `None` = unbounded; `Some(n)` = stop firing after `n` failures.
    failure_limit: Option<usize>,
}

enum ScheduleKind {
    Explicit(HashSet<usize>),
    EveryN(usize),
    SeededRandom { seed: u64, prob_thousandths: u32 },
}

impl FailureSchedule {
    /// Build a schedule that fails on specific attempt numbers
    /// (1-indexed).
    ///
    /// # Example
    ///
    /// ```
    /// use dev_chaos::{FailureMode, FailureSchedule};
    /// let s = FailureSchedule::on_attempts(&[3, 7], FailureMode::Timeout);
    /// assert!(s.maybe_fail(3).is_err());
    /// assert!(s.maybe_fail(4).is_ok());
    /// ```
    pub fn on_attempts(attempts: &[usize], mode: FailureMode) -> Self {
        Self {
            inner: ScheduleKind::Explicit(attempts.iter().copied().collect()),
            mode,
            invocations: AtomicUsize::new(0),
            failures: AtomicUsize::new(0),
            failure_limit: None,
        }
    }

    /// Build a schedule that fails on every Nth attempt (1-indexed).
    ///
    /// # Example
    ///
    /// ```
    /// use dev_chaos::{FailureMode, FailureSchedule};
    /// let s = FailureSchedule::every_n(3, FailureMode::Timeout);
    /// assert!(s.maybe_fail(3).is_err());
    /// assert!(s.maybe_fail(6).is_err());
    /// ```
    pub fn every_n(n: usize, mode: FailureMode) -> Self {
        let n = n.max(1);
        Self {
            inner: ScheduleKind::EveryN(n),
            mode,
            invocations: AtomicUsize::new(0),
            failures: AtomicUsize::new(0),
            failure_limit: None,
        }
    }

    /// Build a deterministic, seeded "random" schedule.
    ///
    /// Each attempt is hashed (with the seed) into a value in `[0, 1000)`
    /// and fails when that value is below `probability * 1000`. The
    /// schedule is fully reproducible from the seed.
    ///
    /// `probability` is clamped to `[0.0, 1.0]`.
    ///
    /// **This is the only non-explicit schedule.** Even so, it is
    /// strictly reproducible; no real RNG state, no clock, no thread.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_chaos::{FailureMode, FailureSchedule};
    ///
    /// let a = FailureSchedule::seeded_random(42, 0.10, FailureMode::IoError);
    /// let b = FailureSchedule::seeded_random(42, 0.10, FailureMode::IoError);
    /// // Same seed => same outcome at every attempt.
    /// for attempt in 1..=100 {
    ///     assert_eq!(a.maybe_fail(attempt).is_err(), b.maybe_fail(attempt).is_err());
    /// }
    /// ```
    pub fn seeded_random(seed: u64, probability: f64, mode: FailureMode) -> Self {
        let p = probability.clamp(0.0, 1.0);
        let prob_thousandths = (p * 1000.0).round() as u32;
        Self {
            inner: ScheduleKind::SeededRandom {
                seed,
                prob_thousandths,
            },
            mode,
            invocations: AtomicUsize::new(0),
            failures: AtomicUsize::new(0),
            failure_limit: None,
        }
    }

    /// Cap the total number of failures this schedule will emit.
    ///
    /// After `n` failures have been emitted via [`maybe_fail`], the
    /// schedule stops firing — every subsequent call returns `Ok(())`,
    /// regardless of attempt number.
    ///
    /// Useful for bounded chaos: you want a few failures to verify
    /// recovery, not an indefinite stream.
    ///
    /// # Example
    ///
    /// ```
    /// use dev_chaos::{FailureMode, FailureSchedule};
    ///
    /// // Fail every attempt, but stop after 3.
    /// let s = FailureSchedule::every_n(1, FailureMode::IoError).limit(3);
    /// let mut failures = 0;
    /// for attempt in 1..=20 {
    ///     if s.maybe_fail(attempt).is_err() {
    ///         failures += 1;
    ///     }
    /// }
    /// assert_eq!(failures, 3);
    /// ```
    ///
    /// [`maybe_fail`]: Self::maybe_fail
    pub fn limit(mut self, n: usize) -> Self {
        self.failure_limit = Some(n);
        self
    }

    /// Check whether the given attempt should fail.
    ///
    /// Returns `Ok(())` if the operation should proceed, or
    /// `Err(InjectedFailure)` if the schedule fires on this attempt.
    ///
    /// If a [`limit`](Self::limit) has been applied and the failure
    /// count has reached it, this returns `Ok(())` regardless of
    /// whether the schedule would otherwise fire.
    pub fn maybe_fail(&self, attempt: usize) -> Result<(), InjectedFailure> {
        self.invocations.fetch_add(1, Ordering::Relaxed);
        if !self.fires(attempt) {
            return Ok(());
        }
        if let Some(limit) = self.failure_limit {
            // fetch_update would be cleanest, but a fetch_add + check
            // is sufficient: we accept that under contention we may
            // emit at most `limit + (concurrency - 1)` failures, which
            // is documented and acceptable for a fixture.
            let prior = self.failures.fetch_add(1, Ordering::Relaxed);
            if prior >= limit {
                return Ok(());
            }
        } else {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
        Err(InjectedFailure {
            mode: self.mode,
            attempt,
        })
    }

    /// Total failures emitted by this schedule so far.
    pub fn failure_count(&self) -> usize {
        // When a limit is in effect, internal counter may exceed
        // the limit by one due to fetch_add ordering; clamp on read.
        let raw = self.failures.load(Ordering::Relaxed);
        match self.failure_limit {
            Some(limit) => raw.min(limit),
            None => raw,
        }
    }

    fn fires(&self, attempt: usize) -> bool {
        match &self.inner {
            ScheduleKind::Explicit(set) => set.contains(&attempt),
            ScheduleKind::EveryN(n) => attempt % *n == 0,
            ScheduleKind::SeededRandom {
                seed,
                prob_thousandths,
            } => {
                // Deterministic mix: combine attempt + seed via splitmix64.
                let mut x =
                    (*seed).wrapping_add((attempt as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                x ^= x >> 31;
                let bucket = (x % 1000) as u32;
                bucket < *prob_thousandths
            }
        }
    }

    /// Total invocations of `maybe_fail` since this schedule was built.
    pub fn invocation_count(&self) -> usize {
        self.invocations.load(Ordering::Relaxed)
    }

    /// Mode this schedule injects.
    pub fn mode(&self) -> FailureMode {
        self.mode
    }
}

/// Verify that recovery logic succeeded after a failure schedule.
///
/// Returns a [`CheckResult`] tagged `chaos`. The verdict follows REPS
/// section 4:
///
/// - `final_state_ok = false` -> `Fail (Critical)`, `regression` tag.
/// - `actual_failures < expected_failures` AND `final_state_ok` ->
///   `Warn (Warning)`, indicating under-injection.
/// - Otherwise -> `Pass`.
///
/// Always carries numeric `Evidence` for `expected_failures`,
/// `actual_failures`, `final_state_ok`.
///
/// # Example
///
/// ```
/// use dev_chaos::assert_recovered;
/// let c = assert_recovered("write_log", 2, 2, true);
/// assert!(matches!(c.verdict, dev_report::Verdict::Pass));
/// ```
pub fn assert_recovered(
    name: impl Into<String>,
    expected_failures: usize,
    actual_failures: usize,
    final_state_ok: bool,
) -> CheckResult {
    let check_name = format!("chaos::{}", name.into());
    let evidence = vec![
        Evidence::numeric("expected_failures", expected_failures as f64),
        Evidence::numeric("actual_failures", actual_failures as f64),
        Evidence::numeric("final_state_ok", if final_state_ok { 1.0 } else { 0.0 }),
    ];

    if !final_state_ok {
        let mut tags = vec![
            "chaos".to_string(),
            "recovery".to_string(),
            "regression".to_string(),
        ];
        tags.sort();
        let mut c = CheckResult::fail(check_name, Severity::Critical).with_detail(format!(
            "system did not recover. expected {expected_failures} injected failures, observed {actual_failures}, final state failed validation"
        ));
        c.tags = tags;
        c.evidence = evidence;
        return c;
    }

    if actual_failures < expected_failures {
        let mut tags = vec!["chaos".to_string(), "recovery".to_string()];
        tags.sort();
        let mut c = CheckResult::warn(check_name, Severity::Warning).with_detail(format!(
            "fewer failures observed than scheduled (expected {expected_failures}, observed {actual_failures})"
        ));
        c.tags = tags;
        c.evidence = evidence;
        return c;
    }

    let mut tags = vec!["chaos".to_string(), "recovery".to_string()];
    tags.sort();
    let mut c = CheckResult::pass(check_name).with_detail(format!(
        "recovered after {actual_failures} injected failure(s)"
    ));
    c.tags = tags;
    c.evidence = evidence;
    c
}

/// Producer wrapper that runs a chaos suite and emits a Report with
/// each scenario's `CheckResult`.
///
/// # Example
///
/// ```no_run
/// use dev_chaos::{assert_recovered, ChaosProducer};
/// use dev_report::Producer;
///
/// fn run() -> Vec<dev_report::CheckResult> {
///     vec![
///         assert_recovered("write_log", 2, 2, true),
///         assert_recovered("rename", 1, 1, true),
///     ]
/// }
///
/// let producer = ChaosProducer::new(run, "my-crate", "0.1.0");
/// let report = producer.produce();
/// assert_eq!(report.checks.len(), 2);
/// ```
pub struct ChaosProducer<F>
where
    F: Fn() -> Vec<CheckResult>,
{
    run: F,
    subject: String,
    subject_version: String,
}

impl<F> ChaosProducer<F>
where
    F: Fn() -> Vec<CheckResult>,
{
    /// Build a new producer.
    pub fn new(run: F, subject: impl Into<String>, subject_version: impl Into<String>) -> Self {
        Self {
            run,
            subject: subject.into(),
            subject_version: subject_version.into(),
        }
    }
}

impl<F> Producer for ChaosProducer<F>
where
    F: Fn() -> Vec<CheckResult>,
{
    fn produce(&self) -> Report {
        let checks = (self.run)();
        let mut r = Report::new(self.subject.clone(), self.subject_version.clone())
            .with_producer("dev-chaos");
        for c in checks {
            r.push(c);
        }
        r.finish();
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dev_report::Verdict;

    #[test]
    fn schedule_fails_on_specified_attempts() {
        let s = FailureSchedule::on_attempts(&[2, 4], FailureMode::IoError);
        assert!(s.maybe_fail(1).is_ok());
        assert!(s.maybe_fail(2).is_err());
        assert!(s.maybe_fail(3).is_ok());
        assert!(s.maybe_fail(4).is_err());
        assert_eq!(s.invocation_count(), 4);
    }

    #[test]
    fn every_n_fires_on_multiples() {
        let s = FailureSchedule::every_n(3, FailureMode::Timeout);
        assert!(s.maybe_fail(1).is_ok());
        assert!(s.maybe_fail(2).is_ok());
        assert!(s.maybe_fail(3).is_err());
        assert!(s.maybe_fail(6).is_err());
        assert!(s.maybe_fail(9).is_err());
        // Beyond 1024-now-arbitrary because we use modulo.
        assert!(s.maybe_fail(3_000).is_err());
    }

    #[test]
    fn limit_caps_total_failures() {
        let s = FailureSchedule::every_n(1, FailureMode::IoError).limit(3);
        let mut failures = 0;
        for attempt in 1..=20 {
            if s.maybe_fail(attempt).is_err() {
                failures += 1;
            }
        }
        assert_eq!(failures, 3);
        assert_eq!(s.failure_count(), 3);
    }

    #[test]
    fn limit_zero_disables_failures() {
        let s = FailureSchedule::every_n(1, FailureMode::IoError).limit(0);
        for attempt in 1..=10 {
            assert!(s.maybe_fail(attempt).is_ok());
        }
        assert_eq!(s.failure_count(), 0);
    }

    #[test]
    fn unlimited_schedule_still_increments_failure_count() {
        let s = FailureSchedule::every_n(1, FailureMode::IoError);
        for attempt in 1..=5 {
            let _ = s.maybe_fail(attempt);
        }
        assert_eq!(s.failure_count(), 5);
    }

    #[test]
    fn limit_works_with_seeded_random() {
        let s = FailureSchedule::seeded_random(42, 1.0, FailureMode::IoError).limit(2);
        let mut failures = 0;
        for attempt in 1..=20 {
            if s.maybe_fail(attempt).is_err() {
                failures += 1;
            }
        }
        assert_eq!(failures, 2);
    }

    #[test]
    fn seeded_random_is_deterministic() {
        let a = FailureSchedule::seeded_random(7, 0.5, FailureMode::IoError);
        let b = FailureSchedule::seeded_random(7, 0.5, FailureMode::IoError);
        for attempt in 1..=200 {
            assert_eq!(
                a.fires(attempt),
                b.fires(attempt),
                "differs at attempt {}",
                attempt
            );
        }
    }

    #[test]
    fn seeded_random_zero_probability_never_fires() {
        let s = FailureSchedule::seeded_random(7, 0.0, FailureMode::IoError);
        for attempt in 1..=1000 {
            assert!(s.maybe_fail(attempt).is_ok());
        }
    }

    #[test]
    fn seeded_random_full_probability_always_fires() {
        let s = FailureSchedule::seeded_random(7, 1.0, FailureMode::IoError);
        for attempt in 1..=200 {
            assert!(s.maybe_fail(attempt).is_err());
        }
    }

    #[test]
    fn injected_failure_converts_to_io_error() {
        let f = InjectedFailure {
            mode: FailureMode::Timeout,
            attempt: 5,
        };
        let e: std::io::Error = f.into();
        assert_eq!(e.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn recovery_check_pass() {
        let c = assert_recovered("write_log", 2, 2, true);
        assert_eq!(c.verdict, Verdict::Pass);
        assert!(c.has_tag("chaos"));
        assert!(c.has_tag("recovery"));
        assert!(!c.has_tag("regression"));
    }

    #[test]
    fn recovery_check_fail_when_state_invalid() {
        let c = assert_recovered("write_log", 2, 2, false);
        assert_eq!(c.verdict, Verdict::Fail);
        assert_eq!(c.severity, Some(Severity::Critical));
        assert!(c.has_tag("regression"));
    }

    #[test]
    fn recovery_check_warns_on_under_injection() {
        let c = assert_recovered("write_log", 5, 2, true);
        assert_eq!(c.verdict, Verdict::Warn);
    }

    #[test]
    fn recovery_check_carries_numeric_evidence() {
        let c = assert_recovered("op", 3, 3, true);
        let labels: Vec<&str> = c.evidence.iter().map(|e| e.label.as_str()).collect();
        assert!(labels.contains(&"expected_failures"));
        assert!(labels.contains(&"actual_failures"));
        assert!(labels.contains(&"final_state_ok"));
    }

    #[test]
    fn chaos_producer_emits_report() {
        let producer = ChaosProducer::new(
            || {
                vec![
                    assert_recovered("a", 1, 1, true),
                    assert_recovered("b", 2, 2, true),
                ]
            },
            "my-crate",
            "0.1.0",
        );
        let report = producer.produce();
        assert_eq!(report.checks.len(), 2);
        assert_eq!(report.producer.as_deref(), Some("dev-chaos"));
        assert_eq!(report.overall_verdict(), Verdict::Pass);
    }
}
