//! Async IO wrappers. Available with the `async-io` feature.
//!
//! Mirror of [`crate::io`] for `tokio::io::AsyncRead` / `AsyncWrite`.
//! Pulls in `tokio` minimally (no runtime, no networking, no scheduler).
//!
//! Schedules are shared with the sync wrappers: a [`FailureSchedule`]
//! built once can be used in either context.
//!
//! ## Attempts and `Poll::Pending`
//!
//! An attempt is one read or write operation, not one poll. When the
//! inner reader or writer returns `Poll::Pending`, the next poll
//! continues the same attempt without consulting the schedule again.
//! This keeps the failure sequence deterministic: it depends only on
//! the sequence of operations, not on how often the runtime had to
//! poll a slow socket or pipe.

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::{FailureMode, FailureSchedule, InjectedFailure};

/// Async equivalent of [`crate::io::ChaosReader`].
///
/// # Example
///
/// ```
/// use dev_chaos::{async_io::AsyncChaosReader, FailureMode, FailureSchedule};
/// use tokio::io::AsyncReadExt;
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let data: &[u8] = b"hello";
/// let schedule = FailureSchedule::on_attempts(&[2], FailureMode::IoError);
/// let mut reader = AsyncChaosReader::new(data, schedule);
/// let mut buf = [0u8; 1];
/// reader.read(&mut buf).await.unwrap();          // attempt 1: ok
/// assert!(reader.read(&mut buf).await.is_err()); // attempt 2: fails
/// # });
/// ```
pub struct AsyncChaosReader<R: AsyncRead + Unpin> {
    inner: R,
    schedule: FailureSchedule,
    attempt: AtomicUsize,
    /// A read passed the schedule check and the inner reader returned
    /// `Pending`; the next poll continues it as the same attempt.
    in_flight: bool,
}

impl<R: AsyncRead + Unpin> AsyncChaosReader<R> {
    /// Wrap `inner` with the given schedule.
    pub fn new(inner: R, schedule: FailureSchedule) -> Self {
        Self {
            inner,
            schedule,
            attempt: AtomicUsize::new(0),
            in_flight: false,
        }
    }

    /// Number of read attempts (successful or failed) so far.
    ///
    /// A read that returns `Poll::Pending` and is polled again counts
    /// as one attempt.
    pub fn attempt_count(&self) -> usize {
        self.attempt.load(Ordering::Relaxed)
    }

    /// Consume the wrapper and return the underlying reader.
    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for AsyncChaosReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.in_flight {
            let n = this.attempt.fetch_add(1, Ordering::Relaxed) + 1;
            if let Err(f) = this.schedule.maybe_fail(n) {
                return Poll::Ready(Err(f.into()));
            }
        }
        let res = Pin::new(&mut this.inner).poll_read(cx, buf);
        this.in_flight = res.is_pending();
        res
    }
}

/// Where an [`AsyncChaosWriter`] is within the current write attempt.
enum WriteState {
    /// No attempt in progress; the next poll starts one.
    Idle,
    /// The attempt passed the schedule and is waiting on the inner writer.
    Passing,
    /// A `PartialWrite` failure fired; waiting to write its one byte.
    PartialByte(InjectedFailure),
}

/// Async equivalent of [`crate::io::ChaosWriter`].
///
/// Same contract as the sync writer: on a `PartialWrite` failure, one
/// byte (when the caller asked for at least one) reaches the inner
/// writer before the error is returned; other failure modes write
/// nothing.
///
/// # Example
///
/// ```
/// use dev_chaos::{async_io::AsyncChaosWriter, FailureMode, FailureSchedule};
/// use tokio::io::AsyncWriteExt;
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let mut sink: Vec<u8> = Vec::new();
/// let schedule = FailureSchedule::on_attempts(&[2], FailureMode::Timeout);
/// let mut writer = AsyncChaosWriter::new(&mut sink, schedule);
/// writer.write_all(b"a").await.unwrap();          // attempt 1: ok
/// assert!(writer.write_all(b"b").await.is_err()); // attempt 2: fails
/// drop(writer);
/// assert_eq!(sink, b"a");
/// # });
/// ```
pub struct AsyncChaosWriter<W: AsyncWrite + Unpin> {
    inner: W,
    schedule: FailureSchedule,
    attempt: AtomicUsize,
    state: WriteState,
}

impl<W: AsyncWrite + Unpin> AsyncChaosWriter<W> {
    /// Wrap `inner` with the given schedule.
    pub fn new(inner: W, schedule: FailureSchedule) -> Self {
        Self {
            inner,
            schedule,
            attempt: AtomicUsize::new(0),
            state: WriteState::Idle,
        }
    }

    /// Number of write attempts so far.
    ///
    /// A write that returns `Poll::Pending` and is polled again counts
    /// as one attempt.
    pub fn attempt_count(&self) -> usize {
        self.attempt.load(Ordering::Relaxed)
    }

    /// Consume the wrapper and return the underlying writer.
    pub fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for AsyncChaosWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if let WriteState::Idle = this.state {
            let n = this.attempt.fetch_add(1, Ordering::Relaxed) + 1;
            this.state = match this.schedule.maybe_fail(n) {
                Ok(()) => WriteState::Passing,
                Err(f) if f.mode == FailureMode::PartialWrite && !buf.is_empty() => {
                    WriteState::PartialByte(f)
                }
                Err(f) => return Poll::Ready(Err(f.into())),
            };
        }
        match std::mem::replace(&mut this.state, WriteState::Idle) {
            WriteState::PartialByte(f) => {
                // Write exactly one byte, then report the injected
                // failure, so the caller sees a torn write.
                let Some(first) = buf.get(..1) else {
                    return Poll::Ready(Err(f.into()));
                };
                match Pin::new(&mut this.inner).poll_write(cx, first) {
                    Poll::Pending => {
                        this.state = WriteState::PartialByte(f);
                        Poll::Pending
                    }
                    Poll::Ready(Ok(_)) => Poll::Ready(Err(f.into())),
                    Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
                }
            }
            // `Passing` (and `Idle`, which cannot occur here because the
            // block above always leaves a non-idle state or returns).
            _ => {
                let res = Pin::new(&mut this.inner).poll_write(cx, buf);
                if res.is_pending() {
                    this.state = WriteState::Passing;
                }
                res
            }
        }
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
    use std::future::poll_fn;
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
        let sink = writer.into_inner();
        assert_eq!(sink, b"a");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_partial_write_emits_one_byte_then_error() {
        // The async writer used to return the error without writing,
        // unlike the sync writer and REPS section 5.
        let sink: Vec<u8> = Vec::new();
        let schedule = FailureSchedule::on_attempts(&[2], FailureMode::PartialWrite);
        let mut w = AsyncChaosWriter::new(sink, schedule);
        w.write_all(b"first").await.unwrap();
        let err = w.write(b"second").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WriteZero);
        assert_eq!(w.into_inner(), b"firsts");
    }

    /// Returns `Pending` (and wakes itself) on every other poll, like a
    /// socket that is not ready yet.
    struct Flaky<T> {
        inner: T,
        ready: bool,
    }

    impl<T: AsyncRead + Unpin> AsyncRead for Flaky<T> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if !self.ready {
                self.ready = true;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            self.ready = false;
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl<T: AsyncWrite + Unpin> AsyncWrite for Flaky<T> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            if !self.ready {
                self.ready = true;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            self.ready = false;
            Pin::new(&mut self.inner).poll_write(cx, buf)
        }
        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }
        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pending_polls_do_not_consume_attempts_on_read() {
        // Each poll used to count as an attempt, so a reader that is not
        // always ready shifted which operation the schedule failed.
        let data: &[u8] = b"abcdef";
        let flaky = Flaky {
            inner: data,
            ready: false,
        };
        let schedule = FailureSchedule::on_attempts(&[3], FailureMode::Timeout);
        let mut r = AsyncChaosReader::new(flaky, schedule);
        let mut buf = [0u8; 1];
        r.read_exact(&mut buf).await.unwrap();
        r.read_exact(&mut buf).await.unwrap();
        assert_eq!(r.attempt_count(), 2);
        let err = r.read_exact(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(r.attempt_count(), 3);
        // The failed attempt consumed no data.
        r.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"c");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pending_polls_do_not_consume_attempts_on_write() {
        let flaky = Flaky {
            inner: Vec::new(),
            ready: false,
        };
        let schedule = FailureSchedule::on_attempts(&[3], FailureMode::PartialWrite);
        let mut w = AsyncChaosWriter::new(flaky, schedule);
        w.write_all(b"a").await.unwrap();
        w.write_all(b"b").await.unwrap();
        assert_eq!(w.attempt_count(), 2);
        // Attempt 3: the one byte is written through a Pending poll too.
        let err = w.write(b"xyz").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::WriteZero);
        assert_eq!(w.attempt_count(), 3);
        w.write_all(b"c").await.unwrap();
        assert_eq!(w.into_inner().inner, b"abxc");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sync_and_async_writers_fail_the_same_attempts() {
        let seq: Vec<&[u8]> = vec![b"aa", b"bb", b"cc", b"dd", b"ee", b"ff"];
        let mut sync_w = crate::io::ChaosWriter::new(
            Vec::new(),
            FailureSchedule::seeded_random(9, 0.4, FailureMode::PartialWrite),
        );
        let mut async_w = AsyncChaosWriter::new(
            Flaky {
                inner: Vec::new(),
                ready: false,
            },
            FailureSchedule::seeded_random(9, 0.4, FailureMode::PartialWrite),
        );
        for chunk in &seq {
            let s = std::io::Write::write(&mut sync_w, chunk).is_err();
            let a = async_w.write(chunk).await.is_err();
            assert_eq!(s, a);
        }
        assert_eq!(sync_w.into_inner(), async_w.into_inner().inner);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_write_on_partial_failure_writes_nothing() {
        let schedule = FailureSchedule::on_attempts(&[1], FailureMode::PartialWrite);
        let mut w = AsyncChaosWriter::new(Vec::new(), schedule);
        let res = poll_fn(|cx| Pin::new(&mut w).poll_write(cx, b"")).await;
        assert_eq!(res.unwrap_err().kind(), io::ErrorKind::WriteZero);
        assert!(w.into_inner().is_empty());
    }
}
