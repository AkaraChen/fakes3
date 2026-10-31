# /// script
# requires-python = ">=3.12"
# dependencies = ["botocore"]
# ///
"""Benchmark every S3 implementation in bench/servers.sh under identical conditions.

Each measurement uses a freshly started container (Docker, host networking, tmpfs data), so
memory and CPU numbers belong to exactly one workload. Results go to bench/results/<ts>.json and
are rendered into bench/RESULTS.md.

    uv run bench/run.py                      # everything
    uv run bench/run.py --servers fakes3,minio --only startup,small_get
"""

import argparse
import datetime
import http.client
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
SERVERS_SH = ROOT / "bench" / "servers.sh"
AK, SK = "fakes3", "fakes3secret"
ALL_SERVERS = ["fakes3", "minio", "localstack", "moto", "s3mock", "juicefs"]

# Run in our own namespace so we never restart the long-lived servers the SDK matrix uses.
PREFIX = os.environ.setdefault("S3BENCH_PREFIX", "s3perf")
os.environ.setdefault("S3BENCH_PORT_OFFSET", "1000")

# name -> (warp subcommand args, op type to report[, duration override])
WARP = {
    "small_put": (["put", "--obj.size", "4KiB", "--concurrent", "16"], "PUT"),
    "small_get": (["get", "--obj.size", "4KiB", "--objects", "1000", "--concurrent", "16"], "GET"),
    # Many writers on one bucket: shows contention on the per-bucket index.
    "small_put_c64": (["put", "--obj.size", "4KiB", "--concurrent", "64"], "PUT"),
    "small_get_c1": (["get", "--obj.size", "4KiB", "--objects", "200", "--concurrent", "1"], "GET"),
    "stat": (["stat", "--obj.size", "4KiB", "--objects", "1000", "--concurrent", "16"], "STAT"),
    "list": (["list", "--obj.size", "1KiB", "--objects", "1000", "--concurrent", "8"], "LIST"),
    "delete": (["delete", "--obj.size", "4KiB", "--objects", "20000", "--batch", "100", "--concurrent", "8"], "DELETE"),
    # Short: every PUT is a new 64 MiB object, and 15 s would fill the host's RAM.
    "large_put": (["put", "--obj.size", "64MiB", "--concurrent", "4", "--disable-multipart"], "PUT", "5s"),
    "large_get": (["get", "--obj.size", "64MiB", "--objects", "16", "--concurrent", "4", "--disable-multipart"], "GET"),
    # warp uploads the parts during preparation, then benchmarks GET ?partNumber= reads.
    "multipart": (["multipart", "--part.size", "5MiB", "--parts", "40", "--concurrent", "8"], "GET"),
    "versioned": (["versioned", "--obj.size", "4KiB", "--objects", "1000", "--concurrent", "16"], None),
    "mixed": (["mixed", "--obj.size", "64KiB", "--objects", "1000", "--concurrent", "16"], None),
}


def sh(*args, check=True, **kw):
    return subprocess.run(args, check=check, text=True, capture_output=True, **kw)


def port(server):
    return int(sh(str(SERVERS_SH), "port", server).stdout.split()[0])


def cgroup(container):
    """The container's cgroup v2 directory, for both the systemd and cgroupfs Docker drivers."""
    cid = sh("docker", "inspect", "-f", "{{.Id}}", container).stdout.strip()
    for c in (f"/sys/fs/cgroup/system.slice/docker-{cid}.scope", f"/sys/fs/cgroup/docker/{cid}"):
        if Path(c, "memory.current").exists():
            return Path(c)
    raise RuntimeError(f"cgroup v2 directory for {container} not found")


def cpu_usec(cg):
    for line in (cg / "cpu.stat").read_text().splitlines():
        if line.startswith("usage_usec"):
            return int(line.split()[1])
    return 0


def mem(cg, name):
    return int((cg / name).read_text())


def list_buckets_probe(p):
    """A SigV4 header-signed ListBuckets, re-signed on every call.

    Presigned URLs would be cheaper, but LocalStack answers a presigned ListBuckets with a 500.
    """
    from botocore.auth import SigV4Auth
    from botocore.awsrequest import AWSRequest
    from botocore.credentials import Credentials

    creds = Credentials(AK, SK)
    url = f"http://127.0.0.1:{p}/"

    def signed_headers():
        req = AWSRequest(method="GET", url=url, headers={"x-amz-content-sha256": "UNSIGNED-PAYLOAD"})
        SigV4Auth(creds, "s3", "us-east-1").add_auth(req)
        return dict(req.headers.items())

    return p, signed_headers


def wait_ok(probe, timeout=180.0, alive=None):
    """Polls a signed ListBuckets until it returns 200; returns the time it did."""
    p, headers = probe
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            conn = http.client.HTTPConnection("127.0.0.1", p, timeout=2)
            conn.request("GET", "/", headers=headers())
            r = conn.getresponse()
            r.read()
            conn.close()
            if r.status == 200:
                return time.monotonic()
        except OSError:
            pass
        if alive and not alive():
            raise RuntimeError("server exited during startup")
        time.sleep(0.001)
    raise TimeoutError(f"port {p}")


def container_alive(server):
    r = sh("docker", "inspect", "-f", "{{.State.Running}}", f"{PREFIX}-{server}", check=False)
    return r.stdout.strip() == "true"


def start(server):
    """Starts a fresh container; returns (seconds until docker run returned, seconds until ready)."""
    sh(str(SERVERS_SH), "stop", server, check=False)
    url = list_buckets_probe(port(server))
    t0 = time.monotonic()
    sh(str(SERVERS_SH), "launch", server)
    t1 = time.monotonic()
    t2 = wait_ok(url, alive=lambda: container_alive(server))
    return t1 - t0, t2 - t0


def stop(server):
    sh(str(SERVERS_SH), "stop", server, check=False)


def docker_baseline(n):
    """Time for `docker run -d` of a no-op container: the overhead every server pays."""
    out = []
    for _ in range(n):
        t0 = time.monotonic()
        sh("docker", "run", "-d", "--rm", "--name", f"{PREFIX}-baseline", "--network", "host", "alpine:3", "sleep", "30")
        out.append(time.monotonic() - t0)
        sh("docker", "rm", "-f", f"{PREFIX}-baseline", check=False)
    return out


def native_startup(n):
    """Process-level startup without Docker, for the servers that ship a single binary."""
    res = {}
    cmds = {
        "fakes3": lambda p, d: [str(ROOT / "target/release/fakes3"), "--port", str(p), "--access-key", AK, "--secret-key", SK],
        "minio": lambda p, d: [str(ROOT / "bench/bin/minio"), "server", d, "--address", f"127.0.0.1:{p}"],
    }
    env = dict(os.environ, MINIO_ROOT_USER=AK, MINIO_ROOT_PASSWORD=SK, MINIO_BROWSER="off")
    for name, cmd in cmds.items():
        times = []
        for _ in range(n):
            p = 9190
            url = list_buckets_probe(p)
            with tempfile.TemporaryDirectory(dir="/dev/shm") as d:
                t0 = time.monotonic()
                proc = subprocess.Popen(cmd(p, d), env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                try:
                    times.append(wait_ok(url, alive=lambda: proc.poll() is None) - t0)
                finally:
                    proc.kill()
                    proc.wait()
        res[name] = times
    return res


def warp(server, args, duration):
    cmd = [
        "docker", "run", "--rm", "--network", "host", "minio/warp:latest", *args,
        "--host", f"127.0.0.1:{port(server)}", "--access-key", AK, "--secret-key", SK,
        "--lookup", "path", "--duration", duration, "--no-color", "--json",
    ]
    r = subprocess.run(cmd, text=True, capture_output=True, timeout=1200)
    out = r.stdout
    i = out.find("{")
    if r.returncode != 0 or i < 0:
        raise RuntimeError((r.stderr or out)[-500:])
    return json.loads(out[i:])


def summarize_warp(d, op):
    ops = d["by_op_type"]
    res = {}
    for name, x in ops.items():
        if op and name != op:
            continue
        t = x["throughput"]
        secs = t["measure_duration_millis"] / 1000
        if secs <= 0:
            st = datetime.datetime.fromisoformat(x["start_time"].replace("Z", "+00:00"))
            en = datetime.datetime.fromisoformat(x["end_time"].replace("Z", "+00:00"))
            secs = max((en - st).total_seconds(), 1e-9)
            n_ops, n_bytes = x["total_requests"], x["total_bytes"]
        else:
            n_ops, n_bytes = t["ops"], t["bytes"]
        lat = {}
        for reqs in x.get("requests_by_client", {}).values():
            seg = reqs[0]
            s = seg.get("single_sized_requests") or {}
            if s:
                lat = {"p50_ms": s.get("dur_median_millis"), "p99_ms": s.get("dur_99_millis")}
            break
        res[name] = {
            "ops_per_sec": n_ops / secs,
            "mib_per_sec": n_bytes / secs / (1 << 20),
            "errors": x.get("total_errors", 0),
            **lat,
        }
    return res


def measure(server, fn):
    """Runs fn against a fresh server and attaches the container's CPU and memory usage."""
    start(server)
    try:
        cg = cgroup(f"{PREFIX}-{server}")
        time.sleep(1)
        idle = mem(cg, "memory.current")
        c0 = cpu_usec(cg)
        t0 = time.monotonic()
        result = fn()
        wall = time.monotonic() - t0
        result.update(
            server_cpu_sec=(cpu_usec(cg) - c0) / 1e6,
            wall_sec=wall,
            mem_idle_mib=idle / (1 << 20),
            mem_peak_mib=mem(cg, "memory.peak") / (1 << 20),
            mem_end_mib=mem(cg, "memory.current") / (1 << 20),
        )
        return result
    finally:
        stop(server)


def run_tool(*args):
    r = sh(str(ROOT / "bench/bin/suite"), *args, check=False)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout).strip()[-500:])
    return json.loads(r.stdout)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--servers", default=",".join(ALL_SERVERS))
    ap.add_argument("--only", default="", help="comma list: startup,suite,footprint,<warp workloads>")
    ap.add_argument("--startup-runs", type=int, default=5)
    ap.add_argument("--duration", default="20s")
    ap.add_argument("--out", default="")
    a = ap.parse_args()
    servers = a.servers.split(",")
    only = set(filter(None, a.only.split(",")))
    want = lambda k: not only or k in only

    results = {"date": datetime.datetime.now().isoformat(timespec="seconds"), "host": host_info(), "servers": {}}
    out = Path(a.out or ROOT / "bench/results" / f"{results['date'].replace(':', '')}.json")
    out.parent.mkdir(parents=True, exist_ok=True)

    def save():
        out.write_text(json.dumps(results, indent=2))

    if want("startup"):
        results["docker_baseline_sec"] = docker_baseline(a.startup_runs)
        results["native_startup_sec"] = native_startup(a.startup_runs)
        save()

    for s in servers:
        r = results["servers"].setdefault(s, {})
        log = lambda *m: print(f"[{s}]", *m, file=sys.stderr, flush=True)
        if want("startup"):
            runs = []
            try:
                for _ in range(a.startup_runs):
                    runs.append(start(s))
                    cg = cgroup(f"{PREFIX}-{s}")
                    time.sleep(2)
                    r["mem_idle_mib"] = mem(cg, "memory.current") / (1 << 20)
                    stop(s)
                r["startup_sec"] = [x[1] for x in runs]
                r["docker_run_sec"] = [x[0] for x in runs]
                log("startup", statistics.median(r["startup_sec"]))
            except Exception as e:  # noqa: BLE001
                r["startup_error"] = str(e)[-500:]
                stop(s)
                log("startup failed", e)
            save()
        if want("footprint"):
            try:
                r["footprint"] = measure(s, lambda: run_tool("-load", "-endpoint", f"http://127.0.0.1:{port(s)}"))
            except Exception as e:  # noqa: BLE001
                r["footprint"] = {"error": str(e)[-500:]}
            log("footprint", r["footprint"])
            save()
        if want("suite"):
            try:
                r["suite"] = measure(s, lambda: run_tool("-tests", "100", "-endpoint", f"http://127.0.0.1:{port(s)}"))
            except Exception as e:  # noqa: BLE001
                r["suite"] = {"error": str(e)[-500:]}
            log("suite", r["suite"])
            save()
        for w, (args, op, *dur) in WARP.items():
            if not want(w):
                continue
            try:
                r[w] = measure(s, lambda: {"ops": summarize_warp(warp(s, args, dur[0] if dur else a.duration), op)})
            except Exception as e:  # noqa: BLE001
                r[w] = {"error": str(e)[-500:]}
                stop(s)
            log(w, r[w])
            save()

    save()
    print(out)


def host_info():
    cpu = next((l.split(":", 1)[1].strip() for l in open("/proc/cpuinfo") if l.startswith("model name")), "?")
    memtotal = next(l.split()[1] for l in open("/proc/meminfo") if l.startswith("MemTotal"))
    return {"cpu": cpu, "cores": os.cpu_count(), "mem_gib": round(int(memtotal) / (1 << 20), 1), "kernel": os.uname().release}


if __name__ == "__main__":
    main()
