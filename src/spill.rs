//! Where a body goes once it reaches the inline threshold (see [`crate::store::BlobWriter`]).
//!
//! On Linux it is written into a `memfd`: the kernel fills tmpfs pages straight from `write`, so
//! the body never page-faults its way into a heap buffer, and the finished blob is served from a
//! read-only mapping that frees the pages when the last `Bytes` of it drops. Other systems have no
//! memfd (an unlinked temporary file would be backed by disk), so there large bodies stay on the
//! heap like small ones: one exact-size allocation when the length is known up front.

use bytes::Bytes;

use crate::memory::{Reservation, Tracked};

#[cfg(target_os = "linux")]
pub use memfd::Spill;

#[cfg(not(target_os = "linux"))]
pub use heap::Spill;

#[cfg(target_os = "linux")]
mod memfd {
    use std::fs::File;
    use std::io::{self, Write};

    use super::*;

    pub struct Spill(File);

    impl Spill {
        pub fn with_capacity(n: u64) -> io::Result<Self> {
            use rustix::fs::{FallocateFlags, MemfdFlags, fallocate, memfd_create};
            let f = File::from(memfd_create("fakes3-blob", MemfdFlags::CLOEXEC)?);
            if n > 0 {
                // Reserve pages up front so writes don't grow the file piecemeal.
                let _ = fallocate(&f, FallocateFlags::KEEP_SIZE, 0, n);
            }
            Ok(Self(f))
        }

        /// Takes over a body that was buffered on the heap until it crossed the threshold.
        pub fn from_vec(buffered: Vec<u8>) -> io::Result<Self> {
            let mut s = Self::with_capacity(0)?;
            s.write(&buffered)?;
            Ok(s)
        }

        pub fn write(&mut self, chunk: &[u8]) -> io::Result<()> {
            self.0.write_all(chunk)
        }

        /// The blob, non-empty. `reservation` pays for it until the last `Bytes` of it drops.
        pub fn finish(self, reservation: Reservation) -> io::Result<Bytes> {
            // SAFETY: the memfd is private to this process and never written again.
            let map = unsafe { memmap2::Mmap::map(&self.0)? };
            Ok(Bytes::from_owner(Tracked::new(map, reservation)))
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod heap {
    use std::io;

    use super::*;

    pub struct Spill(Vec<u8>);

    impl Spill {
        pub fn with_capacity(n: u64) -> io::Result<Self> {
            Ok(Self(Vec::with_capacity(n as usize)))
        }

        /// Takes over a body that was buffered on the heap until it crossed the threshold.
        pub fn from_vec(buffered: Vec<u8>) -> io::Result<Self> {
            Ok(Self(buffered))
        }

        pub fn write(&mut self, chunk: &[u8]) -> io::Result<()> {
            self.0.extend_from_slice(chunk);
            Ok(())
        }

        /// The blob, non-empty. `reservation` pays for it until the last `Bytes` of it drops.
        pub fn finish(mut self, reservation: Reservation) -> io::Result<Bytes> {
            self.0.shrink_to_fit();
            Ok(Bytes::from_owner(Tracked::new(self.0, reservation)))
        }
    }
}
