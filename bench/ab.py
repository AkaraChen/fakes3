# /// script
# requires-python = ">=3.12"
# dependencies = ["psutil"]
# ///
"""Interleaved A/B of two fakes3 binaries on one machine (Linux, macOS or Windows).

Hosted CI runners differ by up to 2x between VMs, so comparing two separate bench runs is
meaningless. This runs warp against A and B alternately (ABBA order per round) on the same
machine and reports the median ops/s and the server CPU time per operation; the latter does not
depend on how fast the load generator is.

    uv run bench/ab.py <binary A> <binary B>   # env: ROUNDS (6), DURATION (10s), WORKLOADS (comma list), AUTH (1), WARP

warp is the binary named by WARP, else bench/bin/warp from bench/setup-native.sh, else the
minio/warp image on Docker's host network.

Both binaries run in the same authentication mode, so the comparison is like for like even when A
predates the `--auth` flag. AUTH=1 (default) verifies signatures: B gets `--auth`, A is started as
is (before the flag existed, verifying was its only mode). AUTH=0 measures the mode that does not
verify: `--no-auth` for both, which B accepts as a deprecated no-op because not verifying is its
default.
"""

import json
import os
import socket
import statistics as st
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import psutil

from native import host_info
from run import summarize_warp

HOST = host_info()
A, B = sys.argv[1], sys.argv[2]
ROUNDS = int(os.environ.get("ROUNDS", "6"))
DURATION = os.environ.get("DURATION", "10s")
AUTH = os.environ.get("AUTH", "1") != "0"
LOCAL_WARP = Path(__file__).resolve().parent / "bin" / ("warp.exe" if os.name == "nt" else "warp")
if os.environ.get("WARP"):
    WARP = [os.environ["WARP"]]
elif LOCAL_WARP.exists():
    WARP = [str(LOCAL_WARP)]
else:
    WARP = ["docker", "run", "--rm", "--network", "host", "minio/warp:latest"]

# name -> (warp args, op type to report; None sums all op types[, duration instead of DURATION])
ALL = {
    "small_get": (["get", "--obj.size", "4KiB", "--objects", "1000", "--concurrent", "16"], "GET"),
    "small_get_c1": (["get", "--obj.size", "4KiB", "--objects", "200", "--concurrent", "1"], "GET"),
    "small_put": (["put", "--obj.size", "4KiB", "--concurrent", "16"], "PUT"),
    "stat": (["stat", "--obj.size", "4KiB", "--objects", "1000", "--concurrent", "16"], "STAT"),
    "list": (["list", "--obj.size", "1KiB", "--objects", "1000", "--concurrent", "8"], "LIST"),
    "versioned": (["versioned", "--obj.size", "4KiB", "--objects", "1000", "--concurrent", "16"], None),
    "large_get": (["get", "--obj.size", "64MiB", "--objects", "8", "--concurrent", "4", "--disable-multipart"], "GET"),
    "large_get_c1": (["get", "--obj.size", "64MiB", "--objects", "8", "--concurrent", "1", "--disable-multipart"], "GET"),
    # Short: every PUT is a new 64 MiB object, and a longer run would fill the machine's memory.
    "large_put": (["put", "--obj.size", "64MiB", "--concurrent", "4", "--disable-multipart"], "PUT", "4s"),
}
WORKLOADS = [w for w in os.environ.get("WORKLOADS", "small_get,small_get_c1,small_put,stat,list,versioned").split(",") if w]


def cpu_sec(proc):
    t = proc.cpu_times()
    return t.user + t.system


def free_port():
    # A new port every run: Windows can refuse to rebind one that still has closing connections.
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def auth_args(binary):
    if not AUTH:
        return ["--no-auth"]
    help_text = subprocess.run([binary, "--help"], capture_output=True, text=True).stdout
    return ["--auth"] if "--auth" in help_text else []


def once(binary, wl):
    args, op, *duration = ALL[wl]
    port = free_port()
    p = subprocess.Popen(
        [binary, "--port", str(port), "--access-key", "fakes3", "--secret-key", "fakes3secret", *auth_args(binary)],
        stdout=subprocess.DEVNULL,
    )
    try:
        for _ in range(200):
            try:
                socket.create_connection(("127.0.0.1", port), 0.1).close()
                break
            except OSError:
                time.sleep(0.05)
        proc = psutil.Process(p.pid)
        c0 = cpu_sec(proc)
        # warp leaves its raw data in the working directory.
        with tempfile.TemporaryDirectory() as scratch:
            r = subprocess.run(
                [*WARP, *args, "--host", f"127.0.0.1:{port}", "--access-key", "fakes3", "--secret-key", "fakes3secret",
                 "--lookup", "path", "--duration", duration[0] if duration else DURATION, "--no-color", "--json"],
                capture_output=True, text=True, check=True, cwd=scratch,
            )
        cpu = cpu_sec(proc) - c0
        d = json.loads(r.stdout[r.stdout.find("{"):])
        ops = sum(x["ops_per_sec"] for x in summarize_warp(d, op).values())
        total = sum(x["total_requests"] for k, x in d["by_op_type"].items() if op is None or k == op)
        # Server CPU also covers warp's preparation phase; per-op it is still comparable A vs B.
        return ops, cpu / max(total, 1) * 1e6
    finally:
        p.kill()
        p.wait()


res = {(wl, w): [] for wl in WORKLOADS for w in "AB"}
for rnd in range(ROUNDS):
    for wl in WORKLOADS:
        order = [("A", A), ("B", B)] if rnd % 2 == 0 else [("B", B), ("A", A)]
        for name, binary in order:
            ops, cpu_us = once(binary, wl)
            res[(wl, name)].append((ops, cpu_us))
            print(f"round {rnd} {wl} {name}: {ops:.0f} ops/s, {cpu_us:.1f} server-µs/op", flush=True)

lines = [
    f"Median of {ROUNDS} interleaved rounds, {DURATION} each, {'signatures verified' if AUTH else 'signatures not verified (--no-auth / default)'}, "
    f"on {HOST['cpu']}, {HOST['cores']} cores, {HOST['os']}.",
    "",
    "| workload | A ops/s | B ops/s | B/A | A server µs/op | B server µs/op | B/A |",
    "|---|---:|---:|---:|---:|---:|---:|",
]
for wl in WORKLOADS:
    a_ops, a_cpu = (st.median(x[i] for x in res[(wl, "A")]) for i in (0, 1))
    b_ops, b_cpu = (st.median(x[i] for x in res[(wl, "B")]) for i in (0, 1))
    lines.append(f"| {wl} | {a_ops:,.0f} | {b_ops:,.0f} | {b_ops / a_ops:.3f} | {a_cpu:.1f} | {b_cpu:.1f} | {b_cpu / a_cpu:.3f} |")
print("\n".join(lines))
with open(os.environ.get("GITHUB_STEP_SUMMARY", os.devnull), "a", encoding="utf-8") as f:
    f.write("\n".join(lines) + "\n")
