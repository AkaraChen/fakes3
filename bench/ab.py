#!/usr/bin/env python3
"""Interleaved A/B of two fakes3 binaries on one machine.

Hosted CI runners differ by up to 2x between VMs, so comparing two separate bench runs is
meaningless. This runs warp against A and B alternately (ABBA order per round) on the same
machine and reports the median ops/s and the server CPU time per operation; the latter does not
depend on how fast the load generator is.

    bench/ab.py <binary A> <binary B>      # env: ROUNDS (6), DURATION (10s), WORKLOADS (comma list)
"""

import json
import os
import socket
import statistics as st
import subprocess
import sys
import time

A, B = sys.argv[1], sys.argv[2]
ROUNDS = int(os.environ.get("ROUNDS", "6"))
DURATION = os.environ.get("DURATION", "10s")
PORT = 9400

# name -> (warp args, op type to report; None sums all op types)
ALL = {
    "small_get": (["get", "--obj.size", "4KiB", "--objects", "1000", "--concurrent", "16"], "GET"),
    "small_get_c1": (["get", "--obj.size", "4KiB", "--objects", "200", "--concurrent", "1"], "GET"),
    "small_put": (["put", "--obj.size", "4KiB", "--concurrent", "16"], "PUT"),
    "stat": (["stat", "--obj.size", "4KiB", "--objects", "1000", "--concurrent", "16"], "STAT"),
    "list": (["list", "--obj.size", "1KiB", "--objects", "1000", "--concurrent", "8"], "LIST"),
    "versioned": (["versioned", "--obj.size", "4KiB", "--objects", "1000", "--concurrent", "16"], None),
    "large_get": (["get", "--obj.size", "64MiB", "--objects", "8", "--concurrent", "4", "--disable-multipart"], "GET"),
}
WORKLOADS = [w for w in os.environ.get("WORKLOADS", "small_get,small_get_c1,small_put,stat,list,versioned").split(",") if w]


def cpu_sec(pid):
    f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
    return (int(f[11]) + int(f[12])) / os.sysconf("SC_CLK_TCK")


def once(binary, wl):
    args, op = ALL[wl]
    p = subprocess.Popen(
        [binary, "--port", str(PORT), "--access-key", "fakes3", "--secret-key", "fakes3secret"],
        stdout=subprocess.DEVNULL,
    )
    try:
        for _ in range(200):
            try:
                socket.create_connection(("127.0.0.1", PORT), 0.1).close()
                break
            except OSError:
                time.sleep(0.05)
        c0 = cpu_sec(p.pid)
        r = subprocess.run(
            ["docker", "run", "--rm", "--network", "host", "minio/warp:latest", *args, "--host", f"127.0.0.1:{PORT}",
             "--access-key", "fakes3", "--secret-key", "fakes3secret", "--lookup", "path", "--duration", DURATION,
             "--no-color", "--json"],
            capture_output=True, text=True, check=True,
        )
        cpu = cpu_sec(p.pid) - c0
        d = json.loads(r.stdout[r.stdout.find("{"):])
        ops_types = [v for k, v in d["by_op_type"].items() if op is None or k == op]
        ops = sum(x["throughput"]["ops"] / (x["throughput"]["measure_duration_millis"] / 1000) for x in ops_types)
        total = sum(x["total_requests"] for x in ops_types)
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
    f"Median of {ROUNDS} interleaved rounds, {DURATION} each.",
    "",
    "| workload | A ops/s | B ops/s | B/A | A server µs/op | B server µs/op | B/A |",
    "|---|---:|---:|---:|---:|---:|---:|",
]
for wl in WORKLOADS:
    a_ops, a_cpu = (st.median(x[i] for x in res[(wl, "A")]) for i in (0, 1))
    b_ops, b_cpu = (st.median(x[i] for x in res[(wl, "B")]) for i in (0, 1))
    lines.append(f"| {wl} | {a_ops:,.0f} | {b_ops:,.0f} | {b_ops / a_ops:.3f} | {a_cpu:.1f} | {b_cpu:.1f} | {b_cpu / a_cpu:.3f} |")
print("\n".join(lines))
with open(os.environ.get("GITHUB_STEP_SUMMARY", "/dev/null"), "a") as f:
    f.write("\n".join(lines) + "\n")
