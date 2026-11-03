//! [OpenDAL] operators backed by an in-process [fakes3] server, for tests.
//!
//! ```no_run
//! # async fn demo() -> opendal_core::Result<()> {
//! let s3 = fakes3_opendal::Fixture::start().await?;
//! let op = s3.operator();
//! op.write("hello.txt", "hi").await?;
//! assert_eq!(op.read("hello.txt").await?.to_vec(), b"hi");
//! s3.shutdown().await;
//! # Ok(())
//! # }
//! ```
//!
//! A [`Fixture`] starts fakes3 on a random localhost port, creates a bucket and builds an
//! [`Operator`] for it on OpenDAL's S3 service. Requests go over HTTP as they would to AWS, so
//! signing, XML parsing, multipart uploads and error mapping all run. The operator:
//!
//! - never reads AWS settings from the environment, `~/.aws` or instance metadata, so credentials
//!   on a developer's machine cannot leak into a test;
//! - brings its own HTTP transport, so it does not depend on the `opendal` crate installing a
//!   default one.
//!
//! # Configure it like production
//!
//! Code under test usually builds its own operator from configuration rather than taking one.
//! The fixture is plain S3 to OpenDAL, so hand that code the fixture's S3 settings, in whichever
//! form it reads, and it runs the same path as in production with only the endpoint changed:
//!
//! ```no_run
//! # async fn demo() -> opendal::Result<()> {
//! let s3 = fakes3_opendal::Fixture::start().await?;
//! let op = opendal::Operator::from_uri(s3.uri())?; // "s3://test?endpoint=http%3A%2F%2F127.0.0.1%3A…"
//! let op = opendal::Operator::via_iter("s3", s3.options())?; // configuration keys
//! let op = opendal::Operator::new(s3.s3().root("/app"))?; // an S3 builder to extend
//! # Ok(())
//! # }
//! ```
//!
//! Operators built this way use OpenDAL's default HTTP transport, which the `opendal` crate
//! installs with its default features, as in production.
//!
//! [`Fixture::server`] reaches what OpenDAL has no API for: snapshots, the virtual clock and the
//! request log.
//!
//! [OpenDAL]: https://opendal.apache.org
//! [fakes3]: https://github.com/AkaraChen/fakes3

use std::fmt;

use opendal_core::{Error, ErrorKind, HttpTransporter, OperationContext, Operator, Result};
use opendal_http_transport_reqwest::ReqwestTransport;
use opendal_service_s3::S3;

pub use fakes3;

/// The bucket a [`Fixture`] creates unless told otherwise.
pub const DEFAULT_BUCKET: &str = "test";

/// The region the fixture's settings name. fakes3 accepts any.
pub const REGION: &str = "us-east-1";

/// A fakes3 server, a bucket on it, and an [`Operator`] for that bucket.
///
/// Dropping the fixture stops the server; [`Fixture::shutdown`] also waits for the port to close.
pub struct Fixture {
    server: fakes3::Server,
    conn: Connection,
    versioning: bool,
    operator: Operator,
}

/// What every operator on the fixture's server shares.
struct Connection {
    endpoint: String,
    bucket: String,
    access_key: String,
    secret_key: String,
    transport: HttpTransporter,
}

impl Connection {
    /// The settings as S3 configuration keys; `s3()` and `options()` must describe the same thing.
    fn options(&self) -> Vec<(String, String)> {
        [
            ("bucket", self.bucket.as_str()),
            ("endpoint", &self.endpoint),
            ("region", REGION),
            ("access_key_id", &self.access_key),
            ("secret_access_key", &self.secret_key),
            ("disable_config_load", "true"),
            ("disable_ec2_metadata", "true"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
    }

    fn s3(&self) -> S3 {
        S3::default()
            .bucket(&self.bucket)
            .endpoint(&self.endpoint)
            .region(REGION)
            .access_key_id(&self.access_key)
            .secret_access_key(&self.secret_key)
            .disable_config_load()
            .disable_ec2_metadata()
    }

    fn operator(&self, configure: impl FnOnce(S3) -> S3) -> Result<Operator> {
        let ctx = OperationContext::new().with_http_transport(self.transport.clone());
        Ok(Operator::new(configure(self.s3()))?.with_context(ctx))
    }
}

impl Fixture {
    /// Starts a server with [`fakes3::Config::default`] and creates the bucket
    /// [`DEFAULT_BUCKET`]. Must be called inside a Tokio runtime.
    pub async fn start() -> Result<Self> {
        Self::builder().start().await
    }

    /// Settings for the server and the bucket.
    pub fn builder() -> FixtureBuilder {
        FixtureBuilder::default()
    }

    /// An operator on the bucket, rooted at `/`.
    pub fn operator(&self) -> &Operator {
        &self.operator
    }

    /// Another operator on the same server. `configure` gets an [`S3`] builder that already points
    /// at the bucket with working credentials, and can change the root, the bucket or any other
    /// option. A different bucket has to exist first; see [`fakes3::Server::create_bucket`].
    ///
    /// ```no_run
    /// # fn demo(s3: &fakes3_opendal::Fixture) -> opendal_core::Result<()> {
    /// let tenant = s3.operator_with(|b| b.root("/tenant-a"))?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn operator_with(&self, configure: impl FnOnce(S3) -> S3) -> Result<Operator> {
        self.conn.operator(configure)
    }

    /// An S3 builder with the fixture's settings, to extend and build the way production code
    /// does: `Operator::new(s3.s3().root("/app"))`. The operator uses OpenDAL's default HTTP
    /// transport.
    pub fn s3(&self) -> S3 {
        self.conn.s3()
    }

    /// The fixture's settings as S3 configuration keys: `bucket`, `endpoint`, `region`,
    /// `access_key_id`, `secret_access_key`, and `disable_config_load` / `disable_ec2_metadata`
    /// so that nothing comes from the environment. For `Operator::via_iter("s3", s3.options())`,
    /// or for code that reads these keys from its own configuration; add `root` as needed.
    pub fn options(&self) -> Vec<(String, String)> {
        self.conn.options()
    }

    /// The same settings as an OpenDAL URI, `s3://<bucket>?endpoint=…&region=…`, for
    /// `Operator::from_uri` or an application that reads a storage URI.
    pub fn uri(&self) -> String {
        let query = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(self.conn.options().into_iter().filter(|(k, _)| k != "bucket"))
            .finish();
        format!("s3://{}?{query}", self.conn.bucket)
    }

    /// The server, for snapshots, the clock and the request log.
    pub fn server(&self) -> &fakes3::Server {
        &self.server
    }

    pub fn bucket(&self) -> &str {
        &self.conn.bucket
    }

    /// `http://127.0.0.1:<port>`.
    pub fn endpoint(&self) -> &str {
        &self.conn.endpoint
    }

    /// The default account's access key, which the fixture's settings sign with.
    pub fn access_key_id(&self) -> &str {
        &self.conn.access_key
    }

    /// The default account's secret key.
    pub fn secret_access_key(&self) -> &str {
        &self.conn.secret_key
    }

    /// Clears the server (buckets, objects, uploads, the request log and the clock; snapshots
    /// stay) and creates the bucket again, empty, so the operators keep working.
    pub fn reset(&self) -> Result<()> {
        self.server.reset();
        self.create_bucket()
    }

    /// Stops the server and waits for the port to close.
    pub async fn shutdown(self) {
        self.server.shutdown().await;
    }

    fn create_bucket(&self) -> Result<()> {
        self.server.create_bucket(&self.conn.bucket).map_err(control_error)?;
        if self.versioning {
            self.server.enable_versioning(&self.conn.bucket).map_err(control_error)?;
        }
        Ok(())
    }
}

impl fmt::Debug for Fixture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fixture")
            .field("endpoint", &self.conn.endpoint)
            .field("bucket", &self.conn.bucket)
            .field("versioning", &self.versioning)
            .finish_non_exhaustive()
    }
}

/// Settings for a [`Fixture`], from [`Fixture::builder`].
#[derive(Clone, Debug)]
pub struct FixtureBuilder {
    config: fakes3::Config,
    bucket: String,
    versioning: bool,
}

impl Default for FixtureBuilder {
    fn default() -> Self {
        Self {
            config: fakes3::Config::default(),
            bucket: DEFAULT_BUCKET.to_owned(),
            versioning: false,
        }
    }
}

impl FixtureBuilder {
    /// Server settings: signature checks, accounts, bucket defaults, the memory limit and so on.
    /// The operators sign with the default account's credentials, `access_key` and `secret_key`.
    pub fn config(mut self, config: fakes3::Config) -> Self {
        self.config = config;
        self
    }

    /// The bucket to create; [`DEFAULT_BUCKET`] unless set.
    pub fn bucket(mut self, name: impl Into<String>) -> Self {
        self.bucket = name.into();
        self
    }

    /// Turns versioning on for the bucket, for OpenDAL's version options on stat, read, list and
    /// delete.
    pub fn versioning(mut self, on: bool) -> Self {
        self.versioning = on;
        self
    }

    /// Starts the server and creates the bucket. Must be called inside a Tokio runtime.
    pub async fn start(self) -> Result<Fixture> {
        let server = fakes3::Server::start(self.config.clone())
            .await
            .map_err(|e| Error::new(ErrorKind::Unexpected, "failed to start fakes3").set_source(e))?;
        let conn = Connection {
            endpoint: server.endpoint(),
            bucket: self.bucket,
            access_key: self.config.access_key,
            secret_key: self.config.secret_key,
            transport: HttpTransporter::new(ReqwestTransport::default()),
        };
        let fixture = Fixture {
            operator: conn.operator(|b| b)?,
            server,
            conn,
            versioning: self.versioning,
        };
        fixture.create_bucket()?;
        Ok(fixture)
    }
}

fn control_error(e: fakes3::ControlError) -> Error {
    let kind = match e {
        fakes3::ControlError::BadRequest(_) => ErrorKind::ConfigInvalid,
        _ => ErrorKind::Unexpected,
    };
    Error::new(kind, "fakes3 refused to set up the bucket").set_source(e)
}
