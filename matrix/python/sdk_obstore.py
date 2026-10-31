"""obstore (Rust object_store bindings). A store is bound to one bucket and has no bucket-level API,
so buckets are created/inspected with botocore as setup only; every scenario that obstore can express
goes through obstore. Checksums: obstore can only *send* SHA256/CRC64NVME (store-level config) and
cannot read checksums back, so the stored value is verified with a botocore HeadObject(ChecksumMode).
"""

import re
import time
from datetime import timedelta
from importlib.metadata import version

import obstore as obs
from obstore.store import S3Store

from common import (
    ACCESS_KEY, ENDPOINT, KiB, META, MiB, REGION, SECRET_KEY, SPECIAL_KEYS, Skip, admin_client, bytes_eq,
    check, checksum_b64, ensure_bucket, eq, expect_error, http, md5hex, rand, run_suite, unique_bucket,
    unquote,
)

SDK = "obstore"
B = unique_bucket("obstore")


def mk(secret=SECRET_KEY, bucket=B, **config):
    return S3Store(bucket, endpoint=ENDPOINT, access_key_id=ACCESS_KEY, secret_access_key=secret, region=REGION,
                   virtual_hosted_style_request=False, client_options={"allow_http": True},
                   retry_config={"max_retries": 0, "backoff": {}, "retry_timeout": timedelta(seconds=30)},
                   **config)


st = mk()


def put(key, data, store=None, **kw):
    return obs.put(store or st, key, data, **kw)


def get(key, store=None, **options):
    return obs.get(store or st, key, options=options or None)


def body(key, **options):
    return bytes(get(key, **options).bytes())


def keys_of(metas):
    return [m["path"] for m in metas]


def setup_bucket():
    ensure_bucket(B)


def put_get_small():
    data = b"hello world"
    p = put("small.txt", data)
    g = get("small.txt")
    bytes_eq(bytes(g.bytes()), data)
    eq(unquote(p["e_tag"]), md5hex(data), "PUT ETag")
    eq(unquote(g.meta["e_tag"]), md5hex(data), "GET ETag")


def put_get_empty():
    put("empty", b"")
    g = get("empty")
    eq(len(bytes(g.bytes())), 0, "body length")
    eq(g.meta["size"], 0, "size")


def put_get_large():
    data = rand(8 * MiB)
    put("large.bin", data, use_multipart=False)
    bytes_eq(body("large.bin"), data)


def special_keys():
    for k in SPECIAL_KEYS:
        data = f"data:{k}".encode()
        put(k, data)
        bytes_eq(body(k), data, f"body of {k!r}")
    keys = keys_of(obs.list(st).collect())
    for k in SPECIAL_KEYS:
        check(k in keys, f"key {k!r} missing from list")


def head_object_metadata():
    put("meta.txt", b"meta", attributes={
        "Content-Type": "text/plain", "Cache-Control": "max-age=60",
        "Content-Disposition": 'attachment; filename="m.txt"', "Content-Encoding": "gzip", **META})
    # obstore.head() returns only ObjectMeta; attributes (incl. user metadata) come back on get().
    a = get("meta.txt").attributes
    eq(a.get("Content-Type"), "text/plain", "Content-Type")
    eq(a.get("Cache-Control"), "max-age=60", "Cache-Control")
    eq(a.get("Content-Disposition"), 'attachment; filename="m.txt"', "Content-Disposition")
    eq(a.get("Content-Encoding"), "gzip", "Content-Encoding")
    std = {"Content-Type", "Cache-Control", "Content-Disposition", "Content-Encoding", "Content-Language",
           "Storage-Class"}
    eq({k.lower(): v for k, v in a.items() if k not in std}, {"color": "Blue", "x-y": "z"}, "user metadata")


def get_range():
    # obstore exposes the parsed Content-Range as GetResult.range = (start, end_exclusive).
    data = rand(100)
    put("range.bin", data)
    for rng, opt, start, end in [("bytes=10-19", (10, 20), 10, 19), ("bytes=-5", {"suffix": 5}, 95, 99),
                                 ("bytes=95-", {"offset": 95}, 95, 99)]:
        g = get("range.bin", range=opt)
        bytes_eq(bytes(g.bytes()), data[start:end + 1], f"body for {rng}")
        eq(tuple(g.range), (start, end + 1), f"range for {rng}")
        eq(g.meta["size"], 100, f"total size for {rng}")


def get_range_invalid():
    put("range-inv.bin", rand(100))
    e = expect_error(lambda: body("range-inv.bin", range=(200, 301)), types=("BaseError",))
    check(re.search(r"\b416\b|InvalidRange", str(e)), f"expected 416/InvalidRange, got {e!r}")


def get_missing_key():
    expect_error(lambda: body("does/not/exist"), types=("FileNotFoundError", "NotFoundError"))


def get_if_none_match():
    p = put("cond.txt", b"cond")
    expect_error(lambda: body("cond.txt", if_none_match=p["e_tag"]), types=("NotModifiedError",))


def get_if_match_fail():
    put("cond2.txt", b"cond")
    expect_error(lambda: body("cond2.txt", if_match='"wrong"'), types=("PreconditionError",))


def put_if_none_match():
    key = f"inm-{time.time_ns()}"
    put(key, b"first", mode="create")
    # object_store maps 412 on a create-mode PUT to AlreadyExists.
    expect_error(lambda: put(key, b"second", mode="create"), types=("AlreadyExistsError", "PreconditionError"))
    bytes_eq(body(key), b"first")


def delete_object():
    put("del.txt", b"x")
    obs.delete(st, "del.txt")
    expect_error(lambda: obs.head(st, "del.txt"), types=("FileNotFoundError", "NotFoundError"))


def delete_missing_object():
    obs.delete(st, "never-existed")


def delete_objects():
    keys = ["dm/1", "dm/2", "dm/3"]
    for k in keys:
        put(k, k.encode())
    obs.delete(st, keys)  # list input -> DeleteObjects
    eq(keys_of(obs.list(st, "dm/").collect()), [], "remaining keys")


def list_v2_basic():
    objs = {"lb/b": rand(3), "lb/a": rand(1), "lb/c": rand(7)}
    for k, v in objs.items():
        put(k, v)
    got = obs.list(st, "lb/").collect()
    eq(keys_of(got), ["lb/a", "lb/b", "lb/c"], "keys")
    for m in got:
        eq(m["size"], len(objs[m["path"]]), f"size of {m['path']}")
        eq(unquote(m["e_tag"]), md5hex(objs[m["path"]]), f"ETag of {m['path']}")


def list_v2_prefix_delimiter():
    for k in ["a/1", "a/2", "b/1", "c"]:
        put(f"pd/{k}", k.encode())
    r = obs.list_with_delimiter(st, "pd")
    # object_store strips the trailing delimiter from common prefixes.
    eq(r["common_prefixes"], ["pd/a", "pd/b"], "CommonPrefixes")
    eq(keys_of(r["objects"]), ["pd/c"], "Contents")


def list_v2_start_after():
    keys = [f"sa/{i}" for i in range(10)]
    for k in keys:
        put(k, k.encode())
    eq(keys_of(obs.list(st, "sa/", offset="sa/4").collect()), keys[5:], "keys")


def copy_object():
    data = rand(1234)
    put("cp/src", data, attributes={"Content-Type": "application/x-test", **META})
    obs.copy(st, "cp/src", "cp/dst")
    g = get("cp/dst")
    bytes_eq(bytes(g.bytes()), data)
    eq(g.attributes.get("Content-Type"), "application/x-test", "Content-Type")
    eq({k: v for k, v in g.attributes.items() if k != "Content-Type"}, {"color": "Blue", "x-y": "z"},
       "user metadata")


def multipart_upload():
    # put(use_multipart=True, chunk_size=5 MiB) of 5 MiB + 5 MiB + 1 KiB -> 3 parts.
    parts = [rand(5 * MiB), rand(5 * MiB), rand(KiB)]
    put("mpu/obj", b"".join(parts), use_multipart=True, chunk_size=5 * MiB)
    etag = unquote(obs.head(st, "mpu/obj")["e_tag"])
    check(re.fullmatch(r"[0-9a-f]{32}-3", etag or ""), f"ETag {etag} not of form <hex>-3")
    bytes_eq(body("mpu/obj"), b"".join(parts))


def high_level_upload():
    data = rand(20 * MiB)
    with obs.open_writer(st, "hl/obj", buffer_size=5 * MiB) as w:
        for i in range(0, len(data), MiB):
            w.write(data[i:i + MiB])
    bytes_eq(body("hl/obj"), data)


def presigned_get():
    data = rand(500)
    put("ps/get", data)
    status, b = http("GET", obs.sign(st, "GET", "ps/get", timedelta(minutes=5)))
    eq(status, 200, "HTTP status")
    bytes_eq(b, data)


def presigned_put():
    data = rand(500)
    status, b = http("PUT", obs.sign(st, "PUT", "ps/put", timedelta(minutes=5)), data)
    eq(status, 200, f"HTTP status ({b[:200]!r})")
    bytes_eq(body("ps/put"), data)


def checksum_scenario(alg):
    def f():
        data, key = rand(1000), f"ck/{alg}"
        put(key, data, store=mk(checksum_algorithm=alg))
        bytes_eq(body(key), data)
        h = admin_client().head_object(Bucket=B, Key=key, ChecksumMode="ENABLED")
        eq(h.get(f"Checksum{alg}"), checksum_b64(alg, data), f"Checksum{alg} (verified via HeadObject)")
    return f


def bad_credentials():
    # No ListBuckets in obstore; list the bucket with a wrong secret instead.
    e = expect_error(lambda: obs.list(mk("wrong-secret")).collect(),
                     types=("PermissionDeniedError", "UnauthenticatedError", "BaseError"))
    check(type(e).__name__ != "GenericError" or re.search(r"\b403\b|SignatureDoesNotMatch|InvalidAccessKeyId",
                                                          str(e)), f"expected 403, got {e!r}")


NO_BUCKET = Skip("obstore has no bucket-level API (a store is bound to one existing bucket)")
NO_MPU = Skip("obstore has no low-level multipart API (only transparent multipart via put/open_writer)")
impl = {name: fn for name, fn in globals().items() if callable(fn) and not name.startswith("_")}
impl.update({
    "checksum_sha256": checksum_scenario("SHA256"),
    "checksum_crc64nvme": checksum_scenario("CRC64NVME"),
})
for sid in ["create_bucket", "head_bucket", "head_bucket_missing", "list_buckets", "delete_bucket_not_empty",
            "delete_bucket"]:
    impl[sid] = NO_BUCKET
for sid in ["multipart_abort", "multipart_list_parts", "multipart_too_small"]:
    impl[sid] = NO_MPU
for alg in ["crc32", "crc32c", "sha1"]:
    impl[f"checksum_{alg}"] = Skip("obstore S3 checksum_algorithm supports only SHA256 and CRC64NVME")
impl.update({
    "list_v2_pagination": Skip("obstore paginates internally; no MaxKeys/continuation-token control"),
    "list_v1": Skip("obstore only uses ListObjectsV2"),
    "copy_object_replace_metadata": Skip("obstore copy has no metadata options (always COPY directive)"),
    "content_md5_bad": Skip("obstore has no Content-MD5 option"),
    "versioning": Skip("obstore has no PutBucketVersioning / ListObjectVersions API"),
    "object_tagging": Skip("obstore can set tags on put but has no GetObjectTagging API"),
})

if __name__ == "__main__":
    setup_bucket()  # create_bucket is a skip, so run_suite won't ensure it
    run_suite(SDK, version("obstore"), B, impl)
