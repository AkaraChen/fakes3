# fakes3

[![ci](https://github.com/AkaraChen/fakes3/actions/workflows/ci.yml/badge.svg)](https://github.com/AkaraChen/fakes3/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/fakes3-sdk)](https://www.npmjs.com/package/fakes3-sdk)
[![crates.io](https://img.shields.io/crates/v/fakes3)](https://crates.io/crates/fakes3)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

English | [简体中文](README.zh-CN.md)

**An in-memory S3 server for tests.** It is ready in milliseconds, answers a request in about 150 µs,
and behaves like AWS S3 wherever your client can tell the difference.

fakes3 drops what tests never need (persistence, fsync, clustering, encryption at rest) and spends the
savings on speed. What a client can observe follows AWS closely: signatures, error codes, ETag rules,
versioning, conditional requests, checksums, ACLs and bucket policies. It is written in Rust on top of
[s3s](https://github.com/Nugine/s3s).

```bash
docker run --rm -p 9000:9000 ghcr.io/akarachen/fakes3
```

## Why fakes3

- **Fast enough to start one per test.** A container is ready about 1 ms after it starts and idles at
  9 MiB. A 6,500-request test suite runs in 0.93 s, against 3.4 s on MinIO and 7.7 s on LocalStack.
- **Behaves like AWS.** It passes 99.1% of a 24-SDK compatibility matrix and 85.8% of the
  [ceph/s3-tests](https://github.com/ceph/s3-tests) cases that AWS itself passes. LocalStack passes
  59.1% of those, MinIO 52.0%. It also passes all of [Apache OpenDAL](https://opendal.apache.org)'s S3
  behavior tests without switching off a single capability ([conformance/OPENDAL.md](conformance/OPENDAL.md)).
- **Built for tests.** Reset state between tests, snapshot and restore fixtures, move a virtual clock to
  expire presigned URLs or run lifecycle rules, and assert on a log of the requests your code made.
- **Runs where your tests run.** Use the Docker image (amd64 and arm64), the Node.js package in-process,
  the Rust library in-process, a static Linux binary, or the GitHub Action.

|                                       | fakes3      | MinIO   | LocalStack | moto    | S3Mock  |
| ------------------------------------- | ----------: | ------: | ---------: | ------: | ------: |
| Ready after container start           | **~1 ms**   | 243 ms  | 10.9 s     | 880 ms  | 3.8 s   |
| Idle memory                           | **9.2 MiB** | 51 MiB  | 156 MiB    | 69 MiB  | 233 MiB |
| 6,500-request test suite              | **0.93 s**  | 3.4 s   | 7.7 s      | 12.9 s  | 7.1 s   |
| SDK matrix (24 SDKs × 43 scenarios)   | **99.1%**   | 96.4%   | 91.4%      | 87.9%   | 85.4%   |
| ceph/s3-tests (684 AWS-passing cases) | **85.8%**   | 52.0%   | 59.1%      |         |         |

Measured on a 4-core GitHub-hosted runner. Methodology, throughput numbers and the full results are in
[docs/benchmarks.md](docs/benchmarks.md).

## Quick start

### Docker

```bash
docker run --rm -p 9000:9000 ghcr.io/akarachen/fakes3
```

The image is `ghcr.io/akarachen/fakes3` for `linux/amd64` and `linux/arm64`, tagged `latest`, `0.1.0`,
`0.1` and `sha-<commit>`. It is Alpine plus a single static binary (about 7 MB compressed), listens on
`0.0.0.0:9000`, and prints `http://0.0.0.0:9000` to stdout when it is ready.

Point any S3 client at it with path-style addressing. Any credentials work:

```python
import boto3
from botocore.config import Config

s3 = boto3.client(
    "s3",
    endpoint_url="http://127.0.0.1:9000",
    aws_access_key_id="fakes3",
    aws_secret_access_key="fakes3secret",
    region_name="us-east-1",
    config=Config(s3={"addressing_style": "path"}),
)
s3.create_bucket(Bucket="demo")
s3.put_object(Bucket="demo", Key="hello.txt", Body=b"hi")
```

### Node.js, in-process

[`fakes3-sdk`](https://www.npmjs.com/package/fakes3-sdk) runs the server inside your Node process.
There is no child process and no Docker:

```bash
npm install --save-dev fakes3-sdk
```

```ts
import { start } from "fakes3-sdk";
import { S3Client, CreateBucketCommand } from "@aws-sdk/client-s3";

const server = await start(); // 127.0.0.1, random free port
const s3 = new S3Client({
  endpoint: server.endpoint,
  region: "us-east-1",
  forcePathStyle: true,
  credentials: { accessKeyId: server.accessKey, secretAccessKey: server.secretKey },
});
await s3.send(new CreateBucketCommand({ Bucket: "demo" }));
// ...
await server.stop();
```

Prebuilt for Linux x64 and arm64, on both glibc and musl (Alpine), with Node.js 18 or later. Every option
and method is documented in the [package README](bindings/node/README.md).

### Rust, in-process

Add [`fakes3`](https://crates.io/crates/fakes3) as a dev-dependency. It builds on Linux only, and the build
compiles OpenSSL, so it needs `perl` and `make`:

```toml
[dev-dependencies]
fakes3 = "0.1"
```

```rust
let server = fakes3::Server::start(fakes3::Config::default()).await?;
let endpoint = server.endpoint(); // "http://127.0.0.1:<random port>"
// build an S3 client for `endpoint` with path-style addressing
server.shutdown().await;
```

For synchronous tests, `Server::start_blocking` runs the server on its own background runtime and stops it
when the handle is dropped. `Config::default()` binds a random port, so parallel tests never collide, and
each `Server` has its own data and clock. `shutdown()` returns once the port can be reused; connections
that are already open keep being served until the client closes them. See
[examples/aws_sdk.rs](examples/aws_sdk.rs) and [examples/blocking.rs](examples/blocking.rs).

### OpenDAL

[`fakes3-opendal`](bindings/opendal/README.md) starts an in-process server with a fresh bucket for
[OpenDAL](https://opendal.apache.org). To OpenDAL it is plain S3: take a ready `Operator`, or hand
your own code the fixture's S3 settings (a URI, configuration keys or an S3 builder) so it builds its
operator the way it does in production. Unlike OpenDAL's `memory` service, you get ETags, versions,
multipart uploads, copies, presigned URLs and S3's conditional writes:

```rust
let s3 = fakes3_opendal::Fixture::start().await?;
s3.operator().write("hello.txt", "hi").await?;
let op = opendal::Operator::from_uri(s3.uri())?; // "s3://test?endpoint=…", as production config
```

### Standalone binary

Every [release](https://github.com/AkaraChen/fakes3/releases) ships static Linux binaries for x86_64 and
aarch64, each with a SHA-256 checksum:

```bash
curl -L https://github.com/AkaraChen/fakes3/releases/latest/download/fakes3-x86_64-unknown-linux-musl.tar.gz | tar xz
./fakes3-x86_64-unknown-linux-musl/fakes3 --port 0   # 0 picks a free port; the address is printed to stdout
```

To build from source, run `cargo install fakes3 --locked`, or `cargo build --release` in a checkout. The build
compiles OpenSSL, so it needs `perl` and `make`.

## Testcontainers

No dedicated module is needed: use a generic container, expose port 9000, and wait for `http://` in the
logs. The container starts in milliseconds, so you can give each test class, or each test, its own.

```python
# Python: testcontainers-python
from testcontainers.core.container import DockerContainer
from testcontainers.core.waiting_utils import wait_for_logs

with DockerContainer("ghcr.io/akarachen/fakes3:latest").with_exposed_ports(9000) as c:
    wait_for_logs(c, "http://")
    endpoint = f"http://{c.get_container_host_ip()}:{c.get_exposed_port(9000)}"
```

```ts
// Node.js: testcontainers
import { GenericContainer, Wait } from "testcontainers";

const container = await new GenericContainer("ghcr.io/akarachen/fakes3:latest")
  .withExposedPorts(9000)
  .withWaitStrategy(Wait.forLogMessage("http://"))
  .start();
const endpoint = `http://${container.getHost()}:${container.getMappedPort(9000)}`;
```

```java
// Java: Testcontainers for Java
GenericContainer<?> s3 = new GenericContainer<>("ghcr.io/akarachen/fakes3:latest")
    .withExposedPorts(9000)
    .waitingFor(Wait.forLogMessage(".*http://.*\\n", 1));
s3.start();
String endpoint = "http://" + s3.getHost() + ":" + s3.getMappedPort(9000);
```

```go
// Go: testcontainers-go
c, err := testcontainers.GenericContainer(ctx, testcontainers.GenericContainerRequest{
    ContainerRequest: testcontainers.ContainerRequest{
        Image:        "ghcr.io/akarachen/fakes3:latest",
        ExposedPorts: []string{"9000/tcp"},
        WaitingFor:   wait.ForLog("http://"),
    },
    Started: true,
})
host, _ := c.Host(ctx)
port, _ := c.MappedPort(ctx, "9000")
endpoint := fmt.Sprintf("http://%s:%s", host, port.Port())
```

## GitHub Actions

[`setup-fakes3`](https://github.com/AkaraChen/setup-fakes3) installs fakes3 on a Linux runner, starts it
in the background, and puts `fakes3 ctl` on `PATH` for later steps. It needs no Docker, so it also works
in container jobs:

```yaml
- uses: AkaraChen/setup-fakes3@v1
  id: s3
  with:
    version: 0.1.0      # default: latest
- run: pytest
  env:
    S3_ENDPOINT: ${{ steps.s3.outputs.endpoint }}   # http://127.0.0.1:9000
```

Inputs mirror the [configuration](#configuration) options; `port: 0` picks a free port, and
`aws-env: true` exports `AWS_ENDPOINT_URL_S3` and credentials for the AWS CLI and SDKs. See the
[action's README](https://github.com/AkaraChen/setup-fakes3#readme) for every input and output.

The Docker image also works as a service container:

```yaml
services:
  s3:
    image: ghcr.io/akarachen/fakes3:0.1.0
    ports: ["9000:9000"]
```

## Test control plane

The same port also serves a small JSON API under `/_fakes3/`. It doesn't clash with S3, because bucket
names can't start with `_`. Use it to put the server in a known state and to inspect what your code did.
`fakes3 ctl` is its command-line client:

```bash
fakes3 ctl reset                              # drop all buckets, objects and uploads; clear the request log and clock
fakes3 ctl snapshot create seeded             # save the current state (object data is shared, not copied)
fakes3 ctl snapshot restore seeded            # roll back to it, as often as you like
fakes3 ctl clock advance 31d                  # move server time; presigned URLs, lifecycle and Object Lock follow it
fakes3 ctl requests list --op PutObject       # the request log: filter by bucket, operation, status, ...
```

In-process, the same operations are methods on the server: `server.reset()`, `server.snapshot("seeded")`,
`server.clock.advance("31d")` and `server.requests.list({ operation: "PutObject" })` in Node.js, and
equivalents on `fakes3::Server` in Rust. Endpoints and semantics are covered in
[docs/control-plane.md](docs/control-plane.md).

## Configuration

Every option can be given as a flag or an environment variable:

| Flag | Environment | Default | Description |
| --- | --- | --- | --- |
| `--host` | `FAKES3_HOST` | `127.0.0.1` (`0.0.0.0` in the image) | Address to listen on |
| `--port` | `FAKES3_PORT` | `9000` | Port to listen on; `0` picks a free one |
| `--access-key` / `--secret-key` | `FAKES3_ACCESS_KEY` / `FAKES3_SECRET_KEY` | `fakes3` / `fakes3secret` | Credentials of the default account, checked only with `--auth` |
| `--auth` | `FAKES3_AUTH` | off | Verify SigV4 / SigV2 signatures (see [Authentication](#authentication)) |
| `--user` | `FAKES3_USERS` | | Another account, `name:access-key:secret[:account-id[:canonical-id]]`; repeatable (comma-separated in the variable) |
| `--domain` | `FAKES3_DOMAIN` | `localhost` | Base domain for virtual-host addressing (`<bucket>.<domain>`); repeatable; `""` turns it off |
| `--bucket-defaults` | `FAKES3_BUCKET_DEFAULTS` | `legacy` | Settings new buckets start with: `legacy` or `aws` (see below) |
| `--memory-limit` | `FAKES3_MEMORY_LIMIT` | `auto` | Cap on object data: `512MiB`, `2GiB`, `unlimited`, or `auto` (75% of the container's or machine's memory) |
| `--inline-threshold` | | `1048576` | Objects at or above this size live in a `memfd` instead of on the heap |
| `--no-request-log` | | | Don't record requests for the control plane's request log |
| `--request-log-capacity` | | `10000` | Requests kept in the log; the oldest are dropped first |

### Authentication

By default fakes3 accepts any credentials, the way LocalStack and moto do, but authorization still
applies:

- A **signed** request is accepted without checking the signature. It acts as the account its access key
  belongs to, or as the default account (the owner of everything) if the key is unknown.
- An **unsigned** request is anonymous. ACLs, bucket policies and Block Public Access decide what it may
  do, as on AWS, so `curl` against a private bucket gets `403 AccessDenied`.
- With **`--auth`**, signatures are verified against the configured credentials, and a bad one is a 403.

Payload integrity is always checked, whichever mode is on: `Content-MD5`, `x-amz-checksum-*` and
aws-chunked trailers. Details are in [docs/auth.md](docs/auth.md).

### Multiple accounts

There is a single account by default. To test cross-account behaviour such as ACL grants, policy
principals, object ownership and `x-amz-expected-bucket-owner`, add more with `--user`:

```bash
fakes3 --user alt:ALTKEY:ALTSECRET --user carol:CAROLKEY:CAROLSECRET:111122223333
```

Account ids and canonical ids are derived deterministically, so tests can hard-code them. See
[docs/multi-user.md](docs/multi-user.md).

### New-bucket defaults

In 2023 AWS changed what new buckets start with. fakes3 keeps the older behaviour by default
(`legacy`), so setup steps like "attach a public policy, then read anonymously" work out of the box.
`--bucket-defaults aws` matches AWS today: ACLs disabled (`BucketOwnerEnforced`), Block Public Access
on, SSE-S3 default encryption, and SSE-C blocked. See [docs/bucket-defaults.md](docs/bucket-defaults.md).

### Memory limit

fakes3 keeps everything in memory, so it accounts for every byte of object data. A write that would
go past the limit fails with `507 Insufficient Storage` instead of the container being OOM-killed
halfway through a test. When the size is known up front, that happens before the body is read.
No mainstream SDK retries a 507. See [docs/memory-limit.md](docs/memory-limit.md).

## What's supported

- **Buckets**: create, delete, head, list (paginated), location, tagging
- **Objects**: PUT, GET, HEAD, DELETE, DeleteObjects, CopyObject (with metadata and tag replacement),
  range reads, tagging, GetObjectAttributes, and appends (`x-amz-write-offset-bytes`). AWS only allows
  appends in S3 Express One Zone directory buckets; fakes3 has no directory buckets and allows them in
  every bucket, with the same rules: the offset must be the current size, and each append is a part
- **Listing**: ListObjectsV2, ListObjects (v1), ListObjectVersions and ListMultipartUploads, with prefix,
  delimiter, pagination and `encoding-type=url`
- **Multipart uploads**: create, UploadPart, UploadPartCopy, ListParts, complete, abort, and
  `?partNumber=` on GET and HEAD
- **Versioning**: enabled and suspended, delete markers, null versions, per-version reads and deletes
- **Object Lock**: GOVERNANCE and COMPLIANCE retention, legal hold, default retention, governance bypass
- **Conditional requests**: `If-Match`, `If-None-Match` (including conditional writes: PUT,
  CopyObject and CompleteMultipartUpload), `If-Modified-Since`, `If-Unmodified-Since`, the
  copy-source variants, and conditional deletes (`If-Match` on DeleteObject, `<ETag>` in DeleteObjects)
- **Checksums**: CRC32, CRC32C, CRC64NVME, SHA-1 and SHA-256, in headers or aws-chunked trailers, with
  COMPOSITE and FULL_OBJECT multipart checksums. CRC64NVME is the default, as on AWS today.
- **Signatures**: SigV4 (headers, presigned URLs, streaming with trailers), SigV2, POST policy uploads
- **Access control**: bucket and object ACLs, Object Ownership, Block Public Access, bucket policies with
  conditions, anonymous access, multiple accounts
- **Encryption**: SSE-C key checks, SSE-S3 and SSE-KMS headers, bucket default encryption. This is
  behaviour only; data is not actually encrypted.
- **CORS**: preflight and response headers
- **Lifecycle**: expiration (including noncurrent versions and delete markers), abort of incomplete
  uploads, and storage-class transitions, all driven by the virtual clock
- **Archive storage classes**: `GLACIER`, `DEEP_ARCHIVE` and the Intelligent-Tiering archive tiers, with
  `InvalidObjectState` on reads, `RestoreObject` and `x-amz-restore` timed by the virtual clock (see
  [docs/archive-storage.md](docs/archive-storage.md))
- **S3 Select**: SQL over CSV and JSON input (see [docs/select.md](docs/select.md))
- **Request IDs**: `x-amz-request-id` and `x-amz-id-2` on every response, matching the request log

Not supported: website endpoints, event notification delivery, server access logging, IAM, STS and
S3 Control. Website and notification *configuration* can be stored and read back. Planned work is in
[TODO.md](TODO.md).

## Platforms

| | Platforms |
| --- | --- |
| Docker image | `linux/amd64`, `linux/arm64` (also runs under Docker Desktop on macOS and Windows) |
| Static binary | x86_64 and aarch64 Linux |
| Node.js package | Linux x64 and arm64, glibc and musl, Node.js ≥ 18 |
| Rust library | Linux |

Running natively on macOS or Windows isn't supported, because large objects are stored in Linux
`memfd`s. Use the Docker image there.

## Security

fakes3 is for tests. **Don't expose it to an untrusted network, and don't store real data in it.**

- Signatures aren't verified by default, so anyone who can reach the port can read and write. With
  `--auth`, the credentials are still plain-text test credentials.
- The control plane (`/_fakes3/*`) has no authentication: anyone who can reach it can wipe all data or
  read the request log. It does reject cross-origin writes from browsers.
- The binary listens on `127.0.0.1` by default, but **the Docker image listens on `0.0.0.0`**. On shared
  machines, publish the port to localhost only (`-p 127.0.0.1:9000:9000`) or keep it on an internal
  network.
- Nothing is persisted. Data is gone when the process exits.

## How it works

- **Metadata** lives in memory: one ordered map per bucket (key → version chain), so a listing is an
  ordered walk.
- **Object data** is immutable, reference-counted `Bytes` segments. Small objects sit on the heap; large
  ones are written to a `memfd` (tmpfs-backed and anonymous, freed when the process exits) and served
  through a read-only mmap.
- Because data is immutable, GET, CopyObject, UploadPartCopy and CompleteMultipartUpload **never copy
  object bytes**. They slice and splice segments and bump reference counts.
- [s3s](https://github.com/Nugine/s3s) handles the protocol (routing, XML, SigV4, aws-chunked). This
  repository implements the storage engine and S3 semantics.

## Documentation

Detailed documentation is in [docs/](docs/). Most of it is written in Chinese.

| | |
| --- | --- |
| [docs/control-plane.md](docs/control-plane.md) | Reset, snapshots, virtual clock, request log: HTTP API, CLI, Rust API |
| [docs/auth.md](docs/auth.md) | Authentication modes in detail |
| [docs/multi-user.md](docs/multi-user.md) | Accounts, ownership and authorization across accounts |
| [docs/bucket-defaults.md](docs/bucket-defaults.md) | `legacy` vs `aws` new-bucket settings, with AWS sources |
| [docs/memory-limit.md](docs/memory-limit.md) | How memory is accounted and capped |
| [docs/archive-storage.md](docs/archive-storage.md) | Glacier, Deep Archive, Intelligent-Tiering, restores, lifecycle transitions |
| [docs/select.md](docs/select.md) | S3 Select coverage |
| [docs/benchmarks.md](docs/benchmarks.md) | SDK matrix, ceph/s3-tests and performance results |
| [conformance/OPENDAL.md](conformance/OPENDAL.md) | Apache OpenDAL's behavior tests against fakes3 |
| [docs/s3s-workarounds.md](docs/s3s-workarounds.md) | Where fakes3 corrects s3s to match AWS |
| [docs/development.md](docs/development.md) | Building, testing, CI and releasing |

## Contributing

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs these on every push and pull request. The SDK compatibility matrix, ceph/s3-tests and the
benchmarks are run manually as GitHub Actions workflows; see [docs/development.md](docs/development.md).

## License

[MIT](LICENSE)
