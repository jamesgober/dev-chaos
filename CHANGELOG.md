# Changelog

## [Unreleased]

## [0.9.5] - 2026-10-09

Determinism and overflow fixes from a review pass, plus the MSRV
rollback to Rust 1.75. The rollback follows `dev-fixtures` 0.9.5
swapping `tempfile` for `mod-tempdir` 1.0, which removed the
`getrandom 0.4.2 -> edition2024` chain that held the dev-* collection
at 1.85.

### Added

- `AsyncChaosReader::into_inner` and `AsyncChaosWriter::into_inner`.
- `VERSION` constant with the crate version as compiled, so tools that
  bundle this crate can report what is actually linked.

### Fixed

- The async wrappers (`async-io` feature) counted every poll as an
  attempt, including polls that returned `Pending`, so which operation
  failed depended on how often the runtime polled. That broke the
  crate's determinism contract. Only completed operations count now.
- The async writer ignored `FailureMode::PartialWrite`. It now emits
  the same one-byte torn write as the sync writer (REPS section 5).
- `MemoryPressure::allocate` did not make its memory resident: the OS
  hands out zeroed pages lazily, so a 512 MiB guard added nothing to
  RSS. Every page is now written once. `try_allocate` does the same on
  success. Allocating is slower as a result (about 55 ms for 512 MiB),
  which is the point of the call.
- `LinearRamp` panicked with "overflow when adding durations" for large
  attempt counts, and attempts above `u32::MAX` wrapped back to small
  delays. It now saturates and stays linear.
- `Clock::advance` and `skew_by` wrapped on overflow, so
  `advance(Duration::MAX)` moved the clock backwards. Both saturate now.
- An empty write to a `CrashWriter` before the crash point failed with
  "crash point reached"; it now returns `Ok(0)`.

### Changed

- `rust-version` lowered from `1.85` to `1.75`. CI's MSRV job now
  builds on 1.75 against an MSRV-compatible lockfile; it was still
  pinned to 1.85.

### Documentation

- `FailureMode::Corruption` said the wrappers return success with
  corrupted bytes; they return an `InvalidData` error, as REPS
  specifies.
- The `limit` comment wrongly said it can over-fire under contention.
- `seeded_random` resolves probability to 0.001; this is now
  documented. Seeded sequences are unchanged from 0.9.4 and pinned by
  a test.
- The async and `ChaosFile` doctests now compile and run.
- README: the tagline no longer promises panic injection, and the
  version snippet and MSRV section are corrected.

[0.9.5]: https://github.com/jamesgober/dev-chaos/releases/tag/v0.9.5

## [0.9.4] - 2026-05-12

Documentation and SEO pass. No code changes.

### Changed

- README header standardized to match the collection-wide template: Rust logo image, MSRV badge between CI and docs.rs, copyright block at bottom.
- Subtitle now reads `FAULT INJECTION & RECOVERY TESTING FOR RUST` (was `FAILURE INJECTION FOR RUST`). Recovery testing is the actual verdict; making it visible up front.
- Tagline rewritten to lead with the developer outcome (inject, verify recovery).
- `## The dev-* collection` section added with the full 14-crate map.
- `Cargo.toml` description rewritten: lists fault types (disk, network, latency, crash points, partial writes).
- `Cargo.toml` keywords retuned: dropped `verification` and `ai-tools`, added `fault-injection` and `testing` for crates.io search.

### Added

- "Part of the `dev-*` verification collection" block on the README, under the intro, linking the umbrella `dev-tools` crate.

[0.9.4]: https://github.com/jamesgober/dev-chaos/releases/tag/v0.9.4

## [0.9.3] - 2026-05-12

### Added

- `examples/chaos_schedule.rs` — runnable demonstration of `FailureSchedule::on_attempts` showing the deterministic firing pattern across a 12-attempt loop.

### Changed

- CI: `actions/checkout` bumped from `v4` to `v5` (removes Node 20 deprecation warnings).

[0.9.3]: https://github.com/jamesgober/dev-chaos/releases/tag/v0.9.3

## [0.9.2] - 2026-05-10

### Added

- New `clock` module — `Clock`, a deterministic in-process time source for testing time-sensitive code (retries, expiry, TTL). Supports `advance(Duration)` and `skew_by(i64)` for forward and backward time movement. Thread-safe via internal `Arc<AtomicI64>`.
- New `memory_pressure` module — `MemoryPressure` RAII guard that allocates and holds a configurable byte budget for the duration of its lifetime. Convenience constructors `allocate_kib`, `allocate_mib`, plus `try_allocate` for OOM-path testing.
- `LatencyInjector::compose_with_schedule(schedule)` returning a `LatencyAndFailure` composer that applies latency *and* checks for scheduled failures in one call.
- New `LatencyAndFailure` type with `apply_blocking(attempt) -> Result<(), InjectedFailure>` and `delay_for(attempt)` accessors.

[0.9.2]: https://github.com/jamesgober/dev-chaos/releases/tag/v0.9.2

## [0.9.1] - 2026-05-09

### Added

- `FailureSchedule::limit(n)` builder caps the total number of failures the schedule will emit. After `n` failures, every subsequent `maybe_fail` returns `Ok(())`. Composes with `on_attempts`, `every_n`, and `seeded_random`.
- `FailureSchedule::failure_count()` accessor reports the total failures emitted so far (clamped to the configured limit when one is set).

### Fixed

- Broken intra-doc link `[`async_io`]` in the crate-level docstring would warn under `cargo doc` when the `async-io` feature is disabled. The link is now a plain code span.

[0.9.1]: https://github.com/jamesgober/dev-chaos/releases/tag/v0.9.1

## [0.9.0] - 2026-05-08

### Added

#### Adoption of dev-report 0.9

- Bumped `dev-report` dep to `0.9`.
- `assert_recovered` now emits `CheckResult`s tagged `chaos` and `recovery` (and `regression` on Fail), with numeric `Evidence` for `expected_failures`, `actual_failures`, `final_state_ok`.

#### Sync IO wrappers (v0.2 milestone)

- `dev_chaos::io::ChaosReader<R: Read>` and `ChaosWriter<W: Write>`.
- `ChaosFile = ChaosWriter<std::fs::File>` with `create` and `append` constructors.
- `FailureMode::to_io_kind()` mapping each mode to an `std::io::ErrorKind`.
- `From<InjectedFailure> for std::io::Error`.
- `PartialWrite` writes one byte then errors so callers observe a torn-write state.

#### Latency injection (v0.4 milestone)

- `dev_chaos::latency::LatencyInjector` with `LatencyProfile::Constant`, `LinearRamp { start, step }`, and `StepSchedule(Vec<(usize, Duration)>)`.
- `delay_for(attempt)` returns the duration; `apply_blocking(attempt)` sleeps the calling thread.
- All profiles deterministic.

#### Crash-restart helpers (v0.5 milestone)

- `dev_chaos::crash::CrashPoint::after_byte(N)` wrapper.
- Truncates at byte N cumulatively; subsequent writes return `WriteZero`.
- `CrashWriter::bytes_written()` and `into_inner()` accessors.

#### Seeded random schedules (v0.6 milestone, opt-in non-determinism)

- `FailureSchedule::seeded_random(seed, probability, mode)`.
- Reproducible across runs and machines for the same seed.
- Uses splitmix64 hashing of `(seed, attempt)`; no real RNG state, no clock.

#### Async IO wrappers (v0.3 milestone, opt-in)

- `async-io` feature flag (off by default).
- `dev_chaos::async_io::AsyncChaosReader<R>` and `AsyncChaosWriter<W>` for `tokio::io::AsyncRead` / `AsyncWrite`.
- Pulls `tokio` minimally (`io-util`, `macros` only).

#### Producer integration

- `ChaosProducer<F>` adapter implementing `dev_report::Producer`.
- Wraps a closure `|| -> Vec<CheckResult>` and emits a multi-check `Report` with `producer = "dev-chaos"`.

### Documentation

- All public items have rustdoc with at least one example.
- REPS.md expanded: §4 (recovery contract + required evidence), §5 (IO wrappers contract), §6 (latency injection), §7 (crash-restart helpers), §8 (producer integration).

[0.9.0]: https://github.com/jamesgober/dev-chaos/releases/tag/v0.9.0

## [0.1.0] - 2026-05-07

### Added

- Initial crate skeleton.
- `FailureMode` enum: IoError, PartialWrite, ConnectionReset, Timeout,
  Corruption, PermissionDenied.
- `FailureSchedule` with `on_attempts(&[usize], mode)` and
  `every_n(usize, mode)` constructors.
- `InjectedFailure` error type.
- `assert_recovered` helper producing a `dev-report::CheckResult`.
- Smoke tests covering schedules and recovery verdicts.

### Note

Name-claim release. IO wrappers, process kill simulators, and
latency injection land in `0.2.x` and beyond.

[Unreleased]: https://github.com/jamesgober/dev-chaos/compare/v0.9.3...HEAD
[0.1.0]: https://github.com/jamesgober/dev-chaos/releases/tag/v0.1.0
