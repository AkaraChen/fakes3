"""boto3 (+ s3transfer for high_level_upload)."""

import io
import re
import time
from importlib.metadata import version

import boto3
from botocore.config import Config

from common import (
    ACCESS_KEY, ENDPOINT, KiB, META, MiB, REGION, SECRET_KEY, SPECIAL_KEYS, Skip, bytes_eq, check,
    checksum_b64, eq, expect_error, http, md5b64, md5hex, rand, run_suite, unique_bucket, unquote,
)

SDK = "boto3"


def mk(secret=SECRET_KEY):
    return boto3.client(
        "s3", endpoint_url=ENDPOINT, region_name=REGION, aws_access_key_id=ACCESS_KEY,
        aws_secret_access_key=secret,
        config=Config(s3={"addressing_style": "path"}, signature_version="s3v4", retries={"max_attempts": 1}),
    )


s3 = mk()
B = unique_bucket("boto3")
VB = f"{B}-v"
DB = f"{B}-d"
NOT_FOUND = dict(codes=("NotFound", "NoSuchKey", "NoSuchBucket", "404"), statuses=(404,))


def put(key, body, bucket=B, **kw):
    return s3.put_object(Bucket=bucket, Key=key, Body=body, **kw)


def get(key, bucket=B, **kw):
    r = s3.get_object(Bucket=bucket, Key=key, **kw)
    return r, r["Body"].read()


def list_all(prefix=""):
    keys = []
    for page in s3.get_paginator("list_objects_v2").paginate(Bucket=B, Prefix=prefix):
        keys += [o["Key"] for o in page.get("Contents", [])]
    return keys


def checksum_scenario(alg):
    def f():
        data, key = rand(1000), f"ck/{alg}"
        put(key, data, ChecksumAlgorithm=alg)
        r, body = get(key, ChecksumMode="ENABLED")
        bytes_eq(body, data)
        eq(r.get(f"Checksum{alg}"), checksum_b64(alg, data), f"Checksum{alg}")
    return f


def create_bucket():
    s3.create_bucket(Bucket=B)


def head_bucket():
    eq(s3.head_bucket(Bucket=B)["ResponseMetadata"]["HTTPStatusCode"], 200, "status")


def head_bucket_missing():
    expect_error(lambda: s3.head_bucket(Bucket=f"{B}-missing"), **NOT_FOUND)


def list_buckets():
    names = [b["Name"] for b in s3.list_buckets()["Buckets"]]
    check(B in names, f"bucket {B} not in ListBuckets")


def put_get_small():
    data = b"hello world"
    p = put("small.txt", data)
    r, body = get("small.txt")
    bytes_eq(body, data)
    eq(unquote(p["ETag"]), md5hex(data), "PUT ETag")
    eq(unquote(r["ETag"]), md5hex(data), "GET ETag")


def put_get_empty():
    put("empty", b"")
    r, body = get("empty")
    eq(len(body), 0, "body length")
    eq(r["ContentLength"], 0, "ContentLength")


def put_get_large():
    data = rand(8 * MiB)
    put("large.bin", data)
    bytes_eq(get("large.bin")[1], data)


def special_keys():
    for k in SPECIAL_KEYS:
        data = f"data:{k}".encode()
        put(k, data)
        bytes_eq(get(k)[1], data, f"body of {k!r}")
    keys = list_all()
    for k in SPECIAL_KEYS:
        check(k in keys, f"key {k!r} missing from list")


def head_object_metadata():
    put("meta.txt", b"meta", ContentType="text/plain", CacheControl="max-age=60",
        ContentDisposition='attachment; filename="m.txt"', ContentEncoding="gzip", Metadata=META)
    r = s3.head_object(Bucket=B, Key="meta.txt")
    eq(r.get("ContentType"), "text/plain", "ContentType")
    eq(r.get("CacheControl"), "max-age=60", "CacheControl")
    eq(r.get("ContentDisposition"), 'attachment; filename="m.txt"', "ContentDisposition")
    eq(r.get("ContentEncoding"), "gzip", "ContentEncoding")
    eq(r.get("Metadata"), {"color": "Blue", "x-y": "z"}, "Metadata")


def get_range():
    data = rand(100)
    put("range.bin", data)
    for rng, start, end in [("bytes=10-19", 10, 19), ("bytes=-5", 95, 99), ("bytes=95-", 95, 99)]:
        r, body = get("range.bin", Range=rng)
        bytes_eq(body, data[start:end + 1], f"body for {rng}")
        eq(r.get("ContentRange"), f"bytes {start}-{end}/100", f"ContentRange for {rng}")


def get_range_invalid():
    put("range-inv.bin", rand(100))
    expect_error(lambda: get("range-inv.bin", Range="bytes=200-300"), codes=("InvalidRange",), statuses=(416,))


def get_missing_key():
    expect_error(lambda: get("does/not/exist"), codes=("NoSuchKey",), statuses=(404,))


def get_if_none_match():
    p = put("cond.txt", b"cond")
    expect_error(lambda: get("cond.txt", IfNoneMatch=p["ETag"]), codes=("NotModified", "304"), statuses=(304,))


def get_if_match_fail():
    put("cond2.txt", b"cond")
    expect_error(lambda: get("cond2.txt", IfMatch='"wrong"'), codes=("PreconditionFailed",), statuses=(412,))


def put_if_none_match():
    key = f"inm-{time.time_ns()}"
    put(key, b"first", IfNoneMatch="*")
    expect_error(lambda: put(key, b"second", IfNoneMatch="*"), codes=("PreconditionFailed",), statuses=(412,))
    bytes_eq(get(key)[1], b"first")


def delete_object():
    put("del.txt", b"x")
    s3.delete_object(Bucket=B, Key="del.txt")
    expect_error(lambda: s3.head_object(Bucket=B, Key="del.txt"), **NOT_FOUND)


def delete_missing_object():
    r = s3.delete_object(Bucket=B, Key="never-existed")
    eq(r["ResponseMetadata"]["HTTPStatusCode"], 204, "status")


def delete_objects():
    keys = ["dm/1", "dm/2", "dm/3"]
    for k in keys:
        put(k, k.encode())
    r = s3.delete_objects(Bucket=B, Delete={"Objects": [{"Key": k} for k in keys]})
    eq(r.get("Errors", []), [], "Errors")
    eq(sorted(d["Key"] for d in r.get("Deleted", [])), keys, "Deleted")
    eq(list_all("dm/"), [], "remaining keys")


def list_v2_basic():
    objs = {"lb/b": rand(3), "lb/a": rand(1), "lb/c": rand(7)}
    for k, v in objs.items():
        put(k, v)
    r = s3.list_objects_v2(Bucket=B, Prefix="lb/")
    eq([o["Key"] for o in r["Contents"]], ["lb/a", "lb/b", "lb/c"], "keys")
    for o in r["Contents"]:
        eq(o["Size"], len(objs[o["Key"]]), f"Size of {o['Key']}")
        eq(unquote(o["ETag"]), md5hex(objs[o["Key"]]), f"ETag of {o['Key']}")
    eq(r["KeyCount"], 3, "KeyCount")


def list_v2_prefix_delimiter():
    for k in ["a/1", "a/2", "b/1", "c"]:
        put(f"pd/{k}", k.encode())
    r = s3.list_objects_v2(Bucket=B, Prefix="pd/", Delimiter="/")
    eq([p["Prefix"] for p in r.get("CommonPrefixes", [])], ["pd/a/", "pd/b/"], "CommonPrefixes")
    eq([o["Key"] for o in r.get("Contents", [])], ["pd/c"], "Contents")


def list_v2_pagination():
    keys = [f"pg/{i:02d}" for i in range(25)]
    for k in keys:
        put(k, k.encode())
    pages, token = [], None
    while True:
        kw = {"ContinuationToken": token} if token else {}
        r = s3.list_objects_v2(Bucket=B, Prefix="pg/", MaxKeys=10, **kw)
        pages.append([o["Key"] for o in r.get("Contents", [])])
        check(len(pages) <= 5, "too many pages")
        if not r.get("IsTruncated"):
            break
        token = r["NextContinuationToken"]
    eq([len(p) for p in pages], [10, 10, 5], "page sizes")
    eq([k for p in pages for k in p], keys, "keys")


def list_v2_start_after():
    keys = [f"sa/{i}" for i in range(10)]
    for k in keys:
        put(k, k.encode())
    r = s3.list_objects_v2(Bucket=B, Prefix="sa/", StartAfter="sa/4")
    eq([o["Key"] for o in r.get("Contents", [])], keys[5:], "keys")


def list_v1():
    keys = [f"l1/{i}" for i in range(6)]
    for k in keys:
        put(k, k.encode())
    r1 = s3.list_objects(Bucket=B, Prefix="l1/", Marker="l1/2", MaxKeys=2)
    eq([o["Key"] for o in r1.get("Contents", [])], ["l1/3", "l1/4"], "page 1")
    eq(r1.get("IsTruncated"), True, "IsTruncated")
    r2 = s3.list_objects(Bucket=B, Prefix="l1/", Marker="l1/4")
    eq([o["Key"] for o in r2.get("Contents", [])], ["l1/5"], "page 2")
    eq(bool(r2.get("IsTruncated")), False, "IsTruncated page 2")


def copy_object():
    data = rand(1234)
    put("cp/src", data, ContentType="application/x-test", Metadata=META)
    s3.copy_object(Bucket=B, Key="cp/dst", CopySource={"Bucket": B, "Key": "cp/src"})
    r, body = get("cp/dst")
    bytes_eq(body, data)
    eq(r.get("ContentType"), "application/x-test", "ContentType")
    eq(r.get("Metadata"), {"color": "Blue", "x-y": "z"}, "Metadata")


def copy_object_replace_metadata():
    data = rand(100)
    put("cpr/src", data, ContentType="application/x-old", Metadata=META)
    s3.copy_object(Bucket=B, Key="cpr/dst", CopySource={"Bucket": B, "Key": "cpr/src"},
                   MetadataDirective="REPLACE", ContentType="application/x-new", Metadata={"fresh": "yes"})
    r = s3.head_object(Bucket=B, Key="cpr/dst")
    eq(r.get("ContentType"), "application/x-new", "ContentType")
    eq(r.get("Metadata"), {"fresh": "yes"}, "Metadata")
    bytes_eq(get("cpr/dst")[1], data)


def multipart_upload():
    key, parts = "mpu/obj", [rand(5 * MiB), rand(5 * MiB), rand(KiB)]
    uid = s3.create_multipart_upload(Bucket=B, Key=key)["UploadId"]
    done = []
    for i, p in enumerate(parts, 1):
        r = s3.upload_part(Bucket=B, Key=key, UploadId=uid, PartNumber=i, Body=p)
        done.append({"PartNumber": i, "ETag": r["ETag"]})
    c = s3.complete_multipart_upload(Bucket=B, Key=key, UploadId=uid, MultipartUpload={"Parts": done})
    check(re.fullmatch(r"[0-9a-f]{32}-3", unquote(c["ETag"]) or ""), f"ETag {c['ETag']} not of form <hex>-3")
    bytes_eq(get(key)[1], b"".join(parts))


def multipart_abort():
    key = "mpu/abort"
    uid = s3.create_multipart_upload(Bucket=B, Key=key)["UploadId"]
    s3.upload_part(Bucket=B, Key=key, UploadId=uid, PartNumber=1, Body=rand(KiB))
    s3.abort_multipart_upload(Bucket=B, Key=key, UploadId=uid)
    expect_error(lambda: s3.list_parts(Bucket=B, Key=key, UploadId=uid), codes=("NoSuchUpload",), statuses=(404,))


def multipart_list_parts():
    key, parts = "mpu/lp", [rand(5 * MiB), rand(KiB)]
    uid = s3.create_multipart_upload(Bucket=B, Key=key)["UploadId"]
    try:
        for i, p in enumerate(parts, 1):
            s3.upload_part(Bucket=B, Key=key, UploadId=uid, PartNumber=i, Body=p)
        r = s3.list_parts(Bucket=B, Key=key, UploadId=uid)
        got = r.get("Parts", [])
        eq([p["PartNumber"] for p in got], [1, 2], "PartNumbers")
        eq([p["Size"] for p in got], [len(p) for p in parts], "Sizes")
        eq([unquote(p["ETag"]) for p in got], [md5hex(p) for p in parts], "ETags")
    finally:
        try:
            s3.abort_multipart_upload(Bucket=B, Key=key, UploadId=uid)
        except Exception:  # noqa: BLE001
            pass


def multipart_too_small():
    key = "mpu/small"
    uid = s3.create_multipart_upload(Bucket=B, Key=key)["UploadId"]
    try:
        done = []
        for i in (1, 2):
            r = s3.upload_part(Bucket=B, Key=key, UploadId=uid, PartNumber=i, Body=rand(KiB))
            done.append({"PartNumber": i, "ETag": r["ETag"]})
        expect_error(lambda: s3.complete_multipart_upload(Bucket=B, Key=key, UploadId=uid,
                                                          MultipartUpload={"Parts": done}),
                     codes=("EntityTooSmall",))
    finally:
        try:
            s3.abort_multipart_upload(Bucket=B, Key=key, UploadId=uid)
        except Exception:  # noqa: BLE001
            pass


def high_level_upload():
    from boto3.s3.transfer import TransferConfig
    data = rand(20 * MiB)
    cfg = TransferConfig(multipart_threshold=8 * MiB, multipart_chunksize=8 * MiB)
    s3.upload_fileobj(io.BytesIO(data), B, "hl/obj", Config=cfg)
    out = io.BytesIO()
    s3.download_fileobj(B, "hl/obj", out, Config=cfg)
    bytes_eq(out.getvalue(), data)


def presigned_get():
    data = rand(500)
    put("ps/get", data)
    url = s3.generate_presigned_url("get_object", Params={"Bucket": B, "Key": "ps/get"}, ExpiresIn=300)
    status, body = http("GET", url)
    eq(status, 200, "HTTP status")
    bytes_eq(body, data)


def presigned_put():
    data = rand(500)
    url = s3.generate_presigned_url("put_object", Params={"Bucket": B, "Key": "ps/put"}, ExpiresIn=300)
    status, body = http("PUT", url, data)
    eq(status, 200, f"HTTP status ({body[:200]!r})")
    bytes_eq(get("ps/put")[1], data)


def content_md5_bad():
    expect_error(lambda: put("md5bad", b"payload", ContentMD5=md5b64(b"other")),
                 codes=("BadDigest", "InvalidDigest"), statuses=(400,))


def bad_credentials():
    expect_error(lambda: mk("wrong-secret").list_buckets(),
                 codes=("SignatureDoesNotMatch", "InvalidAccessKeyId"), statuses=(403,))


def delete_bucket_not_empty():
    put("nonempty", b"x")
    expect_error(lambda: s3.delete_bucket(Bucket=B), codes=("BucketNotEmpty",), statuses=(409,))


def versioning():
    s3.create_bucket(Bucket=VB)
    s3.put_bucket_versioning(Bucket=VB, VersioningConfiguration={"Status": "Enabled"})
    v1, v2 = b"version-1", b"version-2"
    p1, p2 = put("k", v1, bucket=VB), put("k", v2, bucket=VB)
    check(p1.get("VersionId") and p2.get("VersionId") and p1["VersionId"] != p2["VersionId"],
          f"distinct VersionIds expected, got {p1.get('VersionId')} / {p2.get('VersionId')}")
    vs = [v for v in s3.list_object_versions(Bucket=VB, Prefix="k").get("Versions", []) if v["Key"] == "k"]
    eq(len(vs), 2, "version count")
    eq(sorted(v["VersionId"] for v in vs), sorted([p1["VersionId"], p2["VersionId"]]), "VersionIds")
    bytes_eq(get("k", bucket=VB, VersionId=p1["VersionId"])[1], v1, "old version body")
    bytes_eq(get("k", bucket=VB)[1], v2, "latest body")


def object_tagging():
    put("tag.txt", b"t")
    tags = [{"Key": "k1", "Value": "v1"}, {"Key": "k2", "Value": "v 2"}]
    s3.put_object_tagging(Bucket=B, Key="tag.txt", Tagging={"TagSet": tags})
    r = s3.get_object_tagging(Bucket=B, Key="tag.txt")
    eq(sorted(({"Key": t["Key"], "Value": t["Value"]} for t in r.get("TagSet", [])), key=lambda t: t["Key"]),
       tags, "TagSet")


def delete_bucket():
    s3.create_bucket(Bucket=DB)
    for k in ["x", "y/z"]:
        put(k, k.encode(), bucket=DB)
    objs = [{"Key": o["Key"]} for o in s3.list_objects_v2(Bucket=DB).get("Contents", [])]
    s3.delete_objects(Bucket=DB, Delete={"Objects": objs})
    s3.delete_bucket(Bucket=DB)
    expect_error(lambda: s3.head_bucket(Bucket=DB), **NOT_FOUND)


impl = {name: fn for name, fn in globals().items() if callable(fn) and not name.startswith("_")}
impl.update({
    "checksum_crc32": checksum_scenario("CRC32"),
    "checksum_crc32c": checksum_scenario("CRC32C"),
    "checksum_sha1": checksum_scenario("SHA1"),
    "checksum_sha256": checksum_scenario("SHA256"),
    "checksum_crc64nvme": checksum_scenario("CRC64NVME"),
})

if __name__ == "__main__":
    run_suite(SDK, version("boto3"), B, impl, cleanup_buckets=[VB, DB])
