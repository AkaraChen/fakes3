//! `fakes3 ctl`: a command-line client for the control-plane HTTP API (docs/control-plane.md).

use std::process::ExitCode;

use bytes::Bytes;
use clap::{Args, Subcommand};
use http_body_util::{BodyExt, Full};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpStream;

#[derive(Args)]
pub struct Ctl {
    /// Server to control.
    #[arg(long, global = true, default_value = "http://127.0.0.1:9000", env = "FAKES3_ENDPOINT")]
    endpoint: String,

    /// Print the server's JSON response instead of formatted text.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show clock, store, snapshot and request-log state.
    Status,
    /// Show memory use against the limit, and how much data the store holds.
    Stats,
    /// Delete all buckets, objects and uploads, and clear the request log and the clock.
    Reset {
        /// Leave the clock as it is.
        #[arg(long)]
        keep_clock: bool,
        /// Leave the request log as it is.
        #[arg(long)]
        keep_requests: bool,
        /// Also delete all snapshots.
        #[arg(long)]
        drop_snapshots: bool,
        /// Leave the fault rules in place.
        #[arg(long)]
        keep_faults: bool,
    },
    /// Save and roll back the store.
    #[command(subcommand)]
    Snapshot(Snapshot),
    /// Read or move the server's time.
    #[command(subcommand)]
    Clock(Clock),
    /// Inspect the S3 request log.
    #[command(subcommand)]
    Requests(Requests),
    /// Inject errors, delays, truncated bodies and connection resets into S3 requests.
    #[command(subcommand)]
    Faults(Faults),
}

// Parsed once per process; the size of the `add` variant does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Faults {
    /// Add a rule. Give one of --error, --status, --truncate or --reset, and/or --delay.
    Add {
        /// Only this S3 operation, e.g. GetObject (repeat for several).
        #[arg(long = "op")]
        ops: Vec<String>,
        #[arg(long)]
        bucket: Option<String>,
        /// Only keys starting with this prefix.
        #[arg(long)]
        key_prefix: Option<String>,
        /// Chance from 0 to 1 that a matching request is hit (default 1).
        #[arg(long)]
        probability: Option<f64>,
        /// Seed for the probability draws and random delays.
        #[arg(long)]
        seed: Option<u64>,
        /// Hit at most N requests.
        #[arg(long)]
        times: Option<u64>,
        /// Let the first N matching requests through.
        #[arg(long)]
        skip: Option<u64>,
        /// Wait before answering: a duration such as 500ms, or a range such as 100ms..2s.
        #[arg(long)]
        delay: Option<String>,
        /// Answer with this S3 error code, e.g. SlowDown or InternalError.
        #[arg(long)]
        error: Option<String>,
        /// HTTP status of the error (SlowDown is 503 and InternalError 500 by default).
        #[arg(long)]
        status: Option<u16>,
        /// Error message (default: S3's message for the code).
        #[arg(long)]
        message: Option<String>,
        /// Send only the first N bytes of the response body, then close the connection.
        #[arg(long)]
        truncate: Option<u64>,
        /// Reset the connection instead of answering.
        #[arg(long)]
        reset: bool,
    },
    /// List the rules with their counters.
    List,
    /// Remove one rule.
    Delete { id: u64 },
    /// Remove all rules.
    Clear,
}

#[derive(Subcommand)]
enum Snapshot {
    /// Copy the current store into a snapshot.
    Create {
        /// Snapshot name (default: snap-<n>).
        name: Option<String>,
    },
    /// List snapshots.
    List,
    /// Roll the store back to a snapshot.
    Restore { name: String },
    /// Delete a snapshot.
    Delete { name: String },
}

#[derive(Subcommand)]
enum Clock {
    /// Show the server time.
    Get,
    /// Stop time, at the given RFC 3339 time or at the current server time.
    Freeze { at: Option<String> },
    /// Let time run again from the current server time.
    Unfreeze,
    /// Jump to an RFC 3339 time, keeping the frozen or running state.
    Set { at: String },
    /// Move time by a duration such as 30d, 1h30m, 500ms or -2w.
    Advance {
        #[arg(allow_hyphen_values = true)]
        by: String,
    },
    /// Go back to the host clock.
    Reset,
}

#[derive(Subcommand)]
enum Requests {
    /// List logged requests, oldest first.
    List {
        #[arg(long)]
        bucket: Option<String>,
        /// Only keys starting with this prefix.
        #[arg(long)]
        key_prefix: Option<String>,
        /// S3 operation name, e.g. PutObject.
        #[arg(long)]
        op: Option<String>,
        #[arg(long)]
        method: Option<String>,
        /// 404, 4xx, 5xx or error (400 and above).
        #[arg(long)]
        status: Option<String>,
        /// Only requests at or after this RFC 3339 time.
        #[arg(long)]
        since: Option<String>,
        /// Only requests with a sequence number greater than this.
        #[arg(long)]
        after: Option<u64>,
        /// Keep only the most recent N matches.
        #[arg(long)]
        limit: Option<usize>,
        /// Print only the number of matches.
        #[arg(long)]
        count: bool,
    },
    /// Empty the request log.
    Clear,
    /// Show or change the log settings.
    Config {
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        #[arg(long)]
        disable: bool,
        /// Number of requests kept.
        #[arg(long)]
        capacity: Option<usize>,
    },
}

pub async fn run(ctl: Ctl) -> ExitCode {
    match execute(&ctl).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fakes3 ctl: {e}");
            ExitCode::FAILURE
        }
    }
}

type Result<T> = std::result::Result<T, String>;

async fn execute(ctl: &Ctl) -> Result<()> {
    let api = Api::new(&ctl.endpoint)?;
    let (method, path, body) = match &ctl.command {
        Command::Status => ("GET", "/_fakes3".to_owned(), None),
        Command::Stats => ("GET", "/_fakes3/stats".to_owned(), None),
        Command::Reset {
            keep_clock,
            keep_requests,
            drop_snapshots,
            keep_faults,
        } => (
            "POST",
            format!(
                "/_fakes3/reset?clock={}&requests={}&snapshots={drop_snapshots}&faults={}",
                !keep_clock, !keep_requests, !keep_faults
            ),
            None,
        ),
        Command::Faults(f) => match f {
            Faults::Add { .. } => ("POST", "/_fakes3/faults".into(), Some(fault_body(f)?)),
            Faults::List => ("GET", "/_fakes3/faults".into(), None),
            Faults::Delete { id } => ("DELETE", format!("/_fakes3/faults/{id}"), None),
            Faults::Clear => ("DELETE", "/_fakes3/faults".into(), None),
        },
        Command::Snapshot(s) => match s {
            Snapshot::Create { name } => ("POST", "/_fakes3/snapshots".into(), Some(json!({ "name": name }))),
            Snapshot::List => ("GET", "/_fakes3/snapshots".into(), None),
            Snapshot::Restore { name } => ("POST", format!("/_fakes3/snapshots/{}/restore", enc(name)), None),
            Snapshot::Delete { name } => ("DELETE", format!("/_fakes3/snapshots/{}", enc(name)), None),
        },
        Command::Clock(c) => match c {
            Clock::Get => ("GET", "/_fakes3/clock".into(), None),
            Clock::Freeze { at } => ("POST", "/_fakes3/clock/freeze".into(), Some(json!({ "at": at }))),
            Clock::Unfreeze => ("POST", "/_fakes3/clock/unfreeze".into(), None),
            Clock::Set { at } => ("POST", "/_fakes3/clock/set".into(), Some(json!({ "at": at }))),
            Clock::Advance { by } => ("POST", "/_fakes3/clock/advance".into(), Some(json!({ "by": by }))),
            Clock::Reset => ("POST", "/_fakes3/clock/reset".into(), None),
        },
        Command::Requests(r) => match r {
            Requests::List {
                bucket,
                key_prefix,
                op,
                method,
                status,
                since,
                after,
                limit,
                ..
            } => {
                let mut q = form_urlencoded::Serializer::new(String::new());
                let strs = [
                    ("bucket", bucket),
                    ("key_prefix", key_prefix),
                    ("op", op),
                    ("method", method),
                    ("status", status),
                    ("since", since),
                ];
                for (k, v) in strs {
                    if let Some(v) = v {
                        q.append_pair(k, v);
                    }
                }
                if let Some(a) = after {
                    q.append_pair("after", &a.to_string());
                }
                if let Some(l) = limit {
                    q.append_pair("limit", &l.to_string());
                }
                ("GET", format!("/_fakes3/requests?{}", q.finish()), None)
            }
            Requests::Clear => ("DELETE", "/_fakes3/requests".into(), None),
            Requests::Config { enable, disable, capacity } => {
                if !enable && !disable && capacity.is_none() {
                    ("GET", "/_fakes3/requests/config".into(), None)
                } else {
                    let mut b = serde_json::Map::new();
                    if *enable || *disable {
                        b.insert("enabled".into(), json!(*enable));
                    }
                    if let Some(c) = capacity {
                        b.insert("capacity".into(), json!(c));
                    }
                    ("PUT", "/_fakes3/requests/config".into(), Some(Value::Object(b)))
                }
            }
        },
    };
    let resp = api.call(method, &path, body).await?;
    if ctl.json {
        println!("{}", serde_json::to_string_pretty(&resp).unwrap_or_default());
    } else {
        print_human(&ctl.command, &resp);
    }
    Ok(())
}

/// The JSON body of `faults add`.
fn fault_body(f: &Faults) -> Result<Value> {
    let Faults::Add {
        ops,
        bucket,
        key_prefix,
        probability,
        seed,
        times,
        skip,
        delay,
        error,
        status,
        message,
        truncate,
        reset,
    } = f
    else {
        unreachable!("only called for faults add")
    };
    let mut b = serde_json::Map::new();
    let mut put = |k: &str, v: Value| {
        if !v.is_null() {
            b.insert(k.into(), v);
        }
    };
    put("operation", if ops.is_empty() { Value::Null } else { json!(ops) });
    put("bucket", json!(bucket));
    put("key_prefix", json!(key_prefix));
    put("probability", json!(probability));
    put("seed", json!(seed));
    put("times", json!(times));
    put("skip", json!(skip));
    put(
        "delay",
        match delay.as_deref().map(|d| d.split_once("..")) {
            None => Value::Null,
            Some(Some((min, max))) => json!({ "min": min, "max": max }),
            Some(None) => json!(delay),
        },
    );
    if error.is_some() || status.is_some() || message.is_some() {
        put("error", json!({ "code": error, "status": status, "message": message }));
    }
    put("truncate", json!(truncate));
    if *reset {
        put("reset", json!(true));
    }
    Ok(Value::Object(b))
}

fn print_fault(f: &Value) {
    let mut what = Vec::new();
    if !f["delay"].is_null() {
        let d = &f["delay"];
        what.push(if d.is_object() {
            format!("delay {}..{} ms", d["min"], d["max"])
        } else {
            format!("delay {d} ms")
        });
    }
    if !f["error"].is_null() {
        what.push(format!("{} {}", f["error"]["status"], text(&f["error"], "code")));
    }
    if !f["truncate"].is_null() {
        what.push(format!("truncate after {} bytes", f["truncate"]));
    }
    if f["reset"] == true {
        what.push("reset".into());
    }
    let mut when = Vec::new();
    if let Some(ops) = f["operation"].as_array() {
        when.push(ops.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(","));
    }
    if let Some(b) = f["bucket"].as_str() {
        when.push(format!("bucket={b}"));
    }
    if let Some(p) = f["key_prefix"].as_str() {
        when.push(format!("key_prefix={p}"));
    }
    if f["probability"].as_f64().is_some_and(|p| p < 1.0) {
        when.push(format!("p={} seed={}", f["probability"], f["seed"]));
    }
    if f["skip"].as_u64().is_some_and(|n| n > 0) {
        when.push(format!("skip {}", f["skip"]));
    }
    let left = match f["remaining"].as_u64() {
        Some(n) => format!(", {n} left"),
        None => String::new(),
    };
    println!(
        "{:>4}  {:<28} {:<40} matched {} fired {}{left}",
        f["id"],
        what.join(" + "),
        if when.is_empty() {
            "all requests".to_owned()
        } else {
            when.join(" ")
        },
        f["matched"],
        f["fired"],
    );
}

fn enc(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("-")
}

fn print_human(command: &Command, v: &Value) {
    match command {
        Command::Status => {
            println!("version        {}", text(v, "version"));
            print_clock(&v["clock"]);
            let s = &v["store"];
            println!(
                "store          {} buckets, {} objects, {} uploads",
                s["buckets"], s["objects"], s["uploads"]
            );
            println!("new buckets    {} defaults", text(v, "bucket_defaults"));
            println!("auth           {}", text(v, "auth"));
            for u in v["users"].as_array().into_iter().flatten() {
                println!(
                    "user           {} (access key {}, account {}, canonical id {})",
                    text(u, "name"),
                    text(u, "access_key"),
                    text(u, "account_id"),
                    text(u, "canonical_id")
                );
            }
            println!("snapshots      {}", v["snapshots"]);
            print_log_config(&v["request_log"]);
            println!("fault rules    {}", v["faults"]);
        }
        Command::Stats => {
            print_memory(&v["memory"]);
            let s = &v["store"];
            println!(
                "store          {} buckets, {} objects, {} uploads",
                s["buckets"], s["objects"], s["uploads"]
            );
            println!("snapshots      {}", v["snapshots"]);
            print_log_config(&v["requests"]);
        }
        Command::Reset { .. } => println!("reset"),
        Command::Snapshot(Snapshot::List) => {
            for s in v["snapshots"].as_array().into_iter().flatten() {
                print_snapshot(s);
            }
        }
        Command::Snapshot(Snapshot::Delete { name }) => println!("deleted {name}"),
        Command::Snapshot(Snapshot::Restore { .. }) => {
            print!("restored ");
            print_snapshot(v);
        }
        Command::Snapshot(Snapshot::Create { .. }) => {
            print!("created ");
            print_snapshot(v);
        }
        Command::Clock(_) => print_clock(v),
        Command::Requests(Requests::List { count: true, .. }) => println!("{}", v["total"]),
        Command::Requests(Requests::List { .. }) => {
            for r in v["requests"].as_array().into_iter().flatten() {
                let fault = match r["fault"]["type"].as_str() {
                    Some(t) => format!("  [fault {} #{}]", t, r["fault"]["id"]),
                    None => String::new(),
                };
                println!(
                    "{:>6}  {}  {:<6} {:<4} {:<24} {:>8}us  in={} out={}  {}  {}{fault}",
                    r["seq"],
                    text(r, "time"),
                    text(r, "method"),
                    r["status"],
                    text(r, "operation"),
                    r["latency_us"],
                    r["bytes_in"],
                    r["bytes_out"],
                    text(r, "request_id"),
                    text(r, "path"),
                );
            }
            let (shown, total) = (v["requests"].as_array().map_or(0, Vec::len), v["total"].as_u64().unwrap_or(0));
            if total > shown as u64 {
                eprintln!("showing {shown} of {total} matching requests");
            }
        }
        Command::Requests(Requests::Clear) => println!("cleared"),
        Command::Requests(Requests::Config { .. }) => print_log_config(v),
        Command::Faults(Faults::Add { .. }) => {
            print!("added ");
            print_fault(v);
        }
        Command::Faults(Faults::List) => {
            for f in v["faults"].as_array().into_iter().flatten() {
                print_fault(f);
            }
        }
        Command::Faults(Faults::Delete { id }) => println!("deleted {id}"),
        Command::Faults(Faults::Clear) => println!("cleared {}", v["cleared"]),
    }
}

fn print_clock(c: &Value) {
    let mode = if c["frozen"] == true {
        "frozen"
    } else if c["shifted"] == true {
        "running, offset from host clock"
    } else {
        "running, host clock"
    };
    println!("clock          {} ({mode})", text(c, "now"));
}

fn print_memory(m: &Value) {
    let used = m["used"].as_u64().unwrap_or(0);
    match m["limit"].as_u64() {
        Some(limit) => println!(
            "memory         {} of {} ({}%)",
            human(used),
            human(limit),
            used * 100 / limit.max(1)
        ),
        None => println!("memory         {} (no limit)", human(used)),
    }
}

/// Formats a byte count with a binary unit, e.g. `1.5 GiB`.
fn human(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let (mut v, mut i) = (n as f64, 0);
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", UNITS[i]) }
}

fn print_log_config(l: &Value) {
    let state = if l["enabled"] == true { "enabled" } else { "disabled" };
    println!(
        "request log    {state}, {} of {} stored, {} dropped",
        l["stored"], l["capacity"], l["dropped"]
    );
}

fn print_snapshot(s: &Value) {
    println!(
        "{}  {}  {} buckets, {} objects, {} uploads",
        text(s, "name"),
        text(s, "created"),
        s["buckets"],
        s["objects"],
        s["uploads"]
    );
}

struct Api {
    authority: String,
}

impl Api {
    fn new(endpoint: &str) -> Result<Self> {
        let rest = endpoint
            .strip_prefix("http://")
            .ok_or_else(|| format!("unsupported endpoint {endpoint:?}: only http:// is supported"))?;
        let authority = rest.trim_end_matches('/');
        if authority.is_empty() || authority.contains('/') {
            return Err(format!("invalid endpoint {endpoint:?}, expected http://host:port"));
        }
        Ok(Self {
            authority: authority.to_owned(),
        })
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let stream = TcpStream::connect(&self.authority)
            .await
            .map_err(|e| format!("cannot connect to {}: {e}", self.authority))?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| e.to_string())?;
        tokio::spawn(conn);
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let req = http::Request::builder()
            .method(method)
            .uri(path)
            .header(http::header::HOST, &self.authority)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(payload)))
            .map_err(|e| e.to_string())?;
        let resp = sender.send_request(req).await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let bytes = resp.into_body().collect().await.map_err(|e| e.to_string())?.to_bytes();
        let json: Value = serde_json::from_slice(&bytes)
            .map_err(|_| format!("{} did not answer like a fakes3 control plane (HTTP {status})", self.authority))?;
        if status.is_success() {
            return Ok(json);
        }
        let msg = json["error"]["message"].as_str().unwrap_or("request failed");
        Err(format!("{msg} (HTTP {status})"))
    }
}
