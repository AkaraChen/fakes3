"""Client for the control plane (``/_fakes3/*``) on the server's own port."""

from __future__ import annotations

import http.client
import json
from datetime import datetime, timedelta, timezone
from typing import Any, Dict, List, Optional, Union
from urllib.parse import urlencode

DurationLike = Union[timedelta, int, float, str]
TimeLike = Union[datetime, str]


class Control:
    def __init__(self, host: str, port: int) -> None:
        # A server listening on every interface is reached through loopback.
        if host in ("0.0.0.0", ""):
            host = "127.0.0.1"
        elif host == "::":
            host = "::1"
        self._host = host
        self._port = port

    def call(self, method: str, path: str, body: Any = None) -> Any:
        from ._server import FakeS3Error

        # http.client rather than urllib: no proxy settings from the environment get in the way.
        conn = http.client.HTTPConnection(self._host, self._port, timeout=30)
        try:
            payload = None if body is None else json.dumps(body).encode()
            headers = {"Content-Type": "application/json"} if payload is not None else {}
            conn.request(method, path, body=payload, headers=headers)
            resp = conn.getresponse()
            data = resp.read()
        except OSError as e:
            raise FakeS3Error(f"control request {method} {path} failed: {e}") from e
        finally:
            conn.close()
        parsed = json.loads(data) if data else None
        if resp.status >= 400:
            err = (parsed or {}).get("error", {}) if isinstance(parsed, dict) else {}
            raise FakeS3Error(
                err.get("message") or f"HTTP {resp.status}",
                code=err.get("code"),
                status=resp.status,
            )
        return parsed


def format_time(at: TimeLike) -> str:
    """RFC 3339 for the server. A naive datetime is taken as UTC."""
    if isinstance(at, str):
        return at
    if at.tzinfo is None:
        at = at.replace(tzinfo=timezone.utc)
    return at.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")


def parse_time(s: str) -> datetime:
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    return datetime.fromisoformat(s)


def format_duration(by: DurationLike) -> str:
    """A duration the server understands. Numbers are seconds; strings pass through (``"1h30m"``)."""
    if isinstance(by, str):
        return by
    if isinstance(by, timedelta):
        us = (by.days * 86_400 + by.seconds) * 1_000_000 + by.microseconds
    elif isinstance(by, (int, float)) and not isinstance(by, bool):
        us = round(by * 1_000_000)
    else:
        raise TypeError(f"expected a timedelta, a number of seconds or a string, got {by!r}")
    return f"{us}us"


class Clock:
    """The server clock. Every timestamp the server produces reads it, and so does everything that
    depends on time: presigned URL expiry, lifecycle rules, Object Lock retention, archive restores.
    """

    def __init__(self, control: Control) -> None:
        self._c = control

    def _state(self, method: str, path: str, body: Any = None) -> Dict[str, Any]:
        return self._c.call(method, "/_fakes3/clock" + path, body)

    def now(self) -> datetime:
        """The server's current time, timezone-aware (UTC)."""
        return parse_time(self._state("GET", "")["now"])

    def is_frozen(self) -> bool:
        return bool(self._state("GET", "")["frozen"])

    def is_shifted(self) -> bool:
        """Whether the clock differs from the host clock (frozen, or moved)."""
        return bool(self._state("GET", "")["shifted"])

    def freeze(self, at: Optional[TimeLike] = None) -> datetime:
        """Stops time at ``at``, or at the current server time when omitted."""
        body = {"at": format_time(at)} if at is not None else {}
        return parse_time(self._state("POST", "/freeze", body)["now"])

    def unfreeze(self) -> datetime:
        """Lets time run again from the current server time."""
        return parse_time(self._state("POST", "/unfreeze")["now"])

    def set(self, at: TimeLike) -> datetime:
        """Jumps to ``at``, staying frozen or running."""
        return parse_time(self._state("POST", "/set", {"at": format_time(at)})["now"])

    def advance(self, by: DurationLike) -> datetime:
        """Moves time forward (or back, if negative): a ``timedelta``, a number of seconds, or a
        string such as ``"31d"``, ``"1h30m"`` or ``"-2w"``."""
        return parse_time(self._state("POST", "/advance", {"by": format_duration(by)})["now"])

    def reset(self) -> datetime:
        """Back to the host clock."""
        return parse_time(self._state("POST", "/reset")["now"])


class Requests:
    """The request log: every S3 request the server received, in arrival order.

    Records are dicts with the keys ``seq``, ``time``, ``method``, ``path``, ``bucket``, ``key``,
    ``operation``, ``status``, ``latency_us``, ``bytes_in``, ``bytes_out``, ``access_key`` and
    ``request_id``.
    """

    def __init__(self, control: Control) -> None:
        self._c = control

    def _query(self, **filters: Any) -> Dict[str, Any]:
        names = {"operation": "op"}
        params = {}
        for k, v in filters.items():
            if v is None:
                continue
            if k == "since":
                v = format_time(v)
            params[names.get(k, k)] = v
        q = urlencode(params)
        return self._c.call("GET", "/_fakes3/requests" + (f"?{q}" if q else ""))

    def list(
        self,
        *,
        bucket: Optional[str] = None,
        key_prefix: Optional[str] = None,
        operation: Optional[str] = None,
        method: Optional[str] = None,
        status: Union[int, str, None] = None,
        since: Optional[TimeLike] = None,
        after: Optional[int] = None,
        limit: Optional[int] = None,
    ) -> List[Dict[str, Any]]:
        """Records matching every given filter, oldest first. ``status`` is a code (``404``), a
        class (``"4xx"``) or ``"error"`` (400 and above); ``after`` is a ``seq``; ``limit`` keeps
        the most recent matches."""
        return self._query(
            bucket=bucket, key_prefix=key_prefix, operation=operation, method=method,
            status=status, since=since, after=after, limit=limit,
        )["requests"]  # fmt: skip

    def count(
        self,
        *,
        bucket: Optional[str] = None,
        key_prefix: Optional[str] = None,
        operation: Optional[str] = None,
        method: Optional[str] = None,
        status: Union[int, str, None] = None,
        since: Optional[TimeLike] = None,
        after: Optional[int] = None,
    ) -> int:
        """How many records match the filters."""
        return int(
            self._query(
                bucket=bucket, key_prefix=key_prefix, operation=operation, method=method,
                status=status, since=since, after=after, limit=0,
            )["total"]
        )  # fmt: skip

    def clear(self) -> None:
        self._c.call("DELETE", "/_fakes3/requests")

    def config(self) -> Dict[str, Any]:
        """``{"enabled", "capacity", "stored", "dropped"}``."""
        return self._c.call("GET", "/_fakes3/requests/config")

    def configure(self, *, enabled: Optional[bool] = None, capacity: Optional[int] = None) -> Dict[str, Any]:
        """Turns recording on or off, or changes how many records are kept (shrinking drops the
        oldest). Returns the new :meth:`config`."""
        body: Dict[str, Any] = {}
        if enabled is not None:
            body["enabled"] = enabled
        if capacity is not None:
            body["capacity"] = capacity
        return self._c.call("PUT", "/_fakes3/requests/config", body)
