//! Async IO wrappers. Available with the `async-io` feature.
//!
//! Mirror of [`crate::io`] for `tokio::io::AsyncRead` / `AsyncWrite`.
//! Pulls in `tokio` minimally (no runtime, no networking, no scheduler).
//!
//! Schedules are shared with the sync wrappers: a [`FailureSchedule`]
//! built once can be used in either context.

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::FailureSchedule;

/// Async equivalent of [`crate::io::ChaosReader`].
///
/// # Example (ignored: requires a tokio runtime)
///
/// ```ignore
/// use dev_chaos::{async_io::AsyncChaosReader, FailureMode, FailureSchedule};
/// use tokio::io::AsyncReadExt;
///
/// # async fn run() {
/// let data: &[u8] = b"hello";
/// let schedule = FailureSchedule::on_attempts(&[2], FailureMode::IoError);
/// let mut reader = AsyncChaosReader::new(data, schedule);
/// let mut buf = [0u8; 1];
/// reader.read(&mut buf).await.unwrap();
/// # }
/// ```
pub struct AsyncChaosReader<R: AsyncRead + Unpin> {
    inner: R,
    schedule: FailureSchedule,
    attempt: AtomicUsize,
}

impl<R: AsyncRead + Unpin> AsyncChaosReader<R> {
    /// Wrap `inner` with the given schedule.
    pub fn new(inner: R, schedule: FailureSchedule) -> Self {
        Self {
            inner,
            schedule,
            attempt: AtomicUsize::new(0),
        }
    }

    /// Number of poll attempts so far.
    pub fn attempt_count(&self) -> usize {
        self.attempt.load(Ordering::Relaxed)
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for AsyncChaosReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let n = self.attempt.fetch_add(1, Ordering::Relaxed) + 1;
        if let Err(f) = self.schedule.maybe_fail(n) {
            return Poll::Ready(Err(f.into()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

/// Async equivalent of [`crate::io::ChaosWriter`].
///
/// # Example (ignored: requires a tokio runtime)
///
/// ```ignore
/// use dev_chaos::{async_io::AsyncChaosWriter, FailureMode, FailureSchedule};
/// use tokio::io::AsyncWriteExt;
///
/// # async fn run() {
/// let mut sink: Vec<u8> = Vec::new();
/// let schedule = FailureSchedule::on_attempts(&[2], FailureMode::Timeout);
/// let mut writer = AsyncChaosWriter::new(&mut sink, schedule);
/// writer.write_all(b"a").await.unwrap();
/// # }
/// ```
pub struct AsyncChaosWriter<W: AsyncWrite + Unpin> {
    inner: W,
    schedule: FailureSchedule,
    attempt: AtomicUsize,
}

impl<W: AsyncWrite + Unpin> AsyncChaosWriter<W> {
    /// Wrap `inner` with the given schedule.
    pub fn new(inner: W, schedule: FailureSchedule) -> Self {
        Self {
            inner,
            schedule,
            attempt: AtomicUsize::new(0),
        }
    }

    /// Number of write attempts so far.
    pub fn attempt_count(&self) -> usize {
        self.attempt.load(Ordering::Relaxed)
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for AsyncChaosWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let n = self.attempt.fetch_add(1, Ordering::Relaxed) + 1;
        if let Err(f) = self.schedule.maybe_fail(n) {
            return Poll::Ready(Err(f.into()));
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FailureMode;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test(flavor = "current_thread")]
    async fn async_reader_passes_through_then_fails() {
        let data: Vec<u8> = b"hello".to_vec();
        let cursor = std::io::Cursor::new(data);
        let schedule = FailureSchedule::on_attempts(&[2], FailureMode::Timeout);
        let mut reader = AsyncChaosReader::new(cursor, schedule);
        let mut buf = [0u8; 1];
        reader.read_exact(&mut buf).await.unwrap();
        let err = reader.read_exact(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_writer_writes_then_fails() {
        let sink: Vec<u8> = Vec::new();
        let schedule = FailureSchedule::on_attempts(&[2], FailureMode::ConnectionReset);
        let mut writer = AsyncChaosWriter::new(sink, schedule);
        writer.write_all(b"a").await.unwrap();
        let err = writer.write_all(b"b").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionReset);
        let sink = writer.inner;
        assert_eq!(sink, b"a");
    }
}
