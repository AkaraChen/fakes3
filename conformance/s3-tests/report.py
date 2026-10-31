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
                             exc=r.get("exc", ""), phase=r["when"])
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


def classify(tid, t, identity):
    """Automatic category for a non-passing test, or None when it needs a human."""
    f = tid.split("::")[0].rsplit("/", 1)[-1]
    if set(t["markers"]) & NOT_AWS:
        return "not-aws"
    if f in OTHER_SERVICE:
        return "other-service"
    if base_name(tid) in identity:
        return "identity"
    return None


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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("--src")
    ap.add_argument("--json")
    ap.add_argument("--target", default="fakes3", help="server whose failures are broken down")
    a = ap.parse_args()

    servers = sorted(d for d in os.listdir(a.results) if os.path.isdir(os.path.join(a.results, d)))
    data = {s: load(os.path.join(a.results, s)) for s in servers}
    metas = {s: meta(os.path.join(a.results, s)) for s in servers}
    identity = identity_tests(a.src) if a.src else set()
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

    summary = {"s3tests_ref": ref, "servers": {}}
    for s in servers:
        tests = data[s]
        w(f"## {s}: by file (AWS-applicable tests)\n")
        w("| file | pass | fail | error | skip | total |")
        w("|---|--:|--:|--:|--:|--:|")
        files = sorted({tid.split("::")[0] for tid in tests})
        for f in files:
            (p, fl, e, k), n = counts(tests, lambda tid, t, f=f: tid.startswith(f + "::") and aws(tid, t))
            w(f"| {f.rsplit('/', 1)[-1]} | {p} | {fl} | {e} | {k} | {n} |")
        w("")

        cats = collections.Counter()
        for tid, t in tests.items():
            if t["status"] in ("fail", "error"):
                cats[classify(tid, t, identity) or "needs-triage"] += 1
        w(f"### {s}: failing tests by automatic category\n")
        w("| category | tests |")
        w("|---|--:|")
        for c, n in cats.most_common():
            w(f"| {c} | {n} |")
        w("")
        summary["servers"][s] = {
            "tests": {tid: {**t, "category": classify(tid, t, identity)} for tid, t in tests.items()},
        }

    tgt = a.target
    if tgt in data:
        sigs = collections.defaultdict(list)
        for tid, t in data[tgt].items():
            if t["status"] in ("fail", "error") and classify(tid, t, identity) is None:
                sigs[signature(t)].append(tid)
        w(f"## {tgt}: untriaged failures grouped by error (AWS-applicable, single-identity)\n")
        w("| tests | error | also passes on |")
        w("|--:|---|---|")
        others = [s for s in servers if s != tgt]
        for sig, ids in sorted(sigs.items(), key=lambda kv: -len(kv[1])):
            passers = [s for s in others if all(data[s].get(i, {}).get("status") == "pass" for i in ids)]
            esc = sig.replace("|", "\\|")
            w(f"| {len(ids)} | {esc} | {', '.join(passers) or '-'} |")
        w("")
        summary["signatures"] = {k: v for k, v in sigs.items()}

    print("\n".join(out))
    if a.json:
        json.dump(summary, open(a.json, "w"), indent=1)


if __name__ == "__main__":
    sys.exit(main())
