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
returns:

- `Pass` when `actual_failures >= expected_failures` AND `final_state_ok`.
- `Warn` when `actual_failures < expected_failures` AND `final_state_ok`
  (under-injection: schedule fired fewer times than expected).
- `Fail` (Critical) when `final_state_ok` is false, regardless of
  failure counts.

## 5. Safety

This crate MUST NOT execute any failure injection that requires
elevated privileges. Disk-full simulation, kernel-level fault
injection, and similar are out of scope.

This crate MUST NOT modify the user's filesystem or process state
outside of types the user explicitly hands it.
