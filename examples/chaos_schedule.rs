//! Use a `FailureSchedule` to inject deterministic failures on specific
//! attempt numbers, and print which attempts fire.
//!
//! ```text
//! cargo run --example chaos_schedule
//! ```
//!
//! Demonstrates the headline determinism contract of `dev-chaos`:
//! a schedule defined with `on_attempts(...)` produces the same sequence
//! of failures across runs and machines. Pair with a retry loop in tests
//! to verify recovery logic does its job.

use dev_chaos::{FailureMode, FailureSchedule};

fn main() {
    let schedule = FailureSchedule::on_attempts(&[3, 7, 10], FailureMode::IoError);

    println!("attempt | outcome");
    println!("--------+----------------------");
    for attempt in 1..=12 {
        let outcome = match schedule.maybe_fail(attempt) {
            Ok(()) => "ok".to_string(),
            Err(injected) => format!("FAIL: {}", injected.mode.as_str()),
        };
        println!("{:>7} | {}", attempt, outcome);
    }
}
