mod aws;
mod common;
mod objstore;
mod opendal_sdk;
mod rusts3;
mod versions;

use common::Env;

const ALL: [&str; 4] = ["aws-sdk-s3", "object_store", "opendal", "rust-s3"];

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let env = Env::load();
    // Optional: MATRIX_SDKS=comma,separated subset (defaults to all)
    let wanted: Vec<String> = std::env::var("MATRIX_SDKS")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_else(|| ALL.iter().map(|s| s.to_string()).collect());
    // Maintenance: MATRIX_CLEANUP_BUCKETS=a,b removes leftover buckets (all versions) and exits.
    if let Ok(list) = std::env::var("MATRIX_CLEANUP_BUCKETS") {
        let c = common::aws_client(&env, &env.sk);
        for b in list.split(',').filter(|b| !b.is_empty()) {
            common::aws_destroy_bucket(&c, b).await;
        }
        return Ok(());
    }
    eprintln!("rust matrix: server={} endpoint={} out={}", env.server, env.endpoint, env.out);
    for sdk in wanted {
        let report = match sdk.as_str() {
            "aws-sdk-s3" => aws::run(&env).await,
            "object_store" => objstore::run(&env).await,
            "opendal" => opendal_sdk::run(&env).await,
            "rust-s3" => rusts3::run(&env).await,
            other => {
                eprintln!("unknown sdk {other}");
                continue;
            }
        };
        report.write(&env)?;
    }
    Ok(())
}
