#!/usr/bin/env python3
"""Render the results of conformance/warp/run.py as Markdown.

    conformance/warp/report.py RESULTS_DIR
    conformance/warp/report.py --check RESULTS_DIR

RESULTS_DIR holds one <server>.json per server. Servers are shown fakes3 first.

--check prints nothing on success and exits 1 when a fakes3 server (fakes3, fakes3-aws) failed an
S3 case it is expected to pass. Other servers (MinIO, ...) are references and never fail the check;
neither do the MinIO-extension cases.
"""

import argparse
import json
import sys
from pathlib import Path

GROUPS = {
    "s3": "S3 API",
    "minio": "MinIO extensions (no AWS equivalent, not part of the S3 score)",
}


# Cases a fakes3 server fails on purpose (conformance/WARP.md, "What doesn't pass").
EXPECTED_FAILURES = {
    # New buckets block SSE-C with these defaults, as AWS's do.
    "fakes3-aws": {"put-sse-c", "get-sse-c"},
}


def check(reports):
    problems = []
    for r in reports:
        server = r["server"]
        if not server.startswith("fakes3"):
            continue
        expected = EXPECTED_FAILURES.get(server, set())
        for n, c in r["cases"].items():
            if c["group"] == "s3" and c["status"] != "pass" and n not in expected:
                problems.append(f"{server} `{n}`: {(c['messages'] or ['no message'])[0]}")
    return problems


def order(report):
    return (not report["server"].startswith("fakes3"), report["server"])


def score(report, group):
    cases = [c for c in report["cases"].values() if c["group"] == group]
    return sum(c["status"] == "pass" for c in cases), len(cases)


def cell(c):
    if c["status"] == "pass":
        return "pass"
    if not c["requests"]:
        return "**fail** (setup)"
    return f"**fail** ({c['errors']}/{c['requests']})"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("--check", action="store_true", help="exit 1 on an unexpected fakes3 S3 failure")
    a = ap.parse_args()
    reports = sorted((json.loads(p.read_text()) for p in Path(a.results).glob("*.json")), key=order)
    if not reports:
        sys.exit("no results")
    if a.check:
        if not any(r["server"].startswith("fakes3") for r in reports):
            sys.exit("no fakes3 results")
        problems = check(reports)
        if problems:
            print(f"{len(problems)} fakes3 S3 case(s) failed:")
            print("\n".join(f"- {p}" for p in problems))
            sys.exit(1)
        return
    servers = [r["server"] for r in reports]
    names = list(dict.fromkeys(n for r in reports for n in r["cases"]))
    first = {n: next(r["cases"][n] for r in reports if n in r["cases"]) for n in names}

    print("# warp workloads\n")
    print("A case passes when warp reports at least one request, no failed request and no error during")
    print("preparation or cleanup. warp checks sizes, and `atomic` also checks content, ETags and")
    print("listings; see conformance/WARP.md for what this does not cover.\n")

    print("| server | S3 API | MinIO extensions |")
    print("|---|--:|--:|")
    for r in reports:
        s3 = "{}/{}".format(*score(r, "s3"))
        minio = "{}/{}".format(*score(r, "minio"))
        print(f"| {r['server']} | {s3} | {minio} |")
    print()

    for group, title in GROUPS.items():
        print(f"## {title}\n")
        print("| case | exercises | " + " | ".join(servers) + " |")
        print("|---|---|" + "---|" * len(servers))
        for n in names:
            if first[n]["group"] != group:
                continue
            cells = [cell(r["cases"][n]) if n in r["cases"] else "" for r in reports]
            print(f"| `{n}` | {first[n]['what']} | " + " | ".join(cells) + " |")
        print()

    failures = [(r["server"], n, c) for r in reports for n, c in r["cases"].items() if c["status"] != "pass"]
    if failures:
        print("## Failures\n")
        for server, n, c in failures:
            print(f"- {server} `{n}`: {(c['messages'] or ['no message'])[0]}")


if __name__ == "__main__":
    main()
