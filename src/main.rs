use std::io::Write;

use clap::Parser;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(version, about = "An in-memory S3 server for tests")]
struct Opt {
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
}

fn main() -> std::io::Result<()> {
    let opt = Opt::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let config = fakes3::Config {
        host: opt.host,
        port: opt.port,
        access_key: opt.access_key,
        secret_key: opt.secret_key,
        auth: !opt.no_auth,
        inline_threshold: opt.inline_threshold,
    };
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async {
        let server = fakes3::Server::start(config).await?;
        println!("{}", server.endpoint());
        std::io::stdout().flush()?;
        server.wait().await;
        Ok(())
    })
}
