"""minio-py (Minio client).

Low-level multipart (create/upload_part/complete/abort/list_parts) and MaxKeys-paged listing are
only reachable through minio-py's underscore-prefixed methods (`_create_multipart_upload`, ...,
`_list_objects(max_keys=...)`); they are the SDK's own S3 operation wrappers, so we use them.
"""

import io
import re
from datetime import timedelta
from importlib.metadata import version
from urllib.parse import urlparse

from minio import Minio
from minio.commonconfig import ENABLED, REPLACE, CopySource, Tags
from minio.datatypes import Part
from minio.deleteobjects import DeleteObject
from minio.versioningconfig import VersioningConfig

from common import (
    ACCESS_KEY, ENDPOINT, KiB, META, MiB, REGION, SECRET_KEY, SPECIAL_KEYS, Skip, bytes_eq, check, eq,
    expect_error, http, md5hex, rand, run_suite, unique_bucket, unquote,
)

SDK = "minio-py"
_u = urlparse(ENDPOINT)


def mk(secret=SECRET_KEY):
    return Minio(_u.netloc, access_key=ACCESS_KEY, secret_key=secret, secure=_u.scheme == "https", region=REGION)


mc = mk()
B = unique_bucket("miniopy")
VB = f"{B}-v"
DB = f"{B}-d"
NOT_FOUND = dict(codes=("NoSuchKey", "NoSuchBucket", "NotFound", "ResourceNotFound"), statuses=(404,))


def put(key, data, bucket=B, **kw):
    # part_size >= len(data) keeps it a single PutObject (minio-py otherwise goes multipart above 5 MiB).
    kw.setdefault("part_size", max(5 * MiB, len(data)))
    return mc.put_object(bucket, key, io.BytesIO(data), len(data), **kw)


def get(key, bucket=B, **kw):
    r = mc.get_object(bucket, key, **kw)
    try:
        return r, r.read()
    finally:
        r.close()
        r.release_conn()


def user_meta(headers) -> dict:
    return {k.lower()[len("x-amz-meta-"):]: v for k, v in headers.items() if k.lower().startswith("x-amz-meta-")}


def list_all(prefix=None):
    return [o.object_name for o in mc.list_objects(B, prefix=prefix, recursive=True)]


def create_bucket():
    mc.make_bucket(B)


def head_bucket():
    eq(mc.bucket_exists(B), True, "bucket_exists")


def head_bucket_missing():
    # minio-py maps HEAD 404/NoSuchBucket to False.
    eq(mc.bucket_exists(f"{B}-missing"), False, "bucket_exists")


def list_buckets():
    check(B in [b.name for b in mc.list_buckets()], f"bucket {B} not in ListBuckets")


def put_get_small():
    data = b"hello world"
    p = put("small.txt", data)
    r, body = get("small.txt")
    bytes_eq(body, data)
    eq(unquote(p.etag), md5hex(data), "PUT ETag")
    eq(unquote(r.headers.get("ETag")), md5hex(data), "GET ETag")


def put_get_empty():
    put("empty", b"")
    r, body = get("empty")
    eq(len(body), 0, "body length")
    eq(r.headers.get("Content-Length"), "0", "Content-Length")


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
    put("meta.txt", b"meta", content_type="text/plain", metadata={
        "Cache-Control": "max-age=60", "Content-Disposition": 'attachment; filename="m.txt"',
        "Content-Encoding": "gzip", **META})
    st = mc.stat_object(B, "meta.txt")
    h = st.metadata
    eq(st.content_type, "text/plain", "Content-Type")
    eq(h.get("Cache-Control"), "max-age=60", "Cache-Control")
    eq(h.get("Content-Disposition"), 'attachment; filename="m.txt"', "Content-Disposition")
    eq(h.get("Content-Encoding"), "gzip", "Content-Encoding")
    eq(user_meta(h), {"color": "Blue", "x-y": "z"}, "user metadata")


def get_range():
    data = rand(100)
    put("range.bin", data)
    cases = [
        ("bytes=10-19", dict(offset=10, length=10), 10, 19),
        # minio-py offset/length cannot express a suffix range; it accepts raw request headers.
        ("bytes=-5", dict(request_headers={"Range": "bytes=-5"}), 95, 99),
        ("bytes=95-", dict(offset=95), 95, 99),
    ]
    for rng, kw, start, end in cases:
        r, body = get("range.bin", **kw)
        bytes_eq(body, data[start:end + 1], f"body for {rng}")
        eq(r.headers.get("Content-Range"), f"bytes {start}-{end}/100", f"Content-Range for {rng}")


def get_range_invalid():
    put("range-inv.bin", rand(100))
    expect_error(lambda: get("range-inv.bin", offset=200, length=101), codes=("InvalidRange",), statuses=(416,))


def get_missing_key():
    expect_error(lambda: get("does/not/exist"), codes=("NoSuchKey",), statuses=(404,))


def get_if_none_match():
    p = put("cond.txt", b"cond")
    expect_error(lambda: get("cond.txt", request_headers={"If-None-Match": f'"{unquote(p.etag)}"'}),
                 codes=("NotModified",), statuses=(304,))


def get_if_match_fail():
    put("cond2.txt", b"cond")
    expect_error(lambda: get("cond2.txt", request_headers={"If-Match": '"wrong"'}),
                 codes=("PreconditionFailed",), statuses=(412,))


def delete_object():
    put("del.txt", b"x")
    mc.remove_object(B, "del.txt")
    expect_error(lambda: mc.stat_object(B, "del.txt"), **NOT_FOUND)


def delete_missing_object():
    mc.remove_object(B, "never-existed")  # raises on any non-2xx


def delete_objects():
    keys = ["dm/1", "dm/2", "dm/3"]
    for k in keys:
        put(k, k.encode())
    errs = list(mc.remove_objects(B, [DeleteObject(k) for k in keys]))
    eq([str(e) for e in errs], [], "errors")
    eq(list_all("dm/"), [], "remaining keys")


def list_v2_basic():
    objs = {"lb/b": rand(3), "lb/a": rand(1), "lb/c": rand(7)}
    for k, v in objs.items():
        put(k, v)
    got = list(mc.list_objects(B, prefix="lb/", recursive=True))
    eq([o.object_name for o in got], ["lb/a", "lb/b", "lb/c"], "keys")
    for o in got:
        eq(o.size, len(objs[o.object_name]), f"size of {o.object_name}")
        eq(unquote(o.etag), md5hex(objs[o.object_name]), f"etag of {o.object_name}")


def list_v2_prefix_delimiter():
    for k in ["a/1", "a/2", "b/1", "c"]:
        put(f"pd/{k}", k.encode())
    got = list(mc.list_objects(B, prefix="pd/"))  # recursive=False -> delimiter "/"
    eq([o.object_name for o in got if o.is_dir], ["pd/a/", "pd/b/"], "CommonPrefixes")
    eq([o.object_name for o in got if not o.is_dir], ["pd/c"], "Contents")


def list_v2_pagination():
    # minio-py follows continuation tokens itself; page boundaries are not observable, so we assert
    # the combined result of a MaxKeys=10 listing (which needs 3 server round trips).
    keys = [f"pg/{i:02d}" for i in range(25)]
    for k in keys:
        put(k, k.encode())
    got = [o.object_name for o in mc._list_objects(B, prefix="pg/", max_keys=10, encoding_type="url")]
    eq(got, keys, "keys")


def list_v2_start_after():
    keys = [f"sa/{i}" for i in range(10)]
    for k in keys:
        put(k, k.encode())
    got = [o.object_name for o in mc.list_objects(B, prefix="sa/", recursive=True, start_after="sa/4")]
    eq(got, keys[5:], "keys")


def list_v1():
    keys = [f"l1/{i}" for i in range(6)]
    for k in keys:
        put(k, k.encode())
    got = [o.object_name for o in mc.list_objects(B, prefix="l1/", recursive=True, start_after="l1/2",
                                                   use_api_v1=True)]
    eq(got, keys[3:], "keys after marker")
    got = [o.object_name for o in mc._list_objects(B, prefix="l1/", start_after="l1/2", max_keys=2,
                                                    use_api_v1=True)]
    eq(got, keys[3:], "keys after marker, MaxKeys=2")


def copy_object():
    data = rand(1234)
    put("cp/src", data, content_type="application/x-test", metadata=META)
    mc.copy_object(B, "cp/dst", CopySource(B, "cp/src"))
    r, body = get("cp/dst")
    bytes_eq(body, data)
    eq(r.headers.get("Content-Type"), "application/x-test", "Content-Type")
    eq(user_meta(r.headers), {"color": "Blue", "x-y": "z"}, "user metadata")


def copy_object_replace_metadata():
    data = rand(100)
    put("cpr/src", data, content_type="application/x-old", metadata=META)
    mc.copy_object(B, "cpr/dst", CopySource(B, "cpr/src"), metadata_directive=REPLACE,
                   metadata={"Content-Type": "application/x-new", "fresh": "yes"})
    st = mc.stat_object(B, "cpr/dst")
    eq(st.content_type, "application/x-new", "Content-Type")
    eq(user_meta(st.metadata), {"fresh": "yes"}, "user metadata")
    bytes_eq(get("cpr/dst")[1], data)


def _abort(key, uid):
    try:
        mc._abort_multipart_upload(B, key, uid)
    except Exception:  # noqa: BLE001
        pass


def multipart_upload():
    key, parts = "mpu/obj", [rand(5 * MiB), rand(5 * MiB), rand(KiB)]
    uid = mc._create_multipart_upload(B, key, {})
    done = [Part(i, mc._upload_part(B, key, p, None, uid, i)) for i, p in enumerate(parts, 1)]
    c = mc._complete_multipart_upload(B, key, uid, done)
    check(re.fullmatch(r"[0-9a-f]{32}-3", unquote(c.etag) or ""), f"ETag {c.etag} not of form <hex>-3")
    bytes_eq(get(key)[1], b"".join(parts))


def multipart_abort():
    key = "mpu/abort"
    uid = mc._create_multipart_upload(B, key, {})
    mc._upload_part(B, key, rand(KiB), None, uid, 1)
    mc._abort_multipart_upload(B, key, uid)
    expect_error(lambda: mc._list_parts(B, key, uid), codes=("NoSuchUpload",), statuses=(404,))


def multipart_list_parts():
    key, parts = "mpu/lp", [rand(5 * MiB), rand(KiB)]
    uid = mc._create_multipart_upload(B, key, {})
    try:
        for i, p in enumerate(parts, 1):
            mc._upload_part(B, key, p, None, uid, i)
        got = mc._list_parts(B, key, uid).parts
        eq([p.part_number for p in got], [1, 2], "part numbers")
        eq([p.size for p in got], [len(p) for p in parts], "sizes")
        eq([unquote(p.etag) for p in got], [md5hex(p) for p in parts], "etags")
    finally:
        _abort(key, uid)


def multipart_too_small():
    key = "mpu/small"
    uid = mc._create_multipart_upload(B, key, {})
    try:
        done = [Part(i, mc._upload_part(B, key, rand(KiB), None, uid, i)) for i in (1, 2)]
        expect_error(lambda: mc._complete_multipart_upload(B, key, uid, done), codes=("EntityTooSmall",))
    finally:
        _abort(key, uid)


def high_level_upload():
    # put_object with an unknown length streams parts of part_size (minio-py's transfer path).
    data = rand(20 * MiB)
    mc.put_object(B, "hl/obj", io.BytesIO(data), -1, part_size=5 * MiB)
    bytes_eq(get("hl/obj")[1], data)


def presigned_get():
    data = rand(500)
    put("ps/get", data)
    status, body = http("GET", mc.presigned_get_object(B, "ps/get", expires=timedelta(minutes=5)))
    eq(status, 200, "HTTP status")
    bytes_eq(body, data)


def presigned_put():
    data = rand(500)
    status, body = http("PUT", mc.presigned_put_object(B, "ps/put", expires=timedelta(minutes=5)), data)
    eq(status, 200, f"HTTP status ({body[:200]!r})")
    bytes_eq(get("ps/put")[1], data)


def bad_credentials():
    expect_error(lambda: mk("wrong-secret").list_buckets(),
                 codes=("SignatureDoesNotMatch", "InvalidAccessKeyId"), statuses=(403,))


def delete_bucket_not_empty():
    put("nonempty", b"x")
    expect_error(lambda: mc.remove_bucket(B), codes=("BucketNotEmpty",), statuses=(409,))


def versioning():
    mc.make_bucket(VB)
    mc.set_bucket_versioning(VB, VersioningConfig(ENABLED))
    v1, v2 = b"version-1", b"version-2"
    p1, p2 = put("k", v1, bucket=VB), put("k", v2, bucket=VB)
    check(p1.version_id and p2.version_id and p1.version_id != p2.version_id,
          f"distinct version ids expected, got {p1.version_id} / {p2.version_id}")
    vs = [o for o in mc.list_objects(VB, prefix="k", include_version=True) if o.object_name == "k"]
    eq(len(vs), 2, "version count")
    eq(sorted(o.version_id for o in vs), sorted([p1.version_id, p2.version_id]), "version ids")
    bytes_eq(get("k", bucket=VB, version_id=p1.version_id)[1], v1, "old version body")
    bytes_eq(get("k", bucket=VB)[1], v2, "latest body")


def object_tagging():
    put("tag.txt", b"t")
    tags = Tags.new_object_tags()
    tags["k1"], tags["k2"] = "v1", "v 2"
    mc.set_object_tags(B, "tag.txt", tags)
    got = mc.get_object_tags(B, "tag.txt")
    eq(dict(got or {}), {"k1": "v1", "k2": "v 2"}, "tags")


def delete_bucket():
    mc.make_bucket(DB)
    for k in ["x", "y/z"]:
        put(k, k.encode(), bucket=DB)
    objs = [DeleteObject(o.object_name) for o in mc.list_objects(DB, recursive=True)]
    eq([str(e) for e in mc.remove_objects(DB, objs)], [], "delete errors")
    mc.remove_bucket(DB)
    eq(mc.bucket_exists(DB), False, "bucket_exists after delete")


NO_CKSUM = Skip("minio-py has no flexible-checksum (ChecksumAlgorithm/ChecksumMode) API")
impl = {name: fn for name, fn in globals().items() if callable(fn) and not name.startswith("_")}
impl.update({
    "put_if_none_match": Skip("minio-py put_object has no conditional-write option "
                              "(non-allowlisted headers become x-amz-meta-*)"),
    "checksum_crc32": NO_CKSUM, "checksum_crc32c": NO_CKSUM, "checksum_sha1": NO_CKSUM,
    "checksum_sha256": NO_CKSUM, "checksum_crc64nvme": NO_CKSUM,
    "content_md5_bad": Skip("minio-py put_object has no Content-MD5 option "
                            "(a Content-MD5 metadata key becomes x-amz-meta-content-md5)"),
})

if __name__ == "__main__":
    run_suite(SDK, version("minio"), B, impl, cleanup_buckets=[VB, DB])
