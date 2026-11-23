# /// script
# requires-python = ">=3.10"
# ///
"""Renders the JSON written by the startup bench (src/main.rs, startup.mjs) as Markdown.

    uv run bench/startup/report.py result-rust.json result-node.json result-node-rss.json
"""

import json
import statistics
import sys

NAMES = {
    "node": "Node `start()`",
    "rust_async": "Rust `Server::start`",
    "rust_blocking": "Rust `Server::start_blocking`",
    "process": "binary, child process",
}


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p / 100 * len(xs)))]


def f(x):
    return f"{x:.2f}" if x < 10 else f"{x:.1f}" if x < 100 else f"{x:,.0f}"


def main():
    data = {}
    for path in sys.argv[1:]:
        d = json.load(open(path))
        data.update({k: v for k, v in d.items() if k in NAMES or k == "node_rss"})
    order = [k for k in NAMES if k in data]

    print("### Start, one request, stop (sequential)\n")
    print("| | cycles | median | p10–p90 | start | request | stop | first cycle |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|")
    for k in order:
        s = data[k]["sequential"]
        t = s["total"]
        med = lambda c: f(statistics.median(s[c]))
        print(
            f"| {NAMES[k]} | {len(t)} | {f(statistics.median(t))} ms | {f(pct(t, 10))}–{f(pct(t, 90))} ms "
            f"| {med('start')} | {med('request')} | {med('stop')} | {f(s['first']['total'])} ms |"
        )
    if "node" in data and "import" in data["node"]:
        print(f"\nLoading the Node addon (`import \"fakes3-sdk\"`): {f(data['node']['import'])} ms, once per process.")

    print("\n### N cycles at once\n")
    print("| | N | wall time, median of rounds (min–max) | one cycle: median / slowest |")
    print("|---|---:|---:|---:|")
    for k in order:
        for n, rounds in data[k].get("parallel", {}).items():
            w = [r["wall"] for r in rounds]
            print(
                f"| {NAMES[k]} | {n} | {f(statistics.median(w))} ms ({f(min(w))}–{f(max(w))}) "
                f"| {f(statistics.median(r['median'] for r in rounds))} / {f(max(r['max'] for r in rounds))} ms |"
            )
    if r := data.get("node_rss"):
        print(
            f"\n{r['servers']} servers alive in one fresh Node process (one request each) add "
            f"{r['added_mib']:.0f} MiB of RSS to its {r['base_mib']:.0f} MiB."
        )


if __name__ == "__main__":
    main()
