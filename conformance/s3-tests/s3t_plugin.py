"""pytest plugin for the ceph/s3-tests runner: one JSON line per test phase, with markers.

JUnit XML does not carry markers (`fails_on_aws`, `lifecycle`, ...) and flattens the failure to a
blob; this keeps both, one file per process so pytest-xdist workers never interleave writes.
Enabled with `-p s3t_plugin`; output goes to `$S3T_RESULTS_DIR/phases.<pid>.jsonl`.

It also makes the suite's per-test bucket cleanup lenient (`S3T_LENIENT_CLEANUP=1`, the default):
upstream's autouse fixture raises when it cannot delete a leftover bucket, which then fails every later
test on that worker with a setup error, so one object stuck behind a lock hides hundreds of results.
Cleanup errors are recorded in `cleanup-errors.<pid>.jsonl` instead. Nothing a test asserts changes.
"""

import json
import os

import pytest

_NOISE = {"parametrize", "usefixtures", "filterwarnings", "skipif", "skip", "xfail"}
_out = None


def _lenient_cleanup(results_dir):
    from botocore.exceptions import ClientError

    import s3tests.functional as sf

    upstream_nuke = sf.nuke_bucket
    log = open(os.path.join(results_dir, f"cleanup-errors.{os.getpid()}.jsonl"), "a", buffering=1)

    def nuke_bucket(client, bucket):
        try:
            upstream_nuke(client, bucket)
            return
        except ClientError as e:
            first = e
        # Some servers (LocalStack) reject x-amz-bypass-governance-retention on buckets without Object
        # Lock, which AWS accepts; delete without it.
        try:
            for objects in sf.list_versions(client, bucket, 128):
                client.delete_objects(Bucket=bucket, Delete={"Objects": objects, "Quiet": True})
            client.delete_bucket(Bucket=bucket)
        except ClientError as e:
            first = e
        else:
            return
        log.write(json.dumps({"bucket": bucket, "error": str(first)[:500]}) + "\n")

    def nuke_prefixed_buckets(prefix, client=None):
        client = client or sf.get_client()
        for name in sf.get_buckets_list(client, prefix):
            try:
                sf.nuke_bucket(client, name)
            except Exception as e:  # noqa: BLE001 - cleanup must not fail the next test
                log.write(json.dumps({"bucket": name, "error": str(e)[:500]}) + "\n")

    sf.nuke_bucket = nuke_bucket
    sf.nuke_prefixed_buckets = nuke_prefixed_buckets


def pytest_configure(config):
    global _out
    d = os.environ["S3T_RESULTS_DIR"]
    os.makedirs(d, exist_ok=True)
    _out = open(os.path.join(d, f"phases.{os.getpid()}.jsonl"), "a", buffering=1)
    if os.environ.get("S3T_LENIENT_CLEANUP", "1") == "1":
        _lenient_cleanup(d)


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
            # The innermost frame in a test module: the assertion's own line, not assert_raises' helper.
            for entry in reversed(call.excinfo.traceback):
                if os.path.basename(str(entry.path)).startswith("test_"):
                    rec["test_line"] = f"{os.path.basename(str(entry.path))}:{entry.lineno + 1}"
                    try:
                        rec["test_src"] = str(entry.statement).strip()[:300]
                    except Exception:  # noqa: BLE001 - source may be unavailable
                        pass
                    break
    _out.write(json.dumps(rec) + "\n")
