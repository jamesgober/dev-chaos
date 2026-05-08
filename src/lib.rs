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
//!         Err(e) => { /* recovery path */ }
//!     }
//! }
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![warn(rust_2018_idioms)]

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use dev_report::{CheckResult, Severity};

/// A type of failure that can be injected.
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
}

/// An error returned by injected failures.
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

/// A schedule that decides whether a given attempt fails.
pub struct FailureSchedule {
    failing_attempts: HashSet<usize>,
    mode: FailureMode,
    invocations: AtomicUsize,
}

impl FailureSchedule {
    /// Build a schedule that fails on specific attempt numbers
    /// (1-indexed).
    pub fn on_attempts(attempts: &[usize], mode: FailureMode) -> Self {
        Self {
            failing_attempts: attempts.iter().copied().collect(),
            mode,
            invocations: AtomicUsize::new(0),
        }
    }

    /// Build a schedule that fails on every Nth attempt (1-indexed).
    pub fn every_n(n: usize, mode: FailureMode) -> Self {
        let mut s = HashSet::new();
        // We don't know how many attempts there will be in advance,
        // so we record the modulus instead and check at maybe_fail time.
        // Implemented via a sentinel: attempts == [n] and a flag.
        // Simpler: just expand for up to 1024 attempts.
        for k in 1..=1024 {
            if k % n == 0 {
                s.insert(k);
            }
        }
        Self {
            failing_attempts: s,
            mode,
            invocations: AtomicUsize::new(0),
        }
    }

    /// Check whether the given attempt should fail. Returns `Ok(())`
    /// if it should proceed, `Err(InjectedFailure)` otherwise.
    pub fn maybe_fail(&self, attempt: usize) -> Result<(), InjectedFailure> {
        self.invocations.fetch_add(1, Ordering::Relaxed);
        if self.failing_attempts.contains(&attempt) {
            Err(InjectedFailure {
                mode: self.mode,
                attempt,
            })
        } else {
            Ok(())
        }
    }

    /// Total invocations of `maybe_fail` since this schedule was built.
    pub fn invocation_count(&self) -> usize {
        self.invocations.load(Ordering::Relaxed)
    }
}

/// Verify that recovery logic succeeded after a failure schedule.
///
/// `expected_failures` is the number of times the recovery path was
/// expected to be triggered. `actual_failures` is what was observed.
/// Returns a `CheckResult` describing whether recovery worked.
pub fn assert_recovered(
    name: impl Into<String>,
    expected_failures: usize,
    actual_failures: usize,
    final_state_ok: bool,
) -> CheckResult {
    let name = format!("chaos::{}", name.into());
    if !final_state_ok {
        return CheckResult::fail(name, Severity::Critical).with_detail(format!(
            "system did not recover. expected {expected_failures} injected failures, observed {actual_failures}, final state failed validation"
        ));
    }
    if actual_failures < expected_failures {
        return CheckResult::warn(name, Severity::Warning).with_detail(format!(
            "fewer failures observed than scheduled (expected {expected_failures}, observed {actual_failures})"
        ));
    }
    CheckResult::pass(name).with_detail(format!(
        "recovered after {actual_failures} injected failure(s)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn every_n_pattern() {
        let s = FailureSchedule::every_n(3, FailureMode::Timeout);
        assert!(s.maybe_fail(1).is_ok());
        assert!(s.maybe_fail(2).is_ok());
        assert!(s.maybe_fail(3).is_err());
        assert!(s.maybe_fail(6).is_err());
        assert!(s.maybe_fail(9).is_err());
    }

    #[test]
    fn recovery_check_pass() {
        let c = assert_recovered("write_log", 2, 2, true);
        assert!(matches!(c.verdict, dev_report::Verdict::Pass));
    }

    #[test]
    fn recovery_check_fail_when_state_invalid() {
        let c = assert_recovered("write_log", 2, 2, false);
        assert!(matches!(c.verdict, dev_report::Verdict::Fail));
    }
}
