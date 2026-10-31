//! Zero-copy sends of memfd-backed bodies.
//!
//! hyper writes a response body by handing the socket the body's bytes, which for a large object
//! are a slice of a read-only mapping of the object's memfd. Copying out of the mapping costs a
//! full pass over the data in the server. Instead, [`Io`] recognises writes whose source lies
//! inside such a mapping and sends that range with `sendfile(2)` from the memfd, so the kernel
//! hands the page-cache pages to the socket without a user-space copy.
//!
//! The substitution is by address, below the HTTP layer: hyper still builds the status line,
//! headers, `Content-Length`/chunk framing, keep-alive and pipelining, and s3s still decides what
//! the body is (ranges, `partNumber`, multi-segment objects). Anything that does not match, such
//! as heap-backed segments, small slices, or HTTP/2 frames, goes through the ordinary write path.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, IoSlice};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::task::{Context, Poll, ready};

use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::rt::TokioIo;
use memmap2::Mmap;
use parking_lot::RwLock;
use tokio::io::Interest;
use tokio::net::TcpStream;

/// Slices shorter than this are cheaper to copy than to hand to `sendfile`.
const MIN_LEN: usize = 128 << 10;

/// start address -> (length, memfd) of every live mapping that can be sent with `sendfile`.
static REGIONS: RwLock<BTreeMap<usize, (usize, RawFd)>> = RwLock::new(BTreeMap::new());

/// Cleared process-wide if the kernel (or a seccomp filter) turns out not to support `sendfile` on sockets.
static ENABLED: AtomicBool = AtomicBool::new(true);

static WRITE_CHUNK: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
static CHUNK: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

/// Bytes sent with `sendfile` so far, for tests.
static SENT: AtomicU64 = AtomicU64::new(0);

/// File descriptors we may still keep open for registered memfds. Negative until first use.
static FD_BUDGET: AtomicIsize = AtomicIsize::new(isize::MIN);

#[doc(hidden)]
pub fn sent_bytes() -> u64 {
    SENT.load(Ordering::Relaxed)
}

/// A read-only mapping of an immutable memfd that keeps the memfd open and registered, so writes
/// from the mapping can be turned into `sendfile` calls. Used as the owner of the blob's `Bytes`.
pub struct Mapped {
    map: Mmap,
    /// Present only while the range is registered; see [`take_fd_slot`].
    file: Option<File>,
}

impl Mapped {
    pub fn new(file: File, map: Mmap) -> Self {
        let file = take_fd_slot().then_some(file);
        if let Some(f) = &file {
            REGIONS.write().insert(map.as_ptr() as usize, (map.len(), f.as_raw_fd()));
        }
        Self { map, file }
    }
}

impl AsRef<[u8]> for Mapped {
    fn as_ref(&self) -> &[u8] {
        &self.map
    }
}

impl Drop for Mapped {
    fn drop(&mut self) {
        if self.file.is_some() {
            REGIONS.write().remove(&(self.map.as_ptr() as usize));
            FD_BUDGET.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Keeping each large blob's memfd open costs a descriptor, so only do it while well under the
/// process limit; past that, blobs are still served, just by copying from the mapping.
fn take_fd_slot() -> bool {
    let _ = FD_BUDGET.compare_exchange(isize::MIN, fd_budget(), Ordering::Relaxed, Ordering::Relaxed);
    FD_BUDGET.fetch_sub(1, Ordering::Relaxed) > 0 || {
        FD_BUDGET.fetch_add(1, Ordering::Relaxed);
        false
    }
}

fn fd_budget() -> isize {
    let soft = rustix::process::getrlimit(rustix::process::Resource::Nofile).current;
    (soft.unwrap_or(1024) / 2).min(isize::MAX as u64 / 2) as isize
}

/// Raises the soft open-files limit to the hard limit so more large objects can keep their memfd.
pub fn raise_fd_limit() {
    let mut lim = rustix::process::getrlimit(rustix::process::Resource::Nofile);
    if lim.current != lim.maximum {
        lim.current = lim.maximum;
        let _ = rustix::process::setrlimit(rustix::process::Resource::Nofile, lim);
    }
}

/// The memfd and file offset behind `buf`, if it lies inside a registered mapping.
fn locate(buf: &[u8]) -> Option<(RawFd, u64)> {
    if buf.len() < MIN_LEN {
        return None;
    }
    let addr = buf.as_ptr() as usize;
    let regions = REGIONS.read();
    let (&start, &(len, fd)) = regions.range(..=addr).next_back()?;
    (addr + buf.len() <= start + len).then_some((fd, (addr - start) as u64))
}

/// A TCP connection whose large memfd-backed writes go out with `sendfile`.
pub struct Io {
    io: TokioIo<TcpStream>,
    enabled: bool,
}

impl Io {
    /// With `enabled == false` this is a plain connection.
    pub fn new(socket: TcpStream, enabled: bool) -> Self {
        Self {
            io: TokioIo::new(socket),
            enabled,
        }
    }

    fn tcp(&self) -> &TcpStream {
        self.io.inner()
    }

    /// Sends `bufs` ahead of a `sendfile`, letting the kernel hold them back so they share a
    /// packet with the start of the body.
    fn poll_send_more(&self, cx: &mut Context<'_>, bufs: &[IoSlice<'_>]) -> Poll<io::Result<usize>> {
        let tcp = self.tcp();
        loop {
            ready!(tcp.poll_write_ready(cx))?;
            let r = tcp.try_io(Interest::WRITABLE, || {
                let flags = rustix::net::SendFlags::MORE | rustix::net::SendFlags::NOSIGNAL;
                rustix::net::sendmsg(tcp, bufs, &mut rustix::net::SendAncillaryBuffer::default(), flags).map_err(io::Error::from)
            });
            match r {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                r => return Poll::Ready(r),
            }
        }
    }

    fn poll_sendfile(&self, cx: &mut Context<'_>, fd: RawFd, offset: u64, len: usize) -> Poll<io::Result<usize>> {
        let tcp = self.tcp();
        // SAFETY: `fd` belongs to a registered `Mapped`, and `len` bytes of that mapping are held
        // alive by the `Bytes` hyper is writing, so the memfd is still open.
        let file = unsafe { BorrowedFd::borrow_raw(fd) };
        loop {
            ready!(tcp.poll_write_ready(cx))?;
            let mut off = offset;
            let r = tcp.try_io(Interest::WRITABLE, || {
                rustix::fs::sendfile(tcp.as_fd(), file, Some(&mut off), len).map_err(io::Error::from)
            });
            match r {
                Ok(0) => return Poll::Ready(Err(io::ErrorKind::WriteZero.into())),
                Ok(n) => {
                    SENT.fetch_add(n as u64, Ordering::Relaxed);
                    return Poll::Ready(Ok(n));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }
}

impl Read for Io {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: ReadBufCursor<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl Write for Io {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_write_vectored(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[IoSlice<'_>]) -> Poll<io::Result<usize>> {
        let target = (self.enabled && ENABLED.load(Ordering::Relaxed))
            .then(|| bufs.iter().enumerate().find_map(|(i, b)| locate(b).map(|(fd, off)| (i, fd, off))))
            .flatten();
        let Some((i, fd, off)) = target else {
            let wc = *WRITE_CHUNK.get_or_init(|| {
                std::env::var("FAKES3_WRITE_CHUNK")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .filter(|&n: &usize| n > 0)
                    .unwrap_or(usize::MAX)
            });
            if bufs.iter().map(|b| b.len()).sum::<usize>() > wc && bufs[0].len() > wc {
                return Pin::new(&mut self.io).poll_write(cx, &bufs[0][..wc]);
            }
            return Pin::new(&mut self.io).poll_write_vectored(cx, bufs);
        };
        if i > 0 {
            return self.poll_send_more(cx, &bufs[..i]);
        }
        let max = *CHUNK.get_or_init(|| {
            std::env::var("FAKES3_SENDFILE_CHUNK")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|&n: &usize| n > 0)
                .unwrap_or(usize::MAX)
        });
        match ready!(self.poll_sendfile(cx, fd, off, bufs[0].len().min(max))) {
            // No sendfile for this socket or kernel: stop trying and copy instead.
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(libc_errno::INVAL | libc_errno::NOSYS | libc_errno::PERM | libc_errno::OPNOTSUPP)
                ) =>
            {
                ENABLED.store(false, Ordering::Relaxed);
                Pin::new(&mut self.io).poll_write_vectored(cx, bufs)
            }
            r => Poll::Ready(r),
        }
    }
}

mod libc_errno {
    use rustix::io::Errno;
    pub const INVAL: i32 = Errno::INVAL.raw_os_error();
    pub const NOSYS: i32 = Errno::NOSYS.raw_os_error();
    pub const PERM: i32 = Errno::PERM.raw_os_error();
    pub const OPNOTSUPP: i32 = Errno::OPNOTSUPP.raw_os_error();
}
