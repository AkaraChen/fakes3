//! Node.js binding: runs fakes3 inside the Node process.
//!
//! This crate is a thin shell over [`fakes3::Server`]. A method on `fakes3::Server` becomes a
//! wrapper that calls it through [`with_server`]; the TypeScript declarations are generated from
//! the signatures. Server lifecycle and the control plane's reset/snapshot methods live here,
//! [`control`] holds the clock and the request log.

mod control;

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

use napi::bindgen_prelude::*;
use napi_derive::napi;

/// Options for [`start`]. Every field is optional.
#[napi(object)]
pub struct StartOptions {
    /// Address to listen on. Default `127.0.0.1`.
    pub host: Option<String>,
    /// Port to listen on. Default `0`, which picks a free port.
    pub port: Option<u16>,
    /// Access key clients must sign with when `auth` is on. Default `fakes3`.
    pub access_key: Option<String>,
    /// Secret key clients must sign with when `auth` is on. Default `fakes3secret`.
    pub secret_key: Option<String>,
    /// Verify request signatures against `accessKey` / `secretKey`. By default (`false`) a request
    /// signed with any access key and secret is accepted and acts as the bucket owner, while an
    /// unsigned request is anonymous and subject to ACLs, bucket policies and Block Public Access.
    pub auth: Option<bool>,
    /// Objects and parts at or above this many bytes are stored in a memfd instead of on the heap.
    /// Default 1 MiB.
    pub inline_threshold: Option<u32>,
    /// Base domains for virtual-host-style addressing: `<bucket>.<domain>` selects the bucket.
    /// Path-style requests keep working on every host. Default none.
    pub domains: Option<Vec<String>>,
    /// Cap on the object data held in memory: a number of bytes, or a string such as `"512MiB"`,
    /// `"2GiB"`, `"unlimited"` or `"auto"`. Writes past it fail with HTTP 507. Default `"auto"`,
    /// which is 75% of the container's or machine's memory.
    pub memory_limit: Option<Either<f64, String>>,
    /// Settings new buckets start with: `"legacy"` (ACLs enabled, Block Public Access off, no
    /// default encryption) or `"aws"` (ACLs disabled, Block Public Access on, SSE-S3, SSE-C blocked: what AWS
    /// applies to new buckets today). Default `"legacy"`.
    pub bucket_defaults: Option<String>,
    /// Record S3 requests for `server.requests`. Default `true`.
    pub request_log: Option<bool>,
    /// How many requests the log keeps; the oldest are dropped first. Default 10000.
    pub request_log_capacity: Option<u32>,
}

/// A reading of the server's memory accounting.
#[napi(object)]
pub struct MemoryStats {
    /// Bytes of object data currently held.
    pub used: f64,
    /// The configured limit in bytes, omitted (`undefined`) when unlimited.
    pub limit: Option<f64>,
}

/// A running in-process server.
#[napi(js_name = "FakeS3Server")]
pub struct FakeS3Server {
    server: Slot,
    endpoint: String,
    host: String,
    port: u16,
    access_key: String,
    secret_key: String,
}

#[napi]
impl FakeS3Server {
    /// `http://host:port`, ready to use as an S3 endpoint URL (with path-style addressing).
    #[napi(getter)]
    pub fn endpoint(&self) -> String {
        self.endpoint.clone()
    }

    /// The host the server is bound to.
    #[napi(getter)]
    pub fn host(&self) -> String {
        self.host.clone()
    }

    /// The port the server is bound to; the real one when `0` was requested.
    #[napi(getter)]
    pub fn port(&self) -> u32 {
        self.port.into()
    }

    /// The access key clients must sign with.
    #[napi(getter)]
    pub fn access_key(&self) -> String {
        self.access_key.clone()
    }

    /// The secret key clients must sign with.
    #[napi(getter)]
    pub fn secret_key(&self) -> String {
        self.secret_key.clone()
    }

    /// The server's clock: freeze it, move it, or let it run.
    #[napi(getter)]
    pub fn clock(&self) -> control::ClockControl {
        control::ClockControl::new(self.server.clone())
    }

    /// The log of S3 requests the server has received.
    #[napi(getter)]
    pub fn requests(&self) -> control::RequestsControl {
        control::RequestsControl::new(self.server.clone())
    }

    /// Clears all buckets, objects and uploads, and by default the request log and the clock too.
    /// Snapshots are kept unless `options.snapshots` is true.
    #[napi]
    pub fn reset(&self, options: Option<ResetOptions>) -> Result<()> {
        let defaults = fakes3::ResetOptions::default();
        let opts = match options {
            None => defaults,
            Some(o) => fakes3::ResetOptions {
                clock: o.clock.unwrap_or(defaults.clock),
                requests: o.requests.unwrap_or(defaults.requests),
                snapshots: o.snapshots.unwrap_or(defaults.snapshots),
            },
        };
        with_server(&self.server, |s| s.control().reset_with(opts))
    }

    /// Saves the current buckets, objects and uploads. The name (1-64 characters of `A-Za-z0-9._-`)
    /// is generated when omitted; a name that is already taken is an error.
    #[napi]
    pub fn snapshot(&self, name: Option<String>) -> Result<SnapshotInfo> {
        with_server(&self.server, |s| s.snapshot(name.as_deref()))?
            .map(snapshot_info)
            .map_err(control_error)
    }

    /// Rolls buckets, objects and uploads back to a snapshot. The snapshot is kept, so it can be
    /// restored again.
    #[napi]
    pub fn restore(&self, name: String) -> Result<SnapshotInfo> {
        with_server(&self.server, |s| s.restore(&name))?
            .map(snapshot_info)
            .map_err(control_error)
    }

    /// Stored snapshots.
    #[napi]
    pub fn snapshots(&self) -> Result<Vec<SnapshotInfo>> {
        with_server(&self.server, |s| s.control().snapshots().into_iter().map(snapshot_info).collect())
    }

    /// Deletes a snapshot.
    #[napi]
    pub fn delete_snapshot(&self, name: String) -> Result<()> {
        with_server(&self.server, |s| s.control().delete_snapshot(&name))?.map_err(control_error)
    }

    /// Object data currently held in memory, and the limit it is checked against.
    #[napi]
    pub fn memory(&self) -> Result<MemoryStats> {
        let stats = with_server(&self.server, |s| s.memory())?;
        Ok(MemoryStats {
            used: stats.used as f64,
            limit: stats.limit.map(|n| n as f64),
        })
    }

    /// Stops accepting connections and releases the port. Idempotent.
    #[napi]
    pub fn stop<'env>(&self, env: &'env Env) -> Result<PromiseRaw<'env, ()>> {
        let slot = self.server.clone();
        env.spawn_future(async move {
            let server = slot.lock().map_err(poisoned)?.take();
            if let Some(server) = server {
                server.shutdown().await;
            }
            Ok(())
        })
    }
}

/// The server shared with the objects handed out by `clock` and `requests`; `None` once stopped.
pub(crate) type Slot = Arc<Mutex<Option<fakes3::Server>>>;

/// Runs `f` on the running server. Fails once the server is stopped.
pub(crate) fn with_server<T>(slot: &Slot, f: impl FnOnce(&fakes3::Server) -> T) -> Result<T> {
    let guard = slot.lock().map_err(poisoned)?;
    match guard.as_ref() {
        Some(server) => Ok(f(server)),
        None => Err(Error::new(Status::GenericFailure, "server is stopped")),
    }
}

fn poisoned<T>(_: T) -> Error {
    Error::new(Status::GenericFailure, "server state is poisoned")
}

pub(crate) fn control_error(e: fakes3::ControlError) -> Error {
    let status = match e {
        fakes3::ControlError::BadRequest(_) => Status::InvalidArg,
        _ => Status::GenericFailure,
    };
    Error::new(status, e.to_string())
}

/// What `reset()` clears besides buckets, objects and uploads, which it always clears.
#[napi(object)]
pub struct ResetOptions {
    /// Put the clock back on the host clock. Default `true`.
    pub clock: Option<bool>,
    /// Empty the request log. Default `true`.
    pub requests: Option<bool>,
    /// Delete stored snapshots. Default `false`.
    pub snapshots: Option<bool>,
}

/// A stored snapshot.
#[napi(object)]
pub struct SnapshotInfo {
    pub name: String,
    /// Server clock time when the snapshot was taken.
    pub created: DateTime<Utc>,
    pub buckets: f64,
    /// Keys with at least one version (delete markers count).
    pub objects: f64,
    /// Multipart uploads in progress.
    pub uploads: f64,
}

fn snapshot_info(s: fakes3::SnapshotInfo) -> SnapshotInfo {
    SnapshotInfo {
        name: s.name,
        created: s.created.into(),
        buckets: s.counts.buckets as f64,
        objects: s.counts.objects as f64,
        uploads: s.counts.uploads as f64,
    }
}

/// Starts a server on the Node process's async runtime and resolves once it is listening.
#[napi]
pub async fn start(options: Option<StartOptions>) -> Result<FakeS3Server> {
    let defaults = fakes3::Config::default();
    let options = options.unwrap_or(StartOptions {
        host: None,
        port: None,
        access_key: None,
        secret_key: None,
        auth: None,
        inline_threshold: None,
        domains: None,
        memory_limit: None,
        bucket_defaults: None,
        request_log: None,
        request_log_capacity: None,
    });
    let config = fakes3::Config {
        host: options.host.unwrap_or(defaults.host),
        port: options.port.unwrap_or(defaults.port),
        access_key: options.access_key.unwrap_or(defaults.access_key),
        secret_key: options.secret_key.unwrap_or(defaults.secret_key),
        auth: options.auth.unwrap_or(defaults.auth),
        inline_threshold: options.inline_threshold.map_or(defaults.inline_threshold, |n| n as usize),
        domains: options.domains.unwrap_or(defaults.domains),
        request_log: options.request_log.unwrap_or(defaults.request_log),
        request_log_capacity: options.request_log_capacity.map_or(defaults.request_log_capacity, |n| n as usize),
        memory_limit: match options.memory_limit {
            None => defaults.memory_limit,
            Some(Either::A(n)) if n.is_finite() && n >= 0.0 => fakes3::MemoryLimit::Bytes(n as u64),
            Some(Either::A(n)) => return Err(Error::new(Status::InvalidArg, format!("invalid memoryLimit: {n}"))),
            Some(Either::B(s)) => s
                .parse()
                .map_err(|e| Error::new(Status::InvalidArg, format!("invalid memoryLimit {s:?}: {e}")))?,
        },
        bucket_defaults: match options.bucket_defaults {
            None => defaults.bucket_defaults,
            Some(s) => s.parse().map_err(|e| Error::new(Status::InvalidArg, format!("invalid bucketDefaults: {e}")))?,
        },
    };
    let server = fakes3::Server::start(config.clone()).await.map_err(|e| {
        Error::new(
            Status::GenericFailure,
            format!("failed to start fakes3 on {}:{}: {e}", config.host, config.port),
        )
    })?;
    Ok(FakeS3Server {
        endpoint: server.endpoint(),
        host: config.host,
        port: server.addr().port(),
        access_key: config.access_key,
        secret_key: config.secret_key,
        server: Arc::new(Mutex::new(Some(server))),
    })
}
