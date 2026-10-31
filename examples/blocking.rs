//! A server for synchronous code: no async runtime needed to start it.
//!
//!     cargo run --example blocking
//!
//! Prints the endpoint; point any S3 client at it while this runs.

fn main() -> std::io::Result<()> {
    let server = fakes3::Server::start_blocking(fakes3::Config::default())?;
    println!("listening on {}", server.endpoint());
    std::thread::sleep(std::time::Duration::from_secs(1));
    // The server stops when the handle is dropped.
    Ok(())
}
