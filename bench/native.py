# /// script
# requires-python = ">=3.12"
# dependencies = ["botocore", "psutil"]
# ///
"""bench/run.py for machines without Linux containers: macOS, Windows, or Linux without Docker.

Every measurement starts a fresh server process (data, where a server has any, in a new
temporary directory) and runs the same workloads as bench/run.py: startup, the simulated test
suite, the fixed dataset, the warp workloads. CPU time and resident memory are read from the
server's process tree. Results use run.py's format, so bench/report.py renders them.

    bench/setup-native.sh && uv run bench/native.py
    uv run bench/native.py --servers fakes3 --only startup,small_get,large_put

Resident memory is what the OS reports for the processes (RSS; the working set on Windows). It
leaves out data a server keeps outside its own memory: MinIO's files, and on Linux the memfd pages
holding fakes3's large objects until they are first read.
"""

import argparse
import datetime
import json
import os
import platform
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import psutil

from run import AK, ROOT, SK, WARP, list_buckets_probe, run_tool, summarize_warp, wait_ok, warp

BIN = ROOT / "bench" / "bin"
ALL_SERVERS = ["fakes3", "minio", "moto"]
# tmpfs where there is one, like the Docker benchmark; elsewhere the system's temporary directory.
SCRATCH = "/dev/shm" if Path("/dev/shm").is_dir() else None
ENV = dict(os.environ, MINIO_ROOT_USER=AK, MINIO_ROOT_PASSWORD=SK, MINIO_BROWSER="off")


def command(server, port, data):
    """How each server starts; bench/setup-native.sh provides the binaries."""
    return {
        # --auth: as in bench/servers.sh, fakes3 verifies signatures like the other servers do.
        "fakes3": [ROOT / "target" / "release" / "fakes3", "--port", port, "--access-key", AK, "--secret-key", SK, "--auth"],
        "minio": [BIN / "minio", "server", data, "--address", f"127.0.0.1:{port}", "--quiet"],
        "moto": [BIN / "moto" / ("Scripts" if os.name == "nt" else "bin") / "moto_server", "-H", "127.0.0.1", "-p", port],
    }[server]


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Server:
    """A freshly started server, ready to serve; stopped (and its data removed) on exit."""

    def __init__(self, name):
        # A new port every time: Windows can refuse to rebind one that still has closing connections.
        self.port = free_port()
        self.data = tempfile.mkdtemp(prefix=f"s3bench-{name}-", dir=SCRATCH)
        probe = list_buckets_probe(self.port)
        t0 = time.monotonic()
        cmd = [str(c) for c in command(name, self.port, self.data)]
        self.proc = subprocess.Popen(cmd, env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.procs = []
        try:
            self.startup_sec = wait_ok(probe, alive=lambda: self.proc.poll() is None) - t0
            # Launchers (moto's on Windows) run the server as a child process; count the whole tree.
            root = psutil.Process(self.proc.pid)
            self.procs = [root, *root.children(recursive=True)]
        except BaseException:
            self.stop()
            raise

    def _sum(self, f):
        total = 0
        for p in self.procs:
            try:
                total += f(p)
            except psutil.Error:
                pass
        return total

    def cpu_sec(self):
        return self._sum(lambda p: p.cpu_times().user + p.cpu_times().system)

    def rss(self):
        return self._sum(lambda p: p.memory_info().rss)

    def stop(self):
        for p in reversed(self.procs):
            try:
                p.kill()
            except psutil.Error:
                pass
        self.proc.kill()
        self.proc.wait()
        shutil.rmtree(self.data, ignore_errors=True)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.stop()


class PeakRss(threading.Thread):
    """Samples a server's resident memory every 20 ms while in use; `peak` is the highest seen."""

    def __init__(self, server):
        super().__init__(daemon=True)
        self.server, self.peak, self.done = server, server.rss(), threading.Event()

    def run(self):
        while not self.done.wait(0.02):
            self.peak = max(self.peak, self.server.rss())

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, *exc):
        self.done.set()
        self.join()
        self.peak = max(self.peak, self.server.rss())


def measure(name, fn):
    """Runs fn(port) against a fresh server and attaches the server's CPU and memory usage."""
    with Server(name) as s:
        time.sleep(1)
        idle = s.rss()
        with PeakRss(s) as peak:
            c0, t0 = s.cpu_sec(), time.monotonic()
            result = fn(s.port)
            wall, cpu = time.monotonic() - t0, s.cpu_sec() - c0
        mib = 1 << 20
        result.update(
            server_cpu_sec=cpu,
            wall_sec=wall,
            mem_idle_mib=idle / mib,
            mem_peak_mib=peak.peak / mib,
            mem_end_mib=s.rss() / mib,
        )
        return result


def run_warp(port, args, duration):
    # warp leaves its raw data in the working directory; keep that out of the repository.
    with tempfile.TemporaryDirectory() as d:
        return warp(port, args, duration, [str(BIN / "warp")], cwd=d)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--servers", default=",".join(ALL_SERVERS))
    ap.add_argument("--only", default="", help="comma list: startup,suite,footprint,<warp workloads>")
    ap.add_argument("--startup-runs", type=int, default=5)
    ap.add_argument("--duration", default="20s")
    ap.add_argument("--out", default="")
    a = ap.parse_args()
    only = set(filter(None, a.only.split(",")))
    want = lambda k: not only or k in only

    results = {"date": datetime.datetime.now().isoformat(timespec="seconds"), "mode": "native", "host": host_info(), "servers": {}}
    out = Path(a.out or ROOT / "bench/results" / f"{results['date'].replace(':', '')}-native.json")
    out.parent.mkdir(parents=True, exist_ok=True)
    save = lambda: out.write_text(json.dumps(results, indent=2))

    for s in a.servers.split(","):
        r = results["servers"].setdefault(s, {})
        log = lambda *m: print(f"[{s}]", *m, file=sys.stderr, flush=True)

        def attempt(key, fn):
            try:
                r[key] = fn()
            except Exception as e:  # noqa: BLE001
                r[key] = {"error": str(e)[-500:]}
            log(key, r[key])
            save()

        if want("startup"):
            try:
                times, idle = [], []
                for _ in range(a.startup_runs):
                    with Server(s) as srv:
                        times.append(srv.startup_sec)
                        time.sleep(2)
                        idle.append(srv.rss() / (1 << 20))
                r["startup_sec"], r["mem_idle_mib"] = times, statistics.median(idle)
                log("startup", statistics.median(times))
            except Exception as e:  # noqa: BLE001
                r["startup_error"] = str(e)[-500:]
                log("startup failed", e)
            save()
        if want("footprint"):
            attempt("footprint", lambda: measure(s, lambda p: run_tool("-load", "-endpoint", f"http://127.0.0.1:{p}")))
        if want("suite"):
            attempt("suite", lambda: measure(s, lambda p: run_tool("-tests", "100", "-endpoint", f"http://127.0.0.1:{p}")))
        for w, (args, op, *dur) in WARP.items():
            if want(w):
                d = dur[0] if dur else a.duration
                attempt(w, lambda: measure(s, lambda p: {"ops": summarize_warp(run_warp(p, args, d), op)}))

    save()
    print(out)


def host_info():
    system = platform.system()
    if system == "Linux":
        cpu = next((l.split(":", 1)[1].strip() for l in open("/proc/cpuinfo") if l.startswith("model name")), "?")
        name = f"Linux {platform.release()}"
    elif system == "Darwin":
        cpu = subprocess.run(["sysctl", "-n", "machdep.cpu.brand_string"], capture_output=True, text=True).stdout.strip()
        name = f"macOS {platform.mac_ver()[0]}"
    elif system == "Windows":
        import winreg

        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0") as k:
            cpu = winreg.QueryValueEx(k, "ProcessorNameString")[0].strip()
        name = f"Windows {platform.version()}"
    else:
        cpu, name = platform.processor(), f"{system} {platform.release()}"
    mem = psutil.virtual_memory().total
    return {"cpu": cpu, "cores": os.cpu_count(), "mem_gib": round(mem / (1 << 30), 1), "os": name, "arch": platform.machine()}


if __name__ == "__main__":
    main()
