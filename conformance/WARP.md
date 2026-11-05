# MinIO warp workloads

[warp](https://github.com/minio/warp) is MinIO's S3 benchmark, not a conformance suite. It is still useful here:
every workload is a real client (minio-go) driving one feature under concurrency, and one workload, `atomic`, is a
correctness checker. This page records which workloads run cleanly on fakes3. `bench.yml` uses warp to measure
speed; this uses it to find out what works.

- **Pinned:** warp `v1.8.2` (`WARP_VERSION` in `bench/setup-native.sh`), which builds on minio-go
  `v7.3.1-0.20260909183557-78bfa91607c2`.
- **Measured:** run [42017906855](https://github.com/AkaraChen/fakes3/actions/runs/42017906855), 2026-11-05, on
  `ubuntu-24.04`, 3 s per workload. References: fakes3 with `--bucket-defaults aws`, and MinIO
  `cgr.dev/chainguard/minio:latest` as of that date.
- **Result:** all 50 S3 cases pass. The first run had 46, see [History](#history).

These numbers are a different measure from [ceph/s3-tests](S3-TESTS.md): a case here is a whole feature passing
under load, not one assertion about one response, so 50/50 cannot be compared with its 85.8%.

## How to run

```bash
gh workflow run warp.yml --ref <branch> -f servers='fakes3 fakes3-aws minio'
```

Or on any machine with Docker and Go:

```bash
docker build -t fakes3:local .
bench/setup-native.sh warp
conformance/warp/run.py fakes3 minio                 # results in conformance/warp/results/
conformance/warp/run.py fakes3 --cases atomic,get    # just some cases
conformance/warp/report.py conformance/warp/results
```

Servers are whatever `bench/servers.sh list` knows, plus `fakes3-aws`. A run takes about 6 minutes per server.

What the harness does, which matters when reading results:

- **warp's exit code means nothing.** warp exits 0 when every request failed (`fanout`: 24809 requests, 24809
  errors, exit 0). A case passes only if warp's JSON report shows at least one request and no failed request, and
  no `<ERROR>` was printed, because failures during preparation and cleanup never reach the report.
- **Every case gets a fresh server.** warp reuses one bucket name, so a case could otherwise inherit a versioned or
  locked bucket from the previous one, and a crash would fail every later case.
- **fakes3 runs with `--auth`**, so warp's signatures are verified. With the wrong secret key fakes3 rejects the
  signature and the harness reports `fail`.
- **fakes3 listens on both stacks** (`FAKES3_BIND`), because on the runners `<bucket>.localhost` resolves to `::1`.
  Everything else that uses `bench/servers.sh` keeps `127.0.0.1`.
- **The switches do what their names say.** Checked on the wire: `put-sigv2` sends `Authorization: AWS ...`,
  `put-vhost` sends `Host: warp-benchmark-bucket.localhost`, and the `checksum-*` cases send
  `STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER` with `x-amz-trailer`.

## Results

| Server | S3 API | MinIO extensions |
| --- | ---: | ---: |
| fakes3 | 50 / 50 | 0 / 2 |
| fakes3 `--bucket-defaults aws` | 48 / 50 | 0 / 2 |
| MinIO | 38 / 50 | 1 / 2 |

The full table is in the job summary of each run. On fakes3 these all pass: PutObject (plain, multipart, POST form,
Content-MD5, unsigned payload, metadata and tags, storage class, SSE-S3, SSE-C, signature v2, virtual-host
addressing); trailing checksums with all ten algorithms warp offers; full-object and composite checksums on
multipart uploads; GetObject with ranges, versions and encryption; HeadObject; ListObjectsV2 with pagination and
metadata; ListObjectVersions; DeleteObject and DeleteObjects; mixed and versioned load; Object Lock retention;
GET `?partNumber=`; `x-amz-write-offset-bytes` appends; and `atomic`, which finds no torn or stale read, no missed
list-after-write and no ETag that isn't the MD5 of the body.

MinIO's score is not a fair comparison, and it is only here as a reference. Four of its failures come from how
`bench/servers.sh` starts it: no TLS, which SSE-C requires (`put-sse-c`, `get-sse-c`), and no KMS for SSE-S3
(`put-sse-s3`, `get-sse-s3`). `put-vhost` gets a 400, most likely because `MINIO_DOMAIN` isn't set. This build
doesn't know the newer checksum algorithms (`checksum-md5cs`, `-sha512`, `-xxh64`, `-xxh3`, `-xxh128`) or appends
(`append`), and `put-form` gets a 400 that was not looked into.

## What doesn't pass

### SSE-C with `--bucket-defaults aws`

`put-sse-c` and `get-sse-c` fail on purpose: with these defaults new buckets block SSE-C, as AWS's do
([docs/bucket-defaults.md](../docs/bucket-defaults.md)).

### MinIO extensions

`fanout` (POST with `x-minio-fanout-list`) and `zip` (reading inside a zip with `x-minio-extract`) have no AWS
equivalent and fakes3 doesn't implement them. They stay in the table to show what warp can't do here, and don't
count towards the S3 score. `fanout` fails on this MinIO build as well (`Request body cannot be empty.`).

## What this doesn't tell you

- warp checks that requests succeed and that sizes match. Only `atomic` looks at contents, and the objects in these
  cases are too small to be uploaded in parts. A server that returns the right number of wrong bytes passes `get`.
- It doesn't check error responses, ACLs, policies, lifecycle, CORS or response headers. s3-tests and the
  [SDK matrix](../matrix/SPEC.md) cover those.
- Not run: `snowball`, because a server that stores the tar unextracted passes (fakes3 does, and warp can't tell);
  `iceberg`, which needs a REST catalog and isn't S3; `--tls`, `--http2` and RDMA, which fakes3 doesn't serve.
- 3 s of 4 clients finds what always fails, not rare races. A pass from `atomic` isn't a proof.

## History

The first run, on `main` at `1e6f1b4`, scored 46 / 50. All four failures were composite checksums on multipart
uploads (`checksum-mpu-crc32`, `-crc32c`, `-sha1`, `-sha256`), on both bucket defaults, and MinIO passed them:

```text
The x-amz-checksum-crc32 you specified did not match the calculated checksum.
```

minio-go computes the composite checksum correctly but sends it on CompleteMultipartUpload without a part count:
`x-amz-checksum-crc32: <digest>`, with `x-amz-checksum-type: COMPOSITE`. fakes3 compared that as a string with its
own `<digest>-<parts>`, so the right digest was a `BadDigest`.

This reached real applications. A minio-go client with `TrailingHeaders: true` defaults to CRC32C and uses composite
checksums on multipart uploads, so a `PutObject` over 16 MiB failed on fakes3 without the application asking for a
checksum (reproduced with minio-go `v7.4.1`, the version the SDK matrix pins: 20 MiB failed, 16 MiB passed, and
`DisableMultipart` or a full-object checksum avoided it). The SDK matrix missed this because its checksum scenario
sets `DisableMultipart: true`, and the composite cases in s3-tests always send the suffix.

fakes3 now compares the digest when the part count is absent. A wrong digest or a wrong part count is still a
`BadDigest`, and the object keeps the value with the part count. s3-tests stays at 587 / 684
(run [42014335352](https://github.com/AkaraChen/fakes3/actions/runs/42014335352)).

This is a choice, not reproduced AWS behaviour: what AWS does with a value that has no suffix is **unverified**.
AWS's documentation gives the format of the returned value (`<digest>-3`) but not of the request header, and this
has not been tried against real S3. The other implementations are lenient: MinIO's parser makes the suffix optional
(it checks the part count only when one is present and always compares the digest), and LocalStack doesn't compare
composite values on Complete at all.
