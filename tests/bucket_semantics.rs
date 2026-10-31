//! End-to-end tests for the bucket-level S3 semantics, over real sockets with SigV4 auth on.

mod common;

use std::net::SocketAddr;
use std::time::SystemTime;

use common::{Resp, req, send};
use fakes3::{Config, Server};

const DAY_NANOS: i64 = 86_400 * 1_000_000_000;

async fn start() -> (Server, SocketAddr) {
    let server = Server::start(Config::default()).await.unwrap();
    let addr = server.addr();
    (server, addr)
}

async fn make_bucket(addr: SocketAddr, name: &str) {
    let r = send(addr, req("PUT", &format!("/{name}"))).await;
    assert_eq!(r.status, 200, "{}", r.body);
}

async fn put(addr: SocketAddr, path: &str, body: &[u8]) -> Resp {
    send(addr, req("PUT", path).body(body)).await
}

async fn put_config(addr: SocketAddr, bucket: &str, sub: &str, xml: &str) -> Resp {
    send(addr, req("PUT", &format!("/{bucket}")).query(sub).body(xml.as_bytes())).await
}

const PUBLIC_READ_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"r","Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt/pub/*"}]}"#;

#[tokio::test]
async fn anonymous_access_needs_policy_or_acl() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    put(a, "/bkt/pub/x", b"hello").await;
    put(a, "/bkt/priv/x", b"secret").await;

    // Unsigned requests are refused by default.
    assert_eq!(send(a, req("GET", "/bkt/pub/x").anon()).await.status, 403);
    assert_eq!(send(a, req("GET", "/").anon()).await.code(), "AccessDenied");

    let r = send(a, req("PUT", "/bkt").query("policy").body(PUBLIC_READ_POLICY.as_bytes())).await;
    assert_eq!(r.status, 204, "{}", r.body);
    let r = send(a, req("GET", "/bkt/pub/x").anon()).await;
    assert_eq!((r.status, r.body.as_str()), (200, "hello"));
    assert_eq!(send(a, req("GET", "/bkt/priv/x").anon()).await.status, 403);
    // Allow GetObject only: no listing, no writes.
    assert_eq!(send(a, req("GET", "/bkt").anon()).await.status, 403);
    assert_eq!(send(a, req("PUT", "/bkt/pub/y").anon().body(b"z")).await.status, 403);

    // The policy round-trips and reports itself public.
    let r = send(a, req("GET", "/bkt").query("policy")).await;
    assert_eq!(r.body, PUBLIC_READ_POLICY);
    let r = send(a, req("GET", "/bkt").query("policyStatus")).await;
    assert!(r.body.contains("<IsPublic>true</IsPublic>"), "{}", r.body);

    let r = send(a, req("DELETE", "/bkt").query("policy")).await;
    assert_eq!(r.status, 204);
    assert_eq!(send(a, req("GET", "/bkt/pub/x").anon()).await.status, 403);
    assert_eq!(send(a, req("GET", "/bkt").query("policy")).await.code(), "NoSuchBucketPolicy");
}

#[tokio::test]
async fn explicit_deny_beats_everything_but_cannot_lock_out_policy_admin() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    put(a, "/bkt/open", b"1").await;
    put(a, "/bkt/locked/x", b"2").await;
    let policy = r#"{"Statement":[
        {"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt/*"},
        {"Effect":"Deny","Principal":"*","Action":["s3:GetObject","s3:DeleteObject"],"Resource":"arn:aws:s3:::bkt/locked/*"}]}"#;
    assert_eq!(
        send(a, req("PUT", "/bkt").query("policy").body(policy.as_bytes())).await.status,
        204
    );

    assert_eq!(send(a, req("GET", "/bkt/open").anon()).await.status, 200);
    assert_eq!(send(a, req("GET", "/bkt/open")).await.status, 200);
    // Even the signed owner is denied.
    assert_eq!(send(a, req("GET", "/bkt/locked/x")).await.status, 403);
    assert_eq!(send(a, req("GET", "/bkt/locked/x").anon()).await.status, 403);
    assert_eq!(send(a, req("DELETE", "/bkt/locked/x")).await.status, 403);
    assert_eq!(put(a, "/bkt/locked/y", b"3").await.status, 200, "PutObject is not denied");

    // DeleteObjects reports the denied key and deletes the rest.
    put(a, "/bkt/del", b"4").await;
    let body = "<Delete><Object><Key>locked/x</Key></Object><Object><Key>del</Key></Object></Delete>";
    let r = send(a, req("POST", "/bkt").query("delete").body(body.as_bytes())).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.body.contains("<Error><Code>AccessDenied</Code><Key>locked/x</Key>"), "{}", r.body);
    assert!(r.body.contains("<Deleted><Key>del</Key></Deleted>"), "{}", r.body);

    // A deny-everything policy can still be replaced by the owner.
    let deny_all =
        r#"{"Statement":[{"Effect":"Deny","Principal":"*","Action":"s3:*","Resource":["arn:aws:s3:::bkt","arn:aws:s3:::bkt/*"]}]}"#;
    assert_eq!(
        send(a, req("PUT", "/bkt").query("policy").body(deny_all.as_bytes())).await.status,
        204
    );
    assert_eq!(send(a, req("GET", "/bkt/open")).await.status, 403);
    assert_eq!(send(a, req("DELETE", "/bkt").query("policy")).await.status, 204);
    assert_eq!(send(a, req("GET", "/bkt/open")).await.status, 200);
}

#[tokio::test]
async fn policy_conditions_and_validation() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    put(a, "/bkt/logs/1", b"x").await;
    let listing = r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:ListBucket","Resource":"arn:aws:s3:::bkt",
        "Condition":{"StringLike":{"s3:prefix":"logs/*"}}}]}"#;
    assert_eq!(
        send(a, req("PUT", "/bkt").query("policy").body(listing.as_bytes())).await.status,
        204
    );
    let r = send(a, req("GET", "/bkt").query("list-type=2&prefix=logs%2F").anon()).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(
        send(a, req("GET", "/bkt").query("list-type=2&prefix=other%2F").anon()).await.status,
        403
    );
    assert_eq!(send(a, req("GET", "/bkt").query("list-type=2").anon()).await.status, 403);
    // Conditioned on a prefix, the policy is still "public" (not restricted by caller or network).
    let r = send(a, req("GET", "/bkt").query("policyStatus")).await;
    assert!(r.body.contains("<IsPublic>true</IsPublic>"));

    for bad in [
        "not json",
        r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt/*"}]}"#,
        r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::other/*"}]}"#,
        r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt"}]}"#,
    ] {
        let r = send(a, req("PUT", "/bkt").query("policy").body(bad.as_bytes())).await;
        assert_eq!((r.status, r.code()), (400, "MalformedPolicy"), "{bad}: {}", r.body);
    }
}

#[tokio::test]
async fn policy_conditions_read_the_server_clock() {
    let (s, a) = start().await;
    make_bucket(a, "bkt").await;
    put(a, "/bkt/k", b"x").await;
    let until = r#"{"Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt/*",
        "Condition":{"DateLessThan":{"aws:CurrentTime":"2099-01-01T00:00:00Z"}}}]}"#;
    send(a, req("PUT", "/bkt").query("policy").body(until.as_bytes())).await;
    assert_eq!(send(a, req("GET", "/bkt/k").anon()).await.status, 200);
    s.clock().advance(100 * 366 * DAY_NANOS);
    assert_eq!(send(a, req("GET", "/bkt/k").anon()).await.status, 403);
}

#[tokio::test]
async fn source_ip_conditions() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    put(a, "/bkt/k", b"x").await;
    let policy = |cidr: &str| {
        format!(
            r#"{{"Statement":[{{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt/*",
            "Condition":{{"IpAddress":{{"aws:SourceIp":"{cidr}"}}}}}}]}}"#
        )
    };
    send(a, req("PUT", "/bkt").query("policy").body(policy("127.0.0.0/8").as_bytes())).await;
    assert_eq!(send(a, req("GET", "/bkt/k").anon()).await.status, 200);
    send(a, req("PUT", "/bkt").query("policy").body(policy("10.0.0.0/8").as_bytes())).await;
    assert_eq!(send(a, req("GET", "/bkt/k").anon()).await.status, 403);
}

#[tokio::test]
async fn acls_grant_anonymous_access() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    let r = send(a, req("PUT", "/bkt/shared").header("x-amz-acl", "public-read").body(b"hi")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    put(a, "/bkt/private", b"no").await;

    assert_eq!(send(a, req("GET", "/bkt/shared").anon()).await.body, "hi");
    assert_eq!(send(a, req("GET", "/bkt/private").anon()).await.status, 403);
    // Object READ does not allow writes or listing.
    assert_eq!(send(a, req("PUT", "/bkt/shared").anon().body(b"x")).await.status, 403);
    assert_eq!(send(a, req("GET", "/bkt").anon()).await.status, 403);

    let r = send(a, req("GET", "/bkt/shared").query("acl")).await;
    assert!(r.body.contains("http://acs.amazonaws.com/groups/global/AllUsers"), "{}", r.body);
    assert!(r.body.contains("<Permission>READ</Permission>") && r.body.contains("<Permission>FULL_CONTROL</Permission>"));
    let r = send(a, req("GET", "/bkt/private").query("acl")).await;
    assert!(!r.body.contains("AllUsers"), "{}", r.body);

    // Upgrade an object through PutObjectAcl, with a grant header.
    let r = send(
        a,
        req("PUT", "/bkt/private")
            .query("acl")
            .header("x-amz-grant-read", r#"uri="http://acs.amazonaws.com/groups/global/AllUsers""#),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(send(a, req("GET", "/bkt/private").anon()).await.status, 200);

    // Bucket ACL: public-read-write allows anonymous listing and writing.
    assert_eq!(
        send(a, req("PUT", "/bkt").query("acl").header("x-amz-acl", "public-read-write"))
            .await
            .status,
        200
    );
    assert_eq!(send(a, req("GET", "/bkt").query("list-type=2").anon()).await.status, 200);
    assert_eq!(send(a, req("PUT", "/bkt/anon").anon().body(b"w")).await.status, 200);
    assert_eq!(send(a, req("GET", "/bkt").query("acl")).await.body.matches("<Grant>").count(), 3);

    // An overwrite resets the object's ACL.
    put(a, "/bkt/shared", b"again").await;
    assert_eq!(send(a, req("GET", "/bkt/shared").anon()).await.status, 403);

    // A bad canned ACL and mixing styles are rejected.
    let r = send(a, req("PUT", "/bkt/o").header("x-amz-acl", "nope").body(b"")).await;
    assert_eq!((r.status, r.code()), (400, "InvalidArgument"));
    let r = send(
        a,
        req("PUT", "/bkt")
            .query("acl")
            .header("x-amz-acl", "private")
            .header("x-amz-grant-read", "id=\"fakes3\""),
    )
    .await;
    assert_eq!((r.status, r.code()), (400, "InvalidRequest"));
}

#[tokio::test]
async fn acl_body_round_trip() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    let xml = r#"<AccessControlPolicy><Owner><ID>fakes3</ID></Owner><AccessControlList>
        <Grant><Grantee xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="CanonicalUser"><ID>fakes3</ID></Grantee><Permission>FULL_CONTROL</Permission></Grant>
        <Grant><Grantee xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="Group"><URI>http://acs.amazonaws.com/groups/global/AllUsers</URI></Grantee><Permission>READ</Permission></Grant>
        </AccessControlList></AccessControlPolicy>"#;
    let r = send(a, req("PUT", "/bkt").query("acl").body(xml.as_bytes())).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(send(a, req("GET", "/bkt").query("list-type=2").anon()).await.status, 200);
    let r = send(a, req("GET", "/bkt").query("acl")).await;
    assert!(
        r.body.contains("AllUsers") && r.body.contains("<DisplayName>fakes3</DisplayName>"),
        "{}",
        r.body
    );
    // Body and headers together are not allowed.
    let r = send(
        a,
        req("PUT", "/bkt").query("acl").header("x-amz-acl", "private").body(xml.as_bytes()),
    )
    .await;
    assert_eq!((r.status, r.code()), (400, "UnexpectedContent"), "{}", r.body);
}

#[tokio::test]
async fn ownership_controls_and_public_access_block() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    assert_eq!(
        send(a, req("GET", "/bkt").query("ownershipControls")).await.code(),
        "OwnershipControlsNotFoundError"
    );
    assert_eq!(
        send(a, req("GET", "/bkt").query("publicAccessBlock")).await.code(),
        "NoSuchPublicAccessBlockConfiguration"
    );

    let enforced = "<OwnershipControls><Rule><ObjectOwnership>BucketOwnerEnforced</ObjectOwnership></Rule></OwnershipControls>";
    assert_eq!(put_config(a, "bkt", "ownershipControls", enforced).await.status, 200);
    let r = send(a, req("GET", "/bkt").query("ownershipControls")).await;
    assert!(
        r.body.contains("<ObjectOwnership>BucketOwnerEnforced</ObjectOwnership>"),
        "{}",
        r.body
    );

    // ACLs are disabled: setting one fails, reading returns the default, the owner-full-control canned ACL is accepted.
    let r = send(a, req("PUT", "/bkt/o").header("x-amz-acl", "public-read").body(b"x")).await;
    assert_eq!((r.status, r.code()), (400, "AccessControlListNotSupported"), "{}", r.body);
    let r = send(a, req("PUT", "/bkt").query("acl").header("x-amz-acl", "public-read")).await;
    assert_eq!(r.code(), "AccessControlListNotSupported");
    assert_eq!(
        send(a, req("PUT", "/bkt/o").header("x-amz-acl", "bucket-owner-full-control").body(b"x"))
            .await
            .status,
        200
    );
    assert!(!send(a, req("GET", "/bkt/o").query("acl")).await.body.contains("AllUsers"));

    // Back to ObjectWriter: ACLs work again; then BlockPublicAcls and IgnorePublicAcls apply.
    let writer = "<OwnershipControls><Rule><ObjectOwnership>ObjectWriter</ObjectOwnership></Rule></OwnershipControls>";
    put_config(a, "bkt", "ownershipControls", writer).await;
    let block = |acls: bool, ignore: bool| {
        format!(
            "<PublicAccessBlockConfiguration><BlockPublicAcls>{acls}</BlockPublicAcls><IgnorePublicAcls>{ignore}</IgnorePublicAcls>\
             <BlockPublicPolicy>true</BlockPublicPolicy><RestrictPublicBuckets>false</RestrictPublicBuckets></PublicAccessBlockConfiguration>"
        )
    };
    assert_eq!(put_config(a, "bkt", "publicAccessBlock", &block(true, false)).await.status, 200);
    let r = send(a, req("GET", "/bkt").query("publicAccessBlock")).await;
    assert!(
        r.body.contains("<BlockPublicAcls>true</BlockPublicAcls>")
            && r.body.contains("<RestrictPublicBuckets>false</RestrictPublicBuckets>")
    );
    let r = send(a, req("PUT", "/bkt/o").header("x-amz-acl", "public-read").body(b"x")).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"));
    // BlockPublicPolicy refuses a public policy.
    let r = send(a, req("PUT", "/bkt").query("policy").body(PUBLIC_READ_POLICY.as_bytes())).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"));

    // IgnorePublicAcls: an ACL that was set earlier stops working.
    put_config(a, "bkt", "publicAccessBlock", &block(false, false)).await;
    send(a, req("PUT", "/bkt/shared").header("x-amz-acl", "public-read").body(b"x")).await;
    assert_eq!(send(a, req("GET", "/bkt/shared").anon()).await.status, 200);
    put_config(a, "bkt", "publicAccessBlock", &block(false, true)).await;
    assert_eq!(send(a, req("GET", "/bkt/shared").anon()).await.status, 403);

    assert_eq!(send(a, req("DELETE", "/bkt").query("publicAccessBlock")).await.status, 204);
    assert_eq!(send(a, req("GET", "/bkt/shared").anon()).await.status, 200);
    assert_eq!(send(a, req("DELETE", "/bkt").query("ownershipControls")).await.status, 204);
}

#[tokio::test]
async fn restrict_public_buckets_blocks_public_policy_access() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    put(a, "/bkt/pub/x", b"1").await;
    send(a, req("PUT", "/bkt").query("policy").body(PUBLIC_READ_POLICY.as_bytes())).await;
    assert_eq!(send(a, req("GET", "/bkt/pub/x").anon()).await.status, 200);
    let restrict = "<PublicAccessBlockConfiguration><RestrictPublicBuckets>true</RestrictPublicBuckets></PublicAccessBlockConfiguration>";
    put_config(a, "bkt", "publicAccessBlock", restrict).await;
    assert_eq!(send(a, req("GET", "/bkt/pub/x").anon()).await.status, 403);
    assert_eq!(send(a, req("GET", "/bkt/pub/x")).await.status, 200);
}

#[tokio::test]
async fn create_bucket_with_ownership_and_acl() {
    let (_s, a) = start().await;
    let r = send(
        a,
        req("PUT", "/ebk")
            .header("x-amz-object-ownership", "BucketOwnerEnforced")
            .header("x-amz-acl", "public-read"),
    )
    .await;
    assert_eq!((r.status, r.code()), (400, "InvalidBucketAclWithObjectOwnership"), "{}", r.body);
    let r = send(a, req("PUT", "/ebk").header("x-amz-object-ownership", "BucketOwnerEnforced")).await;
    assert_eq!(r.status, 200);
    assert!(
        send(a, req("GET", "/ebk").query("ownershipControls"))
            .await
            .body
            .contains("BucketOwnerEnforced")
    );
    let r = send(a, req("PUT", "/pbk").header("x-amz-acl", "public-read")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(send(a, req("GET", "/pbk").query("list-type=2").anon()).await.status, 200);
}

#[tokio::test]
async fn cors_preflight_and_actual_requests() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    put(a, "/bkt/k", b"x").await;
    let preflight = |origin: &'static str, method: &'static str| {
        req("OPTIONS", "/bkt/k")
            .anon()
            .header("origin", origin)
            .header("access-control-request-method", method)
            .header("access-control-request-headers", "x-amz-meta-a, content-type")
    };
    let r = send(a, preflight("https://app.test", "GET")).await;
    assert_eq!((r.status, r.code()), (403, "AccessForbidden"));
    assert!(r.body.contains("CORS is not enabled for this bucket."));
    assert_eq!(send(a, req("GET", "/bkt").query("cors")).await.code(), "NoSuchCORSConfiguration");

    let cfg = r#"<CORSConfiguration>
        <CORSRule><ID>one</ID><AllowedOrigin>https://*.app.test</AllowedOrigin><AllowedMethod>GET</AllowedMethod><AllowedMethod>PUT</AllowedMethod>
          <AllowedHeader>x-amz-*</AllowedHeader><AllowedHeader>content-type</AllowedHeader><ExposeHeader>ETag</ExposeHeader><MaxAgeSeconds>3000</MaxAgeSeconds></CORSRule>
        <CORSRule><AllowedOrigin>*</AllowedOrigin><AllowedMethod>GET</AllowedMethod></CORSRule></CORSConfiguration>"#;
    assert_eq!(put_config(a, "bkt", "cors", cfg).await.status, 200);
    let r = send(a, req("GET", "/bkt").query("cors")).await;
    assert!(
        r.body.contains("<ID>one</ID>") && r.body.matches("<CORSRule>").count() == 2,
        "{}",
        r.body
    );

    let r = send(a, preflight("https://www.app.test", "PUT")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.header("access-control-allow-origin"), Some("https://www.app.test"));
    assert_eq!(r.header("access-control-allow-methods"), Some("GET, PUT"));
    assert_eq!(r.header("access-control-allow-headers"), Some("x-amz-meta-a, content-type"));
    assert_eq!(r.header("access-control-max-age"), Some("3000"));
    assert_eq!(r.header("access-control-expose-headers"), Some("ETag"));
    assert!(r.header("vary").unwrap().contains("Origin"));

    // PUT from another origin matches no rule; GET falls through to the wildcard rule.
    assert_eq!(send(a, preflight("https://other.test", "PUT")).await.status, 403);
    let r = send(
        a,
        req("OPTIONS", "/bkt/k")
            .anon()
            .header("origin", "https://other.test")
            .header("access-control-request-method", "GET"),
    )
    .await;
    assert_eq!((r.status, r.header("access-control-allow-origin")), (200, Some("*")));
    // Missing Origin.
    assert_eq!(send(a, req("OPTIONS", "/bkt/k").anon()).await.status, 400);

    // Actual requests get the headers, including on errors.
    let r = send(a, req("GET", "/bkt/k").header("origin", "https://www.app.test")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("access-control-allow-origin"), Some("https://www.app.test"));
    assert_eq!(r.header("access-control-expose-headers"), Some("ETag"));
    let r = send(a, req("GET", "/bkt/missing").header("origin", "https://www.app.test")).await;
    assert_eq!(r.status, 404);
    assert_eq!(r.header("access-control-allow-origin"), Some("https://www.app.test"));
    let r = send(a, req("GET", "/bkt/k")).await;
    assert!(r.header("access-control-allow-origin").is_none());
    let r = send(a, req("PUT", "/bkt/k").header("origin", "https://other.test").body(b"y")).await;
    assert!(r.header("access-control-allow-origin").is_none());

    let bad =
        "<CORSConfiguration><CORSRule><AllowedOrigin>*</AllowedOrigin><AllowedMethod>PATCH</AllowedMethod></CORSRule></CORSConfiguration>";
    let r = put_config(a, "bkt", "cors", bad).await;
    assert_eq!((r.status, r.code()), (400, "InvalidRequest"), "{}", r.body);

    assert_eq!(send(a, req("DELETE", "/bkt").query("cors")).await.status, 204);
    assert_eq!(send(a, preflight("https://www.app.test", "GET")).await.status, 403);
}

fn lifecycle_xml(rules: &str) -> String {
    format!("<LifecycleConfiguration>{rules}</LifecycleConfiguration>")
}

#[tokio::test]
async fn lifecycle_round_trip_and_validation() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    assert_eq!(
        send(a, req("GET", "/bkt").query("lifecycle")).await.code(),
        "NoSuchLifecycleConfiguration"
    );
    let xml = lifecycle_xml(
        "<Rule><ID>r1</ID><Filter><Prefix>tmp/</Prefix></Filter><Status>Enabled</Status><Expiration><Days>1</Days></Expiration></Rule>\
         <Rule><ID>r2</ID><Filter><Prefix>a/</Prefix></Filter><Status>Disabled</Status>\
         <NoncurrentVersionExpiration><NoncurrentDays>3</NoncurrentDays><NewerNoncurrentVersions>2</NewerNoncurrentVersions></NoncurrentVersionExpiration>\
         <AbortIncompleteMultipartUpload><DaysAfterInitiation>7</DaysAfterInitiation></AbortIncompleteMultipartUpload></Rule>",
    );
    let r = put_config(a, "bkt", "lifecycle", &xml).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let r = send(a, req("GET", "/bkt").query("lifecycle")).await;
    assert!(
        r.body.contains("<ID>r1</ID>") && r.body.contains("<NewerNoncurrentVersions>2</NewerNoncurrentVersions>"),
        "{}",
        r.body
    );

    for (bad, code) in [
        (lifecycle_xml("<Rule><Filter/><Status>Enabled</Status></Rule>"), "InvalidRequest"),
        (
            lifecycle_xml("<Rule><Filter/><Status>Enabled</Status><Expiration><Days>0</Days></Expiration></Rule>"),
            "InvalidArgument",
        ),
        (
            lifecycle_xml(
                "<Rule><ID>x</ID><Filter/><Status>Enabled</Status><Expiration><Days>1</Days></Expiration></Rule>\
                 <Rule><ID>x</ID><Filter/><Status>Enabled</Status><Expiration><Days>2</Days></Expiration></Rule>",
            ),
            "InvalidArgument",
        ),
        (
            lifecycle_xml("<Rule><Filter/><Status>Enabled</Status><Expiration><Date>2030-01-01T10:00:00.000Z</Date></Expiration></Rule>"),
            "InvalidArgument",
        ),
    ] {
        let r = put_config(a, "bkt", "lifecycle", &bad).await;
        assert_eq!((r.status, r.code()), (400, code), "{bad}: {}", r.body);
    }
    assert_eq!(send(a, req("DELETE", "/bkt").query("lifecycle")).await.status, 204);
    assert_eq!(send(a, req("GET", "/bkt").query("lifecycle")).await.status, 404);
}

#[tokio::test]
async fn lifecycle_expires_current_objects_when_the_clock_moves() {
    let (s, a) = start().await;
    make_bucket(a, "bkt").await;
    let xml = lifecycle_xml(
        "<Rule><ID>tmp</ID><Filter><Prefix>tmp/</Prefix></Filter><Status>Enabled</Status><Expiration><Days>2</Days></Expiration></Rule>",
    );
    put_config(a, "bkt", "lifecycle", &xml).await;
    let r = put(a, "/bkt/tmp/x", b"1").await;
    assert!(r.header("x-amz-expiration").unwrap().contains("rule-id=\"tmp\""), "{:?}", r.headers);
    put(a, "/bkt/keep/x", b"2").await;
    let r = send(a, req("HEAD", "/bkt/tmp/x")).await;
    assert!(r.header("x-amz-expiration").is_some());
    assert!(send(a, req("HEAD", "/bkt/keep/x")).await.header("x-amz-expiration").is_none());

    s.clock().advance(DAY_NANOS);
    assert_eq!(send(a, req("GET", "/bkt/tmp/x")).await.status, 200);
    // 2 days + rounding to midnight: 4 days is past the deadline wherever "now" falls in the day.
    s.clock().advance(3 * DAY_NANOS);
    assert_eq!(send(a, req("GET", "/bkt/tmp/x")).await.status, 404);
    assert_eq!(send(a, req("GET", "/bkt/keep/x")).await.status, 200);
    let r = send(a, req("GET", "/bkt").query("list-type=2")).await;
    assert!(!r.body.contains("tmp/x") && r.body.contains("keep/x"), "{}", r.body);
}

#[tokio::test]
async fn lifecycle_on_versioned_buckets() {
    let (s, a) = start().await;
    make_bucket(a, "bkt").await;
    let on = "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>";
    put_config(a, "bkt", "versioning", on).await;
    put(a, "/bkt/k", b"v1").await;
    put(a, "/bkt/k", b"v2").await;
    put(a, "/bkt/k", b"v3").await;
    let xml = lifecycle_xml(
        "<Rule><ID>cur</ID><Filter/><Status>Enabled</Status><Expiration><Days>1</Days></Expiration>\
         <NoncurrentVersionExpiration><NoncurrentDays>5</NoncurrentDays><NewerNoncurrentVersions>1</NewerNoncurrentVersions></NoncurrentVersionExpiration></Rule>\
         <Rule><ID>clean</ID><Filter/><Status>Enabled</Status><Expiration><ExpiredObjectDeleteMarker>true</ExpiredObjectDeleteMarker></Expiration></Rule>",
    );
    put_config(a, "bkt", "lifecycle", &xml).await;

    s.clock().advance(3 * DAY_NANOS);
    // The current version expired: a delete marker hides the key, old versions remain.
    assert_eq!(send(a, req("GET", "/bkt/k")).await.status, 404);
    let r = send(a, req("GET", "/bkt").query("versions")).await;
    assert_eq!(r.body.matches("<Version>").count(), 3, "{}", r.body);
    assert_eq!(r.body.matches("<DeleteMarker>").count(), 1, "{}", r.body);

    // Once the versions have been noncurrent for 5 days the newest noncurrent one is kept (NewerNoncurrentVersions=1)... until
    // the marker is also removed, which only happens when nothing is behind it.
    s.clock().advance(10 * DAY_NANOS);
    let r = send(a, req("GET", "/bkt").query("versions")).await;
    assert_eq!(r.body.matches("<Version>").count(), 1, "{}", r.body);
    assert_eq!(r.body.matches("<DeleteMarker>").count(), 1, "{}", r.body);
}

#[tokio::test]
async fn lifecycle_aborts_incomplete_multipart_uploads() {
    let (s, a) = start().await;
    make_bucket(a, "bkt").await;
    let r = send(a, req("POST", "/bkt/up").query("uploads")).await;
    let id = r
        .body
        .split("<UploadId>")
        .nth(1)
        .unwrap()
        .split("</UploadId>")
        .next()
        .unwrap()
        .to_owned();
    let xml = lifecycle_xml(
        "<Rule><Filter/><Status>Enabled</Status><AbortIncompleteMultipartUpload><DaysAfterInitiation>2</DaysAfterInitiation></AbortIncompleteMultipartUpload></Rule>",
    );
    put_config(a, "bkt", "lifecycle", &xml).await;
    assert!(send(a, req("GET", "/bkt").query("uploads")).await.body.contains(&id));
    s.clock().advance(4 * DAY_NANOS);
    assert!(!send(a, req("GET", "/bkt").query("uploads")).await.body.contains(&id));
    let r = send(a, req("PUT", "/bkt/up").query(&format!("partNumber=1&uploadId={id}")).body(b"x")).await;
    assert_eq!(r.code(), "NoSuchUpload");
}

#[tokio::test]
async fn website_and_notification_round_trip() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    assert_eq!(
        send(a, req("GET", "/bkt").query("website")).await.code(),
        "NoSuchWebsiteConfiguration"
    );
    let site = "<WebsiteConfiguration><IndexDocument><Suffix>index.html</Suffix></IndexDocument><ErrorDocument><Key>err.html</Key></ErrorDocument>\
        <RoutingRules><RoutingRule><Condition><KeyPrefixEquals>docs/</KeyPrefixEquals></Condition><Redirect><ReplaceKeyPrefixWith>documents/</ReplaceKeyPrefixWith></Redirect></RoutingRule></RoutingRules></WebsiteConfiguration>";
    assert_eq!(put_config(a, "bkt", "website", site).await.status, 200);
    let r = send(a, req("GET", "/bkt").query("website")).await;
    assert!(
        r.body.contains("<Suffix>index.html</Suffix>") && r.body.contains("<Key>err.html</Key>") && r.body.contains("documents/"),
        "{}",
        r.body
    );
    let redirect = "<WebsiteConfiguration><RedirectAllRequestsTo><HostName>example.com</HostName><Protocol>https</Protocol></RedirectAllRequestsTo></WebsiteConfiguration>";
    assert_eq!(put_config(a, "bkt", "website", redirect).await.status, 200);
    assert!(
        send(a, req("GET", "/bkt").query("website"))
            .await
            .body
            .contains("<HostName>example.com</HostName>")
    );
    let bad = "<WebsiteConfiguration><IndexDocument><Suffix>a/b.html</Suffix></IndexDocument></WebsiteConfiguration>";
    assert_eq!(put_config(a, "bkt", "website", bad).await.code(), "InvalidArgument");
    assert_eq!(send(a, req("DELETE", "/bkt").query("website")).await.status, 204);
    assert_eq!(send(a, req("GET", "/bkt").query("website")).await.status, 404);

    let empty = send(a, req("GET", "/bkt").query("notification")).await;
    assert_eq!(empty.status, 200);
    assert!(!empty.body.contains("QueueConfiguration"));
    let n = "<NotificationConfiguration><QueueConfiguration><Id>q1</Id><Queue>arn:aws:sqs:us-east-1:000000000000:q</Queue>\
        <Event>s3:ObjectCreated:*</Event><Filter><S3Key><FilterRule><Name>prefix</Name><Value>in/</Value></FilterRule></S3Key></Filter></QueueConfiguration></NotificationConfiguration>";
    assert_eq!(put_config(a, "bkt", "notification", n).await.status, 200);
    let r = send(a, req("GET", "/bkt").query("notification")).await;
    assert!(
        r.body.contains("<Id>q1</Id>") && r.body.contains("s3:ObjectCreated:*") && r.body.contains("<Value>in/</Value>"),
        "{}",
        r.body
    );
    let bad = "<NotificationConfiguration><QueueConfiguration><Queue>not-an-arn</Queue><Event>s3:ObjectCreated:*</Event></QueueConfiguration></NotificationConfiguration>";
    assert_eq!(put_config(a, "bkt", "notification", bad).await.status, 400);
}

#[tokio::test]
async fn virtual_host_addressing() {
    let (_s, a) = start().await;
    make_bucket(a, "vhb").await;
    put(a, "/vhb/obj", b"data").await;
    let host = format!("vhb.localhost:{}", a.port());
    let r = send(a, req("GET", "/obj").host(&host)).await;
    assert_eq!((r.status, r.body.as_str()), (200, "data"), "{}", r.body);
    let r = send(a, req("PUT", "/new").host(&host).body(b"x")).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(send(a, req("GET", "/vhb/new")).await.body, "x");
    // Listing the bucket: GET / on the bucket host.
    let r = send(a, req("GET", "/").query("list-type=2").host(&host)).await;
    assert!(r.body.contains("<Key>obj</Key>"), "{}", r.body);
    // Other host names stay path-style.
    let r = send(a, req("GET", "/vhb/obj").host(&format!("fakes3:{}", a.port()))).await;
    assert_eq!(r.body, "data");
    let r = send(a, req("GET", "/vhb/obj").host(&format!("localhost:{}", a.port()))).await;
    assert_eq!(r.body, "data");
}

#[tokio::test]
async fn custom_domains() {
    let server = Server::start(Config {
        domains: vec!["s3.test".into(), "other.example".into()],
        ..Config::default()
    })
    .await
    .unwrap();
    let a = server.addr();
    make_bucket(a, "bkk").await;
    put(a, "/bkk/o", b"1").await;
    assert_eq!(send(a, req("GET", "/o").host("bkk.s3.test")).await.body, "1");
    assert_eq!(send(a, req("GET", "/o").host("bkk.other.example:80")).await.body, "1");
    // `localhost` is not configured here, so it is just a path-style host.
    assert_eq!(send(a, req("GET", "/bkk/o").host("localhost")).await.body, "1");
}

fn multipart(boundary: &str, fields: &[(&str, &str)], file: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for (k, v) in fields {
        out.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    out.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"f.txt\"\r\nContent-Type: text/plain\r\n\r\n")
            .as_bytes(),
    );
    out.extend_from_slice(file);
    out.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    out
}

fn iso(t: SystemTime) -> String {
    let d = common::amz_date(t);
    format!(
        "{}-{}-{}T{}:{}:{}Z",
        &d[0..4],
        &d[4..6],
        &d[6..8],
        &d[9..11],
        &d[11..13],
        &d[13..15]
    )
}

/// Builds a signed POST form for `key` in bucket `b` with `extra_conditions` (JSON array items).
fn signed_post(key: &str, expires: SystemTime, extra_conditions: &str, file: &[u8]) -> (String, Vec<u8>) {
    let date = common::amz_date(SystemTime::now());
    let cred = common::credential(&date);
    let policy = format!(
        r#"{{"expiration":"{}","conditions":[{{"bucket":"bkt"}},["eq","$key","{key}"],{{"x-amz-algorithm":"AWS4-HMAC-SHA256"}},{{"x-amz-credential":"{cred}"}},{{"x-amz-date":"{date}"}}{extra_conditions}]}}"#,
        iso(expires)
    );
    let policy_b64 = base64_simd::STANDARD.encode_to_string(policy.as_bytes());
    let sig = common::sign_string(&date, &policy_b64);
    let boundary = "XBOUNDARYX";
    let body = multipart(
        boundary,
        &[
            ("key", key),
            ("x-amz-algorithm", "AWS4-HMAC-SHA256"),
            ("x-amz-credential", &cred),
            ("x-amz-date", &date),
            ("policy", &policy_b64),
            ("x-amz-signature", &sig),
        ],
        file,
    );
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[tokio::test]
async fn post_policy_uploads() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    let soon = SystemTime::now() + std::time::Duration::from_secs(3600);

    let (ct, body) = signed_post("up.txt", soon, r#",["content-length-range",1,10]"#, b"hello");
    let r = send(a, req("POST", "/bkt").anon().header("content-type", &ct).body(&body)).await;
    assert_eq!(r.status, 204, "{}", r.body);
    assert_eq!(send(a, req("GET", "/bkt/up.txt")).await.body, "hello");

    // Conditions are enforced: size range, key equality.
    let (ct, body) = signed_post("big.txt", soon, r#",["content-length-range",1,3]"#, b"hello");
    let r = send(a, req("POST", "/bkt").anon().header("content-type", &ct).body(&body)).await;
    assert_eq!((r.status, r.code()), (400, "EntityTooLarge"), "{}", r.body);
    assert_eq!(send(a, req("GET", "/bkt/big.txt")).await.status, 404);
    let (ct, mut body) = signed_post("a.txt", soon, "", b"x");
    body = String::from_utf8(body)
        .unwrap()
        .replace("name=\"key\"\r\n\r\na.txt", "name=\"key\"\r\n\r\nb.txt")
        .into_bytes();
    let r = send(a, req("POST", "/bkt").anon().header("content-type", &ct).body(&body)).await;
    assert!(r.status >= 400, "{}", r.body);
    assert_eq!(send(a, req("GET", "/bkt/b.txt")).await.status, 404);

    // An expired policy is refused (s3s judges expiry by the host clock, not the server clock).
    let (ct, body) = signed_post("late.txt", SystemTime::now() - std::time::Duration::from_secs(60), "", b"x");
    let r = send(a, req("POST", "/bkt").anon().header("content-type", &ct).body(&body)).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"), "{}", r.body);
}

#[tokio::test]
async fn post_uploads_honor_public_write_acl() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    let form = |ct_body: &(String, Vec<u8>)| (ct_body.0.clone(), ct_body.1.clone());
    let unsigned = || {
        let boundary = "XBOUNDARYX";
        (
            format!("multipart/form-data; boundary={boundary}"),
            multipart(boundary, &[("key", "anon.txt")], b"hi"),
        )
    };
    let (ct, body) = form(&unsigned());
    let r = send(a, req("POST", "/bkt").anon().header("content-type", &ct).body(&body)).await;
    assert_eq!(r.status, 403, "{}", r.body);
    send(a, req("PUT", "/bkt").query("acl").header("x-amz-acl", "public-read-write")).await;
    let r = send(a, req("POST", "/bkt").anon().header("content-type", &ct).body(&body)).await;
    assert_eq!(r.status, 204, "{}", r.body);
    assert_eq!(send(a, req("GET", "/bkt/anon.txt")).await.body, "hi");
}

#[tokio::test]
async fn copy_object_needs_read_access_to_the_source() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    put(a, "/bkt/secret", b"s").await;
    send(a, req("PUT", "/bkt").query("acl").header("x-amz-acl", "public-read-write")).await;
    let r = send(a, req("PUT", "/bkt/copy").anon().header("x-amz-copy-source", "/bkt/secret")).await;
    assert_eq!((r.status, r.code()), (403, "AccessDenied"), "{}", r.body);
}

#[tokio::test]
async fn signed_requests_are_unaffected_by_missing_config() {
    let (_s, a) = start().await;
    make_bucket(a, "bkt").await;
    assert_eq!(put(a, "/bkt/k", b"v").await.status, 200);
    assert_eq!(send(a, req("GET", "/bkt/k")).await.body, "v");
    // A request with a bad signature is rejected whatever the bucket allows.
    let mut r = req("GET", "/bkt/k");
    r.headers.push((
        "authorization",
        "AWS4-HMAC-SHA256 Credential=fakes3/20240101/us-east-1/s3/aws4_request, SignedHeaders=host, Signature=00",
    ));
    r.signed = false;
    send(a, req("PUT", "/bkt").query("acl").header("x-amz-acl", "public-read-write")).await;
    assert_eq!(send(a, r).await.status, 403);
}
