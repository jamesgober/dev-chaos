<h1 align="center">
    <strong>dev-chaos</strong>
    <br>
    <sup><sub>FAILURE INJECTION FOR RUST</sub></sup>
</h1>

<p align="center">
    <a href="https://crates.io/crates/dev-chaos"><img alt="crates.io" src="https://img.shields.io/crates/v/dev-chaos.svg"></a>
    <a href="https://docs.rs/dev-chaos"><img alt="docs.rs" src="https://docs.rs/dev-chaos/badge.svg"></a>
    <a href="https://github.com/jamesgober/dev-chaos/blob/main/LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Apache--2.0-blue.svg"></a>
</p>

<p align="center">
    Disk faults, network failures, panics. Recovery validation.<br>
    Part of the <code>dev-*</code> verification suite.
</p>

---

## What it does

Most code is tested only on the happy path. Real systems fail through:

- Partial writes
- Disk full mid-flush
- Connection resets
- Corrupted reads
- Permission denied
- Process crashes

`dev-chaos` injects these failures on purpose so you can verify that
your recovery logic works.

## Quick start

```toml
[dependencies]
dev-chaos = "0.1"
```

```rust
use dev_chaos::{FailureSchedule, FailureMode, assert_recovered};

// Fail on the 3rd, 7th, and 10th attempt of some operation.
let schedule = FailureSchedule::on_attempts(&[3, 7, 10], FailureMode::IoError);

let mut observed_failures = 0;
for attempt in 1..=12 {
    match schedule.maybe_fail(attempt) {
        Ok(()) => {
            // operation proceeds
        }
        Err(_e) => {
            observed_failures += 1;
            // recovery path runs here
        }
    }
}

let final_state_ok = true; // your invariant check
let check = assert_recovered("my_operation", 3, observed_failures, final_state_ok);
```

## Design choices

- **Deterministic by default.** Schedules are explicit. You know
  which attempt fails before the test runs.
- **No probabilistic chaos.** Random failures are useful for
  exploratory testing but not for repeatable verification. If you
  need that, layer randomness on top of `FailureSchedule`.
- **Recovery is the verdict, not the failure.** A test passes when
  the system recovers, not when the failure was injected.

## What's planned

- IO wrappers: `ChaosFile`, `ChaosNetwork` that inject failures into
  real Read/Write/AsyncRead/AsyncWrite types.
- Process kill simulators with `dev-fixtures` integration.
- Latency injection (slow but-not-failing operations).
- Crash-and-restart helpers paired with WAL recovery testing.

## License

Apache-2.0. See [LICENSE](LICENSE).
