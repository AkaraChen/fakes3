#!/usr/bin/env python3
"""Render the results of conformance/warp/run.py as Markdown.

    conformance/warp/report.py RESULTS_DIR

RESULTS_DIR holds one <server>.json per server. Servers are shown fakes3 first.
"""

import json
import sys
from pathlib import Path

GROUPS = {
    "s3": "S3 API",
    "minio": "MinIO extensions (no AWS equivalent, not part of the S3 score)",
}


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
    reports = sorted((json.loads(p.read_text()) for p in Path(sys.argv[1]).glob("*.json")), key=order)
    if not reports:
        sys.exit("no results")
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
