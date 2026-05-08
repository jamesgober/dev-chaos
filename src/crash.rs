//! Crash-point markers that truncate writes at a known byte offset.
//!
//! `CrashPoint::after_byte(N)` wraps a `Write` so the underlying type
//! receives the first N bytes of the cumulative write stream and then
//! every subsequent write returns `WriteZero`. The wrapper does not
//! kill the process; it simulates "the process crashed after writing
//! N bytes" so recovery code can be exercised in-process.
//!
//! Pair with [`dev-fixtures`'s `TempProject`](https://crates.io/crates/dev-fixtures)
//! to model "crash mid-write, then reopen and recover."

use std::io::{self, Write};

/// A write-truncating crash marker.
///
/// `CrashPoint::after_byte(N).wrap(writer)` returns a writer that
/// passes through up to `N` bytes (cumulative across all `write`
/// calls), then refuses every subsequent byte with
/// `ErrorKind::WriteZero`.
///
/// # Example
///
/// ```
/// use dev_chaos::crash::CrashPoint;
/// use std::io::Write;
///
/// let mut sink: Vec<u8> = Vec::new();
/// let mut crashed = CrashPoint::after_byte(3).wrap(&mut sink);
///
/// crashed.write_all(b"abcd").ok();
/// drop(crashed);
/// assert_eq!(sink, b"abc");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct CrashPoint {
    after: usize,
}

impl CrashPoint {
    /// Crash after `n` bytes have been written cumulatively.
    pub fn after_byte(n: usize) -> Self {
        Self { after: n }
    }

    /// Wrap `writer` with this crash point.
    pub fn wrap<W: Write>(self, writer: W) -> CrashWriter<W> {
        CrashWriter {
            inner: writer,
            after: self.after,
            written: 0,
        }
    }
}

/// Writer wrapper produced by [`CrashPoint::wrap`].
pub struct CrashWriter<W: Write> {
    inner: W,
    after: usize,
    written: usize,
}

impl<W: Write> CrashWriter<W> {
    /// Total bytes successfully passed through to the inner writer.
    pub fn bytes_written(&self) -> usize {
        self.written
    }

    /// Consume the wrapper and return the underlying writer.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for CrashWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written >= self.after {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "crash point reached",
            ));
        }
        let remaining = self.after - self.written;
        let to_write = remaining.min(buf.len());
        if to_write == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "crash point reached",
            ));
        }
        let written = self.inner.write(&buf[..to_write])?;
        self.written += written;
        // If the caller asked for more than we let through, we report
        // the partial write so they can detect the truncation.
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_after_byte_passes_through_then_truncates() {
        let sink: Vec<u8> = Vec::new();
        let mut w = CrashPoint::after_byte(3).wrap(sink);
        w.write_all(b"abcd").ok();
        let sink = w.into_inner();
        assert_eq!(sink, b"abc");
    }

    #[test]
    fn crash_after_zero_writes_nothing() {
        let sink: Vec<u8> = Vec::new();
        let mut w = CrashPoint::after_byte(0).wrap(sink);
        let r = w.write(b"a");
        assert!(r.is_err());
        let sink = w.into_inner();
        assert!(sink.is_empty());
    }

    #[test]
    fn crash_with_large_budget_passes_through() {
        let sink: Vec<u8> = Vec::new();
        let mut w = CrashPoint::after_byte(1_000).wrap(sink);
        w.write_all(b"hello").unwrap();
        let sink = w.into_inner();
        assert_eq!(sink, b"hello");
    }

    #[test]
    fn bytes_written_tracks_progress() {
        let sink: Vec<u8> = Vec::new();
        let mut w = CrashPoint::after_byte(5).wrap(sink);
        w.write_all(b"ab").unwrap();
        assert_eq!(w.bytes_written(), 2);
    }

    #[test]
    fn split_across_writes_still_truncates_at_offset() {
        let sink: Vec<u8> = Vec::new();
        let mut w = CrashPoint::after_byte(4).wrap(sink);
        w.write_all(b"ab").unwrap();
        // Next write_all asks for 4 more, only 2 fit, so write_all
        // succeeds for the first chunk then errors.
        let _ = w.write_all(b"cdef");
        let sink = w.into_inner();
        assert_eq!(sink, b"abcd");
    }
}
