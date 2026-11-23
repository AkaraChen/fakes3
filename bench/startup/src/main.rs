//! What a test pays for its own fakes3: start a server, make one request, stop it.
//!
//! Three ways of running a server are measured, each as a full cycle:
//!
//! - `rust_async`: `fakes3::Server::start` on the program's Tokio runtime, then `shutdown().await`.
//! - `rust_blocking`: `fakes3::Server::start_blocking` (its own runtime, as in a synchronous test),
//!   then `shutdown()`.
//! - `process`: the `fakes3` binary as a child process with `--port 0`; ready once it prints its
//!   address, stopped by killing it and waiting for it to exit.
//!
//! The request is a signed ListBuckets (fakes3 accepts any signature by default) on a new
//! connection. Sequential cycles run one after another; parallel rounds start N cycles at once and
//! time the whole round. Output is JSON (milliseconds), rendered by bench/startup/report.py.
//!
//!     cargo run --release -- --bin path/to/fakes3 --out result-rust.json

use std::net::SocketAddr;
use std::process::Stdio;
use std::time::Instant;

use futures::future::join_all;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Milliseconds spent starting, on the request, and stopping.
#[derive(Clone, Copy)]
struct Cycle {
    start: f64,
    request: f64,
    stop: f64,
}

impl Cycle {
    fn total(&self) -> f64 {
        self.start + self.request + self.stop
    }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn list_buckets(addr: SocketAddr) -> String {
    format!(
        "GET / HTTP/1.1\r\nHost: {addr}\r\n\
         Authorization: AWS4-HMAC-SHA256 Credential=fakes3/20260101/us-east-1/s3/aws4_request, SignedHeaders=host;x-amz-date, Signature=0000\r\n\
         x-amz-date: 20260101T000000Z\r\nx-amz-content-sha256: UNSIGNED-PAYLOAD\r\nConnection: close\r\n\r\n"
    )
}

fn check(resp: &[u8]) -> Res<()> {
    if resp.starts_with(b"HTTP/1.1 200") {
        Ok(())
    } else {
        let head = String::from_utf8_lossy(&resp[..resp.len().min(200)]).into_owned();
        Err(format!("unexpected response: {head}").into())
    }
}

async fn request(addr: SocketAddr) -> Res<()> {
    let mut s = tokio::net::TcpStream::connect(addr).await?;
    s.set_nodelay(true)?;
    s.write_all(list_buckets(addr).as_bytes()).await?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await?;
    check(&buf)
}

fn request_sync(addr: SocketAddr) -> Res<()> {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr)?;
    s.set_nodelay(true)?;
    s.write_all(list_buckets(addr).as_bytes())?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf)?;
    check(&buf)
}

async fn cycle_async() -> Res<Cycle> {
    let t = Instant::now();
    let server = fakes3::Server::start(fakes3::Config::default()).await?;
    let start = ms(t);
    let t = Instant::now();
    request(server.addr()).await?;
    let request = ms(t);
    let t = Instant::now();
    server.shutdown().await;
    Ok(Cycle {
        start,
        request,
        stop: ms(t),
    })
}

fn cycle_blocking() -> Res<Cycle> {
    let t = Instant::now();
    let server = fakes3::Server::start_blocking(fakes3::Config::default())?;
    let start = ms(t);
    let t = Instant::now();
    request_sync(server.addr())?;
    let request = ms(t);
    let t = Instant::now();
    server.shutdown();
    Ok(Cycle {
        start,
        request,
        stop: ms(t),
    })
}

async fn cycle_process(bin: String) -> Res<Cycle> {
    let t = Instant::now();
    let mut child = tokio::process::Command::new(&bin)
        .args(["--port", "0"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
    let line = lines.next_line().await?.ok_or("fakes3 exited before printing its address")?;
    let addr: SocketAddr = line.trim().trim_start_matches("http://").parse()?;
    let start = ms(t);
    let t = Instant::now();
    request(addr).await?;
    let request = ms(t);
    let t = Instant::now();
    child.kill().await?; // SIGKILL / TerminateProcess, then waits for the exit
    Ok(Cycle {
        start,
        request,
        stop: ms(t),
    })
}

#[derive(Clone, Copy)]
enum Mode {
    Async,
    Blocking,
    Process,
}

async fn one(mode: Mode, bin: &str) -> Res<Cycle> {
    match mode {
        Mode::Async => cycle_async().await,
        Mode::Blocking => tokio::task::spawn_blocking(cycle_blocking).await?,
        Mode::Process => cycle_process(bin.to_owned()).await,
    }
}

fn cycles_json(first: Cycle, cycles: &[Cycle]) -> Value {
    let col = |f: fn(&Cycle) -> f64| cycles.iter().map(f).collect::<Vec<_>>();
    json!({
        "first": {"start": first.start, "request": first.request, "stop": first.stop, "total": first.total()},
        "start": col(|c| c.start),
        "request": col(|c| c.request),
        "stop": col(|c| c.stop),
        "total": col(|c| c.total()),
    })
}

async fn sequential(mode: Mode, bin: &str, warmup: usize, n: usize) -> Res<Value> {
    let first = one(mode, bin).await?;
    for _ in 1..warmup {
        one(mode, bin).await?;
    }
    let mut cycles = Vec::with_capacity(n);
    for _ in 0..n {
        cycles.push(one(mode, bin).await?);
    }
    Ok(cycles_json(first, &cycles))
}

/// `n` cycles started at once, `rounds` times: the wall time of each round, and per round the
/// median and slowest single cycle.
async fn parallel(mode: Mode, bin: &str, n: usize, rounds: usize) -> Res<Value> {
    let mut out = Vec::new();
    for _ in 0..rounds {
        let t = Instant::now();
        let tasks = (0..n).map(|_| {
            let bin = bin.to_owned();
            tokio::spawn(async move { one(mode, &bin).await })
        });
        let mut totals = Vec::with_capacity(n);
        for r in join_all(tasks).await {
            totals.push(r??.total());
        }
        let wall = ms(t);
        totals.sort_by(f64::total_cmp);
        out.push(json!({"wall": wall, "median": totals[n / 2], "max": totals[n - 1]}));
    }
    Ok(Value::Array(out))
}

fn arg(args: &[String], name: &str, default: &str) -> String {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
        .unwrap_or_else(|| default.to_owned())
}

fn sizes(s: &str) -> Vec<usize> {
    s.split(',').filter(|x| !x.is_empty()).map(|x| x.parse().expect("size")).collect()
}

#[tokio::main]
async fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().collect();
    let bin = arg(&args, "--bin", "");
    let out = arg(&args, "--out", "");
    let warmup: usize = arg(&args, "--warmup", "10").parse()?;
    let n: usize = arg(&args, "--cycles", "200").parse()?;
    let rounds: usize = arg(&args, "--parallel-rounds", "5").parse()?;
    let par = sizes(&arg(&args, "--parallel", "100,500"));
    let par_process = sizes(&arg(&args, "--process-parallel", "100,250"));

    let mut modes = vec![("rust_async", Mode::Async, par.clone()), ("rust_blocking", Mode::Blocking, vec![])];
    if !bin.is_empty() {
        modes.push(("process", Mode::Process, par_process));
    }
    let mut result = serde_json::Map::new();
    for (name, mode, sizes) in modes {
        let mut r = serde_json::Map::new();
        r.insert("sequential".into(), sequential(mode, &bin, warmup, n).await?);
        eprintln!("[{name}] sequential done");
        let mut p = serde_json::Map::new();
        for size in sizes {
            p.insert(size.to_string(), parallel(mode, &bin, size, rounds).await?);
            eprintln!("[{name}] parallel {size} done");
        }
        r.insert("parallel".into(), Value::Object(p));
        result.insert(name.into(), Value::Object(r));
    }
    let text = serde_json::to_string_pretty(&Value::Object(result))?;
    if out.is_empty() {
        println!("{text}");
    } else {
        std::fs::write(out, text)?;
    }
    Ok(())
}
