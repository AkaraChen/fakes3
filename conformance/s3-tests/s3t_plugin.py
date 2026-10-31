"""pytest plugin for the ceph/s3-tests runner: one JSON line per test phase, with markers.

JUnit XML does not carry markers (`fails_on_aws`, `lifecycle`, ...) and flattens the failure to a
blob; this keeps both, one file per process so pytest-xdist workers never interleave writes.
Enabled with `-p s3t_plugin`; output goes to `$S3T_RESULTS_DIR/phases.<pid>.jsonl`.
"""

import json
import os

import pytest

_NOISE = {"parametrize", "usefixtures", "filterwarnings", "skipif", "skip", "xfail"}
_out = None


def pytest_configure(config):
    global _out
    d = os.environ["S3T_RESULTS_DIR"]
    os.makedirs(d, exist_ok=True)
    _out = open(os.path.join(d, f"phases.{os.getpid()}.jsonl"), "a", buffering=1)


def pytest_unconfigure(config):
    if _out:
        _out.close()


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    outcome = yield
    rep = outcome.get_result()
    rec = {
        "id": item.nodeid,
        "when": rep.when,
        "outcome": rep.outcome,
        "ms": round(rep.duration * 1000, 1),
        "markers": sorted({m.name for m in item.iter_markers()} - _NOISE),
    }
    if rep.outcome == "skipped":
        lr = rep.longrepr
        rec["message"] = str(lr[2] if isinstance(lr, tuple) else lr)[:300]
    elif rep.outcome == "failed":
        crash = getattr(rep.longrepr, "reprcrash", None)
        rec["message"] = (crash.message if crash else str(rep.longrepr))[:1500]
        if crash:
            rec["where"] = f"{os.path.relpath(crash.path)}:{crash.lineno}"
        if call.excinfo is not None:
            rec["exc"] = call.excinfo.typename
    _out.write(json.dumps(rec) + "\n")
