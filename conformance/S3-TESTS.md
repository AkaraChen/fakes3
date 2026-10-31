# ceph/s3-tests baseline

[ceph/s3-tests](https://github.com/ceph/s3-tests) is the S3 conformance suite used by Ceph RGW; about a thousand pytest cases written against real AWS behaviour (and RGW's). This page is the measured baseline of fakes3 against it, with every failure categorised, and the backlog that follows. It complements the 43 scenarios of the SDK matrix ([../matrix/SPEC.md](../matrix/SPEC.md)).

- **Pinned commit:** [`5b3987116eca0b4b8fa57d6b10f3db57bab196b6`](https://github.com/ceph/s3-tests/commit/5b3987116eca0b4b8fa57d6b10f3db57bab196b6) (master, 2026-10-12), 1053 collected test cases: `test_s3.py` 838 (760 functions, parametrized cases counted separately), `test_headers.py` 48, `test_iam.py` 90, `test_sts.py` 36, `test_s3select.py` 34, `test_sns.py` 5, `test_s3control.py` 1, `test_utils.py` 1.
- **Measured:** fakes3 on branch `fix/s3-tests-cheap` (the "cheap fixes" below, merged with `main` after `--auth` became opt-in; the harness starts fakes3 with `--auth`), run [41206032373](https://github.com/AkaraChen/fakes3/actions/runs/41206032373); `main` at the same point, run [41206038045](https://github.com/AkaraChen/fakes3/actions/runs/41206038045), reproduces the earlier baseline exactly (475/816, run [41198487135](https://github.com/AkaraChen/fakes3/actions/runs/41198487135)). MinIO (`cgr.dev/chainguard/minio:latest`) and LocalStack 4.14 are references from that earlier run, run the same way. The numbers will move; the CI run is the source of truth, this file is a snapshot.
- **Baseline definition:** the headline numbers exclude the 237 tests s3-tests itself marks `fails_on_aws` (they pass on RGW and fail on AWS, so they say nothing about AWS compatibility); they are still run and listed separately. `fails_on_rgw` / `fails_on_dbstore` / `fails_on_dho` tests are AWS-valid and stay in.

## How to run

CI (preferred, the suite is chatty and the local machine is shared):

```bash
gh workflow run s3-tests.yml --ref <branch> \
  -f servers=fakes3,fakes3-aws,minio,localstack \
  -f marker='not fails_on_aws' -f keyword='' -f paths=s3tests/functional -f workers=4
```

`servers` accepts anything `bench/servers.sh list` knows plus `fakes3-aws` (fakes3 with `--bucket-defaults aws`); `marker` is a pytest `-m` expression and `keyword` a `-k` expression; leave them empty to run everything. One job per server runs in parallel, a final job renders this report into the job summary and uploads `s3-tests-<server>` artifacts (JUnit XML, per-test JSONL with markers and the failing assertion line, the server log, the generated `s3tests.conf`).
(Until the workflow file is on `main`, `gh workflow run` cannot find it; the branch had a temporary `push` trigger, the same trick `node.yml` used.)

Single server, any machine with Docker and [uv](https://docs.astral.sh/uv/):

```bash
docker build -t fakes3:local .
conformance/s3-tests/run.sh fakes3            # results in conformance/s3-tests/results/fakes3/
python3 conformance/s3-tests/report.py conformance/s3-tests/results --src conformance/s3-tests/.work/s3-tests
```

`run.sh` pins the commit (`S3TESTS_REF`), checks it out under `.work/`, builds a Python 3.12 venv with `boto3==botocore==1.43.123` (`constraints.txt`) plus pytest-xdist and pytest-timeout, writes `s3tests.conf`, starts the server with `bench/servers.sh start`, runs pytest and stops the server. Knobs are documented at the top of the script (`S3T_WORKERS`, `S3T_MARKER`, `S3T_KEYWORD`, `S3T_PATHS`, `S3T_TIMEOUT`, …). `report.py` + `triage.json` produce everything below.

**Runtime.** pytest takes 2m04s for fakes3 with 4 xdist workers (MinIO 3m45s, LocalStack 3m50s) and 5m12s serially (run [41196487837](https://github.com/AkaraChen/fakes3/actions/runs/41196487837)); the Docker image build adds about a minute with a warm cache and about six cold. The serial and the parallel runs agree on the outcome of all 1053 tests, as did two further parallel runs, so xdist is on by default. Lifecycle tests would sleep up to 20 × `lc_debug_interval`; the runner sets the interval to 1 s (they fail for other reasons on every server anyway: nothing here accelerates "days").

### Harness decisions you should know about

- **One credential.** The servers know only `fakes3` / `fakes3secret`. The `alt`, `tenant`, `iam`, `iam root` and `iam alt root` users of `s3tests.conf` all use it (a bogus credential would turn every test into a setup error, because the suite's per-test cleanup logs in as alt and tenant). Consequence: the "second user" is the bucket owner, so tests that expect `AccessDenied` for it or an owner mismatch cannot pass. They are found by static analysis of the s3-tests call graph (any test that reaches `get_alt_*`, `get_tenant_*`, `get_iam_*`, …) and filed under *needs several users* instead of being hidden or hacked around. The main user's id and display name are `fakes3`, which is what fakes3 reports as owner. IAM-account tests skip themselves because `GetUser` does not work (70 skips).
- **Lenient cleanup.** Upstream's autouse fixture raises when it cannot delete a leftover bucket, and then every later test of that worker errors in setup; on MinIO one bucket stuck behind an object lock turned 272 tests into errors, on LocalStack `DeleteObjects` with `x-amz-bypass-governance-retention` on a bucket without Object Lock (accepted by AWS) broke all 980. `s3t_plugin.py` makes the cleanup non-fatal (it retries without the bypass header, and records what it could not delete in `cleanup-errors.*.jsonl`). Nothing a test asserts changes. fakes3 had 0 cleanup errors; MinIO 1290, LocalStack 13, so MinIO's numbers are the least reliable reference.
- **boto3 pinned** (1.43.123); upstream floats. A newer boto3 changes default checksums and signing details and moves results.
- **`api_name` is unset**, so `test_bucket_get_location` (compares with RGW's zonegroup name) skips.
- **Not run:** `[s3 cloud]`, storage-class transitions, web identity (needs Keycloak): skipped or failing as listed in the skipped table.

## Results

### Tests AWS passes (`not fails_on_aws`)

| server | pass | fail | error | skip | total | pass rate | pass rate excl. skips | runtime |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| fakes3 | 513 | 232 | 0 | 71 | 816 | 62.9% | 68.9% | 2m18s |
| fakes3-aws | 417 | 328 | 0 | 71 | 816 | 51.1% | 56.0% | 2m45s |
| localstack | 404 | 341 | 0 | 71 | 816 | 49.5% | 54.2% | 3m50s |
| minio | 358 | 387 | 0 | 71 | 816 | 43.9% | 48.1% | 3m45s |

### All tests

| server | pass | fail | error | skip | total | pass rate | pass rate excl. skips | runtime |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| fakes3 | 569 | 321 | 0 | 163 | 1053 | 54.0% | 63.9% | 2m18s |
| fakes3-aws | 457 | 433 | 0 | 163 | 1053 | 43.4% | 51.3% | 2m45s |
| localstack | 450 | 440 | 0 | 163 | 1053 | 42.7% | 50.6% | 3m50s |
| minio | 386 | 504 | 0 | 163 | 1053 | 36.7% | 43.4% | 3m45s |

## By file (AWS-applicable tests)

| file | fakes3 pass/total | fakes3-aws pass/total | localstack pass/total | minio pass/total |
|---|--:|--:|--:|--:|
| test_headers.py | 23/48 | 22/48 | 26/48 | 23/48 |
| test_iam.py | 0/88 | 0/88 | 0/88 | 2/88 |
| test_s3.py | 467/603 | 372/603 | 355/603 | 313/603 |
| test_s3control.py | 0/1 | 0/1 | 0/1 | 0/1 |
| test_s3select.py | 22/34 | 22/34 | 22/34 | 19/34 |
| test_sns.py | 0/5 | 0/5 | 0/5 | 0/5 |
| test_sts.py | 0/36 | 0/36 | 0/36 | 0/36 |
| test_utils.py | 1/1 | 1/1 | 1/1 | 1/1 |

## fakes3: failures by category

| category | failing tests | groups |
|---|--:|--:|
| Real fakes3 bug (wrong status / code / header / body vs AWS) | 22 | 10 |
| Unimplemented feature | 20 | 5 |
| Needs several users / tenants / IAM / STS (the servers know one credential) | 99 | 3 |
| s3s limitation (verified in the s3s source) | 25 | 5 |
| Test is not AWS-compatible / RGW-specific / ineffective with boto3 (not marked by s3-tests) | 66 | 14 |
| Marked `fails_on_aws` by s3-tests itself (excluded from the baseline) | 89 | 1 |

### Real fakes3 bug (wrong status / code / header / body vs AWS): 22 tests

#### GetBucketPolicyStatus returns NoSuchBucketPolicy for buckets without a policy (5)

The five `*_bucket_policy_status` tests compute `IsPublic` from ACL / policy; fakes3 returns 404 `NoSuchBucketPolicy` whenever no policy document exists, so public-ACL buckets cannot be reported. MinIO passes two of the five. Cost: **medium**.

Tests: `test_get_authpublic_acl_bucket_policy_status`, `test_get_bucket_policy_status`, `test_get_nonpublicpolicy_acl_bucket_policy_status`, `test_get_public_acl_bucket_policy_status`, `test_get_publicpolicy_acl_bucket_policy_status`

#### ListObjects with delimiter + prefix + max-keys reports the wrong IsTruncated / paging (4)

`assert response['IsTruncated'] == is_truncated` (`assert True == False`) in the paged walk over keys with `_` / common prefixes. The v2 variants pass on MinIO and LocalStack. Cost: **medium**.

Tests: `test_bucket_list_delimiter_prefix`, `test_bucket_list_delimiter_prefix_underscore`, `test_bucket_listv2_delimiter_prefix`, `test_bucket_listv2_delimiter_prefix_underscore`

#### POST policy uploads: invalid expiration format accepted, redirect Location not URL-encoded, `tagging` form field ignored, checksum fields rejected (4)

`test_post_object_invalid_date_format` gets 204 instead of 400; `success_action_redirect` Location ends in `etag="…"` while AWS encodes the quotes as `%22`; the `tagging` form field is not applied (`assert [] == [{'Key': '0', …}]`); `x-amz-checksum-*` form fields give 403 instead of 204. Cost: **medium**.

Tests: `test_post_object_invalid_date_format`, `test_post_object_success_redirect_action`, `test_post_object_tags_anonymous_request`, `test_post_object_upload_checksum`

#### SSE-C: CompleteMultipartUpload without the customer key succeeds, the test expects 400 (3)

After the UploadPart check (fixed: a part with another key is now 400 `InvalidRequest`) the three `test_copy_part_enc[…-sse-c-…]` cases still stop at their last step: `CompleteMultipartUpload` on an SSE-C upload without `x-amz-server-side-encryption-customer-*` headers must fail with 400. The CompleteMultipartUpload reference says those headers are 'required only when the object was created using a checksum algorithm', and fakes3 (like S3 today) gives every object one by default, but whether S3 refuses the call when an SDK leaves them out is unconfirmed, and enforcing it would break clients that do not repeat the key on Complete. Not changed. Cost: **unconfirmed**.

Tests: `test_copy_part_enc[sse-c-sse-c-STANDARD-STANDARD-8388608]`, `test_copy_part_enc[sse-kms-sse-c-STANDARD-STANDARD-8388608]`, `test_copy_part_enc[sse-s3-sse-c-STANDARD-STANDARD-8388608]`

#### SigV2 PutObject with a truncated Content-Length answers 403 instead of 400 BadDigest (1)

`assert 403 == 400`; root cause not isolated (probably the same signature path as the SigV2 group, but PutObject paths sign correctly, so it may be a fakes3 body-length check).

Tests: `test_object_create_bad_contentlength_mismatch_below_aws2`

#### ListObjects `Prefix` echo of a control character is URL-encoded in the wrong place (1)

Prefix `\n` comes back as `%0A` after boto3 decoding (`assert '%0A' == '\n'`), i.e. the value is encoded although the request did not ask for `encoding-type=url`, or encoded twice. Fails on MinIO and LocalStack too, so the exact AWS behaviour is unconfirmed. Cost: **cheap**.

Tests: `test_bucket_list_prefix_unreadable`

#### CORS: wildcard origins (`*.suffix`) do not match (1)

`access-control-allow-origin` missing for `Origin: foo.suffix` against a rule with `AllowedOrigins: ['*.suffix']`. AWS allows a single `*` anywhere in the origin. Cost: **medium**.

Tests: `test_cors_origin_response`

#### HEAD of a deleted key in an unversioned bucket has no `x-amz-delete-marker: false` (1)

`KeyError: 'x-amz-delete-marker'` on the 404. The test expects the header; whether AWS sends it for unversioned buckets is unconfirmed (MinIO and LocalStack fail too). Cost: **cheap**.

Tests: `test_delete_marker_nonversioned`

#### Content-Encoding list re-serialised without the space (`deflate,gzip`) (1)

Stored as `deflate, gzip` with `aws-chunked`; fakes3 returns `deflate,gzip`. AWS returns the header the client sent minus `aws-chunked`. Unconfirmed against AWS; fails on MinIO and LocalStack too. Cost: **cheap**.

Tests: `test_object_content_encoding_aws_chunked`

#### GET of a key with C1 control characters (valid UTF-8) returns 404, the test expects 400 (1)

`Key='\xae\x8a-'` is sent as the valid UTF-8 `%C2%AE%C2%8A-`; the test (`fails_on_rgw`, so AWS-valid) wants 400 `Couldn't parse the specified URI.` A key that is not UTF-8 after percent-decoding (`%AE%8A-`) already gets exactly that from s3s (400 `InvalidURI`), so only the C1-control case is open and the rule AWS applies is not documented. Not changed. Cost: **unconfirmed**.

Tests: `test_object_read_unreadable`

### Unimplemented feature: 20 tests

#### s3select (12)

S3 Select: `SelectObjectContent` is implemented only in part (NotImplemented for several query shapes); the remaining shapes below fail on the engine's output.

- 8 × `ClientError NotImplemented on SelectObjectContent` — `test_column_sum_min_max`, `test_count_json_operation`, `test_count_operation`, … (8)
- 1 × `assert -N >= N` — `test_alias_cyclic_refernce`
- 1 × `AssertionError: assert 'An error occ...plemented yet' == 'abNcd$$'` — `test_lowerupper_expressions`
- 1 × `ValueError: too many values to unpack (expected N)` — `test_progress_expressions`
- 1 × `AssertionError: assert -N >= N` — `test_schema_definition`

#### PutBucketLogging / bucket logging (NotImplemented) (5)

`PutBucketLogging is not implemented yet` (501). 5 AWS-applicable tests here; a further 31+ `bucket_logging` tests are marked `fails_on_aws` (they test RGW's journal/flush extension) and fail the same way. Cost: **large**.

Tests: `test_put_bucket_logging`, `test_put_bucket_logging_errors`, `test_put_bucket_logging_permissions`, `test_put_bucket_logging_policy_wildcard`, `test_rm_bucket_logging`

#### GetObjectTorrent (`?torrent`) (1)

`assert 501 == 404`: fakes3 answers NotImplemented; AWS answers 404 NoSuchKey for a missing key and has deprecated the API for new buckets.

Tests: `test_get_object_torrent`

#### ListBuckets `MaxBuckets` / `ContinuationToken` (1)

`list_buckets(MaxBuckets=1)` returns every bucket in the account (`assert 5 == 0` on the first call). The test also assumes an empty account, but the naming tests leave buckets such as `foo-.bar` behind (their names do not carry the test prefix, so the suite's cleanup skips them); it fails the same way in a serial run. Cost: **medium**.

Tests: `test_list_buckets_paginated`

#### s3control (1)

S3 Control API (account-level public access block) is served from a different endpoint than S3.

- 1 × `botocore.exceptions.EndpointConnectionError: Could not connect to the endpoint URL: "http://RGWN.N.N.N.N:N/vN/configuration/publicAccessBloc` — `test_account_public_access_block`

### Needs several users / tenants / IAM / STS (the servers know one credential): 99 tests

#### multi user (36)

The test's call graph reaches `get_alt_*`, `get_tenant_*` or `get_iam_*` (found by static analysis of the s3-tests source). The runner maps every identity to the one credential the servers know, so a second user is the bucket owner and the expected `AccessDenied` / owner mismatch cannot happen. Individual failures were not inspected one by one: a real bug can hide behind the missing second identity.

- 26 × `AssertionError: ClientError not raised` — `test_access_bucket_private_object_private`, `test_access_bucket_private_object_publicread`, `test_access_bucket_private_object_publicreadwrite`, … (26)
- 4 × `AssertionError: assert ('fakesN', 'fakesN') != ('fakesN', 'fakesN')` — `test_create_bucket_bucket_owner_preferred`, `test_create_bucket_object_writer`, `test_put_bucket_ownership_bucket_owner_preferred`, … (4)
- 2 × `ClientError InvalidBucketName on ListObjects` — `test_bucket_policy_different_tenant`, `test_bucket_policy_tenanted_bucket`
- 2 × `KeyError: 'ID'` — `test_object_acl_canned_bucketownerfullcontrol`, `test_object_acl_canned_bucketownerread`
- 1 × `AssertionError: assert N == N` — `test_head_object_404_with_policy_prefix`
- 1 × `AssertionError: assert 'fakesN' == 'NabcdefN...NabcdefN'` — `test_list_multipart_upload_owner`

#### sts (36)

STS (`AssumeRole`, `GetSessionToken`, `GetCallerIdentity`, web identity) needs an STS service and, for web identity, an external Keycloak configured in `[webidentity]`.

- 27 × `RuntimeError: Your config file is missing the "webidentity" section!` — `test_assume_role_with_web_identity`, `test_assume_role_with_web_identity_for_all_values`, `test_assume_role_with_web_identity_for_all_values_deny`, … (27)
- 5 × `AssertionError: NotImplemented` — `test_assume_role_allow`, `test_assume_role_allow_head_nonexistent`, `test_assume_role_creds_expiry`, … (5)
- 2 × `ClientError Unknown on CreateUser` — `test_get_caller_identity_after_assume_role`, `test_get_caller_identity_non_root_user`
- 1 × `ClientError Unknown on GetCallerIdentity` — `test_get_caller_identity_root`
- 1 × `TypeError: 'NoneType' object is not subscriptable` — `test_get_session_token`

#### iam (27)

IAM API calls (`PutUserPolicy`, `CreateUser`, …) go to a server that has no IAM service.

- 23 × `ClientError Unknown on PutUserPolicy` — `test_allow_bucket_actions_in_user_policy`, `test_allow_multipart_actions_in_user_policy`, `test_allow_object_actions_in_user_policy`, … (23)
- 4 × `assert N == N` — `test_list_user_policy_invalid_user`, `test_put_user_policy_invalid_element`, `test_put_user_policy_invalid_user`, … (4)

### s3s limitation (verified in the s3s source): 25 tests

#### SigV2: bad `x-amz-date` / `Date` is 400 InvalidRequest, AWS says 403 (10)

s3s `v2_check_header_auth` parses `x-amz-date` with the SigV4 ISO-8601 parser (`invalid x-amz-date`) and `Date` as HTTP-date (`invalid date`, `missing date`), all mapped to 400 `InvalidRequest`; AWS answers 403 `AccessDenied` / `RequestTimeTooSkewed` and accepts RFC 1123 in `x-amz-date` for SigV2. Cost: **large**.

Tests: `test_bucket_create_bad_date_after_today_aws2`, `test_bucket_create_bad_date_before_epoch_aws2`, `test_bucket_create_bad_date_before_today_aws2`, `test_bucket_create_bad_date_empty_aws2`, `test_bucket_create_bad_date_invalid_aws2`, `test_object_create_bad_date_after_end_aws2`, `test_object_create_bad_date_before_epoch_aws2`, `test_object_create_bad_date_before_today_aws2`, `test_object_create_bad_date_empty_aws2`, `test_object_create_bad_date_invalid_aws2`

#### SigV2: bucket-level requests signed by botocore fail with SignatureDoesNotMatch (8)

botocore signs the CanonicalizedResource of a path-style bucket request as `/bucket/` (trailing slash) although the request line is `/bucket`; verified with a stub server and boto3 1.43.123 (`CreateBucket`, `ListObjects`, `HeadBucket` sign `/bkt/`, `PutObject` signs `/bkt/k`). s3s builds the StringToSign from `req_uri.path()` verbatim (`s3s-sigv2` `create_string_to_sign`, called from `ops/signature.rs` `v2_check_header_auth`), so the signatures differ. Every SigV2 bucket-level call from boto3 is affected, not only these tests. Cost: **medium**.

Tests: `test_bucket_create_bad_authorization_invalid_aws2`, `test_bucket_create_bad_date_none_aws2`, `test_bucket_create_bad_ua_empty_aws2`, `test_bucket_create_bad_ua_none_aws2`, `test_cors_presigned_get_object_tenant_v2`, `test_cors_presigned_get_object_v2`, `test_cors_presigned_put_object_tenant_v2`, `test_cors_presigned_put_object_v2`

#### POST policy violations are 400 InvalidPolicyDocument, AWS says 403 AccessDenied (4)

Condition checks live in s3s `post_policy.rs` (`InvalidPolicyDocument`); AWS answers 403 `AccessDenied: Invalid according to Policy`. MinIO passes all four. Cost: **large**. Also passes on: minio.

Tests: `test_post_object_invalid_request_field_value`, `test_post_object_missing_policy_condition`, `test_post_object_request_missing_policy_specified_field`, `test_post_object_wrong_bucket`

#### SigV4: RFC 1123 `X-Amz-Date` together with `Date` is reported as `missing header: x-amz-date` (2)

`extract_amz_date` in s3s `ops/signature.rs:750` raises `InvalidRequest: missing header: x-amz-date` when the header is not in the compact ISO-8601 form; LocalStack accepts the request. Cost: **large**. Also passes on: localstack.

Tests: `test_object_create_amz_date_and_no_date`, `test_object_create_date_and_amz_date`

#### PutObject with `Transfer-Encoding: chunked` (no Content-Length) is rejected (1)

`MissingContentLength: missing header: content-length`, raised in s3s `ops/signature.rs` (lines 709/875) before fakes3 sees the request. LocalStack accepts it. Cost: **large**. Also passes on: localstack.

Tests: `test_object_write_with_chunked_transfer_encoding`

### Test is not AWS-compatible / RGW-specific / ineffective with boto3 (not marked by s3-tests): 66 tests

#### SSE-KMS tests expect the key id echoed verbatim; AWS (and fakes3) return the key ARN (22)

`assert 'arn:aws:kms:us-east-1:000000000000:key/testkey-1' == 'testkey-1'` for `x-amz-server-side-encryption-aws-kms-key-id`. AWS always returns the full key ARN; RGW echoes what was sent. The `test_copy_enc` / `test_copy_part_enc` cases with an SSE-KMS destination fail on the same comparison (`assert False` inside the `assert` lambda). Fails on MinIO and LocalStack too.

Tests: `test_bucket_policy_put_obj_kms_noenc`, `test_copy_enc[sse-c-sse-kms-STANDARD-STANDARD-1024]`, `test_copy_enc[sse-c-sse-kms-STANDARD-STANDARD-1048576]`, `test_copy_enc[sse-c-sse-kms-STANDARD-STANDARD-1]`, `test_copy_enc[sse-c-sse-kms-STANDARD-STANDARD-8388608]`, `test_copy_enc[sse-kms-sse-kms-STANDARD-STANDARD-1024]`, `test_copy_enc[sse-kms-sse-kms-STANDARD-STANDARD-1048576]`, `test_copy_enc[sse-kms-sse-kms-STANDARD-STANDARD-1]`, `test_copy_enc[sse-kms-sse-kms-STANDARD-STANDARD-8388608]`, `test_copy_enc[sse-s3-sse-kms-STANDARD-STANDARD-1024]`, `test_copy_enc[sse-s3-sse-kms-STANDARD-STANDARD-1048576]`, `test_copy_enc[sse-s3-sse-kms-STANDARD-STANDARD-1]`, `test_copy_enc[sse-s3-sse-kms-STANDARD-STANDARD-8388608]`, `test_copy_part_enc[sse-c-sse-kms-STANDARD-STANDARD-8388608]`, `test_copy_part_enc[sse-kms-sse-kms-STANDARD-STANDARD-8388608]`, `test_copy_part_enc[sse-s3-sse-kms-STANDARD-STANDARD-8388608]`, `test_sse_kms_default_post_object_authenticated_request`, `test_sse_kms_default_upload_1b`, `test_sse_kms_default_upload_1kb`, `test_sse_kms_default_upload_1mb`, `test_sse_kms_default_upload_8mb`, `test_sse_kms_method_head`

#### `check_grants` sorts grants by a key that is None for group grantees (Python 3 TypeError) (10)

`got.sort(key=lambda x: x['Grantee'].get('DisplayName'))` runs whenever the first grant has a DisplayName; a group grant (AllUsers/AuthenticatedUsers) has none, so Python 3 raises `TypeError: '<' not supported between instances of 'NoneType' and 'str'`. fakes3 lists the owner first, like AWS; MinIO and LocalStack fail identically.

Tests: `test_bucket_acl_canned`, `test_bucket_acl_canned_authenticatedread`, `test_bucket_acl_canned_publicreadwrite`, `test_object_acl_canned`, `test_object_acl_canned_authenticatedread`, `test_object_acl_canned_during_create`, `test_object_acl_canned_publicreadwrite`, `test_put_bucket_acl_grant_group_read`, `test_versioned_object_acl`, `test_versioned_object_acl_no_version_specified`

#### Test mutates a header that boto3 re-signs afterwards (the request is valid) (8)

The tests add/remove `Authorization`, `Content-Length` or `x-amz-date` in a `before-call` hook, but botocore's signer rewrites those after the hook (checked with boto3 1.43.123 against a stub server: `Authorization` is replaced, `Content-Length` restored, `Date` re-added). The server correctly accepts the valid request, so `ClientError not raised`. They also fail on MinIO and LocalStack. Most are `fails_on_rgw` with a TODO saying so.

Tests: `test_bucket_create_bad_authorization_empty`, `test_bucket_create_bad_authorization_none`, `test_object_create_bad_authorization_empty`, `test_object_create_bad_authorization_incorrect_aws2`, `test_object_create_bad_authorization_invalid_aws2`, `test_object_create_bad_authorization_none`, `test_object_create_bad_contentlength_none`, `test_object_create_bad_date_none_aws2`

#### GetObject `?partNumber` out of range: test expects 400 InvalidPart (RGW), AWS answers 416 InvalidPartNumber (5)

`assert 416 == 400` (and `'InvalidRequest' == 'InvalidPart'` for the SSE-C variant). fakes3 follows the S3 API reference (416). Fails on MinIO and LocalStack too.

Tests: `test_multipart_get_part`, `test_multipart_single_get_part`, `test_multipart_sse_c_get_part`, `test_non_multipart_get_part`, `test_non_multipart_sse_c_get_part`

#### PutObject `If-None-Match: <etag>` expected to give 412 (RGW extension; AWS accepts only `*`) (5)

Each test passes the `*` cases and then sends `IfNoneMatch=<etag>` expecting 412. The S3 API reference documents only `If-None-Match: *` for PutObject / CompleteMultipartUpload. MinIO passes (it implements the extension); LocalStack fails. Also passes on: minio.

Tests: `test_multipart_put_current_object_if_none_match`, `test_multipart_put_object_if_match`, `test_put_current_object_if_none_match`, `test_put_object_current_if_match`, `test_put_object_if_match`

#### Second CompleteMultipartUpload on a finished upload expected to succeed (RGW `check_previously_completed`) (4)

`NoSuchUpload` on the repeated complete (the tests comment it as an 'extra' complete); `test_multipart_resend_first_finishes_last` lists PartNumber 1 twice in the complete request and gets `InvalidPart`. All fail on MinIO and LocalStack.

Tests: `test_multipart_resend_first_finishes_last`, `test_multipart_reupload_checksum_and_etag`, `test_multipart_upload`, `test_multipart_upload_small`

#### Presigned URL with negative / over-long X-Amz-Expires: test expects 403, S3 answers 400 (3)

`assert 400 == 403`. S3 rejects out-of-range `X-Amz-Expires` with 400 `AuthorizationQueryParametersError`; the test encodes RGW's 403.

Tests: `test_object_raw_get_x_amz_expires_out_max_range`, `test_object_raw_get_x_amz_expires_out_positive_range`, `test_object_raw_put_authenticated_expired`

#### Re-creating an owned bucket expected to fail with BucketAlreadyExists (RGW) (2)

Same call as the CreateBucket group above but the test insists on 409 `BucketAlreadyExists`; on AWS us-east-1 this is a 200. The two groups cannot both pass.

Tests: `test_bucket_recreate_new_acl`, `test_bucket_recreate_overwrite_acl`

#### SSE-C read with the wrong key: test expects 400, AWS answers 403 (2)

`assert 403 == 400` for GET with a well-formed but different customer key; S3 reports 403 `AccessDenied` for a key that does not match the stored MD5 (400 is for malformed/missing headers). Fails on MinIO too.

Tests: `test_encryption_sse_c_multipart_bad_download`, `test_encryption_sse_c_other_key`

#### Bucket policy used by the test has no `Resource` (invalid on AWS too) (1)

`MalformedPolicy: Syntax errors in policy` for a statement with only Principal/Action; AWS rejects it as well. The test relies on RGW.

Tests: `test_bucket_logging_owner`

#### Test expects a bucket policy with only an Allow to deny the owner (1)

Referer-conditioned `Allow` policy; the test then expects the bucket owner's own GET to be 403. An Allow can never remove the owner's access, so only RGW's policy engine behaves like that (`fails_on_rgw` TODO says it is being fixed).

Tests: `test_bucket_policy_set_condition_operator_end_with_IfExists`

#### Needs RGW's accelerated lifecycle clock (`rgw_lc_debug_interval`) (1)

The test waits `lc_debug_interval` seconds for 'one day' to pass; AWS and fakes3 count real days (fakes3 has a virtual clock, but s3-tests cannot drive it).

Tests: `test_delete_marker_expiration`

#### Non-ASCII user metadata expected to round-trip verbatim; AWS returns RFC 2047 `=?UTF-8?B?…?=` (1)

`'=?UTF-8?B?SGVsbG8gV29ybGTDqQ==?=' == 'Hello World\xe9'`. fakes3 deliberately encodes like AWS (via `s3s-rfc2047`); boto3 does not decode.

Tests: `test_object_set_get_unicode_metadata`

#### SSE-KMS without a key id expected to fail; AWS falls back to the managed `aws/s3` key (1)

`ClientError not raised` for `x-amz-server-side-encryption: aws:kms` alone. MinIO and LocalStack reject it, AWS accepts it. Also passes on: localstack, minio.

Tests: `test_sse_kms_no_key`

### Marked `fails_on_aws` by s3-tests itself (excluded from the baseline): 89 tests

- 31 × `ClientError NotImplemented on PutBucketLogging` — `test_bucket_logging_bucket_acl_required`, `test_bucket_logging_bucket_auth_type`, `test_bucket_logging_copy_objects`, … (31)
- 12 × `AssertionError: ClientError not raised` — `test_bucket_create_naming_dns_dash_dot`, `test_bucket_create_naming_dns_dot_dash`, `test_bucket_list_unordered`, … (12)
- 9 × `KeyError: 'Errors'` — `test_delete_objects_current_if_match`, `test_delete_objects_current_if_match_last_modified_time`, `test_delete_objects_current_if_match_size`, … (9)
- 7 × `assert N == N` — `test_lifecycle_deletemarker_expiration`, `test_lifecycle_deletemarker_expiration_with_days_tag`, `test_lifecycle_expiration`, … (7)
- 6 × `TypeError: '<' not supported between instances of 'NoneType' and 'str'` — `test_bucket_acl_canned_during_create`, `test_bucket_acl_grant_userid_fullcontrol`, `test_bucket_acl_grant_userid_read`, … (6)
- 6 × `ClientError InvalidRequest on PutBucketLifecycleConfiguration` — `test_lifecycle_expiration_noncur_tags1`, `test_lifecycle_expiration_size_gt`, `test_lifecycle_expiration_size_lt`, … (6)
- 4 × `assert False` — `test_copy_enc[unencrypted-sse-kms-STANDARD-STANDARD-1024]`, `test_copy_enc[unencrypted-sse-kms-STANDARD-STANDARD-1048576]`, `test_copy_enc[unencrypted-sse-kms-STANDARD-STANDARD-1]`, … (4)
- 3 × `ClientError NoSuchKey on DeleteObject` — `test_delete_object_current_if_match`, `test_delete_object_if_match`, `test_delete_object_version_if_match`
- 2 × `AssertionError: assert N == N` — `test_lifecycle_multipart_expiration`, `test_object_header_acl_grants`
- 1 × `KeyError: 'Summary'` — `test_account_usage`
- 1 × `ClientError UnresolvableGrantByEmailAddress on PutBucketAcl` — `test_bucket_acl_grant_email`
- 1 × `KeyError: 'x-rgw-object-count'` — `test_bucket_head_extended`
- 1 × `AssertionError: assert 'FULL_CONTROL' == 'READ'` — `test_bucket_header_acl_grants`
- 1 × `ClientError Unknown on CreateRole` — `test_bucket_logging_requester_assumed_role`
- 1 × `AssertionError: assert False` — `test_copy_part_enc[unencrypted-sse-kms-STANDARD-STANDARD-8388608]`
- 1 × `KeyError: 'X-RGW-Object-Count'` — `test_head_bucket_usage`
- 1 × `ClientError AccessDenied on ListBuckets` — `test_list_buckets_anonymous`
- 1 × `ClientError MalformedPolicy on PutBucketPolicy` — `test_put_bucket_logging_tenant_s`

## fakes3: skipped tests

| skipped | reason |
|--:|---|
| 74 | ceph extension to bucket logging not supported at client |
| 70 | [iam root] user does not belong to an account |
| 11 | [s3 cloud] section missing cloud_storage_class |
| 4 | requires 3 or more storage classes |
| 1 | got empty parameter set for (source_mode_key, source_storage_class, dest_storage_class) |
| 1 | requires 2 or more storage classes |
| 1 | no cloud_storage_class configured |
| 1 | no api_name configured |

## fakes3-aws (`--bucket-defaults aws`) against fakes3

112 tests pass with the legacy defaults and fail with the AWS defaults; 0 go the other way. s3-tests predates the 2023 changes (ACLs, public policies and SSE-C all assumed available), so these are the tests that no longer apply to a bucket created the way AWS creates it today.

- 28 × `ClientError InvalidBucketAclWithObjectOwnership on CreateBucket` — `test_post_object_anonymous_request`, `test_post_object_authenticated_no_content_type`, `test_post_object_authenticated_request_bad_access_key`, … (28)
- 23 × `ClientError AccessDenied on PutObject` — `test_get_sse_c_encrypted_object_attributes`, `test_copy_enc[sse-c-unencrypted-STANDARD-STANDARD-8388608]`, `test_copy_enc[sse-c-sse-s3-STANDARD-STANDARD-1]`, … (23)
- 12 × `ClientError AccessDenied on CopyObject` — `test_copy_enc[unencrypted-sse-c-STANDARD-STANDARD-1]`, `test_copy_enc[unencrypted-sse-c-STANDARD-STANDARD-1024]`, `test_copy_enc[unencrypted-sse-c-STANDARD-STANDARD-1048576]`, … (12)
- 11 × `ClientError AccessDenied on PutBucketPolicy` — `test_multipart_upload_on_a_bucket_with_policy`, `test_bucket_policy`, `test_bucketv2_policy`, … (11)
- 9 × `ClientError AccessControlListNotSupported on PutObjectAcl` — `test_object_acl`, `test_object_acl_write`, `test_object_acl_writeacp`, … (9)
- 8 × `ClientError AccessControlListNotSupported on PutBucketAcl` — `test_bucket_list_objects_anonymous`, `test_bucket_listv2_objects_anonymous`, `test_expected_bucket_owner`, … (8)
- 6 × `assert N == N` — `test_block_public_put_bucket_acls`, `test_object_presigned_put_object_with_acl`, `test_object_presigned_put_object_with_acl_tenant`, … (6)
- 4 × `AssertionError: assert '' == 'ServerSideEn...NotFoundError'` — `test_get_bucket_encryption_s3`, `test_get_bucket_encryption_kms`, `test_delete_bucket_encryption_s3`, … (4)
- 3 × `ClientError AccessControlListNotSupported on PutObject` — `test_object_raw_get_bucket_acl`, `test_object_raw_response_headers`, `test_object_raw_authenticated_bucket_acl`
- 2 × `ClientError AccessDenied on CreateMultipartUpload` — `test_encryption_sse_c_multipart_upload`, `test_encryption_sse_c_unaligned_multipart_upload`
- 1 × `assert False == True` — `test_bucket_concurrent_set_canned_acl`
- 1 × `AssertionError: assert 'AccessContro...tNotSupported' == 'InvalidArgument'` — `test_bucket_acl_grant_nonexist_user`
- 1 × `AssertionError: assert 'AccessContro...tNotSupported' == 'Unresolvable...yEmailAddress'` — `test_bucket_acl_grant_email_not_exist`
- 1 × `AssertionError: assert N == N` — `test_block_public_object_canned_acls`
- 1 × `AssertionError: ClientError not raised` — `test_create_bucket_no_ownership_controls`
- … 1 more error shapes


## Prioritised backlog

Numbers are failing AWS-applicable tests on fakes3 with the default (`legacy`) bucket defaults; today 513 of 816 pass (62.9%).

1. **Cheap fakes3 fixes: done on `fix/s3-tests-cheap`, +38 tests (475 → 513/816, 58.2% → 62.9%; 0 newly failing; fakes3-aws 382 → 417).** Request ids on every response, error body and request-log record; `CreateBucket` on an owned bucket → 200; `GetObject ?partNumber` part checksum + `x-amz-checksum-type`; SSE-C `UploadPart` with another key → 400 `InvalidRequest`; ListObjects `<Marker/>`, `Owner` (v1, v2 with `fetch-owner`), no empty `Delimiter`, `%20` for spaces under `encoding-type=url`; Object Lock mode change needs the bypass, `InvalidRetentionPeriod`; `DeleteObjects` > 1000 keys → `MalformedXML`; `Content-MD5` empty / not 16 bytes → `InvalidDigest`; generated lifecycle rule ID; SSE headers on GET/HEAD → 400; `x-amz-version-id` only for Enabled versioning; `MFADelete: Disabled` without token; default `Content-Type`; 11 tags → `InvalidTag`; header tags sorted; NotPrincipal+Allow → `MalformedPolicy`; request-level checksum on Complete → `BadDigest`; `x-amz-expected-bucket-owner` → 403.
   Not done from the original list (46 tests): the three `test_copy_part_enc[…-sse-c-…]` cases (they stop at `CompleteMultipartUpload` without the SSE-C key, not at UploadPart: unconfirmed AWS behaviour, see the group above), `test_object_read_unreadable` (the non-UTF-8 key case already gave 400; the test sends valid UTF-8 C1 controls), and `test_json_column_sum_min_max` (its bucket is now created twice fine, then it hits `SelectObjectContent` NotImplemented).
2. **Second identity support in fakes3 (unlocks the 36 "multi-user" tests plus whatever they hide).** Add repeatable credentials (e.g. `--user name:access:secret[:canonical-id]`, tenants as a naming rule) and point `[s3 alt]` / `[s3 tenant]` at them. Those 36 tests are the bulk of S3 access-control semantics (bucket / object ACL grants between users, policy principals, `test_access_*`, ownership, cross-user copy and multipart owner fields); today they cannot tell us anything. Re-triage afterwards: a few will turn into real bugs.
3. **Medium fixes, 15 tests:** `GetBucketPolicyStatus` computed from ACL/policy when no policy exists (5), delimiter + prefix + max-keys pagination (4), POST-policy extras: expiration format, redirect `%22`, `tagging` field, checksum fields (4), CORS wildcard origins (1), `ListBuckets` `MaxBuckets`/`ContinuationToken` (1).
4. **s3s-bound, 25 tests; needs upstream patches or a fakes3 pre-processing layer.** (a) SigV2 on bucket-level requests (8): botocore signs `/bucket/`, s3s signs the wire path `/bucket`; this breaks every SigV2 bucket call from boto3, so it is the one with real user impact; the fix belongs in `s3s` (use `/bucket/` as the canonical resource of a bucket-level path-style request). (b) SigV2 date errors map to 400 instead of 403 (10). (c) POST policy violations give 400 `InvalidPolicyDocument`, AWS 403 (4). (d) `Transfer-Encoding: chunked` PutObject (1) and RFC 1123 `x-amz-date` (2).
5. **Unimplemented features:** bucket logging (5 tests + 31 `fails_on_aws` ones that test RGW's extension; large, probably not worth it for a test double), `GetObjectTorrent` (1, AWS deprecated it), S3 Select edge shapes (12 failing of 34; the engine exists), SNS / S3 Control (6, other services), IAM / STS (63, other services — recommend leaving out of scope and filtering `-k 'not iam and not sts'`).
6. **Keep out of the target:** the 66 test-defect tests (ineffective header manipulation, py3 sort bug, RGW-only expectations) and the 89 marked ones. Do not "fix" fakes3 for them; most contradict AWS. A `-k`/`--deselect` list derived from `triage.json` could make the CI run green-or-explained; not done here.

### Not run / could not get working

- Only path-style addressing over plain HTTP was exercised (`is_secure = False`, host `127.0.0.1`); TLS and virtual-host addressing are not covered by this runner.
- LocalStack and MinIO use the same single credential, so their "multi-user" failures are caused by the harness, not the product; their reference value is for the single-identity groups only.
- IAM-account tests (70) skip themselves before running; web-identity STS needs an external Keycloak.
- Root causes of five groups are marked "unconfirmed" in their text (`list-prefix-newline`, `content-encoding-format`, `delete-marker-header`, `v2-put-contentlength`, parts of `post-policy-extras`); `test-defect` classification of tests that fail on all three servers rests on the S3 API reference and on behaviour I know of AWS, not on a live AWS run (the repo has no AWS account).
