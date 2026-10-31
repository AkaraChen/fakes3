//! MD5, the one digest S3 forces on every byte written (it is the ETag).
//!
//! OpenSSL's hand-scheduled assembly is about 25% faster than the pure-Rust `md-5` crate (and
//! than `md5-asm`). It is built from source by `openssl-sys` (`vendored`) so the static musl
//! image keeps working, and called through the low-level `MD5_*` functions rather than EVP:
//! EVP's provider tables would pull most of libcrypto into the binary, MD5_* only the MD5 code.
//! For large bodies, [`Pipelined`] moves hashing to a blocking thread so it overlaps with
//! receiving, checksumming and storing the data.

use std::ffi::{c_int, c_uchar, c_void};

use bytes::Bytes;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// Links the vendored libcrypto that provides the symbols below.
use openssl_sys as _;

/// `MD5_CTX` is 92 bytes in OpenSSL 3; this is larger and suitably aligned.
#[repr(C, align(8))]
struct Md5Ctx([u8; 128]);

unsafe extern "C" {
    fn MD5_Init(c: *mut Md5Ctx) -> c_int;
    fn MD5_Update(c: *mut Md5Ctx, data: *const c_void, len: usize) -> c_int;
    fn MD5_Final(md: *mut c_uchar, c: *mut Md5Ctx) -> c_int;
}

pub struct Md5(Box<Md5Ctx>);

impl Md5 {
    pub fn new() -> Self {
        let mut ctx = Box::new(Md5Ctx([0; 128]));
        // SAFETY: ctx is a writable buffer at least as large as MD5_CTX.
        unsafe { MD5_Init(&mut *ctx) };
        Self(ctx)
    }

    pub fn update(&mut self, data: &[u8]) {
        // SAFETY: ctx was initialized by MD5_Init; data is a valid slice.
        unsafe { MD5_Update(&mut *self.0, data.as_ptr().cast(), data.len()) };
    }

    pub fn finish(mut self) -> [u8; 16] {
        let mut out = [0u8; 16];
        // SAFETY: out has room for the 16-byte digest; ctx was initialized by MD5_Init.
        unsafe { MD5_Final(out.as_mut_ptr(), &mut *self.0) };
        out
    }
}

/// MD5 over a sequence of buffers.
pub fn md5<B: AsRef<[u8]>>(parts: impl IntoIterator<Item = B>) -> [u8; 16] {
    let mut h = Md5::new();
    for p in parts {
        h.update(p.as_ref());
    }
    h.finish()
}

/// MD5 computed on a blocking thread from chunks sent to it. Chunks are reference-counted
/// `Bytes`, so handing them over copies nothing; the bounded queue keeps memory in check if
/// hashing falls behind the network.
pub struct Pipelined {
    tx: mpsc::Sender<Bytes>,
    worker: JoinHandle<[u8; 16]>,
}

impl Pipelined {
    pub fn spawn() -> Self {
        let (tx, mut rx) = mpsc::channel::<Bytes>(64);
        let worker = tokio::task::spawn_blocking(move || {
            let mut h = Md5::new();
            while let Some(chunk) = rx.blocking_recv() {
                h.update(&chunk);
            }
            h.finish()
        });
        Self { tx, worker }
    }

    pub async fn update(&self, chunk: Bytes) {
        // The worker only stops after the sender is dropped, so this cannot fail.
        let _ = self.tx.send(chunk).await;
    }

    pub async fn finish(self) -> [u8; 16] {
        drop(self.tx);
        self.worker.await.expect("MD5 worker does not panic")
    }
}

/// Inline for small bodies (a thread hop costs more than hashing them), pipelined for large.
pub enum Streaming {
    Inline(Md5),
    Pipelined(Pipelined),
}

impl Streaming {
    pub fn new(pipelined: bool) -> Self {
        if pipelined {
            Self::Pipelined(Pipelined::spawn())
        } else {
            Self::Inline(Md5::new())
        }
    }

    pub async fn update(&mut self, chunk: &Bytes) {
        match self {
            Self::Inline(h) => h.update(chunk),
            Self::Pipelined(p) => p.update(chunk.clone()).await,
        }
    }

    pub async fn finish(self) -> [u8; 16] {
        match self {
            Self::Inline(h) => h.finish(),
            Self::Pipelined(p) => p.finish().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(hex::encode(md5([b""])), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex::encode(md5([b"abc"])), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hex::encode(md5([&b"a"[..], b"b", b"c"])), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pipelined_matches_inline() {
        let data: Vec<u8> = (0..5_000_000u32).map(|i| i.wrapping_mul(2_654_435_761) as u8).collect();
        let data = Bytes::from(data);
        let mut p = Streaming::new(true);
        for chunk in data.chunks(65_536) {
            p.update(&data.slice_ref(chunk)).await;
        }
        assert_eq!(p.finish().await, md5([&data]));
    }
}
