#!/usr/bin/env python3
"""Run warp's workloads against S3 servers and record which ones work (see conformance/WARP.md).

    conformance/warp/run.py [server...]    # default: fakes3 fakes3-aws minio

<server> is a bench/servers.sh name, or `fakes3-aws`: fakes3 started with --bucket-defaults aws.
Each case gets a freshly started server, so a case can't inherit a versioned bucket from the last
one and a server that dies only fails the case that killed it. Results go to <out>/<server>.json;
conformance/warp/report.py renders them.

warp exits 0 even when every request failed, so a case passes on what warp reports, never on its
exit code: at least one request, no failed request, and no error printed outside the measured phase
(warp reports preparation and cleanup failures only as log lines). A failing case is not a failing
run: the exit code is non-zero only when the run itself broke.

    --cases a,b        only these cases
    --duration 3s      time each workload runs
    --out DIR          result directory (default conformance/warp/results)

    WARP               warp binary (default bench/bin/warp, from `bench/setup-native.sh warp`)
    WARP_NO_SERVER     set to test a server that is already running (needs WARP_PORT)
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SERVERS_SH = ROOT / "bench" / "servers.sh"
WARP = os.environ.get("WARP") or str(ROOT / "bench" / "bin" / "warp")
AK, SK = "fakes3", "fakes3secret"

# Our own namespace: bench/run.py uses s3perf and offset 1000, the SDK matrix the defaults.
os.environ.setdefault("S3BENCH_PREFIX", "s3warp")
os.environ.setdefault("S3BENCH_PORT_OFFSET", "2000")

SMALL = ["--obj.size", "16KiB"]
LARGE = ["--obj.size", "12MiB", "--part.size", "5MiB", "--concurrent", "2"]


def put(*args):
    return ["put", *SMALL, *args]


def checksum(name):
    return put("--checksum", name)


def multipart_checksum(name):
    return ["put", *LARGE, "--checksum", name]


# name -> (group, warp args, what it exercises). Groups: `s3` is the AWS S3 API; `minio` needs an
# extension AWS doesn't have, so it says nothing about S3 compatibility.
CASES = {
    "put": ("s3", put(), "PutObject"),
    "put-multipart": ("s3", ["put", *LARGE], "multipart upload through PutObject"),
    "put-form": ("s3", put("--post"), "POST form upload"),
    "put-md5": ("s3", put("--md5"), "Content-MD5"),
    "put-unsigned": ("s3", put("--disable-sha256-payload"), "payload not hashed by the client"),
    "put-metadata": ("s3", put("--add-metadata", "warp=rand:512", "--tag", "warp=tag"), "user metadata, tags on PutObject"),
    "put-class": ("s3", put("--storage-class", "REDUCED_REDUNDANCY"), "storage class"),
    "put-sse-s3": ("s3", put("--sse-s3-encrypt"), "SSE-S3"),
    "put-sse-c": ("s3", put("--encrypt"), "SSE-C"),
    "put-sigv2": ("s3", put("--signature", "S3V2"), "signature v2"),
    "put-vhost": ("s3", put("--lookup", "host"), "virtual-host addressing"),
    **{
        f"checksum-{name.lower()}": ("s3", checksum(name), f"trailing {name} checksum")
        for name in ("CRC32", "CRC32C", "CRC64NVME", "SHA1", "SHA256", "MD5CS", "SHA512", "XXH64", "XXH3", "XXH128")
    },
    **{
        f"checksum-mpu-{name.lower()}": ("s3", multipart_checksum(name), f"{name} checksum on multipart upload")
        for name in ("CRC32", "CRC32C", "CRC32-FO", "CRC32C-FO", "CRC64NVME", "SHA1", "SHA256")
    },
    "get": ("s3", ["get", *SMALL, "--objects", "200"], "GetObject"),
    "get-range": ("s3", ["get", *SMALL, "--objects", "200", "--range"], "random byte ranges"),
    "get-range-size": ("s3", ["get", *SMALL, "--objects", "200", "--range", "--range-size", "1KiB"], "fixed-size byte ranges"),
    "get-versions": ("s3", ["get", *SMALL, "--objects", "100", "--versions", "2"], "GetObject by version id"),
    "get-sse-s3": ("s3", ["get", *SMALL, "--objects", "200", "--sse-s3-encrypt"], "reading SSE-S3 objects"),
    "get-sse-c": ("s3", ["get", *SMALL, "--objects", "200", "--encrypt"], "reading SSE-C objects with the key"),
    "stat": ("s3", ["stat", *SMALL, "--objects", "200"], "HeadObject"),
    "stat-versions": ("s3", ["stat", *SMALL, "--objects", "100", "--versions", "2"], "HeadObject by version id"),
    "list": ("s3", ["list", "--obj.size", "1KiB", "--objects", "500"], "ListObjectsV2"),
    "list-max-keys": ("s3", ["list", "--obj.size", "1KiB", "--objects", "500", "--max-keys", "7"], "ListObjectsV2 pagination"),
    "list-metadata": ("s3", ["list", "--obj.size", "1KiB", "--objects", "500", "--metadata"], "listing with metadata"),
    "list-versions": ("s3", ["list", "--obj.size", "1KiB", "--objects", "200", "--versions", "3"], "ListObjectVersions"),
    "delete": ("s3", ["delete", "--obj.size", "1KiB", "--objects", "2000", "--batch", "50"], "DeleteObjects"),
    "delete-one": ("s3", ["delete", "--obj.size", "1KiB", "--objects", "2000", "--batch", "1"], "DeleteObject"),
    "mixed": ("s3", ["mixed", *SMALL, "--objects", "200"], "GET, HEAD, PUT and DELETE at once"),
    "versioned": ("s3", ["versioned", *SMALL, "--objects", "200"], "the same on a versioned bucket"),
    "retention": ("s3", ["retention", *SMALL, "--objects", "100", "--versions", "2"], "Object Lock, PutObjectRetention"),
    "multipart": ("s3", ["multipart", "--part.size", "5MiB", "--parts", "20"], "UploadPart, GET ?partNumber="),
    "multipart-put": ("s3", ["multipart-put", "--part.size", "5MiB", "--parts", "3", "--part.concurrent", "2", "--concurrent", "2"], "full multipart upload cycle"),
    "atomic": ("s3", ["atomic", "--obj.size", "256KiB"], "torn or stale reads, list-after-write, ETag = MD5"),
    "atomic-randsize": ("s3", ["atomic", "--obj.size", "1MiB", "--obj.randsize"], "the same when overwrites change the length"),
    "append": ("s3", ["append", *SMALL], "x-amz-write-offset-bytes (S3 Express on AWS)"),
    "fanout": ("minio", ["fanout", *SMALL, "--copies", "3"], "POST with x-minio-fanout-list"),
    "zip": ("minio", ["zip", "--obj.size", "1KiB", "--files", "100"], "GET inside a zip, x-minio-extract"),
}


def servers_sh(*args, check=True):
    return subprocess.run([str(SERVERS_SH), *args], check=check, text=True, capture_output=True)


def start(server):
    name = "fakes3" if server == "fakes3-aws" else server
    if os.environ.get("WARP_NO_SERVER"):
        return int(os.environ["WARP_PORT"])
    env = dict(os.environ)
    if server == "fakes3-aws":
        env["FAKES3_RUN_ARGS"] = "-e FAKES3_BUCKET_DEFAULTS=aws"
    subprocess.run([str(SERVERS_SH), "start", name], check=True, text=True, capture_output=True, env=env)
    return int(servers_sh("port", name).stdout.split()[0])


def stop(server):
    if not os.environ.get("WARP_NO_SERVER"):
        servers_sh("stop", "fakes3" if server == "fakes3-aws" else server, check=False)


def distinct(messages, limit=3):
    """The first few different messages, ignoring the object names and ids that vary between them."""
    seen = {}
    for m in messages:
        m = m.strip()
        seen.setdefault(re.sub(r"[0-9a-zA-Z()._-]{16,}|\d+", "#", m), m[:300])
    return list(seen.values())[:limit]


def parse_object(text, at):
    try:
        return json.JSONDecoder().raw_decode(text, at)[0]
    except json.JSONDecodeError:
        return None


def judge(returncode, out, err):
    log = out + err
    logged = re.findall(r"<ERROR> *(.*)", log)
    # The report starts on the first line that opens an object; error lines before it may hold braces.
    start = re.search(r"^\{", out, re.M)
    total = parse_object(out, start.start()) if start else None
    if total is None or "total" not in total:
        # A failed preparation ends the run with `warp: {"error": {"cause": {"message": ...}}}`.
        fatal = re.search(r"^warp: (?=\{)", out, re.M)
        cause = (parse_object(out, fatal.end()) or {}).get("error", {}) if fatal else {}
        message = cause.get("cause", {}).get("message") or cause.get("message")
        tail = [line for line in log.splitlines() if line.strip()][-3:]
        messages = [message] if message else logged or tail
        return {"status": "fail", "requests": 0, "errors": 0, "ops": {}, "messages": distinct(messages), "exit": returncode}
    t = total["total"]
    requests, errors = t["total_requests"], t["total_errors"]
    ok = returncode == 0 and requests > 0 and errors == 0 and not logged
    return {
        "status": "pass" if ok else "fail",
        "requests": requests,
        "errors": errors,
        "ops": {op: x["total_requests"] for op, x in total["by_op_type"].items()},
        "messages": distinct([*(t.get("first_errors") or []), *logged]),
        "exit": returncode,
    }


def run_case(port, args, duration):
    # warp's default of 20 clients measures the machine, and this only needs every path to run.
    defaults = [] if "--concurrent" in args else ["--concurrent", "4"]
    # Virtual-host requests need a name, `<bucket>.127.0.0.1` isn't one.
    host = "localhost" if "--lookup" in args else "127.0.0.1"
    lookup = [] if "--lookup" in args else ["--lookup", "path"]
    cmd = [
        WARP, *args, *defaults, *lookup,
        "--host", f"{host}:{port}", "--access-key", AK, "--secret-key", SK,
        "--duration", duration, "--no-color", "--json",
    ]
    # warp leaves its raw data in the working directory; keep that out of the repository.
    with tempfile.TemporaryDirectory() as d:
        try:
            r = subprocess.run(cmd, text=True, capture_output=True, timeout=300, cwd=d)
        except subprocess.TimeoutExpired:
            return {"status": "fail", "requests": 0, "errors": 0, "ops": {}, "messages": ["timed out after 300s"], "exit": None}
    return judge(r.returncode, r.stdout, r.stderr)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("servers", nargs="*", default=["fakes3", "fakes3-aws", "minio"])
    ap.add_argument("--cases", default="", help="comma list (default: all)")
    ap.add_argument("--duration", default="3s")
    ap.add_argument("--out", default=str(ROOT / "conformance" / "warp" / "results"))
    a = ap.parse_args()

    cases = [c for c in a.cases.split(",") if c] or list(CASES)
    unknown = sorted(set(cases) - set(CASES))
    if unknown:
        sys.exit(f"unknown cases: {', '.join(unknown)}")
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)

    for server in a.servers:
        results = {}
        for name in cases:
            group, args, what = CASES[name]
            t0 = time.monotonic()
            try:
                port = start(server)
                r = run_case(port, args, a.duration)
            finally:
                stop(server)
            results[name] = {"group": group, "what": what, "args": args, "seconds": round(time.monotonic() - t0, 1), **r}
            detail = f"{r['errors']}/{r['requests']}" if r["status"] == "fail" else r["requests"]
            print(f"{server} {name}: {r['status']} ({detail})", *r["messages"][:1], flush=True)
        report = {"server": server, "duration": a.duration, "cases": results}
        (out / f"{server}.json").write_text(json.dumps(report, indent=1) + "\n")


if __name__ == "__main__":
    main()
