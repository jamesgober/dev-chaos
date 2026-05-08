use dev_chaos::{assert_recovered, FailureMode, FailureSchedule};

#[test]
fn schedule_on_attempts() {
    let s = FailureSchedule::on_attempts(&[1, 5], FailureMode::IoError);
    assert!(s.maybe_fail(1).is_err());
    assert!(s.maybe_fail(2).is_ok());
    assert!(s.maybe_fail(5).is_err());
}

#[test]
fn schedule_every_n() {
    let s = FailureSchedule::every_n(2, FailureMode::Timeout);
    assert!(s.maybe_fail(1).is_ok());
    assert!(s.maybe_fail(2).is_err());
    assert!(s.maybe_fail(4).is_err());
}

#[test]
fn recovery_pass() {
    let c = assert_recovered("op", 3, 3, true);
    assert!(matches!(c.verdict, dev_report::Verdict::Pass));
}

#[test]
fn recovery_fail_on_invalid_state() {
    let c = assert_recovered("op", 3, 3, false);
    assert!(matches!(c.verdict, dev_report::Verdict::Fail));
}

#[test]
fn recovery_warn_on_fewer_failures() {
    let c = assert_recovered("op", 5, 2, true);
    assert!(matches!(c.verdict, dev_report::Verdict::Warn));
}
