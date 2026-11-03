# Apache OpenDAL behavior tests

[OpenDAL](https://opendal.apache.org)'s behavior tests are the suite every OpenDAL service has to pass. They are
generated from the operator's capabilities, so a server that runs them with no capability overrides supports
everything OpenDAL's S3 service can do. OpenDAL's own CI runs them against AWS S3, Silo (a MinIO fork),
SeaweedFS, Ceph RGW and Cloudflare R2, each with a list of capabilities switched off for what that server lacks.
This page is the measured baseline of fakes3 against them, with every failure triaged.

- **Pinned:** OpenDAL [`v0.59.5`](https://github.com/apache/opendal/tree/v0.59.5)
  (`fce9cdefdea7c8e54dfbaf3c88c3b6d5ec68146e`, 2026-10-15), the version `fakes3-opendal` builds on.
- **Measured:** fakes3 `v0.1.1` (`17f2890`), 2026-11-03, on a local machine.
- **Result:** all 12 failures are features fakes3 lacks. None of them points at a bug in OpenDAL.

## How to run

```bash
conformance/opendal/run.sh                 # all four configs; results in conformance/opendal/results/
conformance/opendal/run.sh versioned       # just one
OPENDAL_REF=main conformance/opendal/run.sh
```

The script fetches OpenDAL at the pinned tag into `conformance/opendal/.work/`, builds the `behavior` test binary
of the `opendal` crate with `--features tests,services-s3`, and for every config starts a fresh fakes3 with
`--auth` (so OpenDAL's SigV4 signing is verified), creates the bucket `test` and runs the binary with
`OPENDAL_TEST=s3`. It writes the libtest output to `<config>.log` and the failing test names to `<config>.failed`.
Knobs are at the top of the script. A run needs Rust, curl and python3; the first build takes a few minutes,
after that each config runs in about 5 seconds.

What the harness does on its own, which matters when reading results: every test runs under a fresh random
root (`OPENDAL_DISABLE_RANDOM_ROOT` is not set), and the operator is wrapped in OpenDAL's `RetryLayer` with up to
4 retries, so a transient error would be retried rather than reported.

## Results

| Config | Settings | Tests | Passed | Failed |
| --- | --- | ---: | ---: | ---: |
| `versioned` | versioning on, no capability overrides | 152 | 140 | 12 |
| `unversioned` | versioning off; the 9 version capabilities overridden off (OpenDAL's `aws_s3` setup, minus its `write_can_append` override) | 143 | 132 | 11 |
| `list-v1` | as `unversioned`, with ListObjects v1 (`disable_list_objects_v2`) | 143 | 132 | 11 |
| `virtual-host` | as `unversioned`, with virtual-host-style addressing (`test.localhost`) | 143 | 132 | 11 |

The three unversioned configs fail the same 11 tests. `versioned` fails those and
`test_restore_with_if_not_exists_conflict`, which only runs with the version capabilities on.

OpenDAL `main` at `26387047f5187fe7ea2fc6a0d8400c65e74dc0b3` (2026-11-03) has three more tests
(`test_batch_delete_with_version_keeps_newer_objects`, `test_delete_with_version_keeps_newer_object`,
`test_restore_not_found_with_matching_prefix`). fakes3 passes all three, and fails the same 12 / 11 as on
`v0.59.5`.

## Failures

| Test | Capability | Cause | Side |
| --- | --- | --- | --- |
| `test_write_with_append` | `write_can_append` | PutObject with `x-amz-write-offset-bytes` is not implemented: the header is ignored and the request overwrites the object with the appended chunk | fakes3 |
| `test_write_with_append_returns_metadata` | `write_can_append` | same | fakes3 |
| `test_writer_with_append` | `write_can_append` | same | fakes3 |
| `test_copy_with_if_not_exists_to_existing_file` | `copy_with_if_not_exists` | CopyObject ignores `If-None-Match: *` on the destination and overwrites it | fakes3 |
| `test_copier_with_if_not_exists_to_existing_file` | `copy_with_if_not_exists` | same | fakes3 |
| `test_copy_with_if_match_mismatch` | `copy_with_if_match` | CopyObject ignores `If-Match` on the destination | fakes3 |
| `test_copy_if_not_changed` | `copy_with_if_match` | same: the copy succeeds against a stale destination ETag | fakes3 |
| `test_restore_with_if_not_exists_conflict` | `restore_with_if_not_exists` | OpenDAL restores a version by copying it over the key with `If-None-Match: *`; same cause | fakes3 |
| `test_stat_with_override_cache_control` | `stat_with_override_cache_control` | HeadObject ignores the `response-cache-control` query parameter (GetObject applies it) | fakes3 |
| `test_stat_with_override_content_disposition` | `stat_with_override_content_disposition` | HeadObject ignores `response-content-disposition` | fakes3 |
| `test_stat_with_override_content_type` | `stat_with_override_content_type` | HeadObject ignores `response-content-type` | fakes3 |
| `test_batch_delete_with_if_match` | `delete_with_if_match` | DeleteObjects ignores the `<ETag>` of an `<Object>` entry; DeleteObject's `If-Match` is already supported | fakes3 |

All of these are in AWS's API model (the one s3s is generated from): `x-amz-write-offset-bytes` on PutObject,
`If-Match` / `If-None-Match` on CopyObject, the `response-*` parameters on HeadObject and `ETag` on DeleteObjects'
`ObjectIdentifier`. s3s parses every one of them; fakes3 does not act on them yet.

Append is the one AWS only offers in S3 Express One Zone directory buckets. fakes3 has no directory buckets, so
supporting it means accepting it on every bucket, which is still better than the current behaviour of silently
replacing the object.

## Capability overrides compared

To pass the suite today, fakes3 needs these overrides on a versioned bucket:
`write_can_append`, `copy_with_if_not_exists`, `copy_with_if_match`, `restore_with_if_not_exists`,
`delete_with_if_match`, `stat_with_override_cache_control`, `stat_with_override_content_disposition`,
`stat_with_override_content_type`. The S3-compatible targets in OpenDAL's CI at the same version:

| Target | Overrides |
| --- | ---: |
| AWS S3 with versioning (needs credentials) | 1 (`write_can_append`) |
| Silo with versioning | 5 |
| fakes3 with versioning | 8 |
| Silo | 13 |
| SeaweedFS | 14 |

Supporting append and the CopyObject conditions takes fakes3 to 4; the HeadObject overrides and conditional
DeleteObjects to 0, which no other S3-compatible target in that CI reaches, AWS included.
