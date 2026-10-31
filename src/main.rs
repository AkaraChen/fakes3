use std::io::Write;

use clap::Parser;

mod ctl;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(version, about = "An in-memory S3 server for tests")]
struct Opt {
    /// Talk to a running server's control plane instead of serving.
    #[command(subcommand)]
    command: Option<Cmd>,

    #[arg(long, default_value = "127.0.0.1", env = "FAKES3_HOST")]
    host: String,

    /// Port to listen on; 0 picks a free port. The bound address is printed to stdout.
    #[arg(long, default_value_t = 9000, env = "FAKES3_PORT")]
    port: u16,

    #[arg(long, default_value = "fakes3", env = "FAKES3_ACCESS_KEY")]
    access_key: String,

    #[arg(long, default_value = "fakes3secret", env = "FAKES3_SECRET_KEY")]
    secret_key: String,

    /// Accept requests without verifying signatures.
    #[arg(long)]
    no_auth: bool,

    /// Objects and parts at or above this size are stored in a memfd instead of on the heap.
    #[arg(long, default_value_t = 1 << 20)]
    inline_threshold: usize,

    /// Base domain for virtual-host-style addressing (`<bucket>.<domain>`); repeatable.
    /// `--domain ""` turns virtual-host addressing off.
    #[arg(long = "domain", default_value = "localhost", env = "FAKES3_DOMAIN", value_delimiter = ',')]
    domains: Vec<String>,
    /// Do not record S3 requests for the control plane's request log.
    #[arg(long)]
    no_request_log: bool,

    /// Requests kept by the request log; the oldest are dropped first.
    #[arg(long, default_value_t = fakes3::DEFAULT_REQUEST_LOG_CAPACITY)]
    request_log_capacity: usize,
    /// Most object data fakes3 will hold; a write beyond it fails with HTTP 507 instead of the
    /// process being OOM-killed. A size like `512MiB` or `2GiB`, `unlimited`, or `auto` (75% of the
    /// container's or machine's memory).
    #[arg(long, default_value = "auto", env = "FAKES3_MEMORY_LIMIT")]
    memory_limit: fakes3::MemoryLimit,

    /// Settings new buckets start with: `legacy` (ACLs enabled, Block Public Access off, no default
    /// encryption) or `aws` (what AWS applies to new buckets today: ACLs disabled, Block Public
    /// Access on, SSE-S3, SSE-C blocked). Only affects buckets created afterwards.
    #[arg(long, default_value = "legacy", env = "FAKES3_BUCKET_DEFAULTS", value_name = "legacy|aws")]
    bucket_defaults: fakes3::BucketDefaults,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Control a running server: reset, snapshots, clock, request log.
    Ctl(ctl::Ctl),
}

fn main() -> std::io::Result<std::process::ExitCode> {
    let opt = Opt::parse();
    if let Some(Cmd::Ctl(c)) = opt.command {
        return Ok(tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(ctl::run(c)));
    }
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();
    let config = fakes3::Config {
        host: opt.host,
        port: opt.port,
        access_key: opt.access_key,
        secret_key: opt.secret_key,
        auth: !opt.no_auth,
        inline_threshold: opt.inline_threshold,
        domains: opt.domains.into_iter().filter(|d| !d.is_empty()).collect(),
        request_log: !opt.no_request_log,
        request_log_capacity: opt.request_log_capacity,
        memory_limit: opt.memory_limit,
        bucket_defaults: opt.bucket_defaults,
    };
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async {
        let server = fakes3::Server::start(config).await?;
        println!("{}", server.endpoint());
        std::io::stdout().flush()?;
        server.wait().await;
        Ok(std::process::ExitCode::SUCCESS)
    })
}
