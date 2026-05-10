<h1 align="center">
    <strong>dev-chaos</strong>
    <br>
    <sup><sub>FAILURE INJECTION FOR RUST</sub></sup>
</h1>

<p align="center">
    <a href="https://crates.io/crates/dev-chaos"><img alt="crates.io" src="https://img.shields.io/crates/v/dev-chaos.svg"></a>
    <a href="https://crates.io/crates/dev-chaos"><img alt="downloads" src="https://img.shields.io/crates/d/dev-chaos.svg"></a>
    <a href="https://github.com/jamesgober/dev-chaos/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/jamesgober/dev-chaos/actions/workflows/ci.yml/badge.svg"></a>
    <a href="https://docs.rs/dev-chaos"><img alt="docs.rs" src="https://docs.rs/dev-chaos/badge.svg"></a>
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
dev-chaos = "0.9.1"
```

Opt-in features:

```toml
[dependencies]
dev-chaos = { version = "0.9.1", features = ["async-io"] }
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
let _check = assert_recovered("my_operation", 3, observed_failures, final_state_ok);
```

The returned `CheckResult` carries `chaos` + `recovery` tags and
numeric `Evidence` for `expected_failures`, `actual_failures`,
`final_state_ok`.

## IO wrappers

Inject failures into a real `Read`/`Write` without touching the
system under test:

```rust
use dev_chaos::{io::ChaosWriter, FailureMode, FailureSchedule};
use std::io::Write;

let sink: Vec<u8> = Vec::new();
let schedule = FailureSchedule::on_attempts(&[2], FailureMode::PartialWrite);
let mut w = ChaosWriter::new(sink, schedule);

w.write_all(b"first").unwrap();        // attempt 1: ok
let _ = w.write(b"second");             // attempt 2: 1 byte then PartialWrite
let inner = w.into_inner();
assert_eq!(inner, b"firsts");
```

`ChaosFile`, `ChaosReader`, and `ChaosWriter` cover sync IO. With the
`async-io` feature, `AsyncChaosReader` and `AsyncChaosWriter` cover
`tokio::io`.

## Crash-restart helpers

Model "process crashed mid-write" without crashing the process:

```rust
use dev_chaos::crash::CrashPoint;
use std::io::Write;

let sink: Vec<u8> = Vec::new();
let mut w = CrashPoint::after_byte(3).wrap(sink);
let _ = w.write_all(b"abcde");  // truncated at 3 bytes
let inner = w.into_inner();
assert_eq!(inner, b"abc");
```

## Latency injection

Simulate slow but successful operations:

```rust
use dev_chaos::latency::{LatencyInjector, LatencyProfile};
use std::time::Duration;

let inj = LatencyInjector::new(LatencyProfile::LinearRamp {
    start: Duration::from_micros(10),
    step:  Duration::from_micros(5),
});
for attempt in 1..=5 {
    inj.apply_blocking(attempt);
    // ... call the operation under test ...
}
```

## Seeded random schedules

When you need probabilistic exploration but reproducible results:

```rust
use dev_chaos::{FailureMode, FailureSchedule};

// Same seed produces the same sequence on every run, on every machine.
let schedule = FailureSchedule::seeded_random(42, 0.05, FailureMode::Timeout);
```

## Producer trait

```rust
use dev_chaos::{assert_recovered, ChaosProducer};
use dev_report::Producer;

let producer = ChaosProducer::new(
    || vec![
        assert_recovered("write_log", 2, 2, true),
        assert_recovered("rename",    1, 1, true),
    ],
    "my-crate",
    "0.1.0",
);
let report = producer.produce();
```

## Design choices

- **Deterministic by default.** Schedules are explicit. You know
  which attempt fails before the test runs.
- **Random failures are opt-in and seeded.** `seeded_random` is
  reproducible from the seed; no clock, no thread, no real RNG state.
- **Recovery is the verdict, not the failure.** A test passes when
  the system recovers, not when the failure was injected.

## Status

`v0.9.x` is the pre-1.0 stabilization line. APIs are expected to be
near-final; minor adjustments may still happen ahead of `1.0`.
Determinism is the contract: the same `(schedule, attempt)` pair
always produces the same outcome.

## Minimum supported Rust version

`1.85` — pinned in `Cargo.toml` via `rust-version` and verified by
the MSRV job in CI. (Bumped from 1.75 to align with the suite's
shared MSRV after sibling crates picked up dependencies that require
`edition2024`.)

## License

Apache-2.0. See [LICENSE](LICENSE).
