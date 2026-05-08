//! Synchronous IO wrappers that inject failures into real `Read`/`Write`
//! types.
//!
//! Each wrapper holds a [`FailureSchedule`] and increments an attempt
//! counter on every read/write call. When the schedule fires, the call
//! returns an `io::Error` derived from the [`FailureMode`]. On
//! non-failing attempts, the wrapper delegates to the underlying type
//! and preserves its bytes-on-disk behavior.
//!
//! [`FailureMode`]: crate::FailureMode

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::FailureSchedule;

/// Wraps a `Read` and injects failures per a [`FailureSchedule`].
///
/// On non-failing attempts, behavior is identical to the underlying
/// reader. On failing attempts, returns an `io::Error` with the
/// schedule's [`FailureMode`](crate::FailureMode) mapped to a
/// matching `ErrorKind`.
///
/// # Example
///
/// ```
/// use dev_chaos::{io::ChaosReader, FailureMode, FailureSchedule};
/// use std::io::Read;
///
/// let data: &[u8] = b"hello";
/// let schedule = FailureSchedule::on_attempts(&[2], FailureMode::IoError);
/// let mut reader = ChaosReader::new(data, schedule);
///
/// let mut buf = [0u8; 1];
/// reader.read(&mut buf).unwrap();           // attempt 1: ok
/// assert!(reader.read(&mut buf).is_err()); // attempt 2: fails
/// ```
pub struct ChaosReader<R: Read> {
    inner: R,
    schedule: FailureSchedule,
    attempt: AtomicUsize,
}

impl<R: Read> ChaosReader<R> {
    /// Wrap `inner` with the given schedule.
    pub fn new(inner: R, schedule: FailureSchedule) -> Self {
        Self {
            inner,
            schedule,
            attempt: AtomicUsize::new(0),
        }
    }

    /// Number of read attempts (successful or failed) so far.
    pub fn attempt_count(&self) -> usize {
        self.attempt.load(Ordering::Relaxed)
    }

    /// Consume the wrapper and return the underlying reader.
    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for ChaosReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.attempt.fetch_add(1, Ordering::Relaxed) + 1;
        if let Err(f) = self.schedule.maybe_fail(n) {
            return Err(f.into());
        }
        self.inner.read(buf)
    }
}

/// Wraps a `Write` and injects failures per a [`FailureSchedule`].
///
/// On `PartialWrite` failures, the wrapper writes one byte (when the
/// caller asked for at least one) before returning the error, so the
/// caller observes a partial-flush state.
///
/// On all other failure modes, the wrapper returns the error without
/// writing any bytes.
///
/// # Example
///
/// ```
/// use dev_chaos::{io::ChaosWriter, FailureMode, FailureSchedule};
/// use std::io::Write;
///
/// let mut sink: Vec<u8> = Vec::new();
/// let schedule = FailureSchedule::on_attempts(&[2], FailureMode::IoError);
/// let mut writer = ChaosWriter::new(&mut sink, schedule);
///
/// writer.write_all(b"a").unwrap();        // attempt 1: ok
/// assert!(writer.write_all(b"b").is_err()); // attempt 2: fails
/// drop(writer);
/// assert_eq!(sink, b"a");
/// ```
pub struct ChaosWriter<W: Write> {
    inner: W,
    schedule: FailureSchedule,
    attempt: AtomicUsize,
}

impl<W: Write> ChaosWriter<W> {
    /// Wrap `inner` with the given schedule.
    pub fn new(inner: W, schedule: FailureSchedule) -> Self {
        Self {
            inner,
            schedule,
            attempt: AtomicUsize::new(0),
        }
    }

    /// Number of write attempts (successful or failed) so far.
    pub fn attempt_count(&self) -> usize {
        self.attempt.load(Ordering::Relaxed)
    }

    /// Consume the wrapper and return the underlying writer.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for ChaosWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.attempt.fetch_add(1, Ordering::Relaxed) + 1;
        if let Err(f) = self.schedule.maybe_fail(n) {
            // Partial-write semantics: write one byte then error so
            // the caller sees a torn state.
            if matches!(f.mode, crate::FailureMode::PartialWrite) && !buf.is_empty() {
                let _ = self.inner.write(&buf[..1])?;
            }
            return Err(f.into());
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Convenience: a `ChaosWriter<File>`.
///
/// Open a real file and wrap it for failure injection on writes.
///
/// # Example (ignored: requires a real filesystem path)
///
/// ```ignore
/// use dev_chaos::{io::ChaosFile, FailureMode, FailureSchedule};
/// use std::io::Write;
///
/// let schedule = FailureSchedule::on_attempts(&[3], FailureMode::PartialWrite);
/// let mut f = ChaosFile::create("/tmp/x.log", schedule).unwrap();
/// f.write_all(b"data").unwrap();
/// ```
pub type ChaosFile = ChaosWriter<std::fs::File>;

impl ChaosFile {
    /// Create a new file at `path` and wrap it.
    pub fn create(
        path: impl AsRef<std::path::Path>,
        schedule: FailureSchedule,
    ) -> io::Result<Self> {
        let f = std::fs::File::create(path)?;
        Ok(Self::new(f, schedule))
    }

    /// Open an existing file at `path` for appending and wrap it.
    pub fn append(
        path: impl AsRef<std::path::Path>,
        schedule: FailureSchedule,
    ) -> io::Result<Self> {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self::new(f, schedule))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FailureMode;

    #[test]
    fn reader_passes_through_when_schedule_does_not_fire() {
        let data: &[u8] = b"hello";
        let schedule = FailureSchedule::on_attempts(&[10], FailureMode::IoError);
        let mut r = ChaosReader::new(data, schedule);
        let mut buf = [0u8; 5];
        let n = r.read(&mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn reader_fails_when_schedule_fires() {
        let data: &[u8] = b"hello";
        let schedule = FailureSchedule::on_attempts(&[1], FailureMode::Timeout);
        let mut r = ChaosReader::new(data, schedule);
        let mut buf = [0u8; 5];
        let err = r.read(&mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn reader_attempt_count_increments() {
        let data: &[u8] = b"abc";
        let schedule = FailureSchedule::on_attempts(&[], FailureMode::IoError);
        let mut r = ChaosReader::new(data, schedule);
        let mut buf = [0u8; 1];
        for _ in 0..3 {
            let _ = r.read(&mut buf);
        }
        assert_eq!(r.attempt_count(), 3);
    }

    #[test]
    fn writer_passes_through_bytes() {
        let sink: Vec<u8> = Vec::new();
        let schedule = FailureSchedule::on_attempts(&[], FailureMode::IoError);
        let mut w = ChaosWriter::new(sink, schedule);
        w.write_all(b"hello").unwrap();
        let sink = w.into_inner();
        assert_eq!(sink, b"hello");
    }

    #[test]
    fn writer_fails_on_scheduled_attempt() {
        let sink: Vec<u8> = Vec::new();
        let schedule = FailureSchedule::on_attempts(&[2], FailureMode::ConnectionReset);
        let mut w = ChaosWriter::new(sink, schedule);
        w.write_all(b"a").unwrap();
        let err = w.write_all(b"b").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);
        let sink = w.into_inner();
        assert_eq!(sink, b"a");
    }

    #[test]
    fn writer_partial_write_emits_one_byte_then_error() {
        let sink: Vec<u8> = Vec::new();
        let schedule = FailureSchedule::on_attempts(&[1], FailureMode::PartialWrite);
        let mut w = ChaosWriter::new(sink, schedule);
        let err = w.write(b"abcd").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WriteZero);
        let sink = w.into_inner();
        assert_eq!(sink, b"a");
    }

    #[test]
    fn chaos_file_writes_and_truncates_on_partial() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt");
        let schedule = FailureSchedule::on_attempts(&[2], FailureMode::PartialWrite);
        let mut f = ChaosFile::create(&path, schedule).unwrap();
        f.write_all(b"first").unwrap();
        let _ = f.write(b"second"); // attempt 2: PartialWrite -> 1 byte then err
        drop(f);
        let bytes = std::fs::read(&path).unwrap();
        // "first" + "s" (one byte from "second" before failure).
        assert_eq!(bytes, b"firsts");
    }

    #[test]
    fn into_inner_returns_underlying() {
        let data: &[u8] = b"x";
        let schedule = FailureSchedule::on_attempts(&[], FailureMode::IoError);
        let r = ChaosReader::new(data, schedule);
        let inner = r.into_inner();
        assert_eq!(inner, b"x");
    }
}
