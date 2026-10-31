"""Shared harness for the Python SDK matrix: config, scenario runner, assertions, checksums.

Each sdk_*.py builds a dict {scenario_id: callable | Skip("reason")} and calls run_suite().
Only botocore (present in both uv environments) is used for out-of-band setup/cleanup.
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import secrets
import threading
import time
import traceback
import urllib.error
import urllib.request
import zlib

ENDPOINT = os.environ.get("S3_ENDPOINT", "http://127.0.0.1:9100")
ACCESS_KEY = os.environ.get("S3_ACCESS_KEY", "fakes3")
SECRET_KEY = os.environ.get("S3_SECRET_KEY", "fakes3secret")
REGION = os.environ.get("S3_REGION", "us-east-1")
SERVER = os.environ.get("MATRIX_SERVER", "fakes3")
OUT = os.environ.get("MATRIX_OUT") or os.path.join(os.path.dirname(__file__), "..", "results", SERVER)
TIMEOUT_S = 60
VERBOSE = os.environ.get("MATRIX_VERBOSE") == "1"

KiB = 1024
MiB = 1024 * KiB
SPECIAL_KEYS = ["sp ace/üñí©ødé/a+b=c&d?e#f.txt", "a//b", "~tilde!*'()"]
META = {"color": "Blue", "x-y": "z"}

SCENARIOS = [
    "create_bucket", "head_bucket", "head_bucket_missing", "list_buckets",
    "put_get_small", "put_get_empty", "put_get_large", "special_keys", "head_object_metadata",
    "get_range", "get_range_invalid", "get_missing_key", "get_if_none_match", "get_if_match_fail",
    "put_if_none_match", "delete_object", "delete_missing_object", "delete_objects",
    "list_v2_basic", "list_v2_prefix_delimiter", "list_v2_pagination", "list_v2_start_after", "list_v1",
    "copy_object", "copy_object_replace_metadata",
    "multipart_upload", "multipart_abort", "multipart_list_parts", "multipart_too_small",
    "high_level_upload", "presigned_get", "presigned_put",
    "checksum_crc32", "checksum_crc32c", "checksum_sha1", "checksum_sha256", "checksum_crc64nvme",
    "content_md5_bad", "bad_credentials", "delete_bucket_not_empty", "versioning", "object_tagging",
    "delete_bucket",
]


class Skip:
    """Marker: the SDK has no API for this scenario."""

    def __init__(self, reason: str):
        self.reason = reason


class AssertionFailed(Exception):
    pass


def unique_bucket(sdk: str) -> str:
    return f"m-{sdk}-{secrets.token_hex(4)}"


def rand(n: int) -> bytes:
    return os.urandom(n)


def md5hex(b: bytes) -> str:
    return hashlib.md5(b).hexdigest()


def unquote(s):
    return None if s is None else str(s).strip('"')


def check(cond, msg: str):
    if not cond:
        raise AssertionFailed(msg)


def _canon(v):
    """Order-insensitive for dicts (metadata/header order is not significant), order-sensitive for lists."""
    if isinstance(v, dict):
        return {str(k): _canon(v[k]) for k in sorted(v, key=str)}
    if isinstance(v, (list, tuple)):
        return [_canon(x) for x in v]
    return v


def eq(actual, expected, what: str):
    if _canon(actual) != _canon(expected):
        raise AssertionFailed(f"{what}: expected {expected!r}, got {actual!r}")


def bytes_eq(actual: bytes, expected: bytes, what: str = "body"):
    if bytes(actual) != bytes(expected):
        raise AssertionFailed(f"{what} mismatch (got {len(actual)} bytes, expected {len(expected)})")


def first_line(s) -> str:
    s = str(s).strip()
    return s.splitlines()[0] if s else ""


def err_info(e: BaseException) -> tuple[str, int | None]:
    """Best-effort (code, http status) from botocore / minio / obstore / builtin exceptions."""
    code, status = "", None
    resp = getattr(e, "response", None)
    if isinstance(resp, dict):  # botocore ClientError
        code = resp.get("Error", {}).get("Code", "") or ""
        status = resp.get("ResponseMetadata", {}).get("HTTPStatusCode")
    else:
        code = getattr(e, "code", "") or ""
        st = getattr(resp, "status", None) or getattr(e, "status_code", None) or getattr(e, "status", None)
        if isinstance(st, int):
            status = st
    return str(code), status


def fmt_error(e: BaseException) -> str:
    t = type(e).__name__
    code, _ = err_info(e)
    if code and code != t and code not in str(e):
        t = f"{t}({code})"
    msg = first_line(e) or repr(e)
    return f"{t}: {msg}"[:300]


def expect_error(fn, codes=(), statuses=(), types=(), what="operation"):
    """fn must raise an error whose code, HTTP status or exception class name matches."""
    try:
        fn()
    except AssertionFailed:
        raise
    except Exception as e:  # noqa: BLE001
        code, status = err_info(e)
        names = {c.__name__ for c in type(e).__mro__}
        if code in codes or status in statuses or names & set(types):
            return e
        want = "/".join([*codes, *map(str, statuses), *types])
        raise AssertionFailed(
            f"{what}: expected {want}, got {type(e).__name__} code={code} status={status}: {first_line(e)}"
        ) from e
    raise AssertionFailed(f"{what}: expected error {'/'.join([*codes, *map(str, statuses), *types])}, but it succeeded")


# ---------------------------------------------------------------- plain HTTP (for presigned URLs)

def http(method: str, url: str, data: bytes | None = None) -> tuple[int, bytes]:
    req = urllib.request.Request(url, data=data, method=method)
    if data is not None:
        req.add_header("Content-Type", "")  # urllib would add application/x-www-form-urlencoded
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT_S) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


# ---------------------------------------------------------------- checksums (reference implementations)

def _table32(poly):
    t = []
    for i in range(256):
        c = i
        for _ in range(8):
            c = (c >> 1) ^ poly if c & 1 else c >> 1
        t.append(c)
    return t


def _table64(poly):
    t = []
    for i in range(256):
        c = i
        for _ in range(8):
            c = (c >> 1) ^ poly if c & 1 else c >> 1
        t.append(c)
    return t


_T32C = _table32(0x82F63B78)
_T64NVME = _table64(0x9A6C9329AC4BC9B5)


def crc32c(b: bytes) -> int:
    c = 0xFFFFFFFF
    for x in b:
        c = _T32C[(c ^ x) & 0xFF] ^ (c >> 8)
    return c ^ 0xFFFFFFFF


def crc64nvme(b: bytes) -> int:
    c = 0xFFFFFFFFFFFFFFFF
    for x in b:
        c = _T64NVME[(c ^ x) & 0xFF] ^ (c >> 8)
    return c ^ 0xFFFFFFFFFFFFFFFF


assert zlib.crc32(b"123456789") == 0xCBF43926
assert crc32c(b"123456789") == 0xE3069283
assert crc64nvme(b"123456789") == 0xAE8B14860A799888


def checksum_b64(alg: str, data: bytes) -> str:
    alg = alg.upper()
    if alg == "CRC32":
        raw = zlib.crc32(data).to_bytes(4, "big")
    elif alg == "CRC32C":
        raw = crc32c(data).to_bytes(4, "big")
    elif alg == "CRC64NVME":
        raw = crc64nvme(data).to_bytes(8, "big")
    elif alg == "SHA1":
        raw = hashlib.sha1(data).digest()
    elif alg == "SHA256":
        raw = hashlib.sha256(data).digest()
    else:
        raise ValueError(alg)
    return base64.b64encode(raw).decode()


def md5b64(b: bytes) -> str:
    return base64.b64encode(hashlib.md5(b).digest()).decode()


# ---------------------------------------------------------------- out-of-band admin (botocore)

def admin_client(secret: str | None = None):
    import botocore.session
    from botocore.config import Config

    return botocore.session.get_session().create_client(
        "s3", endpoint_url=ENDPOINT, region_name=REGION, aws_access_key_id=ACCESS_KEY,
        aws_secret_access_key=secret or SECRET_KEY,
        config=Config(s3={"addressing_style": "path"}, retries={"max_attempts": 1},
                      request_checksum_calculation="when_required",
                      response_checksum_validation="when_required"),
    )


def ensure_bucket(name: str):
    s3 = admin_client()
    try:
        s3.head_bucket(Bucket=name)
    except Exception:  # noqa: BLE001
        try:
            s3.create_bucket(Bucket=name)
        except Exception:  # noqa: BLE001
            pass


def nuke_bucket(name: str):
    """Best-effort: abort uploads, delete all versions/objects, delete the bucket."""
    s3 = admin_client()
    try:
        for u in s3.list_multipart_uploads(Bucket=name).get("Uploads", []):
            try:
                s3.abort_multipart_upload(Bucket=name, Key=u["Key"], UploadId=u["UploadId"])
            except Exception:  # noqa: BLE001
                pass
    except Exception:  # noqa: BLE001
        pass
    for _ in range(50):
        try:
            r = s3.list_object_versions(Bucket=name)
            objs = [{"Key": v["Key"], "VersionId": v["VersionId"]}
                    for v in r.get("Versions", []) + r.get("DeleteMarkers", [])]
        except Exception:  # noqa: BLE001
            try:
                objs = [{"Key": o["Key"]} for o in s3.list_objects_v2(Bucket=name).get("Contents", [])]
            except Exception:  # noqa: BLE001
                break
        if not objs:
            break
        for i in range(0, len(objs), 1000):
            try:
                s3.delete_objects(Bucket=name, Delete={"Objects": objs[i:i + 1000], "Quiet": True})
            except Exception:  # noqa: BLE001
                for o in objs[i:i + 1000]:
                    try:
                        s3.delete_object(Bucket=name, **o)
                    except Exception:  # noqa: BLE001
                        pass
    try:
        s3.delete_bucket(Bucket=name)
    except Exception:  # noqa: BLE001
        pass


# ---------------------------------------------------------------- runner

def _run_with_timeout(fn, timeout):
    box = {}

    def target():
        try:
            fn()
        except BaseException as e:  # noqa: BLE001
            box["err"] = e

    th = threading.Thread(target=target, daemon=True)
    th.start()
    th.join(timeout)
    if th.is_alive():
        raise TimeoutError(f"timed out after {timeout}s")
    if "err" in box:
        raise box["err"]


def run_suite(sdk: str, version: str, bucket: str, impl: dict, cleanup_buckets=(), ensure=True):
    results = []
    for sid in SCENARIOS:
        s = impl.get(sid)
        if s is None or isinstance(s, Skip):
            reason = s.reason if isinstance(s, Skip) else "not implemented for this SDK"
            results.append({"scenario": sid, "status": "skip", "error": reason})
            continue
        t0 = time.perf_counter()
        try:
            _run_with_timeout(s, TIMEOUT_S)
            r = {"scenario": sid, "status": "pass", "ms": round((time.perf_counter() - t0) * 1000, 1)}
        except BaseException as e:  # noqa: BLE001
            r = {"scenario": sid, "status": "fail", "ms": round((time.perf_counter() - t0) * 1000, 1),
                 "error": fmt_error(e)}
            if VERBOSE:
                traceback.print_exception(e)
        results.append(r)
        if VERBOSE or r["status"] == "fail":
            print(f"[{sdk}] {sid}: {r['status']}{' - ' + r['error'] if 'error' in r else ''}", flush=True)
        # Later scenarios need the bucket even if the SDK under test failed to create it.
        if sid == "create_bucket" and ensure:
            ensure_bucket(bucket)
    for b in [bucket, *cleanup_buckets]:
        try:
            nuke_bucket(b)
        except Exception:  # noqa: BLE001
            pass
    os.makedirs(OUT, exist_ok=True)
    doc = {"sdk": sdk, "lang": "python", "version": version, "server": SERVER, "results": results}
    with open(os.path.join(OUT, f"{sdk}.json"), "w") as f:
        json.dump(doc, f, indent=2, ensure_ascii=False)
        f.write("\n")
    n = lambda st: sum(1 for x in results if x["status"] == st)  # noqa: E731
    print(f"{sdk} {version} on {SERVER}: pass={n('pass')} fail={n('fail')} skip={n('skip')}", flush=True)
