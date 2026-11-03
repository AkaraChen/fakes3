//! Code under test builds its own operator from the fixture's S3 settings, through the `opendal`
//! crate as an application has it (default features plus `services-s3`): URI, configuration keys,
//! S3 builder, or its own configuration struct.

use fakes3_opendal::{Fixture, fakes3};
use opendal::{ErrorKind, Operator, Result};

/// What an application might read from its configuration file.
struct StorageConfig {
    bucket: String,
    endpoint: String,
    access_key_id: String,
    secret_access_key: String,
    root: String,
}

/// The application's own constructor, as it would run in production.
fn connect(cfg: &StorageConfig) -> Result<Operator> {
    let s3 = opendal::services::S3::default()
        .bucket(&cfg.bucket)
        .endpoint(&cfg.endpoint)
        .region("us-east-1")
        .access_key_id(&cfg.access_key_id)
        .secret_access_key(&cfg.secret_access_key)
        .root(&cfg.root);
    Operator::new(s3)
}

#[tokio::test]
async fn from_uri_via_iter_and_builder_reach_the_fixture() -> Result<()> {
    let s3 = Fixture::start().await?;
    // The fixture's own operator is the type the `opendal` crate exports.
    let check: Operator = s3.operator().clone();

    Operator::from_uri(s3.uri())?.write("from-uri", "1").await?;
    assert_eq!(check.read("from-uri").await?.to_vec(), b"1");

    let mut options = s3.options();
    options.push(("root".into(), "/via-iter".into()));
    Operator::via_iter("s3", options)?.write("k", "2").await?;
    assert_eq!(check.read("via-iter/k").await?.to_vec(), b"2");

    Operator::new(s3.s3().root("/builder"))?.write("k", "3").await?;
    assert_eq!(check.read("builder/k").await?.to_vec(), b"3");
    Ok(())
}

#[tokio::test]
async fn an_application_config_struct_works_unchanged() -> Result<()> {
    let s3 = Fixture::start().await?;
    let op = connect(&StorageConfig {
        bucket: s3.bucket().to_owned(),
        endpoint: s3.endpoint().to_owned(),
        access_key_id: s3.access_key_id().to_owned(),
        secret_access_key: s3.secret_access_key().to_owned(),
        root: "/app".to_owned(),
    })?;
    op.write_with("lock", "a").if_not_exists(true).await?;
    let err = op.write_with("lock", "b").if_not_exists(true).await.unwrap_err();
    assert_eq!(err.kind(), ErrorKind::ConditionNotMatch);
    assert!(s3.operator().exists("app/lock").await?);
    Ok(())
}

#[tokio::test]
async fn the_uri_carries_credentials_that_need_escaping() -> Result<()> {
    // With signatures verified, a secret that lost a character in the URI would be rejected. AWS
    // secrets contain `/` and `+`; access keys are alphanumeric (`/` separates SigV4's credential).
    let config = fakes3::Config {
        auth: true,
        access_key: "AKIDESCAPETEST".into(),
        secret_key: "s3cr3t+/=&?#% ok".into(),
        ..Default::default()
    };
    let s3 = Fixture::builder().config(config).bucket("escaped.bucket-1").start().await?;
    let op = Operator::from_uri(s3.uri())?;
    op.write("k", "v").await?;
    assert_eq!(op.read("k").await?.to_vec(), b"v");
    Ok(())
}
