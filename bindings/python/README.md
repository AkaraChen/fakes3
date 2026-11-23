# fakes3-sdk

English | [简体中文](https://github.com/AkaraChen/fakes3/blob/main/bindings/python/README.zh-CN.md)

Python package and pytest fixtures for [fakes3](https://github.com/AkaraChen/fakes3), an in-memory S3
server for tests. The wheel contains a prebuilt fakes3 binary: no Docker, nothing downloaded at run
time. A server starts in a few milliseconds, so every test can have its own.

```bash
pip install fakes3-sdk
```

The package is imported as `fakes3`. It has no dependencies of its own; the examples use boto3.

## pytest

Installing the package registers the fixtures. `fakes3_server` gives each test a fresh, empty server
and stops it when the test ends, whether it passed, failed or was interrupted:

```python
import urllib.error
import urllib.request

import pytest


def test_upload(fakes3_server):
    s3 = fakes3_server.boto3_client()  # boto3 client for this server, path-style
    s3.create_bucket(Bucket="demo")
    s3.put_object(Bucket="demo", Key="hello.txt", Body=b"hi")

    assert fakes3_server.requests.count(operation="PutObject") == 1


def test_link_expires(fakes3_server):
    s3 = fakes3_server.boto3_client()
    s3.create_bucket(Bucket="share")
    s3.put_object(Bucket="share", Key="report.csv", Body=b"...")
    url = s3.generate_presigned_url(
        "get_object", Params={"Bucket": "share", "Key": "report.csv"}, ExpiresIn=60
    )

    fakes3_server.clock.advance("2m")  # the server's clock, not your machine's
    with pytest.raises(urllib.error.HTTPError) as e:
        urllib.request.urlopen(url)
    assert e.value.code == 403  # AccessDenied: Request has expired
```

To hand your own code the connection details instead, use `fakes3_server.endpoint`,
`fakes3_server.access_key` and `fakes3_server.secret_key` (any credentials work unless `auth=True`).

| Fixture | Scope | |
| --- | --- | --- |
| `fakes3_server` | function | A new server for every test |
| `fakes3_session_server` | session | One server for the whole run (one per worker with pytest-xdist). Tests share its data unless they call `reset()` or `restore()` |
| `fakes3_options` | session | Keyword arguments for `fakes3.start()` used by both. Override it in `conftest.py` |

```python
# conftest.py
import pytest

@pytest.fixture(scope="session")
def fakes3_options():
    return {"bucket_defaults": "aws", "memory_limit": "1GiB"}
```

Sharing a server and resetting it between tests is also an option, when tests seed a lot of data once:

```python
@pytest.fixture
def s3_store(fakes3_session_server):
    fakes3_session_server.restore("seeded")  # a snapshot taken after seeding
    yield fakes3_session_server
```

## Without pytest

```python
import fakes3

with fakes3.start() as server:      # 127.0.0.1, a free port
    s3 = server.boto3_client()
    ...
# stopped here

server = fakes3.start()
...
server.stop()                        # safe to call more than once
```

`start()` returns once the server accepts requests. The server runs as a child process and never
outlives yours: `stop()`, leaving the `with` block, garbage collection and interpreter exit all stop
it, and if your process is killed outright the server notices its stdin pipe closing and exits too.

### Options

All are keyword arguments to `start()` and optional:

| Option | Default | Description |
| --- | --- | --- |
| `host` | `"127.0.0.1"` | Address to listen on |
| `port` | `0` | Port to listen on; `0` picks a free one |
| `access_key` / `secret_key` | `"fakes3"` / `"fakes3secret"` | Credentials of the default account, checked only with `auth=True` |
| `auth` | `False` | Verify signatures. By default any credentials work: a signed request acts as the account its access key belongs to (the default account if none does), and an unsigned request is anonymous, subject to ACLs, bucket policies and Block Public Access |
| `users` | none | More accounts: dicts with `name`, `access_key`, `secret_key` and optionally `account_id`, `canonical_id` |
| `bucket_defaults` | `"legacy"` | Settings new buckets start with: `"legacy"`, or `"aws"` for what AWS applies today (ACLs disabled, Block Public Access on, SSE-S3, SSE-C blocked) |
| `memory_limit` | `"auto"` | Cap on object data: bytes, `"512MiB"`, `"2GiB"`, `"unlimited"`, or `"auto"` (75% of the container's or machine's memory). Writes past it fail with HTTP 507 |
| `domains` | `["localhost"]` | Base domains for virtual-host addressing (`<bucket>.<domain>`); path-style always works |
| `inline_threshold` | `1048576` | On Linux, objects at or above this many bytes are stored in a `memfd` instead of on the heap |
| `request_log` / `request_log_capacity` | `True` / `10000` | Whether to record requests for `server.requests`, and how many to keep |
| `args` | none | Extra command-line arguments for the `fakes3` binary |
| `startup_timeout` | `10` | Seconds to wait for the server to start |

`FAKES3_*` environment variables are ignored; only these options configure the server.

## `FakeS3`

| Member | Description |
| --- | --- |
| `endpoint`, `host`, `port` | Where the server listens. `endpoint` is `http://host:port`, ready for `endpoint_url` |
| `access_key`, `secret_key`, `region` | Credentials of the default account, and `"us-east-1"` |
| `boto3_client(**overrides)`, `boto3_kwargs()` | A boto3 S3 client for this server (path-style), or the arguments to build one. Needs boto3 installed |
| `stop()`, `running`, `pid` | Stop the server; whether it is running; its process id |
| `reset(clock=True, requests=True, snapshots=False)` | Clear all buckets, objects and uploads, and by default the request log and clock. Snapshots are kept unless `snapshots=True` |
| `snapshot(name=None)`, `restore(name)`, `snapshots()`, `delete_snapshot(name)` | Save the current state and roll back to it, as often as you like. Object data is shared, not copied |
| `memory()`, `stats()`, `status()` | Object data held (`{"used", "limit"}`), data counts, and the server's configuration |
| `clock` | The server clock (below) |
| `requests` | The request log (below) |

A control call the server rejects raises `fakes3.FakeS3Error`, with the server's `code`
(`"NotFound"`, `"Conflict"`...) and HTTP `status`. After `stop()`, calls raise `server is stopped`.

### Virtual clock

Every timestamp the server produces reads this clock, and so does everything that depends on time:
presigned URL expiry, lifecycle rules, Object Lock retention and archive restores.

```python
from datetime import datetime, timedelta, timezone

server.clock.freeze(datetime(2030, 1, 2, tzinfo=timezone.utc))  # freeze() alone freezes at the current time
server.clock.advance("31d")              # also "1h30m", "-2w", a timedelta, or a number of seconds
server.clock.set(at)                     # jump, keeping the frozen or running state
server.clock.unfreeze()                  # let time run again
server.clock.reset()                     # back to the host clock
server.clock.now()                       # aware datetime in UTC; also is_frozen(), is_shifted()
```

Naive datetimes are taken as UTC. Clients sign requests with the host's time, so move the server
clock forward, not back: a presigned URL that the host clock considers expired stays expired.

### Request log

Assert on what your code actually sent:

```python
server.requests.list(operation="PutObject", status="error", limit=10)
server.requests.count(bucket="demo")        # matches, ignoring limit
server.requests.clear()
server.requests.configure(enabled=True, capacity=50_000)
```

Records are dicts, in arrival order:
`{seq, time, method, path, bucket, key, operation, status, latency_us, bytes_in, bytes_out, access_key, request_id}`.
Filters: `bucket`, `key_prefix`, `operation`, `method`, `status` (`404`, `"4xx"` or `"error"`), `since`,
`after` (a `seq`) and `limit` (keep only the most recent matches).

Control calls are not synchronized with in-flight requests, so make them between tests, while no
requests are pending.

## Command line

The package also installs the `fakes3` command, the same binary as the release archives:

```bash
fakes3 --port 0          # prints the endpoint once it is listening
fakes3 ctl --endpoint http://127.0.0.1:9000 clock advance 1d
```

## Platforms

| Wheel | Platform |
| --- | --- |
| `manylinux_2_17_x86_64`, `musllinux_1_1_x86_64` | Linux x86_64 (static binary: any glibc or musl, Alpine included) |
| `manylinux_2_17_aarch64`, `musllinux_1_1_aarch64` | Linux aarch64 (static) |
| `macosx_11_0_arm64` | macOS on Apple silicon |
| `macosx_10_12_x86_64` | macOS on Intel |
| `win_amd64` | Windows x64 |

Python 3.9 or later. On other platforms, set `FAKES3_BINARY` to a fakes3 binary you built
(`cargo install fakes3`).

## Security

fakes3 is for tests. Signatures aren't verified by default and the control plane has no
authentication, so don't expose the server to an untrusted network or store real data in it. It
listens on `127.0.0.1` unless you pass `host`.

## Building from source

```bash
cd bindings/python
pip install .                         # runs cargo build --release; needs Rust, perl and make
pip install pytest boto3 && pytest
```

`python build_backend.py --binary path/to/fakes3 --platform <tag>` wraps an existing binary in a wheel.

## License

MIT
