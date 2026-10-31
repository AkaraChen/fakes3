#!/usr/bin/env python3
"""Summarise ceph/s3-tests runs (written by run.sh) as Markdown.

    report.py RESULTS_DIR --src PATH_TO_S3_TESTS_CHECKOUT [--json summary.json] [--target fakes3]

RESULTS_DIR holds one directory per server (junit.xml, phases.*.jsonl, meta.json).
Without --src the "needs more than one identity" classification is skipped.
"""

import argparse
import ast
import collections
import glob
import json
import os
import re
import sys
import xml.etree.ElementTree as ET

# Tests AWS itself fails (or that only make sense on RGW) are not a signal about AWS compatibility.
NOT_AWS = {"fails_on_aws", "fails_on_s3"}
# Test files for services other than S3 proper.
OTHER_SERVICE = {
    "test_iam.py": "IAM",
    "test_sts.py": "STS",
    "test_sns.py": "SNS",
    "test_s3control.py": "S3 Control",
    "test_s3select.py": "S3 Select",
}
# A test that (transitively) calls one of these needs a second user, a tenant or IAM credentials.
IDENTITY = re.compile(r"^(get_(alt|tenant|iam|sts|v2_tenant)_|create_iam|get_main_account|get_alt)")
STATUSES = ["pass", "fail", "error", "skip"]


def load(server_dir):
    """test id -> {status, markers, message, where, ms} from the plugin's JSONL, backed by junit.xml."""
    tests = {}
    for path in glob.glob(os.path.join(server_dir, "phases.*.jsonl")):
        for line in open(path):
            r = json.loads(line)
            t = tests.setdefault(r["id"], {"markers": r["markers"], "ms": 0.0, "status": "pass"})
            t["ms"] += r["ms"]
            if r["outcome"] == "failed":
                st = "fail" if r["when"] == "call" else "error"
                if t["status"] in ("pass", "skip") or st == "fail":
                    t.update(status=st, message=r.get("message", ""), where=r.get("where", ""),
                             exc=r.get("exc", ""), phase=r["when"], test_line=r.get("test_line", ""),
                             test_src=r.get("test_src", ""))
            elif r["outcome"] == "skipped" and t["status"] == "pass" and r["when"] != "teardown":
                t.update(status="skip", message=r.get("message", ""))
    junit = os.path.join(server_dir, "junit.xml")
    if os.path.exists(junit):
        for case in ET.parse(junit).getroot().iter("testcase"):
            cls = case.get("classname", "")
            tid = cls.replace(".", "/") + ".py::" + case.get("name", "")
            if tid in tests:
                continue
            # No plugin record: the worker died (timeout kill, crash) before it could write one.
            bad = case.find("failure") if case.find("failure") is not None else case.find("error")
            skipped = case.find("skipped")
            node = bad if bad is not None else skipped
            tests[tid] = {
                "markers": [], "ms": float(case.get("time", 0)) * 1000,
                "status": "error" if bad is not None else "skip" if skipped is not None else "pass",
                "message": (node.get("message") or "")[:1500] if node is not None else "",
                "where": "junit-only", "exc": "WorkerCrash",
            }
    return tests


def meta(server_dir):
    try:
        return json.load(open(os.path.join(server_dir, "meta.json")))
    except OSError:
        return {}


def identity_tests(src):
    """Names of test functions that reach a user/tenant/IAM helper, following direct calls.

    Calls resolve within the test module first, then in s3tests/functional/__init__.py (where the
    shared helpers live); helpers imported from iam.py all count as identity helpers.
    """
    func_dir = os.path.join(src, "s3tests", "functional")
    graph = {}  # (module, name) -> called names
    for path in glob.glob(os.path.join(func_dir, "*.py")):
        mod = os.path.basename(path)
        for fn in ast.walk(ast.parse(open(path).read())):
            if isinstance(fn, (ast.FunctionDef, ast.AsyncFunctionDef)):
                calls = {n.func.id for n in ast.walk(fn) if isinstance(n, ast.Call) and isinstance(n.func, ast.Name)}
                graph.setdefault((mod, fn.name), set()).update(calls)
    iam_helpers = {name for (mod, name) in graph if mod == "iam.py"}
    memo = {}

    def reaches(mod, name, seen=()):
        if IDENTITY.match(name) or name in iam_helpers:
            return True
        key = next((k for k in ((mod, name), ("__init__.py", name)) if k in graph), None)
        if key is None or key in seen:
            return False
        if key not in memo:
            memo[key] = any(reaches(key[0], n, seen + (key,)) for n in graph[key])
        return memo[key]

    return {name for (mod, name) in graph if name.startswith("test_") and reaches(mod, name)}


def base_name(tid):
    return re.sub(r"\[.*\]$", "", tid.split("::")[-1])


def glob_re(pat):
    return re.compile("".join(".*" if c == "*" else re.escape(c) for c in pat) + r"\Z")


class Triage:
    """Hand-written failure groups (triage.json) first, then rules from markers, files and call graph."""

    def __init__(self, path, identity):
        self.identity = identity
        self.groups = {}
        self.rules = []  # (regex, group)
        if path and os.path.exists(path):
            for g in json.load(open(path))["groups"]:
                self.groups[g["id"]] = g
                self.rules += [(glob_re(t), g["id"]) for t in g["tests"]]

    def classify(self, tid, t):
        """-> (category, group id or None)"""
        f = tid.split("::")[0].rsplit("/", 1)[-1]
        name = tid.split("::")[-1]
        if set(t["markers"]) & NOT_AWS:
            return "marked-not-aws", None
        for rx, gid in self.rules:
            if rx.match(name) or rx.match(base_name(tid)):
                return self.groups[gid]["category"], gid
        if f in ("test_iam.py", "test_sts.py"):
            return "identity", "auto-" + f[5:-3]
        if f in OTHER_SERVICE:
            return "unimplemented", "auto-" + f[5:-3]
        if base_name(tid) in self.identity:
            return "identity", "auto-multi-user"
        return "untriaged", None


def signature(t):
    msg = (t.get("message") or "").splitlines()
    first = msg[0] if msg else ""
    m = re.search(r"\((\w+)\) when calling the (\w+) operation", first)
    if m:
        return f"ClientError {m.group(1)} on {m.group(2)}"
    first = re.sub(r"0x[0-9a-f]+|[0-9a-f]{16,}|\d+", "N", first)
    first = re.sub(r"s3t-[a-z0-9]+-N", "BUCKET", first)
    return first[:140] or t.get("exc", "?")


def counts(tests, pred=lambda tid, t: True):
    c = collections.Counter(t["status"] for tid, t in tests.items() if pred(tid, t))
    return [c[s] for s in STATUSES], sum(c.values())


def pct(passed, total):
    return f"{100 * passed / total:.1f}%" if total else "-"


AUTO_WHY = {
    "multi-user": "The test's call graph reaches `get_alt_*`, `get_tenant_*` or `get_iam_*` (found by static analysis of the s3-tests source). The runner maps every identity to the one credential the servers know, so a second user is the bucket owner and the expected `AccessDenied` / owner mismatch cannot happen. Individual failures were not inspected one by one: a real bug can hide behind the missing second identity.",
    "iam": "IAM API calls (`PutUserPolicy`, `CreateUser`, …) go to a server that has no IAM service.",
    "sts": "STS (`AssumeRole`, `GetSessionToken`, `GetCallerIdentity`, web identity) needs an STS service and, for web identity, an external Keycloak configured in `[webidentity]`.",
    "s3select": "S3 Select: `SelectObjectContent` is implemented only in part (NotImplemented for several query shapes); the remaining shapes below fail on the engine's output.",
    "s3control": "S3 Control API (account-level public access block) is served from a different endpoint than S3.",
    "sns": "SNS topics / bucket notifications delivery.",
}
CATEGORIES = [
    ("bug", "Real fakes3 bug (wrong status / code / header / body vs AWS)"),
    ("unimplemented", "Unimplemented feature"),
    ("identity", "Needs several users / tenants / IAM / STS (the servers know one credential)"),
    ("s3s", "s3s limitation (verified in the s3s source)"),
    ("test-defect", "Test is not AWS-compatible / RGW-specific / ineffective with boto3 (not marked by s3-tests)"),
    ("marked-not-aws", "Marked `fails_on_aws` by s3-tests itself (excluded from the baseline)"),
    ("untriaged", "Not yet triaged"),
]


def esc(x):
    return x.replace("|", "\\|").replace("\n", " ")


def rollup(rows, limit=40):
    """[(tid, t)] -> markdown bullets of 'N x error: sample names', biggest first."""
    sigs = collections.defaultdict(list)
    for tid, t in rows:
        sigs[signature(t)].append(tid.split("::")[-1])
    lines = []
    for sig, names in sorted(sigs.items(), key=lambda kv: -len(kv[1])):
        shown = ", ".join(f"`{n}`" for n in names[:3]) + (f", … ({len(names)})" if len(names) > 3 else "")
        lines.append(f"- {len(names)} × `{esc(sig)}` — {shown}")
    return lines[:limit] + ([f"- … {len(lines) - limit} more error shapes"] if len(lines) > limit else [])


def failure_sections(tests, triage, others, w):
    """The categorised list of every failing test of one server."""
    buckets = collections.defaultdict(lambda: collections.defaultdict(list))  # category -> group -> [(tid, t)]
    for tid, t in sorted(tests.items()):
        if t["status"] in ("fail", "error"):
            cat, gid = triage.classify(tid, t)
            buckets[cat][gid].append((tid, t))
    total = lambda cat: sum(len(v) for v in buckets[cat].values())  # noqa: E731
    w("| category | failing tests | groups |")
    w("|---|--:|--:|")
    for cat, label in CATEGORIES:
        if buckets[cat]:
            w(f"| {label} | {total(cat)} | {len(buckets[cat])} |")
    w("")
    for cat, label in CATEGORIES:
        if not buckets[cat]:
            continue
        w(f"### {label}: {total(cat)} tests\n")
        groups = buckets[cat]
        if cat in ("marked-not-aws", "untriaged"):
            w("\n".join(rollup([r for rows in groups.values() for r in rows])) + "\n")
            continue
        for gid, rows in sorted(groups.items(), key=lambda kv: -len(kv[1])):
            g = triage.groups.get(gid)
            names = [tid.split("::")[-1] for tid, _ in rows]
            if g:
                passers = sorted(s for s in others if all(others[s].get(tid, {}).get("status") == "pass" for tid, _ in rows))
                extra = f" Cost: **{g['cost']}**." if g.get("cost") else ""
                ref = f" Also passes on: {', '.join(passers)}." if passers else ""
                w(f"#### {g['title']} ({len(rows)})\n")
                w(f"{g['why']}{extra}{ref}\n")
            else:
                w(f"#### {gid.removeprefix('auto-').replace('-', ' ')} ({len(rows)})\n")
                w(AUTO_WHY.get(gid.removeprefix("auto-"), "") + "\n")
                w("\n".join(rollup(rows)) + "\n")
                continue
            w("Tests: " + ", ".join(f"`{n}`" for n in names) + "\n")
    return buckets


def skip_reasons(tests, w):
    c = collections.Counter(t.get("message", "")[:100] for t in tests.values() if t["status"] == "skip")
    if c:
        w("| skipped | reason |")
        w("|--:|---|")
        for reason, n in c.most_common():
            w(f"| {n} | {esc(reason.removeprefix('Skipped: '))} |")
        w("")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("--src")
    ap.add_argument("--json")
    ap.add_argument("--triage", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), "triage.json"))
    ap.add_argument("--target", default="fakes3", help="server whose failures are broken down")
    a = ap.parse_args()

    servers = sorted(d for d in os.listdir(a.results) if os.path.isdir(os.path.join(a.results, d)))
    data = {s: load(os.path.join(a.results, s)) for s in servers}
    metas = {s: meta(os.path.join(a.results, s)) for s in servers}
    identity = identity_tests(a.src) if a.src else set()
    triage = Triage(a.triage, identity)
    aws = lambda tid, t: not (set(t["markers"]) & NOT_AWS)  # noqa: E731

    out = []
    w = out.append
    ref = next((m["s3tests_ref"] for m in metas.values() if m.get("s3tests_ref")), "?")
    w(f"# ceph/s3-tests results (`{ref[:10]}`)\n")
    for title, pred in (("Tests AWS passes (`not fails_on_aws`)", aws), ("All tests", lambda tid, t: True)):
        w(f"## {title}\n")
        w("| server | pass | fail | error | skip | total | pass rate | pass rate excl. skips | runtime |")
        w("|---|--:|--:|--:|--:|--:|--:|--:|--:|")
        for s in servers:
            (p, f, e, k), n = counts(data[s], pred)
            sec = metas[s].get("seconds")
            w(f"| {s} | {p} | {f} | {e} | {k} | {n} | {pct(p, n)} | {pct(p, n - k)} | "
              f"{f'{sec // 60}m{sec % 60:02d}s' if sec is not None else '-'} |")
        w("")

    w("## By file (AWS-applicable tests)\n")
    w("| file | " + " | ".join(f"{s} pass/total" for s in servers) + " |")
    w("|---|" + "--:|" * len(servers))
    files = sorted({tid.split("::")[0] for s in servers for tid in data[s]})
    for f in files:
        cells = []
        for s in servers:
            (p, _, _, _), n = counts(data[s], lambda tid, t, f=f: tid.startswith(f + "::") and aws(tid, t))
            cells.append(f"{p}/{n}")
        w(f"| {f.rsplit('/', 1)[-1]} | " + " | ".join(cells) + " |")
    w("")

    summary = {"s3tests_ref": ref, "servers": {}}
    for s in servers:
        summary["servers"][s] = {tid: {**t, "category": triage.classify(tid, t)[0], "group": triage.classify(tid, t)[1]}
                                 for tid, t in data[s].items()}
    tgt = a.target
    if tgt in data:
        w(f"## {tgt}: failures by category\n")
        failure_sections(data[tgt], triage, {s: data[s] for s in servers if s != tgt}, w)
        w(f"## {tgt}: skipped tests\n")
        skip_reasons(data[tgt], w)

    print("\n".join(out))
    if a.json:
        json.dump(summary, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    sys.exit(main())
