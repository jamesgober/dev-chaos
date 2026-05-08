use dev_chaos::{
    assert_recovered,
    crash::CrashPoint,
    io::{ChaosReader, ChaosWriter},
    latency::{LatencyInjector, LatencyProfile},
    ChaosProducer, FailureMode, FailureSchedule,
};
use dev_report::Producer;
use std::io::{Read, Write};
use std::time::Duration;

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
fn schedule_seeded_random_is_reproducible() {
    let a = FailureSchedule::seeded_random(99, 0.3, FailureMode::IoError);
    let b = FailureSchedule::seeded_random(99, 0.3, FailureMode::IoError);
    let mut a_seq = Vec::new();
    let mut b_seq = Vec::new();
    for attempt in 1..=50 {
        a_seq.push(a.maybe_fail(attempt).is_err());
        b_seq.push(b.maybe_fail(attempt).is_err());
    }
    assert_eq!(a_seq, b_seq);
}

#[test]
fn recovery_pass() {
    let c = assert_recovered("op", 3, 3, true);
    assert!(matches!(c.verdict, dev_report::Verdict::Pass));
    assert!(c.has_tag("chaos"));
    assert!(c.has_tag("recovery"));
}

#[test]
fn recovery_fail_on_invalid_state() {
    let c = assert_recovered("op", 3, 3, false);
    assert!(matches!(c.verdict, dev_report::Verdict::Fail));
    assert!(c.has_tag("regression"));
}

#[test]
fn recovery_warn_on_fewer_failures() {
    let c = assert_recovered("op", 5, 2, true);
    assert!(matches!(c.verdict, dev_report::Verdict::Warn));
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
fn chaos_reader_round_trip_with_failure() {
    let data: &[u8] = b"hello";
    let schedule = FailureSchedule::on_attempts(&[2], FailureMode::IoError);
    let mut r = ChaosReader::new(data, schedule);
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf).unwrap();
    assert!(r.read_exact(&mut buf).is_err());
}

#[test]
fn chaos_writer_partial_write() {
    let sink: Vec<u8> = Vec::new();
    let schedule = FailureSchedule::on_attempts(&[1], FailureMode::PartialWrite);
    let mut w = ChaosWriter::new(sink, schedule);
    let _ = w.write(b"abcd");
    let inner = w.into_inner();
    assert_eq!(inner, b"a");
}

#[test]
fn latency_injector_constant_profile() {
    let inj = LatencyInjector::new(LatencyProfile::Constant(Duration::from_micros(2)));
    assert_eq!(inj.delay_for(1), Duration::from_micros(2));
    assert_eq!(inj.delay_for(50), Duration::from_micros(2));
}

#[test]
fn crash_point_truncates_at_offset() {
    let sink: Vec<u8> = Vec::new();
    let mut w = CrashPoint::after_byte(3).wrap(sink);
    let _ = w.write_all(b"abcde");
    let inner = w.into_inner();
    assert_eq!(inner, b"abc");
}

#[test]
fn chaos_producer_emits_report() {
    let producer = ChaosProducer::new(
        || vec![assert_recovered("a", 1, 1, true)],
        "my-crate",
        "0.1.0",
    );
    let report = producer.produce();
    assert_eq!(report.checks.len(), 1);
    assert_eq!(report.producer.as_deref(), Some("dev-chaos"));
}
