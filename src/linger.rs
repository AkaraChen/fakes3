//! Lingering close for client connections.
//!
//! A connection can end with a request body only partly read: an error answered before the body
//! was needed (an invalid header, a missing bucket, a failed precondition) closes it. A socket
//! closed with unread data, or receiving more after it closed, answers with a reset, and on macOS
//! and Windows a reset makes the client drop the response it has not read yet: the client sees
//! `connection reset` instead of the S3 error. So, like nginx's `lingering_close`, closing sends FIN
//! first and then reads and discards whatever the client still sends, until it closes its end or
//! [`LINGER`] passes.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::Sleep;

/// How long a closing connection keeps reading what the client still sends.
const LINGER: Duration = Duration::from_secs(2);

/// A client connection whose shutdown lingers; reads and writes go straight to the socket.
pub struct Lingering {
    io: TcpStream,
    /// Set once the write half is shut down.
    deadline: Option<Pin<Box<Sleep>>>,
}

impl Lingering {
    pub fn new(io: TcpStream) -> Self {
        Self { io, deadline: None }
    }
}

impl AsyncRead for Lingering {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for Lingering {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_write_vectored(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.deadline.is_none() {
            ready!(Pin::new(&mut this.io).poll_shutdown(cx))?;
            this.deadline = Some(Box::pin(tokio::time::sleep(LINGER)));
        }
        let deadline = this.deadline.as_mut().expect("set above");
        let mut buf = [0; 16 << 10];
        loop {
            if deadline.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Ok(()));
            }
            let mut rb = ReadBuf::new(&mut buf);
            match ready!(Pin::new(&mut this.io).poll_read(cx, &mut rb)) {
                Ok(()) if !rb.filled().is_empty() => {}
                // The client closed its end (or reset it): nothing more will arrive.
                _ => return Poll::Ready(Ok(())),
            }
        }
    }
}
