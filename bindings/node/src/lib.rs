//! Node.js binding: runs fakes3 inside the Node process.
//!
//! This file is a thin shell over [`fakes3::Server`]. A control-plane method that lands on
//! `fakes3::Server` becomes a wrapper in the `impl FakeS3Server` block below that calls it through
//! [`FakeS3Server::server`]; the TypeScript declarations are generated from the signatures.

use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::*;
use napi_derive::napi;

/// Options for [`start`]. Every field is optional.
#[napi(object)]
pub struct StartOptions {
    /// Address to listen on. Default `127.0.0.1`.
    pub host: Option<String>,
    /// Port to listen on. Default `0`, which picks a free port.
    pub port: Option<u16>,
    /// Access key clients must sign with. Default `fakes3`.
    pub access_key: Option<String>,
    /// Secret key clients must sign with. Default `fakes3secret`.
    pub secret_key: Option<String>,
    /// Skip signature verification. Default `false`.
    pub no_auth: Option<bool>,
    /// Objects and parts at or above this many bytes are stored in a memfd instead of on the heap.
    /// Default 1 MiB.
    pub inline_threshold: Option<u32>,
    /// Cap on the object data held in memory: a number of bytes, or a string such as `"512MiB"`,
    /// `"2GiB"`, `"unlimited"` or `"auto"`. Writes past it fail with HTTP 507. Default `"auto"`,
    /// which is 75% of the container's or machine's memory.
    pub memory_limit: Option<Either<f64, String>>,
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
    server: Arc<Mutex<Option<fakes3::Server>>>,
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

    /// Object data currently held in memory, and the limit it is checked against.
    #[napi]
    pub fn memory(&self) -> Result<MemoryStats> {
        let stats = self.server(|s| s.memory())?;
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

impl FakeS3Server {
    /// The running server, for wrappers around `fakes3::Server` methods. Fails once stopped.
    fn server<T>(&self, f: impl FnOnce(&fakes3::Server) -> T) -> Result<T> {
        let guard = self.server.lock().map_err(poisoned)?;
        match guard.as_ref() {
            Some(server) => Ok(f(server)),
            None => Err(Error::new(Status::GenericFailure, "server is stopped")),
        }
    }
}

fn poisoned<T>(_: T) -> Error {
    Error::new(Status::GenericFailure, "server state is poisoned")
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
        no_auth: None,
        inline_threshold: None,
        memory_limit: None,
    });
    let config = fakes3::Config {
        host: options.host.unwrap_or(defaults.host),
        port: options.port.unwrap_or(defaults.port),
        access_key: options.access_key.unwrap_or(defaults.access_key),
        secret_key: options.secret_key.unwrap_or(defaults.secret_key),
        auth: !options.no_auth.unwrap_or(false),
        inline_threshold: options.inline_threshold.map_or(defaults.inline_threshold, |n| n as usize),
        memory_limit: match options.memory_limit {
            None => defaults.memory_limit,
            Some(Either::A(n)) if n.is_finite() && n >= 0.0 => fakes3::MemoryLimit::Bytes(n as u64),
            Some(Either::A(n)) => return Err(Error::new(Status::InvalidArg, format!("invalid memoryLimit: {n}"))),
            Some(Either::B(s)) => s
                .parse()
                .map_err(|e| Error::new(Status::InvalidArg, format!("invalid memoryLimit {s:?}: {e}")))?,
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
