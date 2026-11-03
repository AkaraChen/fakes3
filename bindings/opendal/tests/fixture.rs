//! The fixture against OpenDAL's S3 service, end to end over HTTP.

use fakes3_opendal::{Fixture, fakes3};
use opendal_core::{ErrorKind, Result};

#[tokio::test]
async fn round_trip_through_the_s3_service() -> Result<()> {
    let s3 = Fixture::start().await?;
    // The same type the `opendal` crate re-exports.
    let op: opendal::Operator = s3.operator().clone();
    op.write("dir/hello.txt", "hi").await?;
    assert_eq!(op.read("dir/hello.txt").await?.to_vec(), b"hi");
    let meta = op.stat("dir/hello.txt").await?;
    assert_eq!(meta.content_length(), 2);
    assert!(meta.etag().is_some());
    let names: Vec<_> = op.list("dir/").await?.into_iter().map(|e| e.path().to_owned()).collect();
    assert_eq!(names, ["dir/hello.txt"]);
    op.delete("dir/hello.txt").await?;
    assert!(!op.exists("dir/hello.txt").await?);
    s3.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn conditional_writes_follow_s3() -> Result<()> {
    let s3 = Fixture::start().await?;
    let op = s3.operator();
    op.write_with("lock", "a").if_not_exists(true).await?;
    let err = op.write_with("lock", "b").if_not_exists(true).await.unwrap_err();
    assert_eq!(err.kind(), ErrorKind::ConditionNotMatch);
    let etag = op.stat("lock").await?.etag().unwrap().to_owned();
    op.write_with("lock", "c").if_match(&etag).await?;
    let err = op.write_with("lock", "d").if_match(&etag).await.unwrap_err();
    assert_eq!(err.kind(), ErrorKind::ConditionNotMatch);
    assert_eq!(op.read("lock").await?.to_vec(), b"c");
    Ok(())
}

#[tokio::test]
async fn versioned_bucket() -> Result<()> {
    let s3 = Fixture::builder().versioning(true).start().await?;
    let op = s3.operator();
    let first = op.write("k", "one").await?.version().unwrap().to_owned();
    op.write("k", "two").await?;
    assert_eq!(op.read_with("k").version(&first).await?.to_vec(), b"one");
    let versions = op.list_with("k").versions(true).await?;
    assert_eq!(versions.len(), 2);
    // The bucket comes back versioned after a reset.
    s3.reset()?;
    assert!(op.write("k", "three").await?.version().is_some());
    Ok(())
}

#[tokio::test]
async fn the_request_log_shows_what_opendal_sent() -> Result<()> {
    let s3 = Fixture::start().await?;
    let mut w = s3.operator().writer_with("big").chunk(5 << 20).await?;
    w.write(vec![7u8; 12 << 20]).await?;
    w.close().await?;
    let count = |op: &str| {
        s3.server()
            .requests(&fakes3::RequestFilter {
                operation: Some(op.into()),
                ..Default::default()
            })
            .1
    };
    assert_eq!(count("CreateMultipartUpload"), 1);
    assert_eq!(count("UploadPart"), 3);
    assert_eq!(count("CompleteMultipartUpload"), 1);
    assert_eq!(s3.operator().stat("big").await?.content_length(), 12 << 20);
    Ok(())
}

#[tokio::test]
async fn reset_empties_the_bucket_but_keeps_it() -> Result<()> {
    let s3 = Fixture::start().await?;
    s3.operator().write("k", "v").await?;
    s3.reset()?;
    assert!(!s3.operator().exists("k").await?);
    s3.operator().write("k", "v").await?;
    Ok(())
}

#[tokio::test]
async fn operator_with_changes_the_root_and_the_bucket() -> Result<()> {
    let s3 = Fixture::builder().bucket("primary").start().await?;
    assert_eq!(s3.bucket(), "primary");
    s3.operator_with(|b| b.root("/tenant-a"))?.write("x", "1").await?;
    assert!(s3.operator().exists("tenant-a/x").await?);

    s3.server().create_bucket("secondary").unwrap();
    let other = s3.operator_with(|b| b.bucket("secondary"))?;
    other.write("y", "2").await?;
    assert!(!s3.operator().exists("y").await?);
    Ok(())
}

#[tokio::test]
async fn signatures_check_out_when_auth_is_on() -> Result<()> {
    let config = fakes3::Config {
        auth: true,
        access_key: "AKIDTEST".into(),
        secret_key: "test-secret".into(),
        ..Default::default()
    };
    let s3 = Fixture::builder().config(config).start().await?;
    s3.operator().write("k", "v").await?;
    let wrong = s3.operator_with(|b| b.secret_access_key("not-the-secret"))?;
    assert_eq!(wrong.write("k", "v").await.unwrap_err().kind(), ErrorKind::PermissionDenied);
    Ok(())
}

#[tokio::test]
async fn bad_bucket_names_are_config_errors() {
    let err = Fixture::builder().bucket("Not_Valid").start().await.unwrap_err();
    assert_eq!(err.kind(), ErrorKind::ConfigInvalid);
}
