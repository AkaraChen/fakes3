"""s3fs (fsspec) — runs in its own uv environment (s3fs_env/) because s3fs pins aiobotocore/botocore.

Scenarios use fsspec/s3fs methods. Request parameters that s3fs forwards to boto (e.g. `pipe(...,
ContentType=...)`, `copy(..., MetadataDirective=...)`) are part of s3fs's documented API and are used.
Where s3fs exposes no way to *read* a value (checksums, Cache-Control, ListParts) verification goes
through `fs.call_s3`, s3fs's raw-call escape hatch, but the operation under test is always s3fs's.
"""

import os
import re
import tempfile
import time
from importlib.metadata import version

from s3fs import S3FileSystem

from common import (
    ACCESS_KEY, ENDPOINT, KiB, META, MiB, REGION, SECRET_KEY, SPECIAL_KEYS, Skip, bytes_eq, check,
    checksum_b64, eq, expect_error, http, md5b64, md5hex, rand, run_suite, unique_bucket, unquote,
)

SDK = "s3fs"


# s3fs.mkdir() turns client_kwargs["region_name"] into a LocationConstraint, which S3 rejects for
# us-east-1; the idiomatic setup there is to leave it out and let botocore pick up the region from env.
if REGION == "us-east-1":
    os.environ["AWS_DEFAULT_REGION"] = REGION
    _CLIENT_KWARGS = {}
else:
    _CLIENT_KWARGS = {"region_name": REGION}


def mk(secret=SECRET_KEY, **kw):
    return S3FileSystem(
        key=ACCESS_KEY, secret=secret, endpoint_url=ENDPOINT, client_kwargs=_CLIENT_KWARGS,
        config_kwargs={"s3": {"addressing_style": "path"}, "retries": {"max_attempts": 1}},
        skip_instance_cache=True, use_listings_cache=False, **kw,
    )


fs = mk()
B = unique_bucket("s3fs")
VB = f"{B}-v"
DB = f"{B}-d"


def p(key, bucket=B):
    return f"{bucket}/{key}"


def put(key, data, bucket=B, **kw):
    fs.pipe_file(p(key, bucket), data, **kw)


def cat(key, bucket=B, **kw):
    return fs.cat_file(p(key, bucket), **kw)


def head(key, **kw):
    return fs.call_s3("head_object", Bucket=B, Key=key, **kw)


def list_keys(prefix=""):
    fs.invalidate_cache()
    return [x[len(B) + 1:] for x in fs.find(p(prefix) if prefix else B)]


def create_bucket():
    fs.mkdir(B)


def head_bucket():
    fs.invalidate_cache()
    eq(fs.info(B)["type"], "directory", "info type")


def head_bucket_missing():
    expect_error(lambda: fs.info(f"{B}-missing"), types=("FileNotFoundError",))


def list_buckets():
    fs.invalidate_cache()
    check(B in fs.ls(""), f"bucket {B} not in ls('')")


def put_get_small():
    data = b"hello world"
    put("small.txt", data)
    bytes_eq(cat("small.txt"), data)
    eq(unquote(fs.info(p("small.txt"), refresh=True)["ETag"]), md5hex(data), "ETag")


def put_get_empty():
    put("empty", b"")
    eq(len(cat("empty")), 0, "body length")
    eq(fs.info(p("empty"), refresh=True)["size"], 0, "size")


def put_get_large():
    data = rand(8 * MiB)
    put("large.bin", data)  # below 2 * chunksize (50 MiB) -> single PutObject
    bytes_eq(cat("large.bin"), data)


def special_keys():
    for k in SPECIAL_KEYS:
        data = f"data:{k}".encode()
        put(k, data)
        bytes_eq(cat(k), data, f"body of {k!r}")
    keys = list_keys()
    for k in SPECIAL_KEYS:
        check(k in keys, f"key {k!r} missing from list")


def head_object_metadata():
    put("meta.txt", b"meta", ContentType="text/plain", CacheControl="max-age=60",
        ContentDisposition='attachment; filename="m.txt"', ContentEncoding="gzip", Metadata=META)
    eq(fs.info(p("meta.txt"), refresh=True).get("ContentType"), "text/plain", "info ContentType")
    eq(fs.metadata(p("meta.txt"), refresh=True), {"color": "Blue", "x-y": "z"}, "metadata()")
    h = head("meta.txt")  # s3fs has no accessor for Cache-Control / Content-Disposition / Content-Encoding
    eq(h.get("CacheControl"), "max-age=60", "CacheControl")
    eq(h.get("ContentDisposition"), 'attachment; filename="m.txt"', "ContentDisposition")
    eq(h.get("ContentEncoding"), "gzip", "ContentEncoding")


def get_range():
    # s3fs ranges are python-slice style (end exclusive, negative start = from end); Content-Range is
    # not exposed, so only bodies are checked.
    data = rand(100)
    put("range.bin", data)
    bytes_eq(cat("range.bin", start=10, end=20), data[10:20], "body for bytes=10-19")
    bytes_eq(cat("range.bin", start=-5), data[95:], "body for bytes=-5")
    bytes_eq(cat("range.bin", start=95), data[95:], "body for bytes=95-")


def get_range_invalid():
    put("range-inv.bin", rand(100))
    e = expect_error(lambda: cat("range-inv.bin", start=200, end=301), types=("OSError",))
    check("range" in str(e).lower() or "416" in str(e), f"expected InvalidRange/416, got {e!r}")


def get_missing_key():
    expect_error(lambda: cat("does/not/exist"), types=("FileNotFoundError",))


def put_if_none_match():
    key = f"inm-{time.time_ns()}"
    put(key, b"first", mode="create")
    # s3fs translates 412 PreconditionFailed into OSError(EINVAL, <server message>).
    e = expect_error(lambda: put(key, b"second", mode="create"), types=("FileExistsError", "OSError"))
    check(isinstance(e, FileExistsError) or re.search(r"pre-?condition", str(e), re.I), f"expected 412, got {e!r}")
    bytes_eq(cat(key), b"first")


def delete_object():
    put("del.txt", b"x")
    fs.rm_file(p("del.txt"))
    expect_error(lambda: fs.info(p("del.txt"), refresh=True), types=("FileNotFoundError",))


def delete_missing_object():
    fs.rm_file(p("never-existed"))


def delete_objects():
    keys = ["dm/1", "dm/2", "dm/3"]
    for k in keys:
        put(k, k.encode())
    fs.rm([p(k) for k in keys])  # bulk DeleteObjects
    eq(list_keys("dm/"), [], "remaining keys")


def list_v2_basic():
    objs = {"lb/b": rand(3), "lb/a": rand(1), "lb/c": rand(7)}
    for k, v in objs.items():
        put(k, v)
    fs.invalidate_cache()
    got = fs.ls(p("lb"), detail=True)
    eq([o["name"] for o in got], [p("lb/a"), p("lb/b"), p("lb/c")], "keys")
    for o in got:
        k = o["name"][len(B) + 1:]
        eq(o["size"], len(objs[k]), f"size of {k}")
        eq(unquote(o["ETag"]), md5hex(objs[k]), f"ETag of {k}")


def list_v2_prefix_delimiter():
    for k in ["a/1", "a/2", "b/1", "c"]:
        put(f"pd/{k}", k.encode())
    fs.invalidate_cache()
    got = fs.ls(p("pd"), detail=True)  # ListObjectsV2 Prefix="pd/" Delimiter="/"
    eq([o["name"] for o in got if o["type"] == "directory"], [p("pd/a"), p("pd/b")], "CommonPrefixes")
    eq([o["name"] for o in got if o["type"] == "file"], [p("pd/c")], "Contents")


def copy_object():
    data = rand(1234)
    put("cp/src", data, ContentType="application/x-test", Metadata=META)
    fs.copy(p("cp/src"), p("cp/dst"))
    bytes_eq(cat("cp/dst"), data)
    eq(fs.info(p("cp/dst"), refresh=True).get("ContentType"), "application/x-test", "ContentType")
    eq(fs.metadata(p("cp/dst"), refresh=True), {"color": "Blue", "x-y": "z"}, "metadata()")


def copy_object_replace_metadata():
    data = rand(100)
    put("cpr/src", data, ContentType="application/x-old", Metadata=META)
    fs.copy(p("cpr/src"), p("cpr/dst"), MetadataDirective="REPLACE", ContentType="application/x-new",
            Metadata={"fresh": "yes"})
    eq(fs.info(p("cpr/dst"), refresh=True).get("ContentType"), "application/x-new", "ContentType")
    eq(fs.metadata(p("cpr/dst"), refresh=True), {"fresh": "yes"}, "metadata()")
    bytes_eq(cat("cpr/dst"), data)


def multipart_upload():
    # S3File with a 5 MiB block size issues one UploadPart per flushed block.
    parts = [rand(5 * MiB), rand(5 * MiB), rand(KiB)]
    with fs.open(p("mpu/obj"), "wb", block_size=5 * MiB) as f:
        for part in parts:
            f.write(part)
            f.flush()
    etag = unquote(fs.info(p("mpu/obj"), refresh=True)["ETag"])
    check(re.fullmatch(r"[0-9a-f]{32}-3", etag or ""), f"ETag {etag} not of form <hex>-3")
    bytes_eq(cat("mpu/obj"), b"".join(parts))


def multipart_abort():
    f = fs.open(p("mpu/abort"), "wb", block_size=5 * MiB)
    f.write(rand(5 * MiB))
    f.flush()  # starts the multipart upload and uploads part 1
    uid = f.mpu["UploadId"]
    f.discard()  # AbortMultipartUpload
    f.closed = True  # discard() leaves the buffer gone; stop __del__ from trying to flush it
    expect_error(lambda: fs.call_s3("list_parts", Bucket=B, Key="mpu/abort", UploadId=uid),
                 codes=("NoSuchUpload",), statuses=(404,), types=("FileNotFoundError",))


def high_level_upload():
    data = rand(20 * MiB)
    with tempfile.TemporaryDirectory() as d:
        src, dst = os.path.join(d, "src"), os.path.join(d, "dst")
        with open(src, "wb") as f:
            f.write(data)
        fs.put_file(src, p("hl/obj"), chunksize=5 * MiB)  # multipart above 2 * chunksize
        fs.get_file(p("hl/obj"), dst)
        with open(dst, "rb") as f:
            bytes_eq(f.read(), data)


def presigned_get():
    data = rand(500)
    put("ps/get", data)
    status, body = http("GET", fs.url(p("ps/get"), expires=300))
    eq(status, 200, "HTTP status")
    bytes_eq(body, data)


def presigned_put():
    data = rand(500)
    status, body = http("PUT", fs.url(p("ps/put"), expires=300, client_method="put_object"), data)
    eq(status, 200, f"HTTP status ({body[:200]!r})")
    bytes_eq(cat("ps/put"), data)


def checksum_scenario(alg):
    def f():
        data, key = rand(1000), f"ck/{alg}"
        put(key, data, ChecksumAlgorithm=alg)
        bytes_eq(cat(key), data)
        eq(head(key, ChecksumMode="ENABLED").get(f"Checksum{alg}"), checksum_b64(alg, data), f"Checksum{alg}")
    return f


def content_md5_bad():
    e = expect_error(lambda: put("md5bad", b"payload", ContentMD5=md5b64(b"other")), types=("OSError",))
    check(re.search(r"BadDigest|InvalidDigest|Content-MD5|\b400\b", str(e), re.I), f"expected BadDigest, got {e!r}")


def bad_credentials():
    expect_error(lambda: mk("wrong-secret").ls(""), codes=("SignatureDoesNotMatch", "InvalidAccessKeyId"),
                 statuses=(403,), types=("PermissionError",))


def delete_bucket_not_empty():
    put("nonempty", b"x")
    e = expect_error(lambda: fs.rmdir(B), types=("OSError",))
    check("not empty" in str(e).lower() or "BucketNotEmpty" in str(e), f"expected BucketNotEmpty, got {e!r}")


def versioning():
    vfs = mk(version_aware=True)
    vfs.mkdir(VB)
    vfs.make_bucket_versioned(VB)
    v1, v2 = b"version-1", b"version-2"
    vfs.pipe_file(p("k", VB), v1)
    vfs.pipe_file(p("k", VB), v2)
    vs = [v for v in vfs.object_version_info(p("k", VB)) if v["Key"] == "k"]
    eq(len(vs), 2, "version count")
    old = next((v["VersionId"] for v in vs if not v.get("IsLatest")), None)
    check(old, f"no non-latest version in {vs}")
    bytes_eq(vfs.cat_file(p("k", VB), version_id=old), v1, "old version body")
    bytes_eq(vfs.cat_file(p("k", VB)), v2, "latest body")


def object_tagging():
    put("tag.txt", b"t")
    fs.put_tags(p("tag.txt"), {"k1": "v1", "k2": "v 2"})
    eq(fs.get_tags(p("tag.txt")), {"k1": "v1", "k2": "v 2"}, "tags")


def delete_bucket():
    fs.mkdir(DB)
    for k in ["x", "y/z"]:
        put(k, k.encode(), bucket=DB)
    fs.invalidate_cache()
    fs.rm(fs.find(DB))
    fs.rmdir(DB)
    fs.invalidate_cache()
    eq(fs.exists(DB), False, "exists after delete")
    expect_error(lambda: fs.info(DB), types=("FileNotFoundError",))


impl = {name: fn for name, fn in globals().items() if callable(fn) and not name.startswith("_")}
impl.update({
    "checksum_crc32": checksum_scenario("CRC32"),
    "checksum_crc32c": checksum_scenario("CRC32C"),
    "checksum_sha1": checksum_scenario("SHA1"),
    "checksum_sha256": checksum_scenario("SHA256"),
    "checksum_crc64nvme": checksum_scenario("CRC64NVME"),
    "get_if_none_match": Skip("s3fs cat/open have no conditional-read (If-None-Match) option"),
    "get_if_match_fail": Skip("s3fs cat/open have no conditional-read (If-Match) option"),
    "list_v2_pagination": Skip("s3fs paginates listings internally; no MaxKeys/continuation-token control"),
    "list_v2_start_after": Skip("s3fs listing has no StartAfter option"),
    "list_v1": Skip("s3fs only uses ListObjectsV2"),
    "multipart_list_parts": Skip("s3fs has no ListParts API"),
    "multipart_too_small": Skip("s3fs has no API to complete a multipart upload with chosen part sizes "
                                "(S3File enforces >= 5 MiB blocks)"),
})

if __name__ == "__main__":
    run_suite(SDK, version("s3fs"), B, impl, cleanup_buckets=[VB, DB])
