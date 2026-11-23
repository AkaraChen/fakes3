"""The server process never outlives its owner: stop(), exceptions, failing tests, Ctrl-C, exit, kill."""

import json
import os
import signal
import socket
import subprocess
import sys
import textwrap
import time

import pytest

import fakes3


def accepts_connections(port):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=1):
            return True
    except OSError:
        return False


def process_alive(pid):
    if os.name == "nt":
        import ctypes

        SYNCHRONIZE = 0x00100000
        kernel32 = ctypes.windll.kernel32
        handle = kernel32.OpenProcess(SYNCHRONIZE, False, pid)
        if not handle:
            return False
        try:
            return kernel32.WaitForSingleObject(handle, 0) != 0  # WAIT_OBJECT_0: it has exited
        finally:
            kernel32.CloseHandle(handle)
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    # A zombie still answers to kill(0) until its parent reaps it; it is no longer running.
    try:
        with open(f"/proc/{pid}/stat") as f:
            return f.read().rsplit(")", 1)[1].split()[0] != "Z"
    except OSError:
        return True


def assert_gone(pid, port, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not process_alive(pid) and not accepts_connections(port):
            return
        time.sleep(0.05)
    pytest.fail(f"fakes3 (pid {pid}, port {port}) is still running")


def test_stop():
    server = fakes3.start()
    pid, port = server.pid, server.port
    assert accepts_connections(port)
    server.stop()
    assert not server.running
    assert_gone(pid, port)
    server.stop()  # again: nothing happens
    with pytest.raises(fakes3.FakeS3Error, match="server is stopped"):
        server.reset()


def test_context_manager_stops_on_exception():
    with pytest.raises(RuntimeError):
        with fakes3.start() as server:
            pid, port = server.pid, server.port
            raise RuntimeError("test failed")
    assert_gone(pid, port)


def test_garbage_collected_server_is_stopped():
    import gc

    server = fakes3.start()
    pid, port = server.pid, server.port
    del server
    gc.collect()
    assert_gone(pid, port)


def test_startup_failure_is_reported():
    with pytest.raises(fakes3.FakeS3Error, match="failed to start") as e:
        fakes3.start(memory_limit="not-a-size")
    assert "memory" in str(e.value).lower()


def test_port_in_use_is_reported():
    with fakes3.start() as first:
        with pytest.raises(fakes3.FakeS3Error, match="failed to start"):
            fakes3.start(port=first.port)


def test_missing_binary_is_reported(tmp_path):
    with pytest.raises(fakes3.FakeS3Error, match="cannot run"):
        fakes3.start(binary=str(tmp_path / "no-such-fakes3"))


def test_options_reach_the_server():
    users = [{"name": "alice", "access_key": "AKALICE", "secret_key": "alice-secret"}]
    with fakes3.start(access_key="root", secret_key="root-secret", auth=True, users=users) as server:
        status = server.status()
        assert status["auth"] == "strict"
        assert [u["name"] for u in status["users"]][-1] == "alice"
        s3 = server.boto3_client()
        s3.create_bucket(Bucket="okay")
        bad = server.boto3_client(aws_secret_access_key="wrong")
        with pytest.raises(Exception, match="SignatureDoesNotMatch"):
            bad.list_buckets()


CHILD = textwrap.dedent(
    """
    import json, sys, time
    import fakes3

    server = fakes3.start()
    print(json.dumps({"pid": server.pid, "port": server.port}), flush=True)
    if sys.argv[1] == "exit":
        sys.exit(0)
    time.sleep(120)
    """
)


def run_child(tmp_path, mode, **popen):
    script = tmp_path / "child.py"
    script.write_text(CHILD)
    proc = subprocess.Popen(
        [sys.executable, str(script), mode], stdout=subprocess.PIPE, text=True, **popen
    )
    info = json.loads(proc.stdout.readline())
    if mode != "exit":
        assert accepts_connections(info["port"])
    return proc, info["pid"], info["port"]


def test_interpreter_exit_without_stop(tmp_path):
    proc, pid, port = run_child(tmp_path, "exit")
    assert proc.wait(timeout=30) == 0
    assert_gone(pid, port)


def test_owner_killed(tmp_path):
    # No cleanup code runs in a killed process: the server exits because its stdin closes.
    proc, pid, port = run_child(tmp_path, "sleep")
    proc.kill()
    proc.wait(timeout=30)
    assert_gone(pid, port)


@pytest.mark.skipif(os.name == "nt", reason="SIGINT is POSIX; Windows is covered by the Ctrl-Break test")
def test_owner_interrupted_with_ctrl_c(tmp_path):
    proc, pid, port = run_child(tmp_path, "sleep")
    proc.send_signal(signal.SIGINT)  # KeyboardInterrupt in the child
    proc.wait(timeout=30)
    assert_gone(pid, port)


@pytest.mark.skipif(os.name != "nt", reason="Windows console control events")
def test_owner_interrupted_with_ctrl_break(tmp_path):
    proc, pid, port = run_child(
        tmp_path, "sleep", creationflags=subprocess.CREATE_NEW_PROCESS_GROUP
    )
    proc.send_signal(signal.CTRL_BREAK_EVENT)
    proc.wait(timeout=30)
    assert_gone(pid, port)


# The inner session records each server it got, and a session fixture checks, while pytest is still
# running, that the servers of finished tests are gone: teardown stopped them, not process exit.
INNER_CONFTEST = textwrap.dedent(
    """
    import json, os, socket, pytest

    SEEN = []

    @pytest.fixture(autouse=True)
    def record(request):
        # Set up first, torn down last: by the end of the test the server's teardown has run.
        if "fakes3_server" in request.fixturenames:
            SEEN.append(request.getfixturevalue("fakes3_server").port)
        yield

    def refused(port):
        try:
            socket.create_connection(("127.0.0.1", port), timeout=1).close()
            return False
        except OSError:
            return True

    @pytest.fixture(scope="session", autouse=True)
    def check_stopped():
        yield
        with open(os.environ["FAKES3_RESULT"], "w") as f:
            json.dump({"ports": SEEN, "refused": [refused(p) for p in SEEN]}, f)
    """
)


def run_inner(pytester, tmp_path, monkeypatch, test_source):
    result_file = tmp_path / "result.json"
    pytester.makeconftest(INNER_CONFTEST)
    pytester.makepyfile(test_source)
    monkeypatch.setenv("FAKES3_RESULT", str(result_file))
    result = pytester.runpytest_subprocess("-p", "no:cacheprovider")
    return result, json.loads(result_file.read_text())


def test_failing_tests_stop_their_servers(pytester, tmp_path, monkeypatch):
    result, seen = run_inner(
        pytester,
        tmp_path,
        monkeypatch,
        """
        def test_fails(fakes3_server):
            fakes3_server.boto3_client().create_bucket(Bucket="b")
            assert False, "boom"

        def test_errors(fakes3_server):
            raise RuntimeError("boom")

        def test_passes(fakes3_server):
            assert fakes3_server.running
        """,
    )
    result.assert_outcomes(passed=1, failed=2)
    assert len(seen["ports"]) == 3 and len(set(seen["ports"])) == 3
    assert all(seen["refused"])


def test_keyboard_interrupt_in_a_test_stops_its_server(pytester, tmp_path, monkeypatch):
    result, seen = run_inner(
        pytester,
        tmp_path,
        monkeypatch,
        """
        def test_interrupted(fakes3_server):
            raise KeyboardInterrupt
        """,
    )
    assert result.ret == pytest.ExitCode.INTERRUPTED
    assert len(seen["ports"]) == 1
    assert all(seen["refused"])
