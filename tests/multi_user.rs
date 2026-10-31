//! Several accounts (`Config::users`): ownership of buckets and objects, ACL grants between
//! accounts, bucket policy principals, owners in listings, copies across accounts and
//! `x-amz-expected-bucket-owner`. `docs/multi-user.md` has the model.

mod common;

use std::net::SocketAddr;

use common::{Resp, req, send};
use fakes3::{Config, Server, UserConfig};

const MAIN_ID: &str = "fakes3";
const ALT_ID: &str = "56789abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234";
const CAROL_ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

const ALT: (&str, &str) = ("ALTKEY", "ALTSECRET");
const CAROL: (&str, &str) = ("CAROLKEY", "CAROLSECRET");

fn users() -> Vec<UserConfig> {
    vec![
        format!("alt:{}:{}:222222222222:{ALT_ID}", ALT.0, ALT.1).parse().unwrap(),
        format!("carol:{}:{}:333333333333:{CAROL_ID}", CAROL.0, CAROL.1).parse().unwrap(),
    ]
}

async fn start_with(auth: bool) -> (Server, SocketAddr) {
    let server = Server::start(Config {
        users: users(),
        auth,
        ..Config::default()
    })
    .await
    .unwrap();
    let addr = server.addr();
    (server, addr)
}

async fn start() -> (Server, SocketAddr) {
    start_with(false).await
}

/// A request made by the default account (`fakes3`).
fn main_req<'a>(method: &'a str, path: &'a str) -> common::Req<'a> {
    req(method, path)
}

fn alt_req<'a>(method: &'a str, path: &'a str) -> common::Req<'a> {
    req(method, path).signed_by(ALT.0, ALT.1)
}

fn carol_req<'a>(method: &'a str, path: &'a str) -> common::Req<'a> {
    req(method, path).signed_by(CAROL.0, CAROL.1)
}

/// Text of the first `<name>` element at or after `from`.
fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let start = xml.find(&format!("<{name}>"))? + name.len() + 2;
    let end = xml[start..].find(&format!("</{name}>"))?;
    Some(&xml[start..start + end])
}

fn owner_of(xml: &str) -> (String, String) {
    let owner = tag(xml, "Owner").expect("an Owner");
    (tag(owner, "ID").unwrap().to_owned(), tag(owner, "DisplayName").unwrap().to_owned())
}

fn grant_xml(owner: &str, grants: &[(&str, &str)]) -> String {
    let grants: String = grants
        .iter()
        .map(|(id, perm)| {
            format!(
                r#"<Grant><Grantee xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:type="CanonicalUser"><ID>{id}</ID></Grantee><Permission>{perm}</Permission></Grant>"#
            )
        })
        .collect();
    format!("<AccessControlPolicy><Owner><ID>{owner}</ID></Owner><AccessControlList>{grants}</AccessControlList></AccessControlPolicy>")
}

async fn ok(r: impl std::future::Future<Output = Resp>) -> Resp {
    let r = r.await;
    assert!(r.status < 300, "{} {}", r.status, r.body);
    r
}

async fn status(r: impl std::future::Future<Output = Resp>) -> u16 {
    r.await.status
}

fn policy(statements: &str) -> String {
    format!(r#"{{"Version":"2012-10-17","Statement":[{statements}]}}"#)
}

#[tokio::test]
async fn status_lists_the_accounts_without_secrets() {
    let (_s, a) = start().await;
    let r = ok(send(a, req("GET", "/_fakes3").anon())).await;
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    let users = v["users"].as_array().unwrap();
    assert_eq!(users.len(), 3);
    assert_eq!(users[0]["name"], "fakes3");
    assert_eq!(users[0]["account_id"], "000000000000");
    assert_eq!(users[0]["canonical_id"], MAIN_ID);
    assert_eq!(users[1]["name"], "alt");
    assert_eq!(users[1]["access_key"], ALT.0);
    assert_eq!(users[1]["account_id"], "222222222222");
    assert_eq!(users[1]["canonical_id"], ALT_ID);
    assert!(!r.body.contains(ALT.1) && !r.body.contains("fakes3secret"), "{}", r.body);
}

#[tokio::test]
async fn bad_user_configuration_fails_to_start() {
    let clash = Server::start(Config {
        users: vec!["a:fakes3:s".parse().unwrap()],
        ..Config::default()
    })
    .await;
    let e = clash.err().expect("the default access key is taken");
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
    assert!(e.to_string().contains("access key"), "{e}");
}

#[tokio::test]
async fn buckets_belong_to_their_creator() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/main-bkt"))).await;
    ok(send(a, alt_req("PUT", "/alt-bkt"))).await;

    let main_list = ok(send(a, main_req("GET", "/"))).await;
    assert!(
        main_list.body.contains("<Name>main-bkt</Name>") && !main_list.body.contains("alt-bkt"),
        "{}",
        main_list.body
    );
    assert_eq!(owner_of(&main_list.body), (MAIN_ID.into(), "fakes3".into()));
    let alt_list = ok(send(a, alt_req("GET", "/"))).await;
    assert!(
        alt_list.body.contains("<Name>alt-bkt</Name>") && !alt_list.body.contains("main-bkt"),
        "{}",
        alt_list.body
    );
    assert_eq!(owner_of(&alt_list.body), (ALT_ID.into(), "alt".into()));
    let carol_list = ok(send(a, carol_req("GET", "/"))).await;
    assert!(!carol_list.body.contains("<Name>"), "{}", carol_list.body);

    // A name somebody else holds is taken; one's own is the usual 200.
    let r = send(a, alt_req("PUT", "/main-bkt")).await;
    assert_eq!((r.status, r.code()), (409, "BucketAlreadyExists"), "{}", r.body);
    assert_eq!(send(a, main_req("PUT", "/main-bkt")).await.status, 200);
    // And the existing bucket was not touched.
    assert_eq!(send(a, main_req("GET", "/main-bkt")).await.status, 200);

    // Anonymous requests still cannot create or list buckets.
    assert_eq!(send(a, req("PUT", "/anon").anon()).await.status, 403);
    assert_eq!(send(a, req("GET", "/").anon()).await.status, 403);
}

#[tokio::test]
async fn a_private_bucket_is_closed_to_other_accounts() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/bkt"))).await;
    ok(send(a, main_req("PUT", "/bkt/k").body(b"v"))).await;
    for who in [alt_req, carol_req] {
        assert_eq!(status(send(a, who("GET", "/bkt/k"))).await, 403);
        assert_eq!(status(send(a, who("GET", "/bkt"))).await, 403);
        assert_eq!(status(send(a, who("HEAD", "/bkt"))).await, 403);
        assert_eq!(status(send(a, who("PUT", "/bkt/new").body(b"x"))).await, 403);
        assert_eq!(status(send(a, who("DELETE", "/bkt/k"))).await, 403);
        assert_eq!(status(send(a, who("GET", "/bkt").query("acl"))).await, 403);
        assert_eq!(
            status(send(a, who("PUT", "/bkt").query("acl").header("x-amz-acl", "public-read"))).await,
            403
        );
        assert_eq!(
            status(send(a, who("PUT", "/bkt").query("policy").body(policy("").as_bytes()))).await,
            403
        );
        assert_eq!(status(send(a, who("DELETE", "/bkt"))).await, 403);
    }
    assert_eq!(status(send(a, req("GET", "/bkt/k").anon())).await, 403);
    // The owner is unaffected and nothing was written.
    assert_eq!(send(a, main_req("GET", "/bkt/k")).await.body, "v");
    assert_eq!(send(a, main_req("GET", "/bkt/new")).await.status, 404);
}

#[tokio::test]
async fn an_unknown_access_key_acts_as_the_default_account_without_auth() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/bkt"))).await;
    ok(send(a, main_req("PUT", "/bkt/k").body(b"v"))).await;
    let r = send(a, req("GET", "/bkt/k").signed_by("SOMEONE", "else")).await;
    assert_eq!((r.status, r.body.as_str()), (200, "v"));
    // ... and it owns what it creates.
    ok(send(a, req("PUT", "/other").signed_by("SOMEONE", "else"))).await;
    assert_eq!(send(a, main_req("GET", "/other")).await.status, 200);
}

#[tokio::test]
async fn acl_grants_to_a_canonical_id_open_exactly_that_account() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/bkt"))).await;
    ok(send(a, main_req("PUT", "/bkt/k").body(b"v"))).await;
    let grant = format!(r#"id="{ALT_ID}""#);
    ok(send(a, main_req("PUT", "/bkt").query("acl").header("x-amz-grant-read", &grant))).await;

    // READ: alt lists but cannot read the object (the object's ACL is its own), write, or see the ACL.
    let r = ok(send(a, alt_req("GET", "/bkt"))).await;
    assert!(r.body.contains("<Key>k</Key>"));
    assert_eq!(status(send(a, alt_req("HEAD", "/bkt"))).await, 200);
    assert_eq!(status(send(a, alt_req("GET", "/bkt/k"))).await, 403);
    assert_eq!(status(send(a, alt_req("PUT", "/bkt/n").body(b"x"))).await, 403);
    assert_eq!(status(send(a, alt_req("GET", "/bkt").query("acl"))).await, 403);
    assert_eq!(status(send(a, carol_req("GET", "/bkt"))).await, 403);

    // The grant reads back with the account's name, under the bucket owner.
    let acl = ok(send(a, main_req("GET", "/bkt").query("acl"))).await;
    assert_eq!(owner_of(&acl.body), (MAIN_ID.into(), "fakes3".into()));
    assert!(
        acl.body.contains(&format!("<ID>{ALT_ID}</ID>")) && acl.body.contains("<DisplayName>alt</DisplayName>"),
        "{}",
        acl.body
    );

    // Replace it with WRITE + READ_ACP through the body: the owner's own grant is gone from the list
    // but the owner can still do everything.
    let body = grant_xml(MAIN_ID, &[(ALT_ID, "WRITE"), (ALT_ID, "READ_ACP")]);
    ok(send(a, main_req("PUT", "/bkt").query("acl").body(body.as_bytes()))).await;
    ok(send(a, alt_req("PUT", "/bkt/n").body(b"x"))).await;
    ok(send(a, alt_req("GET", "/bkt").query("acl"))).await;
    assert_eq!(status(send(a, alt_req("GET", "/bkt"))).await, 403);
    assert_eq!(status(send(a, main_req("GET", "/bkt"))).await, 200);
    // An id that belongs to nobody is refused.
    let body = grant_xml(MAIN_ID, &[("nobody", "READ")]);
    assert_eq!(
        send(a, main_req("PUT", "/bkt").query("acl").body(body.as_bytes())).await.status,
        400
    );
}

#[tokio::test]
async fn authenticated_read_means_any_signed_account_but_not_anonymous() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/bkt").header("x-amz-acl", "authenticated-read"))).await;
    ok(send(
        a,
        main_req("PUT", "/bkt/k").header("x-amz-acl", "authenticated-read").body(b"v"),
    ))
    .await;
    assert_eq!(send(a, alt_req("GET", "/bkt/k")).await.body, "v");
    assert_eq!(status(send(a, carol_req("GET", "/bkt"))).await, 200);
    assert_eq!(status(send(a, req("GET", "/bkt").anon())).await, 403);
    assert_eq!(status(send(a, req("GET", "/bkt/k").anon())).await, 403);
    assert_eq!(status(send(a, alt_req("PUT", "/bkt/n").body(b"x"))).await, 403);
}

#[tokio::test]
async fn public_read_write_lets_another_account_write_objects_it_then_owns() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/bkt").header("x-amz-acl", "public-read-write"))).await;
    ok(send(a, main_req("PUT", "/bkt/mine").body(b"m"))).await;
    ok(send(a, alt_req("PUT", "/bkt/theirs").body(b"t"))).await;

    // Legacy buckets are ObjectWriter: the uploader owns the object.
    let acl = ok(send(a, alt_req("GET", "/bkt/theirs").query("acl"))).await;
    assert_eq!(owner_of(&acl.body), (ALT_ID.into(), "alt".into()));
    let acl = ok(send(a, main_req("GET", "/bkt/mine").query("acl"))).await.body;
    assert_eq!(owner_of(&acl), (MAIN_ID.into(), "fakes3".into()), "{acl}");
    assert_eq!(status(send(a, alt_req("GET", "/bkt/mine").query("acl"))).await, 403);
    assert_eq!(
        status(send(a, alt_req("GET", "/bkt/mine"))).await,
        403,
        "the bucket's public-read ACL is for listing"
    );

    // The uploader can read and replace its own object; the bucket owner cannot read it ...
    assert_eq!(send(a, alt_req("GET", "/bkt/theirs")).await.body, "t");
    assert_eq!(status(send(a, main_req("GET", "/bkt/theirs"))).await, 403);
    assert_eq!(status(send(a, main_req("HEAD", "/bkt/theirs"))).await, 403);
    assert_eq!(status(send(a, main_req("GET", "/bkt/theirs").query("acl"))).await, 403);
    // ... but sees it in listings, owned by the uploader, and may delete it.
    for list in ["/bkt", "/bkt?list-type=2&fetch-owner=true"] {
        let (path, query) = list.split_once('?').unwrap_or((list, ""));
        let r = ok(send(a, main_req("GET", path).query(query))).await;
        let theirs = r.body.split("<Contents>").find(|c| c.contains("<Key>theirs</Key>")).unwrap();
        assert_eq!(owner_of(theirs), (ALT_ID.into(), "alt".into()), "{list}");
        let mine = r.body.split("<Contents>").find(|c| c.contains("<Key>mine</Key>")).unwrap();
        assert_eq!(owner_of(mine), (MAIN_ID.into(), "fakes3".into()), "{list}");
    }
    assert_eq!(status(send(a, main_req("DELETE", "/bkt/theirs"))).await, 204);
    assert_eq!(status(send(a, alt_req("GET", "/bkt/theirs"))).await, 404);
}

#[tokio::test]
async fn bucket_owner_full_control_gives_the_bucket_owner_a_grant() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/bkt").header("x-amz-acl", "public-read-write"))).await;
    ok(send(
        a,
        alt_req("PUT", "/bkt/full")
            .header("x-amz-acl", "bucket-owner-full-control")
            .body(b"f"),
    ))
    .await;
    ok(send(
        a,
        alt_req("PUT", "/bkt/read").header("x-amz-acl", "bucket-owner-read").body(b"r"),
    ))
    .await;

    let acl = ok(send(a, alt_req("GET", "/bkt/full").query("acl"))).await.body;
    assert_eq!(owner_of(&acl), (ALT_ID.into(), "alt".into()));
    assert!(
        acl.matches("<Grant>").count() == 2 && acl.contains(&format!("<ID>{MAIN_ID}</ID>")),
        "{acl}"
    );
    assert_eq!(send(a, main_req("GET", "/bkt/full")).await.body, "f");
    assert_eq!(send(a, main_req("GET", "/bkt/read")).await.body, "r");
    // READ is not WRITE_ACP; FULL_CONTROL is.
    assert_eq!(status(send(a, main_req("GET", "/bkt/read").query("acl"))).await, 403);
    ok(send(a, main_req("PUT", "/bkt/full").query("acl").header("x-amz-acl", "private"))).await;
    // Having given the ACL away, the bucket owner is out of it again.
    assert_eq!(status(send(a, main_req("GET", "/bkt/full"))).await, 403);
}

fn ownership(kind: &str) -> String {
    format!("<OwnershipControls><Rule><ObjectOwnership>{kind}</ObjectOwnership></Rule></OwnershipControls>")
}

#[tokio::test]
async fn object_ownership_decides_who_owns_an_upload() {
    let (_s, a) = start().await;
    let public = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"*","Resource":["arn:aws:s3:::BKT","arn:aws:s3:::BKT/*"]}]}"#;
    for (kind, plain, full_control) in [
        ("BucketOwnerEnforced", MAIN_ID, MAIN_ID),
        ("BucketOwnerPreferred", ALT_ID, MAIN_ID),
        ("ObjectWriter", ALT_ID, ALT_ID),
    ] {
        let bkt = kind.to_lowercase();
        ok(send(a, main_req("PUT", &format!("/{bkt}")))).await;
        ok(send(
            a,
            main_req("PUT", &format!("/{bkt}"))
                .query("ownershipControls")
                .body(ownership(kind).as_bytes()),
        ))
        .await;
        let p = public.replace("BKT", &bkt);
        ok(send(a, main_req("PUT", &format!("/{bkt}")).query("policy").body(p.as_bytes()))).await;

        let owner = |key: String| {
            let path = format!("/{bkt}/{key}");
            async move {
                let r = ok(send(a, alt_req("GET", &path).query("acl"))).await;
                owner_of(&r.body).0
            }
        };
        ok(send(a, alt_req("PUT", &format!("/{bkt}/plain")).body(b"x"))).await;
        assert_eq!(owner("plain".into()).await, plain, "{kind}: PutObject");
        ok(send(
            a,
            alt_req("PUT", &format!("/{bkt}/full"))
                .header("x-amz-acl", "bucket-owner-full-control")
                .body(b"x"),
        ))
        .await;
        assert_eq!(
            owner("full".into()).await,
            full_control,
            "{kind}: PutObject with bucket-owner-full-control"
        );

        let copy_source = format!("/{bkt}/plain");
        ok(send(
            a,
            alt_req("PUT", &format!("/{bkt}/copy")).header("x-amz-copy-source", &copy_source),
        ))
        .await;
        assert_eq!(owner("copy".into()).await, plain, "{kind}: CopyObject");
        ok(send(
            a,
            alt_req("PUT", &format!("/{bkt}/copy-full"))
                .header("x-amz-copy-source", &copy_source)
                .header("x-amz-acl", "bucket-owner-full-control"),
        ))
        .await;
        assert_eq!(
            owner("copy-full".into()).await,
            full_control,
            "{kind}: CopyObject with bucket-owner-full-control"
        );

        // Multipart: the owner is decided when the upload starts, and listed for it.
        let up = |full: bool| {
            let key = if full { "mpu-full" } else { "mpu" };
            let mut r = alt_req("POST", Box::leak(format!("/{bkt}/{key}").into_boxed_str())).query("uploads");
            if full {
                r = r.header("x-amz-acl", "bucket-owner-full-control");
            }
            r
        };
        ok(send(a, up(false))).await;
        ok(send(a, up(true))).await;
        let list = ok(send(a, alt_req("GET", &format!("/{bkt}")).query("uploads"))).await;
        let uploads: Vec<&str> = list.body.split("<Upload>").skip(1).collect();
        let owner_of_upload = |key: &str| {
            let u = uploads.iter().find(|u| u.contains(&format!("<Key>{key}</Key>"))).unwrap();
            let init = tag(u, "Initiator").unwrap();
            assert_eq!(tag(init, "ID"), Some(ALT_ID), "{kind}: the initiator is who asked");
            assert_eq!(tag(init, "DisplayName"), Some("alt"));
            owner_of(u).0
        };
        assert_eq!(owner_of_upload("mpu"), plain, "{kind}: CreateMultipartUpload");
        assert_eq!(
            owner_of_upload("mpu-full"),
            full_control,
            "{kind}: CreateMultipartUpload with bucket-owner-full-control"
        );

        if kind == "BucketOwnerEnforced" {
            // ACLs are off: only the owner-full-control canned ACL is accepted.
            let r = send(
                a,
                alt_req("PUT", &format!("/{bkt}/private")).header("x-amz-acl", "private").body(b"x"),
            )
            .await;
            assert_eq!((r.status, r.code()), (400, "AccessControlListNotSupported"), "{}", r.body);
        }
    }
}

#[tokio::test]
async fn policy_principals_pick_accounts() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/bkt"))).await;
    ok(send(a, main_req("PUT", "/bkt/k").body(b"v"))).await;
    let put = |p: String| async move { ok(send(a, main_req("PUT", "/bkt").query("policy").body(p.as_bytes()))).await };

    for principal in [
        r#"{"AWS":"arn:aws:iam::222222222222:root"}"#,
        r#"{"AWS":"222222222222"}"#,
        r#"{"AWS":["arn:aws:iam::222222222222:user/alt"]}"#,
        &format!(r#"{{"CanonicalUser":"{ALT_ID}"}}"#),
    ] {
        put(policy(&format!(
            r#"{{"Effect":"Allow","Principal":{principal},"Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt/*"}}"#
        )))
        .await;
        assert_eq!(status(send(a, alt_req("GET", "/bkt/k"))).await, 200, "{principal}");
        assert_eq!(status(send(a, carol_req("GET", "/bkt/k"))).await, 403, "{principal}");
        assert_eq!(status(send(a, req("GET", "/bkt/k").anon())).await, 403, "{principal}");
        assert_eq!(status(send(a, alt_req("GET", "/bkt"))).await, 403, "only GetObject was granted");
    }

    // A user of that account with another name is somebody else.
    put(policy(r#"{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::222222222222:user/bob"},"Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt/*"}"#)).await;
    assert_eq!(status(send(a, alt_req("GET", "/bkt/k"))).await, 403);

    // An explicit Deny beats everything, including an ACL grant, and applies to the right account only.
    ok(send(
        a,
        main_req("PUT", "/bkt").query("acl").header("x-amz-acl", "authenticated-read"),
    ))
    .await;
    put(policy(
        r#"{"Effect":"Deny","Principal":{"AWS":"arn:aws:iam::222222222222:root"},"Action":"s3:ListBucket","Resource":"arn:aws:s3:::bkt"}"#,
    ))
    .await;
    assert_eq!(status(send(a, alt_req("GET", "/bkt"))).await, 403);
    assert_eq!(status(send(a, carol_req("GET", "/bkt"))).await, 200);
    assert_eq!(status(send(a, main_req("GET", "/bkt"))).await, 200);

    // Deny everyone: the bucket owner still manages the policy; others do not.
    put(policy(
        r#"{"Effect":"Deny","Principal":"*","Action":"s3:*","Resource":["arn:aws:s3:::bkt","arn:aws:s3:::bkt/*"]}"#,
    ))
    .await;
    assert_eq!(status(send(a, main_req("GET", "/bkt/k"))).await, 403);
    assert_eq!(status(send(a, alt_req("GET", "/bkt").query("policy"))).await, 403);
    assert_eq!(status(send(a, main_req("GET", "/bkt").query("policy"))).await, 200);
    ok(send(a, main_req("DELETE", "/bkt").query("policy"))).await;
    assert_eq!(status(send(a, main_req("GET", "/bkt/k"))).await, 200);
}

#[tokio::test]
async fn a_policy_cannot_open_objects_the_bucket_owner_does_not_own() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/bkt").header("x-amz-acl", "public-read-write"))).await;
    ok(send(a, main_req("PUT", "/bkt/mine").body(b"m"))).await;
    ok(send(a, alt_req("PUT", "/bkt/theirs").body(b"t"))).await;
    let p = policy(r#"{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::bkt/*"}"#);
    ok(send(a, main_req("PUT", "/bkt").query("policy").body(p.as_bytes()))).await;
    assert_eq!(send(a, carol_req("GET", "/bkt/mine")).await.body, "m");
    assert_eq!(send(a, req("GET", "/bkt/mine").anon()).await.body, "m");
    assert_eq!(status(send(a, carol_req("GET", "/bkt/theirs"))).await, 403);
    assert_eq!(status(send(a, main_req("GET", "/bkt/theirs"))).await, 403);
    assert_eq!(send(a, alt_req("GET", "/bkt/theirs")).await.body, "t");
}

#[tokio::test]
async fn a_missing_key_is_404_for_those_who_may_list_and_403_for_the_rest() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/bkt"))).await;
    assert_eq!(status(send(a, alt_req("HEAD", "/bkt/nope"))).await, 403);
    let p = policy(
        r#"{"Effect":"Allow","Principal":"*","Action":"s3:ListBucket","Resource":"arn:aws:s3:::bkt","Condition":{"StringLike":{"s3:prefix":"public/*"}}}"#,
    );
    ok(send(a, main_req("PUT", "/bkt").query("policy").body(p.as_bytes()))).await;
    assert_eq!(status(send(a, alt_req("HEAD", "/bkt/public/nope"))).await, 404);
    assert_eq!(status(send(a, alt_req("HEAD", "/bkt/private/nope"))).await, 403);
    ok(send(a, main_req("PUT", "/bkt").query("acl").header("x-amz-acl", "public-read"))).await;
    assert_eq!(status(send(a, alt_req("HEAD", "/bkt/private/nope"))).await, 404);
}

#[tokio::test]
async fn multipart_uploads_report_their_initiator_and_owner() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/bkt").header("x-amz-acl", "public-read-write"))).await;
    let u1 = ok(send(a, main_req("POST", "/bkt/one").query("uploads"))).await;
    let u2 = ok(send(a, alt_req("POST", "/bkt/two").query("uploads"))).await;
    let (id1, id2) = (
        tag(&u1.body, "UploadId").unwrap().to_owned(),
        tag(&u2.body, "UploadId").unwrap().to_owned(),
    );

    for who in [main_req, alt_req] {
        let r = ok(send(a, who("GET", "/bkt").query("uploads"))).await;
        let ups: Vec<&str> = r.body.split("<Upload>").skip(1).collect();
        assert_eq!(ups.len(), 2, "{}", r.body);
        for (u, key, id, who) in [(ups[0], "one", MAIN_ID, "fakes3"), (ups[1], "two", ALT_ID, "alt")] {
            assert_eq!(tag(u, "Key"), Some(key));
            assert_eq!(owner_of(u), (id.into(), who.into()));
            let init = tag(u, "Initiator").unwrap();
            assert_eq!((tag(init, "ID"), tag(init, "DisplayName")), (Some(id), Some(who)));
        }
    }
    // ListParts says the same, and each initiator can abort its own upload.
    let r = ok(send(a, alt_req("GET", "/bkt/two").query(&format!("uploadId={id2}")))).await;
    assert_eq!(tag(tag(&r.body, "Initiator").unwrap(), "ID"), Some(ALT_ID));
    assert_eq!(
        status(send(a, alt_req("DELETE", "/bkt/one").query(&format!("uploadId={id1}")))).await,
        204,
        "write access to the bucket"
    );
    ok(send(a, alt_req("DELETE", "/bkt/two").query(&format!("uploadId={id2}")))).await;
}

#[tokio::test]
async fn copies_need_read_on_the_source_and_write_on_the_destination() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/src"))).await;
    ok(send(a, main_req("PUT", "/src/k").body(b"v"))).await;
    ok(send(a, alt_req("PUT", "/dst"))).await;

    let copy = |src: &'static str| alt_req("PUT", "/dst/copy").header("x-amz-copy-source", src);
    assert_eq!(status(send(a, copy("/src/k"))).await, 403, "alt cannot read the source");
    assert_eq!(status(send(a, alt_req("GET", "/dst/copy"))).await, 404);

    // Grant READ on the object: now the copy goes through and the copy is alt's.
    let body = grant_xml(MAIN_ID, &[(MAIN_ID, "FULL_CONTROL"), (ALT_ID, "READ")]);
    ok(send(a, main_req("PUT", "/src/k").query("acl").body(body.as_bytes()))).await;
    ok(send(a, copy("/src/k"))).await;
    assert_eq!(send(a, alt_req("GET", "/dst/copy")).await.body, "v");
    let acl = ok(send(a, alt_req("GET", "/dst/copy").query("acl"))).await;
    assert_eq!(owner_of(&acl.body).0, ALT_ID);
    // The ACL does not travel with the copy.
    assert!(!acl.body.contains(MAIN_ID), "{}", acl.body);
    // Into somebody else's bucket needs write there too.
    let into_main = alt_req("PUT", "/src/from-alt").header("x-amz-copy-source", "/dst/copy");
    assert_eq!(status(send(a, into_main)).await, 403);
    // UploadPartCopy follows the same rule.
    let up = ok(send(a, alt_req("POST", "/dst/mpu").query("uploads"))).await;
    let id = tag(&up.body, "UploadId").unwrap();
    let q = format!("partNumber=1&uploadId={id}");
    assert_eq!(
        status(send(
            a,
            carol_req("PUT", "/dst/mpu").query(&q).header("x-amz-copy-source", "/src/k")
        ))
        .await,
        403
    );
    assert_eq!(
        status(send(a, alt_req("PUT", "/dst/mpu").query(&q).header("x-amz-copy-source", "/src/k"))).await,
        200
    );
}

#[tokio::test]
async fn expected_bucket_owner_is_the_buckets_owners_account_id() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/main-bkt"))).await;
    ok(send(a, alt_req("PUT", "/alt-bkt"))).await;
    ok(send(
        a,
        main_req("PUT", "/main-bkt").query("acl").header("x-amz-acl", "public-read-write"),
    ))
    .await;
    let get = |who: fn(&'static str, &'static str) -> common::Req<'static>, path: &'static str, owner: &'static str| {
        send(a, who("GET", path).header("x-amz-expected-bucket-owner", owner))
    };
    assert_eq!(get(main_req, "/main-bkt", "000000000000").await.status, 200);
    assert_eq!(get(alt_req, "/main-bkt", "000000000000").await.status, 200);
    assert_eq!(get(alt_req, "/main-bkt", "222222222222").await.status, 403);
    assert_eq!(get(main_req, "/alt-bkt", "000000000000").await.status, 403);
    assert_eq!(get(alt_req, "/alt-bkt", "222222222222").await.status, 200);
    // For copies the source has its own header.
    ok(send(a, main_req("PUT", "/main-bkt/k").body(b"v"))).await;
    let copy = |owner: &'static str| {
        send(
            a,
            alt_req("PUT", "/alt-bkt/c")
                .header("x-amz-copy-source", "/main-bkt/k")
                .header("x-amz-source-expected-bucket-owner", owner),
        )
    };
    assert_eq!(copy("222222222222").await.status, 403);
    assert_eq!(copy("000000000000").await.status, 403, "the source object is not readable by alt");
    ok(send(
        a,
        main_req("PUT", "/main-bkt/k").query("acl").header("x-amz-acl", "public-read"),
    ))
    .await;
    assert_eq!(copy("000000000000").await.status, 200);
}

#[tokio::test]
async fn delete_markers_and_versions_carry_their_creators() {
    let (_s, a) = start().await;
    ok(send(a, main_req("PUT", "/bkt").header("x-amz-acl", "public-read-write"))).await;
    let on = "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>";
    ok(send(a, main_req("PUT", "/bkt").query("versioning").body(on.as_bytes()))).await;
    ok(send(a, main_req("PUT", "/bkt/k").body(b"1"))).await;
    ok(send(a, alt_req("PUT", "/bkt/k").body(b"2"))).await;
    ok(send(a, alt_req("DELETE", "/bkt/k"))).await;
    let r = ok(send(a, main_req("GET", "/bkt").query("versions"))).await;
    let marker = r.body.split("<DeleteMarker>").nth(1).unwrap();
    assert_eq!(owner_of(marker).0, ALT_ID, "{}", r.body);
    let versions: Vec<&str> = r.body.split("<Version>").skip(1).collect();
    let owners: Vec<_> = versions.iter().map(|v| owner_of(v).0).collect();
    assert_eq!(owners, [ALT_ID, MAIN_ID], "newest first: {}", r.body);
}

#[tokio::test]
async fn owners_survive_snapshot_and_restore() {
    let (s, a) = start().await;
    ok(send(a, alt_req("PUT", "/alt-bkt").header("x-amz-acl", "public-read-write"))).await;
    ok(send(a, main_req("PUT", "/alt-bkt/k").body(b"v"))).await;
    ok(send(a, carol_req("PUT", "/alt-bkt/c").body(b"v"))).await;
    ok(send(a, carol_req("POST", "/alt-bkt/u").query("uploads"))).await;
    s.snapshot(Some("snap")).unwrap();
    s.reset();
    assert_eq!(send(a, alt_req("GET", "/alt-bkt")).await.status, 404);
    s.restore("snap").unwrap();

    assert_eq!(send(a, alt_req("GET", "/")).await.body.matches("<Name>").count(), 1);
    assert_eq!(send(a, main_req("GET", "/")).await.body.matches("<Name>").count(), 0);
    let r = ok(send(a, alt_req("GET", "/alt-bkt"))).await;
    let c = r.body.split("<Contents>").find(|c| c.contains("<Key>c</Key>")).unwrap();
    assert_eq!(owner_of(c).0, CAROL_ID);
    let k = r.body.split("<Contents>").find(|c| c.contains("<Key>k</Key>")).unwrap();
    assert_eq!(owner_of(k).0, MAIN_ID);
    let ups = ok(send(a, alt_req("GET", "/alt-bkt").query("uploads"))).await;
    assert_eq!(tag(tag(&ups.body, "Initiator").unwrap(), "ID"), Some(CAROL_ID));
    assert_eq!(status(send(a, main_req("GET", "/alt-bkt"))).await, 200, "public-read-write is back");
    assert_eq!(status(send(a, main_req("GET", "/alt-bkt").query("acl"))).await, 403);
}

#[tokio::test]
async fn with_auth_each_account_signs_with_its_own_secret() {
    let (_s, a) = start_with(true).await;
    ok(send(a, main_req("PUT", "/main-bkt"))).await;
    ok(send(a, alt_req("PUT", "/alt-bkt"))).await;
    let alt_list = ok(send(a, alt_req("GET", "/"))).await;
    assert_eq!(owner_of(&alt_list.body).0, ALT_ID);
    assert!(alt_list.body.contains("alt-bkt") && !alt_list.body.contains("main-bkt"));

    // Somebody else's secret, or a key nobody has, is a signature failure, not another account.
    let r = send(a, req("GET", "/").signed_by(ALT.0, CAROL.1)).await;
    assert_eq!((r.status, r.code()), (403, "SignatureDoesNotMatch"), "{}", r.body);
    let r = send(a, req("GET", "/").signed_by("NOBODY", "x")).await;
    assert_eq!(r.status, 403, "{}", r.body);

    // Authorization is the same as without auth: alt cannot see main's bucket.
    assert_eq!(status(send(a, alt_req("GET", "/main-bkt"))).await, 403);
    ok(send(
        a,
        main_req("PUT", "/main-bkt").query("acl").header("x-amz-acl", "public-read"),
    ))
    .await;
    assert_eq!(status(send(a, alt_req("GET", "/main-bkt"))).await, 200);
}
