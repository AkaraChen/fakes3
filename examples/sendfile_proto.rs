//! Prototype for estimating what `sendfile(2)` could save on large GETs.
//!
//! Serves memfd-backed 64 MiB objects over loopback TCP with a trivial length-prefixed protocol,
//! comparing `write(2)` from a read-only mmap (what hyper does today) against `sendfile(2)` from
//! the memfd. The client is a separate process so server and client CPU can be told apart.
//!
//!     cargo run --release --example sendfile_proto -- [--size-mib 64] [--objects 16] [--conns 4] [--secs 4] [--rounds 3]

use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::AsFd;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use memmap2::Mmap;

struct Blob {
    file: File,
    map: Mmap,
}

fn make_blob(size: usize, seed: u8) -> Blob {
    let fd = rustix::fs::memfd_create("proto", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = File::from(fd);
    let chunk: Vec<u8> = (0..1 << 20).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect();
    for _ in 0..size >> 20 {
        file.write_all(&chunk).unwrap();
    }
    let map = unsafe { Mmap::map(&file).unwrap() };
    Blob { file, map }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// One `write` of the whole mapping, as a blocking socket allows.
    Write,
    /// `write` in 256 KiB pieces.
    Write256k,
    Sendfile,
    /// `sendfile` in 256 KiB pieces.
    Sendfile256k,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Write => "write_mmap",
            Mode::Write256k => "write_mmap_256k",
            Mode::Sendfile => "sendfile",
            Mode::Sendfile256k => "sendfile_256k",
        }
    }
}

fn serve_conn(mut s: TcpStream, blobs: Arc<Vec<Blob>>, mode: Mode) {
    s.set_nodelay(true).unwrap();
    let mut req = [0u8; 8];
    while s.read_exact(&mut req).is_ok() {
        let b = &blobs[u64::from_le_bytes(req) as usize % blobs.len()];
        let len = b.map.len();
        if s.write_all(&(len as u64).to_le_bytes()).is_err() {
            return;
        }
        let r = match mode {
            Mode::Write => s.write_all(&b.map[..]),
            Mode::Write256k => b.map.chunks(256 << 10).try_for_each(|c| s.write_all(c)),
            Mode::Sendfile | Mode::Sendfile256k => {
                let piece = if mode == Mode::Sendfile { usize::MAX } else { 256 << 10 };
                let mut off = 0u64;
                let mut r = Ok(());
                while (off as usize) < len {
                    let left = (len - off as usize).min(piece);
                    match rustix::fs::sendfile(s.as_fd(), b.file.as_fd(), Some(&mut off), left) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(e) => {
                            r = Err(e.into());
                            break;
                        }
                    }
                }
                r
            }
        };
        if r.is_err() {
            return;
        }
    }
}

fn cpu_secs() -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let rest: Vec<&str> = stat[stat.rfind(')').unwrap() + 2..].split(' ').collect();
    (rest[11].parse::<f64>().unwrap() + rest[12].parse::<f64>().unwrap()) / 100.0
}

fn arg<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// `client <addr> <conns> <secs> <read-buf-bytes> <objects>`: prints `bytes cpu_secs`.
fn client(args: &[String]) {
    let addr = args[0].clone();
    let conns: usize = args[1].parse().unwrap();
    let secs: f64 = args[2].parse().unwrap();
    let rbuf: usize = args[3].parse().unwrap();
    let objects: u64 = args[4].parse().unwrap();
    let total = Arc::new(AtomicU64::new(0));
    let deadline = Instant::now() + Duration::from_secs_f64(secs);
    let cpu0 = cpu_secs();
    let hs: Vec<_> = (0..conns)
        .map(|c| {
            let addr = addr.clone();
            let total = total.clone();
            std::thread::spawn(move || {
                let mut s = TcpStream::connect(addr).unwrap();
                s.set_nodelay(true).unwrap();
                let mut buf = vec![0u8; rbuf];
                let mut i = c as u64;
                while Instant::now() < deadline {
                    s.write_all(&(i % objects).to_le_bytes()).unwrap();
                    i += 1;
                    let mut len = [0u8; 8];
                    s.read_exact(&mut len).unwrap();
                    let mut left = u64::from_le_bytes(len) as usize;
                    while left > 0 {
                        let n = s.read(&mut buf[..left.min(rbuf)]).unwrap();
                        assert!(n > 0);
                        left -= n;
                        total.fetch_add(n as u64, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    println!("{} {}", total.load(Ordering::Relaxed), cpu_secs() - cpu0);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("client") {
        return client(&args[1..]);
    }
    let size = arg(&args, "--size-mib", 64usize) << 20;
    let objects = arg(&args, "--objects", 16usize);
    let conns = arg(&args, "--conns", 4usize);
    let secs = arg(&args, "--secs", 4.0f64);
    let rounds = arg(&args, "--rounds", 3usize);
    let blobs = Arc::new((0..objects).map(|i| make_blob(size, i as u8)).collect::<Vec<_>>());
    let exe = std::env::current_exe().unwrap();
    println!(
        "{objects} x {} MiB objects, {conns} connections, {secs}s per run, {} cores",
        size >> 20,
        std::thread::available_parallelism().unwrap()
    );
    println!(
        "{:<18} {:>8} {:>10} {:>10} {:>10} {:>12}",
        "mode", "rbuf", "GB/s", "srv cpu s", "cli cpu s", "srv cpu/GB"
    );
    let mut summary: Vec<(String, f64)> = Vec::new();
    for round in 0..rounds {
        for rbuf in [64usize << 10, 256 << 10] {
            for mode in [Mode::Write, Mode::Write256k, Mode::Sendfile, Mode::Sendfile256k] {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let addr = listener.local_addr().unwrap();
                let stop = Arc::new(AtomicBool::new(false));
                let acceptor = {
                    let (blobs, stop) = (blobs.clone(), stop.clone());
                    std::thread::spawn(move || {
                        for s in listener.incoming() {
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                            let blobs = blobs.clone();
                            std::thread::spawn(move || serve_conn(s.unwrap(), blobs, mode));
                        }
                    })
                };
                let cpu0 = cpu_secs();
                let t0 = Instant::now();
                let out = Command::new(&exe)
                    .args([
                        "client",
                        &addr.to_string(),
                        &conns.to_string(),
                        &secs.to_string(),
                        &rbuf.to_string(),
                        &objects.to_string(),
                    ])
                    .stdout(Stdio::piped())
                    .output()
                    .unwrap();
                let wall = t0.elapsed().as_secs_f64();
                let srv_cpu = cpu_secs() - cpu0;
                stop.store(true, Ordering::Relaxed);
                let _ = TcpStream::connect(addr);
                let _ = acceptor.join();
                let text = String::from_utf8_lossy(&out.stdout);
                let mut it = text.split_whitespace();
                let bytes: f64 = it.next().unwrap().parse().unwrap();
                let cli_cpu: f64 = it.next().unwrap().parse().unwrap();
                let gbps = bytes / wall / 1e9;
                println!(
                    "{:<18} {:>8} {:>10.2} {:>10.2} {:>10.2} {:>12.3}  (round {round})",
                    mode.name(),
                    format!("{}K", rbuf >> 10),
                    gbps,
                    srv_cpu,
                    cli_cpu,
                    srv_cpu / (bytes / 1e9)
                );
                summary.push((format!("{} rbuf={}K", mode.name(), rbuf >> 10), gbps));
            }
        }
    }
    println!("\nmedian GB/s");
    let mut keys: Vec<String> = summary.iter().map(|(k, _)| k.clone()).collect();
    keys.dedup();
    keys.sort();
    keys.dedup();
    for k in keys {
        let mut v: Vec<f64> = summary.iter().filter(|(n, _)| *n == k).map(|(_, g)| *g).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("{k:<30} {:.2}", v[v.len() / 2]);
    }
}
