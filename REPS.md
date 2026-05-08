# dev-chaos — Project Specification (REPS)

> Rust Engineering Project Specification.
> Normative language follows RFC 2119.

## 1. Purpose

`dev-chaos` MUST inject controlled failures into a system under test
and provide primitives to verify recovery. Output MUST be
`dev-report::CheckResult` or `Report`.

## 2. Scope

This crate MUST provide:

- A `FailureMode` enum for common failure types.
- A `FailureSchedule` for deterministic, attempt-based injection.
- A recovery-verification helper (`assert_recovered`).

This crate SHOULD provide (later versions):

- IO wrappers (`ChaosFile`, `ChaosReader`, `ChaosWriter`) that inject
  failures into real Read/Write types.
- Async equivalents for `tokio::io`.
- Process kill simulators paired with `dev-fixtures`.
- Latency injection.
- Crash-restart helpers for WAL recovery testing.

This crate MUST NOT:

- Inject failures probabilistically by default. Determinism is the
  contract.
- Replace mutation testing crates (`mutagen`, etc.).
- Run the system under test. Composition with the user's harness
  is the user's responsibility.

## 3. Determinism

A given `FailureSchedule` MUST produce the same sequence of failures
for the same sequence of attempts. This MUST hold across runs and
across machines.

If randomness is later added, it MUST be opt-in, seeded, and
reproducible from the seed.

## 4. Recovery contract

`assert_recovered(name, expected_failures, actual_failures, final_state_ok)`
returns a `CheckResult` tagged `chaos` and `recovery`:

- `Pass` when `actual_failures >= expected_failures` AND `final_state_ok`.
- `Warn` (Warning) when `actual_failures < expected_failures` AND
  `final_state_ok` (under-injection: schedule fired fewer times than
  expected).
- `Fail` (Critical) when `final_state_ok` is false, regardless of
  failure counts. Additionally tagged `regression`.

### 4.1 Required evidence

Every `CheckResult` from `assert_recovered` MUST carry numeric
`Evidence` for:

- `expected_failures`
- `actual_failures`
- `final_state_ok` (1.0 = true, 0.0 = false)

## 5. IO wrappers

`dev_chaos::io` provides synchronous wrappers around `Read`/`Write`:

- `ChaosReader<R>` and `ChaosWriter<W>` consume a `FailureSchedule`
  and inject `io::Error` per [`FailureMode`] mapping.
- `ChaosFile = ChaosWriter<File>` for filesystem writes.
- On non-failing attempts, behavior MUST be byte-identical to the
  underlying reader/writer.
- `PartialWrite` MUST emit exactly one byte (when the caller asked
  for at least one) before returning the error, so the caller
  observes a torn-write state.

The `async-io` feature pulls in `tokio::io` and adds
`AsyncChaosReader<R>` and `AsyncChaosWriter<W>` with the same
contract.

## 6. Latency injection

`dev_chaos::latency::LatencyInjector` returns deterministic per-attempt
delays via `LatencyProfile::{Constant, LinearRamp, StepSchedule}`. The
type is intentionally side-effect-free: `delay_for(attempt)` returns
the duration; the caller decides when to sleep.

## 7. Crash-restart helpers

`dev_chaos::crash::CrashPoint::after_byte(N)` wraps a `Write` so that
the underlying writer receives at most `N` bytes cumulatively before
all subsequent writes return `WriteZero`. This models "process
crashed mid-write" without actually crashing the process.

## 8. Producer integration

This crate MUST provide `ChaosProducer<F>` implementing
`dev_report::Producer`. Given a closure returning
`Vec<CheckResult>`, the producer emits a finalized `Report` with
`producer = "dev-chaos"`.

## 9. Safety

This crate MUST NOT execute any failure injection that requires
elevated privileges. Disk-full simulation, kernel-level fault
injection, and similar are out of scope.

This crate MUST NOT modify the user's filesystem or process state
outside of types the user explicitly hands it.
