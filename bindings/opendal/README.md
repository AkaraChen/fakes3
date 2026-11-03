# fakes3-opendal

[![crates.io](https://img.shields.io/crates/v/fakes3-opendal)](https://crates.io/crates/fakes3-opendal)
[![docs.rs](https://img.shields.io/docsrs/fakes3-opendal)](https://docs.rs/fakes3-opendal)

[OpenDAL](https://opendal.apache.org) operators backed by an in-process
[fakes3](https://github.com/AkaraChen/fakes3) server, for tests.

OpenDAL's `memory` service is the cheapest storage to test against, but it skips the S3 service
entirely and is far more lenient than S3: it has no ETags, no versions, no multipart uploads and
`if_not_exists` is its only conditional write. Code that passes on `memory` can still fail on S3.
`fakes3-opendal` gives you an operator on OpenDAL's real S3 service, talking HTTP to a server that
behaves like AWS, at almost the same cost: the server starts in about a millisecond, inside your
test process.

```toml
[dev-dependencies]
fakes3-opendal = "0.1"
```

```rust
use fakes3_opendal::Fixture;

#[tokio::test]
async fn writes_a_file() -> opendal::Result<()> {
    let s3 = Fixture::start().await?; // a server on a random port and a bucket named "test"
    let op = s3.operator();           // an opendal::Operator on that bucket
    op.write("hello.txt", "hi").await?;
    assert_eq!(op.read("hello.txt").await?.to_vec(), b"hi");
    Ok(())
}
```

Like fakes3 itself, it builds on Linux only, and the build compiles OpenSSL, so it needs `perl` and
`make`.

## What you get

- **The real S3 code path.** Signing, XML parsing, multipart uploads, conditional requests and the
  mapping of S3 errors to `opendal::ErrorKind` all run, against a server that passes most of
  [ceph/s3-tests](https://github.com/AkaraChen/fakes3#why-fakes3) and every one of
  [OpenDAL's own S3 behavior tests](https://github.com/AkaraChen/fakes3/blob/main/conformance/OPENDAL.md),
  with no capability switched off.
- **Isolation.** Every fixture has its own server and port. The operator never reads AWS settings
  from the environment, `~/.aws` or instance metadata, so credentials on your machine cannot leak
  into a test.
- **No global state.** The operator carries its own HTTP transport. It does not need the `opendal`
  crate to install a default one, and does not install one either.
- **The server at hand.** `s3.server()` is a [`fakes3::Server`](https://docs.rs/fakes3): snapshots,
  a virtual clock, and a log of the requests OpenDAL sent.

## Options

```rust
use fakes3_opendal::{Fixture, fakes3};

let s3 = Fixture::builder()
    .bucket("data")      // default: "test"
    .versioning(true)    // for version options on stat, read, list and delete
    .config(fakes3::Config {
        auth: true,      // verify signatures
        ..Default::default()
    })
    .start()
    .await?;

// More operators on the same server: another root, another bucket, other S3 options.
let tenant = s3.operator_with(|b| b.root("/tenant-a"))?;
s3.server().create_bucket("other").unwrap();
let other = s3.operator_with(|b| b.bucket("other"))?;

// Empty the server between tests; the bucket is created again.
s3.reset()?;
```

Assert on what OpenDAL actually sent:

```rust
let mut w = s3.operator().writer_with("big").chunk(5 << 20).await?;
w.write(vec![0u8; 12 << 20]).await?;
w.close().await?;

let (_, uploads) = s3.server().requests(&fakes3::RequestFilter {
    operation: Some("UploadPart".into()),
    ..Default::default()
});
assert_eq!(uploads, 3);
```

## Versions

The crate follows OpenDAL's releases, so its version moves independently of fakes3's.

| fakes3-opendal | OpenDAL | fakes3 |
| --- | --- | --- |
| 0.1 | 0.59 | 0.1.1 or later |

## License

[MIT](LICENSE)
