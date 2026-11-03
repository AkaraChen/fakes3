# Apache OpenDAL behavior tests

[OpenDAL](https://opendal.apache.org)'s behavior tests are the suite every OpenDAL service has to pass. They are
generated from the operator's capabilities, so a server that runs them with no capability overrides supports
everything OpenDAL's S3 service can do. OpenDAL's own CI runs them against AWS S3, Silo (a MinIO fork),
SeaweedFS, Ceph RGW and Cloudflare R2, each with a list of capabilities switched off for what that server lacks.
This page records how fakes3 does against them.

- **Pinned:** OpenDAL [`v0.59.5`](https://github.com/apache/opendal/tree/v0.59.5)
  (`fce9cdefdea7c8e54dfbaf3c88c3b6d5ec68146e`, 2026-10-15), the version `fakes3-opendal` builds on.
- **Measured:** fakes3 `main` at `c3d2d4f`, 2026-11-03, on a local machine.
- **Result:** every test passes in every config, with no capability overrides. The first run, on `v0.1.1`, had
  12 failures, all of them features fakes3 lacked and none a bug in OpenDAL; see [History](#history).

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
| `versioned` | versioning on, no capability overrides | 152 | 152 | 0 |
| `unversioned` | versioning off; the 9 version capabilities overridden off (OpenDAL's `aws_s3` setup, minus its `write_can_append` override) | 143 | 143 | 0 |
| `list-v1` | as `unversioned`, with ListObjects v1 (`disable_list_objects_v2`) | 143 | 143 | 0 |
| `virtual-host` | as `unversioned`, with virtual-host-style addressing (`test.localhost`) | 143 | 143 | 0 |

OpenDAL `main` at `26387047f5187fe7ea2fc6a0d8400c65e74dc0b3` (2026-11-03) has three more tests
(`test_batch_delete_with_version_keeps_newer_objects`, `test_delete_with_version_keeps_newer_object`,
`test_restore_not_found_with_matching_prefix`) and passes in full as well: 155 / 155 on `versioned`, 145 / 145 on
the other three.

One of these features departs from AWS on purpose: AWS only accepts appends (`x-amz-write-offset-bytes`) in S3
Express One Zone directory buckets. fakes3 has no directory buckets and accepts them in every bucket, with AWS's
rules (the offset must be the current size, each append is a part). It is written from AWS's documentation and
has not been checked against S3 Express itself.

## Capability overrides compared

The S3-compatible targets in OpenDAL's CI at `v0.59.5`, by the capabilities each has to switch off to pass:

| Target | Overrides | Needs credentials |
| --- | ---: | --- |
| fakes3 with versioning | 0 | no |
| AWS S3 with versioning | 1 (`write_can_append`) | yes |
| Silo with versioning | 5 | no |
| Silo | 13 | no |
| SeaweedFS | 14 | no |

OpenDAL's CI drops every setup that needs credentials from pull requests opened from forks, so there the
capabilities Silo and SeaweedFS switch off (append, conditional copy, conditional delete, conditional restore) are
not exercised at all.

## History

The first run, on fakes3 `v0.1.1` (`17f2890`), passed 140 / 152 on `versioned` and 132 / 143 on the other three.
All 12 failures were features fakes3 lacked, all of them in AWS's API model and parsed by s3s:

| Tests | Capability | What was missing | Fixed in |
| --- | --- | --- | --- |
| `test_write_with_append`, `test_write_with_append_returns_metadata`, `test_writer_with_append` | `write_can_append` | PutObject with `x-amz-write-offset-bytes`: the header was ignored and the request replaced the object with the appended chunk | [#7](https://github.com/AkaraChen/fakes3/pull/7) |
| `test_copy_with_if_not_exists_to_existing_file`, `test_copier_with_if_not_exists_to_existing_file` | `copy_with_if_not_exists` | `If-None-Match: *` on the CopyObject destination | [#7](https://github.com/AkaraChen/fakes3/pull/7) |
| `test_copy_with_if_match_mismatch`, `test_copy_if_not_changed` | `copy_with_if_match` | `If-Match` on the CopyObject destination | [#7](https://github.com/AkaraChen/fakes3/pull/7) |
| `test_restore_with_if_not_exists_conflict` | `restore_with_if_not_exists` | the same: OpenDAL restores a version by copying it over the key with `If-None-Match: *` | [#7](https://github.com/AkaraChen/fakes3/pull/7) |
| `test_stat_with_override_cache_control`, `_content_disposition`, `_content_type` | `stat_with_override_*` | the `response-*` query parameters on HeadObject (GetObject applied them) | [#8](https://github.com/AkaraChen/fakes3/pull/8) |
| `test_batch_delete_with_if_match` | `delete_with_if_match` | the `<ETag>` of an `<Object>` entry in DeleteObjects (DeleteObject's `If-Match` worked) | [#8](https://github.com/AkaraChen/fakes3/pull/8) |

At that point fakes3 needed 8 overrides, more than Silo with versioning.
