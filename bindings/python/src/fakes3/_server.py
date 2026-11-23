"""Starting and stopping a fakes3 server process."""

from __future__ import annotations

import atexit
import collections
import os
import shutil
import subprocess
import threading
import weakref
from typing import Any, Deque, Dict, Iterable, List, Optional, Union
from urllib.parse import urlsplit

from ._control import Clock, Control, Requests

__all__ = ["FakeS3", "FakeS3Error", "binary_path", "start"]

_EXE = "fakes3.exe" if os.name == "nt" else "fakes3"


class FakeS3Error(Exception):
    """A server that failed to start, or a control call the server rejected.

    ``code`` is the control plane's error code (``"NotFound"``, ``"Conflict"``...) when there is one,
    and ``status`` its HTTP status.
    """

    def __init__(self, message: str, code: Optional[str] = None, status: Optional[int] = None) -> None:
        super().__init__(message)
        self.code = code
        self.status = status


def binary_path() -> str:
    """Path of the fakes3 executable this package runs.

    ``FAKES3_BINARY`` overrides it; otherwise it is the one bundled in the wheel.
    """
    override = os.environ.get("FAKES3_BINARY")
    if override:
        return override
    bundled = os.path.join(os.path.dirname(os.path.abspath(__file__)), "bin", _EXE)
    if os.path.isfile(bundled):
        return bundled
    found = shutil.which("fakes3")
    if found:
        return found
    raise FakeS3Error(
        "the fakes3 executable is missing: this installation has no prebuilt binary for your "
        "platform. Set FAKES3_BINARY to the path of a fakes3 binary."
    )


# Servers that are still running, stopped when the interpreter exits.
_live: "weakref.WeakSet[FakeS3]" = weakref.WeakSet()


@atexit.register
def _stop_all() -> None:
    for server in list(_live):
        server.stop()


def _terminate(proc: "subprocess.Popen[bytes]") -> None:
    """Ends the process and waits for it. Safe to call more than once."""
    if proc.poll() is None:
        try:
            proc.terminate()
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
        except OSError:
            pass
    # stdout and stderr are closed by the threads reading them, once they reach end of file.
    if proc.stdin is not None:
        try:
            proc.stdin.close()
        except OSError:
            pass


def _drain(stream: Any, sink: Deque[str]) -> None:
    try:
        for raw in iter(stream.readline, b""):
            sink.append(raw.decode("utf-8", "replace").rstrip("\r\n"))
    except (OSError, ValueError):
        pass
    finally:
        stream.close()


SizeLike = Union[int, str]


class FakeS3:
    """A running fakes3 server, in a child process.

    Use :func:`start` to create one. It is also a context manager that stops the server on exit.
    """

    def __init__(
        self,
        *,
        host: str = "127.0.0.1",
        port: int = 0,
        access_key: str = "fakes3",
        secret_key: str = "fakes3secret",
        auth: bool = False,
        users: Optional[Iterable[Union[str, Dict[str, str]]]] = None,
        bucket_defaults: Optional[str] = None,
        memory_limit: Optional[SizeLike] = None,
        domains: Optional[Iterable[str]] = None,
        inline_threshold: Optional[int] = None,
        request_log: bool = True,
        request_log_capacity: Optional[int] = None,
        args: Iterable[str] = (),
        binary: Optional[str] = None,
        startup_timeout: float = 10.0,
    ) -> None:
        self._access_key = access_key
        self._secret_key = secret_key
        self._lock = threading.Lock()
        self._proc: Optional[subprocess.Popen[bytes]] = None
        self._output: Deque[str] = collections.deque(maxlen=200)

        argv = [
            binary or binary_path(),
            "--host", host,
            "--port", str(port),
            "--access-key", access_key,
            "--secret-key", secret_key,
            "--exit-on-stdin-close",
        ]  # fmt: skip
        if auth:
            argv.append("--auth")
        for user in users or ():
            argv += ["--user", _user_arg(user)]
        if bucket_defaults is not None:
            argv += ["--bucket-defaults", bucket_defaults]
        if memory_limit is not None:
            argv += ["--memory-limit", str(memory_limit)]
        if domains is not None:
            domains = list(domains)
            argv += [f"--domain={d}" for d in domains] if domains else ["--domain="]
        if inline_threshold is not None:
            argv += ["--inline-threshold", str(inline_threshold)]
        if not request_log:
            argv.append("--no-request-log")
        if request_log_capacity is not None:
            argv += ["--request-log-capacity", str(request_log_capacity)]
        argv += list(args)

        # Settings come from the arguments only, not from FAKES3_* variables in the environment.
        env = {k: v for k, v in os.environ.items() if not k.upper().startswith("FAKES3_")}
        try:
            # stdin is a pipe only this process holds: when it exits, however it exits, the server
            # sees end of file and exits too (--exit-on-stdin-close).
            proc = subprocess.Popen(
                argv,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                env=env,
            )
        except OSError as e:
            raise FakeS3Error(f"cannot run {argv[0]}: {e}") from e
        self._proc = proc
        _live.add(self)
        weakref.finalize(self, _terminate, proc)
        try:
            endpoint = self._wait_ready(proc, startup_timeout)
        except BaseException:
            self.stop()
            raise
        self._endpoint = endpoint
        parts = urlsplit(endpoint)
        self._host = parts.hostname or host
        self._port = parts.port or 0
        self._control = Control(self._host, self._port)
        self.clock = Clock(self._control)
        """The server's virtual clock."""
        self.requests = Requests(self._control)
        """The request log."""

    def _wait_ready(self, proc: "subprocess.Popen[bytes]", timeout: float) -> str:
        # The server prints its endpoint on the first line of stdout once it is listening. Reading is
        # done on threads, as pipes cannot be polled on Windows.
        first: List[str] = []
        ready = threading.Event()
        output = self._output  # the threads must not keep `self` alive

        def read_stdout() -> None:
            line = proc.stdout.readline()  # type: ignore[union-attr]
            first.append(line.decode("utf-8", "replace").strip())
            ready.set()
            _drain(proc.stdout, output)

        threading.Thread(target=read_stdout, name="fakes3-stdout", daemon=True).start()
        threading.Thread(
            target=_drain, args=(proc.stderr, output), name="fakes3-stderr", daemon=True
        ).start()

        if not ready.wait(timeout):
            raise FakeS3Error(f"fakes3 did not start within {timeout:g}s{self._tail()}")
        endpoint = first[0]
        if not endpoint.startswith("http://"):
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                pass
            code = proc.poll()
            status = f" (exit code {code})" if code is not None else ""
            raise FakeS3Error(f"fakes3 failed to start{status}{self._tail()}")
        return endpoint

    def _tail(self) -> str:
        lines = list(self._output)[-20:]
        return (":\n" + "\n".join(lines)) if lines else ""

    # -- connection details -------------------------------------------------------------------

    @property
    def endpoint(self) -> str:
        """``http://host:port``, ready for an S3 client's ``endpoint_url``."""
        return self._endpoint

    @property
    def host(self) -> str:
        return self._host

    @property
    def port(self) -> int:
        return self._port

    @property
    def access_key(self) -> str:
        return self._access_key

    @property
    def secret_key(self) -> str:
        return self._secret_key

    @property
    def region(self) -> str:
        return "us-east-1"

    @property
    def pid(self) -> int:
        """Process id of the server."""
        assert self._proc is not None
        return self._proc.pid

    @property
    def running(self) -> bool:
        proc = self._proc
        return proc is not None and proc.poll() is None

    def boto3_kwargs(self) -> Dict[str, Any]:
        """Keyword arguments for ``boto3.client("s3", ...)``: endpoint, credentials, region and
        path-style addressing."""
        from botocore.config import Config

        return {
            "endpoint_url": self.endpoint,
            "aws_access_key_id": self.access_key,
            "aws_secret_access_key": self.secret_key,
            "region_name": self.region,
            "config": Config(s3={"addressing_style": "path"}),
        }

    def boto3_client(self, **overrides: Any) -> Any:
        """A boto3 S3 client for this server. Needs ``boto3`` installed; keyword arguments override
        the defaults from :meth:`boto3_kwargs`."""
        import boto3

        return boto3.client("s3", **{**self.boto3_kwargs(), **overrides})

    # -- lifecycle ----------------------------------------------------------------------------

    def stop(self) -> None:
        """Stops the server and waits for the process to exit. Safe to call more than once."""
        with self._lock:
            proc = self._proc
            if proc is None:
                return
            _terminate(proc)
            _live.discard(self)

    def __enter__(self) -> "FakeS3":
        return self

    def __exit__(self, *exc: Any) -> None:
        self.stop()

    def __repr__(self) -> str:
        state = "running" if self.running else "stopped"
        endpoint = getattr(self, "_endpoint", None)
        return f"<FakeS3 {endpoint} {state}>"

    # -- control plane ------------------------------------------------------------------------

    def _call(self, method: str, path: str, body: Any = None) -> Any:
        if not self.running:
            raise FakeS3Error("server is stopped")
        return self._control.call(method, path, body)

    def reset(self, *, clock: bool = True, requests: bool = True, snapshots: bool = False) -> None:
        """Clears all buckets, objects and uploads. By default also clears the request log and
        returns the clock to the host clock; snapshots are kept unless ``snapshots=True``."""
        q = f"clock={_b(clock)}&requests={_b(requests)}&snapshots={_b(snapshots)}"
        self._call("POST", f"/_fakes3/reset?{q}")

    def snapshot(self, name: Optional[str] = None) -> Dict[str, Any]:
        """Saves the current state under ``name`` (``snap-<n>`` when omitted). Object data is shared,
        not copied. Returns ``{"name", "created", "buckets", "objects", "uploads"}``."""
        return self._call("POST", "/_fakes3/snapshots", {"name": name} if name is not None else {})

    def restore(self, name: str) -> Dict[str, Any]:
        """Rolls the data back to snapshot ``name``. The snapshot is kept, so it can be restored
        again. The clock and request log are not touched."""
        return self._call("POST", f"/_fakes3/snapshots/{_seg(name)}/restore")

    def snapshots(self) -> List[Dict[str, Any]]:
        """The saved snapshots, oldest first."""
        return self._call("GET", "/_fakes3/snapshots")["snapshots"]

    def delete_snapshot(self, name: str) -> None:
        self._call("DELETE", f"/_fakes3/snapshots/{_seg(name)}")

    def memory(self) -> Dict[str, Optional[int]]:
        """``{"used": bytes of object data held, "limit": the memory limit or None}``."""
        return self._call("GET", "/_fakes3/stats")["memory"]

    def stats(self) -> Dict[str, Any]:
        """Memory, data counts, snapshots and request log state (``GET /_fakes3/stats``)."""
        return self._call("GET", "/_fakes3/stats")

    def status(self) -> Dict[str, Any]:
        """Everything in :meth:`stats`, plus version, clock, auth mode and accounts."""
        return self._call("GET", "/_fakes3")


def start(**options: Any) -> FakeS3:
    """Starts a fakes3 server on a free port of 127.0.0.1 and returns once it accepts requests.

    Every option of :class:`FakeS3` can be passed as a keyword argument.
    """
    return FakeS3(**options)


def _b(v: bool) -> str:
    return "true" if v else "false"


def _seg(name: str) -> str:
    from urllib.parse import quote

    return quote(name, safe="")


def _user_arg(user: Union[str, Dict[str, str]]) -> str:
    if isinstance(user, str):
        return user
    fields = [user["name"], user["access_key"], user["secret_key"]]
    if user.get("canonical_id") and not user.get("account_id"):
        raise ValueError("a user with canonical_id needs an account_id too")
    for k in ("account_id", "canonical_id"):
        if user.get(k):
            fields.append(user[k])
    return ":".join(fields)

