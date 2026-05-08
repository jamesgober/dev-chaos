# Changelog

## [Unreleased]

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

[Unreleased]: https://github.com/jamesgober/dev-chaos/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/jamesgober/dev-chaos/releases/tag/v0.1.0
