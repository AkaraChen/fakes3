//! `Config::bucket_defaults`: what a new bucket starts with in `legacy` and `aws` mode, over real
//! sockets with SigV4 auth on. The `aws` expectations are the documented AWS defaults:
//! <https://docs.aws.amazon.com/AmazonS3/latest/userguide/about-object-ownership.html> and
//! <https://docs.aws.amazon.com/AmazonS3/latest/userguide/access-control-block-public-access.html>.

mod common;

use std::net::SocketAddr;

use common::{Resp, req, send};
use fakes3::{BucketDefaults, Config, Server};

async fn start(bucket_defaults: BucketDefaults) -> (Server, SocketAddr) {
    let server = Server::start(Config {
        bucket_defaults,
        ..Config::default()
    })
    .await
    .unwrap();
    let addr = server.addr();
    (server, addr)
}

async fn make_bucket(a: SocketAddr, name: &str) {
    let r = send(a, req("PUT", &format!("/{name}"))).await;
    assert_eq!(r.status, 200, "{}", r.body);
}

async fn get_sub(a: SocketAddr, bucket: &str, sub: &str) -> Resp {
    send(a, req("GET", &format!("/{bucket}")).query(sub)).await
}

async fn put_sub(a: SocketAddr, bucket: &str, sub: &str, xml: &str) -> Resp {
    send(a, req("PUT", &format!("/{bucket}")).query(sub).body(xml.as_bytes())).await
}

const ALL_BLOCKED: [&str; 4] = [
    "<BlockPublicAcls>true</BlockPublicAcls>",
    "<IgnorePublicAcls>true</IgnorePublicAcls>",
    "<BlockPublicPolicy>true</BlockPublicPolicy>",
    "<RestrictPublicBuckets>true</RestrictPublicBuckets>",
];

const PUBLIC_READ_POLICY: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt/*"}]}"#;

fn ownership(kind: &str) -> String {
    format!("<OwnershipControls><Rule><ObjectOwnership>{kind}</ObjectOwnership></Rule></OwnershipControls>")
}

#[tokio::test]
async fn legacy_is_the_default_and_keeps_acls_on_and_public_access_unblocked() {
    assert_eq!(Config::default().bucket_defaults, BucketDefaults::Legacy);
    let (_s, a) = start(BucketDefaults::Legacy).await;
    make_bucket(a, "bkt").await;
    assert_eq!(
        get_sub(a, "bkt", "ownershipControls").await.code(),
        "OwnershipControlsNotFoundError"
    );
    assert_eq!(
        get_sub(a, "bkt", "publicAccessBlock").await.code(),
        "NoSuchPublicAccessBlockConfiguration"
    );

    // ACLs work and a public policy can be attached straight away.
    let r = send(a, req("PUT", "/bkt/o").header("x-amz-acl", "public-read").body(b"hi")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(send(a, req("GET", "/bkt/o").anon()).await.body, "hi");
    let r = send(a, req("PUT", "/pub").header("x-amz-acl", "public-read")).await;
    assert_eq!(r.status, 200, "{}", r.body);
}

#[tokio::test]
async fn aws_new_buckets_have_acls_disabled_and_block_public_access_on() {
    let (_s, a) = start(BucketDefaults::Aws).await;
    make_bucket(a, "bkt").await;

    let r = get_sub(a, "bkt", "ownershipControls").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(
        r.body.contains("<ObjectOwnership>BucketOwnerEnforced</ObjectOwnership>"),
        "{}",
        r.body
    );
    let r = get_sub(a, "bkt", "publicAccessBlock").await;
    assert_eq!(r.status, 200, "{}", r.body);
    for setting in ALL_BLOCKED {
        assert!(r.body.contains(setting), "{setting} in {}", r.body);
    }

    // With ACLs disabled the ACL is read as owner-only; setting one fails with a 400, while the
    // owner-full-control canned ACL (and no ACL at all) is accepted.
    let r = get_sub(a, "bkt", "acl").await;
    assert!(r.status == 200 && !r.body.contains("AllUsers"), "{}", r.body);
    let r = send(a, req("PUT", "/bkt/o").header("x-amz-acl", "public-read").body(b"x")).await;
    assert_eq!((r.status, r.code()), (400, "AccessControlListNotSupported"), "{}", r.body);
    let r = send(a, req("PUT", "/bkt").query("acl").header("x-amz-acl", "public-read")).await;
    assert_eq!((r.status, r.code()), (400, "AccessControlListNotSupported"), "{}", r.body);
    for acl in [Some("bucket-owner-full-control"), None] {
        let mut put = req("PUT", "/bkt/ok").body(b"x");
        if let Some(acl) = acl {
            put = put.header("x-amz-acl", acl);
        }
        assert_eq!(send(a, put).await.status, 200);
    }
}

#[tokio::test]
async fn aws_create_bucket_with_acl_headers_fails_unless_ownership_is_set() {
    let (_s, a) = start(BucketDefaults::Aws).await;
    let public_read = ("x-amz-acl", "public-read");
    let grant = ("x-amz-grant-read", r#"uri="http://acs.amazonaws.com/groups/global/AllUsers""#);
    for (name, header) in [("canned", public_read), ("grant", grant)] {
        let bucket = format!("/{name}");
        let r = send(a, req("PUT", &bucket).header(header.0, header.1)).await;
        assert_eq!(
            (r.status, r.code()),
            (400, "InvalidBucketAclWithObjectOwnership"),
            "{name}: {}",
            r.body
        );
        assert_eq!(send(a, req("HEAD", &bucket)).await.status, 404, "{name} was created anyway");
    }
    // `private` gives full control only to the owner, which is what disabled ACLs means.
    let r = send(a, req("PUT", "/priv").header("x-amz-acl", "private")).await;
    assert_eq!(r.status, 200, "{}", r.body);

    // Asking for ACLs explicitly turns them on; Block Public Access still refuses public ACLs.
    let r = send(
        a,
        req("PUT", "/writer")
            .header("x-amz-object-ownership", "ObjectWriter")
            .header("x-amz-acl", "public-read"),
    )
    .await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"), "{}", r.body);
    assert_eq!(send(a, req("HEAD", "/writer")).await.status, 404);
    let r = send(
        a,
        req("PUT", "/writer")
            .header("x-amz-object-ownership", "ObjectWriter")
            .header("x-amz-acl", "authenticated-read"),
    )
    .await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"), "{}", r.body);
}

#[tokio::test]
async fn aws_create_bucket_with_explicit_ownership() {
    let (_s, a) = start(BucketDefaults::Aws).await;
    for kind in ["ObjectWriter", "BucketOwnerPreferred", "BucketOwnerEnforced"] {
        let name = format!("b-{}", kind.to_ascii_lowercase());
        let r = send(a, req("PUT", &format!("/{name}")).header("x-amz-object-ownership", kind)).await;
        assert_eq!(r.status, 200, "{kind}: {}", r.body);
        let r = get_sub(a, &name, "ownershipControls").await;
        assert!(
            r.body.contains(&format!("<ObjectOwnership>{kind}</ObjectOwnership>")),
            "{kind}: {}",
            r.body
        );
        // Block Public Access is on whatever the ownership setting is.
        let r = get_sub(a, &name, "publicAccessBlock").await;
        assert!(ALL_BLOCKED.iter().all(|s| r.body.contains(s)), "{kind}: {}", r.body);
        // ACLs can be set exactly when the setting enables them (a private one, with BPA on).
        let r = send(a, req("PUT", &format!("/{name}/o")).header("x-amz-acl", "private").body(b"x")).await;
        assert_eq!(r.status == 200, kind != "BucketOwnerEnforced", "{kind}: {} {}", r.status, r.body);
    }
}

#[tokio::test]
async fn aws_anonymous_access_needs_every_default_relaxed() {
    let (_s, a) = start(BucketDefaults::Aws).await;
    make_bucket(a, "bkt").await;
    send(a, req("PUT", "/bkt/o").body(b"hello")).await;
    assert_eq!(send(a, req("GET", "/bkt/o").anon()).await.status, 403);

    // Block Public Access: a public policy cannot even be attached.
    let policy = |a| send(a, req("PUT", "/bkt").query("policy").body(PUBLIC_READ_POLICY.as_bytes()));
    let r = policy(a).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"), "{}", r.body);
    assert_eq!(send(a, req("GET", "/bkt/o").anon()).await.status, 403);

    // Relax Block Public Access: the policy is accepted and anonymous reads work.
    assert_eq!(send(a, req("DELETE", "/bkt").query("publicAccessBlock")).await.status, 204);
    assert_eq!(
        get_sub(a, "bkt", "publicAccessBlock").await.code(),
        "NoSuchPublicAccessBlockConfiguration"
    );
    assert_eq!(policy(a).await.status, 204);
    let r = send(a, req("GET", "/bkt/o").anon()).await;
    assert_eq!((r.status, r.body.as_str()), (200, "hello"));
    send(a, req("DELETE", "/bkt").query("policy")).await;
    assert_eq!(send(a, req("GET", "/bkt/o").anon()).await.status, 403);

    // ACL route: still refused while ownership is enforced, even with Block Public Access gone.
    let r = send(a, req("PUT", "/bkt/shared").header("x-amz-acl", "public-read").body(b"acl")).await;
    assert_eq!(r.code(), "AccessControlListNotSupported");
    assert_eq!(put_sub(a, "bkt", "ownershipControls", &ownership("ObjectWriter")).await.status, 200);
    let r = send(a, req("PUT", "/bkt/shared").header("x-amz-acl", "public-read").body(b"acl")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(send(a, req("GET", "/bkt/shared").anon()).await.body, "acl");

    // DeleteBucketOwnershipControls re-enables ACLs too, which is how AWS documents opting out.
    make_bucket(a, "other").await;
    send(a, req("DELETE", "/other").query("publicAccessBlock")).await;
    assert_eq!(send(a, req("DELETE", "/other").query("ownershipControls")).await.status, 204);
    let r = send(a, req("PUT", "/other/o").header("x-amz-acl", "public-read").body(b"x")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(send(a, req("GET", "/other/o").anon()).await.status, 200);
}

#[tokio::test]
async fn defaults_apply_at_creation_only_snapshots_and_resets_keep_bucket_state() {
    let (s, a) = start(BucketDefaults::Aws).await;
    make_bucket(a, "relaxed").await;
    make_bucket(a, "strict").await;
    send(a, req("DELETE", "/relaxed").query("publicAccessBlock")).await;
    send(a, req("DELETE", "/relaxed").query("ownershipControls")).await;
    s.snapshot(Some("mixed")).unwrap();

    // Tighten the relaxed bucket and drop the strict one, then roll back.
    put_sub(a, "relaxed", "ownershipControls", &ownership("BucketOwnerEnforced")).await;
    send(a, req("DELETE", "/strict")).await;
    s.restore("mixed").unwrap();
    assert_eq!(
        get_sub(a, "relaxed", "publicAccessBlock").await.code(),
        "NoSuchPublicAccessBlockConfiguration"
    );
    assert_eq!(
        get_sub(a, "relaxed", "ownershipControls").await.code(),
        "OwnershipControlsNotFoundError"
    );
    let r = get_sub(a, "strict", "publicAccessBlock").await;
    assert!(ALL_BLOCKED.iter().all(|s| r.body.contains(s)), "{}", r.body);
    assert!(get_sub(a, "strict", "ownershipControls").await.body.contains("BucketOwnerEnforced"));

    // The mode survives a reset: buckets created afterwards start from the defaults again.
    s.reset();
    make_bucket(a, "relaxed").await;
    let r = get_sub(a, "relaxed", "publicAccessBlock").await;
    assert!(ALL_BLOCKED.iter().all(|s| r.body.contains(s)), "{}", r.body);
    // And restoring puts the relaxed state back untouched.
    s.restore("mixed").unwrap();
    assert_eq!(
        get_sub(a, "relaxed", "publicAccessBlock").await.code(),
        "NoSuchPublicAccessBlockConfiguration"
    );
}

#[tokio::test]
async fn control_plane_reports_the_mode() {
    for (mode, name) in [(BucketDefaults::Legacy, "legacy"), (BucketDefaults::Aws, "aws")] {
        let (_s, a) = start(mode).await;
        let r = send(a, req("GET", "/_fakes3")).await;
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["bucket_defaults"], name, "{}", r.body);
    }
}
