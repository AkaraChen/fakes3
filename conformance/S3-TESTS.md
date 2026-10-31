# ceph/s3-tests baseline

[ceph/s3-tests](https://github.com/ceph/s3-tests) is the S3 conformance suite used by Ceph RGW; about a thousand pytest cases written against real AWS behaviour (and RGW's). This page is the measured baseline of fakes3 against it, with every failure categorised, and the backlog that follows. It complements the 43 scenarios of the SDK matrix ([../matrix/SPEC.md](../matrix/SPEC.md)).

- **Pinned commit:** [`5b3987116eca0b4b8fa57d6b10f3db57bab196b6`](https://github.com/ceph/s3-tests/commit/5b3987116eca0b4b8fa57d6b10f3db57bab196b6) (master, 2026-10-12), 1053 collected test cases (918 in scope, see below): `test_s3.py` 838 (760 functions, parametrized cases counted separately), `test_headers.py` 48, `test_iam.py` 90, `test_sts.py` 36, `test_s3select.py` 34, `test_sns.py` 5, `test_s3control.py` 1, `test_utils.py` 1.
- **Scope:** IAM, STS, SNS and S3 Control are other AWS services, not S3; fakes3 does not implement them and has no reason to. `run.sh` leaves them out (`out_of_scope` in `triage.json`: the files `test_iam.py` 90, `test_sts.py` 36, `test_sns.py` 5, `test_s3control.py` 1, every test marked `iam_*` / `*_policy` / `*sts*` / `webidentity` / `sns` / `s3control`, and `test_bucket_logging_requester_assumed_role`, which needs `CreateRole`): 135 tests, none of which ever passed (65 failing, 70 skipping themselves for lack of an IAM account; 3 of them are in `test_s3.py`). They are neither run nor counted: 918 tests are collected, 684 of them AWS-applicable. `S3T_ALL=1` runs everything again. Against the old denominator, `main` is 521/816 = 63.8% and 521/684 = 76.2% on the tests in scope; this branch is 534/684 = 78.1%.
- **Measured:** fakes3 and fakes3-aws on `fix/s3-tests-remaining` merged with `main` (S3 Select and the direct list rendering included; the harness starts fakes3 with `--auth`), run [41218256465](https://github.com/AkaraChen/fakes3/actions/runs/41218256465); `main` at the same point (`9b3793f`), run [41218258875](https://github.com/AkaraChen/fakes3/actions/runs/41218258875) (the old, whole-suite scope: 577/1053, 521/816). Per test the branch has 13 newly passing and 0 newly failing on fakes3, 9 and 0 on fakes3-aws; the other 905 in-scope outcomes are unchanged. MinIO (`cgr.dev/chainguard/minio:latest`) and LocalStack 4.14 were re-run with the new scope on the same branch (run [41219165279](https://github.com/AkaraChen/fakes3/actions/runs/41219165279)). The numbers will move; the CI run is the source of truth, this file is a snapshot.
- **Baseline definition:** the headline numbers exclude the 234 in-scope tests s3-tests itself marks `fails_on_aws` (they pass on RGW and fail on AWS, so they say nothing about AWS compatibility); they are still run and listed separately. `fails_on_rgw` / `fails_on_dbstore` / `fails_on_dho` tests are AWS-valid and stay in.

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
| fakes3 | 534 | 147 | 0 | 3 | 684 | 78.1% | 78.4% | 2m20s |
| fakes3-aws | 434 | 247 | 0 | 3 | 684 | 63.5% | 63.7% | 2m14s |
| localstack | 404 | 277 | 0 | 3 | 684 | 59.1% | 59.3% | 3m35s |
| minio | 356 | 325 | 0 | 3 | 684 | 52.0% | 52.3% | 4m24s |

### All tests

| server | pass | fail | error | skip | total | pass rate | pass rate excl. skips | runtime |
|---|--:|--:|--:|--:|--:|--:|--:|--:|
| fakes3 | 590 | 235 | 0 | 93 | 918 | 64.3% | 71.5% | 2m20s |
| fakes3-aws | 474 | 351 | 0 | 93 | 918 | 51.6% | 57.5% | 2m14s |
| localstack | 450 | 375 | 0 | 93 | 918 | 49.0% | 54.5% | 3m35s |
| minio | 384 | 441 | 0 | 93 | 918 | 41.8% | 46.5% | 4m24s |

## By file (AWS-applicable tests)

| file | fakes3 pass/total | fakes3-aws pass/total | localstack pass/total | minio pass/total |
|---|--:|--:|--:|--:|
| test_headers.py | 24/48 | 23/48 | 26/48 | 23/48 |
| test_s3.py | 479/601 | 380/601 | 355/601 | 313/601 |
| test_s3select.py | 30/34 | 30/34 | 22/34 | 19/34 |
| test_utils.py | 1/1 | 1/1 | 1/1 | 1/1 |

## fakes3: failures by category

| category | failing tests | groups |
|---|--:|--:|
| Real fakes3 bug (wrong status / code / header / body vs AWS) | 4 | 2 |
| Unimplemented feature | 5 | 1 |
| Needs several users / tenants / IAM / STS (the servers know one credential) | 36 | 1 |
| s3s limitation (verified in the s3s source) | 28 | 6 |
| Test is not AWS-compatible / RGW-specific / ineffective with boto3 (not marked by s3-tests) | 74 | 21 |
| Marked `fails_on_aws` by s3-tests itself (excluded from the baseline) | 88 | 1 |

### Real fakes3 bug (wrong status / code / header / body vs AWS): 4 tests

#### SSE-C: CompleteMultipartUpload without the customer key succeeds, the test expects 400 (3)

After the UploadPart check (fixed: a part with another key is now 400 `InvalidRequest`) the three `test_copy_part_enc[…-sse-c-…]` cases still stop at their last step: `CompleteMultipartUpload` on an SSE-C upload without `x-amz-server-side-encryption-customer-*` headers must fail with 400. The CompleteMultipartUpload reference says those headers are 'required only when the object was created using a checksum algorithm', and fakes3 (like S3 today) gives every object one by default, but whether S3 refuses the call when an SDK leaves them out is unconfirmed, and enforcing it would break clients that do not repeat the key on Complete. Not changed. Cost: **unconfirmed**.

Tests: `test_copy_part_enc[sse-c-sse-c-STANDARD-STANDARD-8388608]`, `test_copy_part_enc[sse-kms-sse-c-STANDARD-STANDARD-8388608]`, `test_copy_part_enc[sse-s3-sse-c-STANDARD-STANDARD-8388608]`

#### GET of a key with C1 control characters (valid UTF-8) returns 404, the test expects 400 (1)

`Key='\xae\x8a-'` is sent as the valid UTF-8 `%C2%AE%C2%8A-`; the test (`fails_on_rgw`, so AWS-valid) wants 400 `Couldn't parse the specified URI.` A key that is not UTF-8 after percent-decoding (`%AE%8A-`) already gets exactly that from s3s (400 `InvalidURI`), so only the C1-control case is open and the rule AWS applies is not documented. Not changed. Cost: **unconfirmed**.

Tests: `test_object_read_unreadable`

### Unimplemented feature: 5 tests

#### PutBucketLogging / bucket logging (NotImplemented) (5)

`PutBucketLogging is not implemented yet` (501). 5 AWS-applicable tests here; a further 31+ `bucket_logging` tests are marked `fails_on_aws` (they test RGW's journal/flush extension) and fail the same way. Cost: **large**.

Tests: `test_put_bucket_logging`, `test_put_bucket_logging_errors`, `test_put_bucket_logging_permissions`, `test_put_bucket_logging_policy_wildcard`, `test_rm_bucket_logging`

### Needs several users / tenants / IAM / STS (the servers know one credential): 36 tests

#### multi user (36)

The test's call graph reaches `get_alt_*`, `get_tenant_*` or `get_iam_*` (found by static analysis of the s3-tests source). The runner maps every identity to the one credential the servers know, so a second user is the bucket owner and the expected `AccessDenied` / owner mismatch cannot happen. Individual failures were not inspected one by one: a real bug can hide behind the missing second identity.

- 26 × `AssertionError: ClientError not raised` — `test_access_bucket_private_object_private`, `test_access_bucket_private_object_publicread`, `test_access_bucket_private_object_publicreadwrite`, … (26)
- 4 × `AssertionError: assert ('fakesN', 'fakesN') != ('fakesN', 'fakesN')` — `test_create_bucket_bucket_owner_preferred`, `test_create_bucket_object_writer`, `test_put_bucket_ownership_bucket_owner_preferred`, … (4)
- 2 × `ClientError InvalidBucketName on ListObjects` — `test_bucket_policy_different_tenant`, `test_bucket_policy_tenanted_bucket`
- 2 × `KeyError: 'ID'` — `test_object_acl_canned_bucketownerfullcontrol`, `test_object_acl_canned_bucketownerread`
- 1 × `AssertionError: assert N == N` — `test_head_object_404_with_policy_prefix`
- 1 × `AssertionError: assert 'fakesN' == 'NabcdefN...NabcdefN'` — `test_list_multipart_upload_owner`

### s3s limitation (verified in the s3s source): 28 tests

#### SigV2: bad `x-amz-date` / `Date` is 400 InvalidRequest, AWS says 403 (10)

s3s `v2_check_header_auth` parses `x-amz-date` with the SigV4 ISO-8601 parser (`invalid x-amz-date`) and `Date` as HTTP-date (`invalid date`, `missing date`), all mapped to 400 `InvalidRequest`; AWS answers 403 `AccessDenied` / `RequestTimeTooSkewed` and accepts RFC 1123 in `x-amz-date` for SigV2. Cost: **large**.

Tests: `test_bucket_create_bad_date_after_today_aws2`, `test_bucket_create_bad_date_before_epoch_aws2`, `test_bucket_create_bad_date_before_today_aws2`, `test_bucket_create_bad_date_empty_aws2`, `test_bucket_create_bad_date_invalid_aws2`, `test_object_create_bad_date_after_end_aws2`, `test_object_create_bad_date_before_epoch_aws2`, `test_object_create_bad_date_before_today_aws2`, `test_object_create_bad_date_empty_aws2`, `test_object_create_bad_date_invalid_aws2`

#### SigV2: bucket-level requests signed by botocore fail with SignatureDoesNotMatch (8)

botocore signs the CanonicalizedResource of a path-style bucket request as `/bucket/` (trailing slash) although the request line is `/bucket`; verified with a stub server and boto3 1.43.123 (`CreateBucket`, `ListObjects`, `HeadBucket` sign `/bkt/`, `PutObject` signs `/bkt/k`). s3s builds the StringToSign from `req_uri.path()` verbatim (`s3s-sigv2` `create_string_to_sign`, called from `ops/signature.rs` `v2_check_header_auth`), so the signatures differ. Every SigV2 bucket-level call from boto3 is affected, not only these tests. Cost: **medium**.

Tests: `test_bucket_create_bad_authorization_invalid_aws2`, `test_bucket_create_bad_date_none_aws2`, `test_bucket_create_bad_ua_empty_aws2`, `test_bucket_create_bad_ua_none_aws2`, `test_cors_presigned_get_object_tenant_v2`, `test_cors_presigned_get_object_v2`, `test_cors_presigned_put_object_tenant_v2`, `test_cors_presigned_put_object_v2`

#### POST policy violations are 400 InvalidPolicyDocument, AWS says 403 AccessDenied (4)

Condition checks live in s3s `post_policy.rs` (`InvalidPolicyDocument`); AWS answers 403 `AccessDenied: Invalid according to Policy`. MinIO passes all four. Cost: **large**. Also passes on: minio.

Tests: `test_post_object_invalid_request_field_value`, `test_post_object_missing_policy_condition`, `test_post_object_request_missing_policy_specified_field`, `test_post_object_wrong_bucket`

#### POST policy uploads: lenient expiration parsing, `tagging` form field ignored, `x-amz-checksum-*` fields refused (s3s) (3)

All three live in s3s before fakes3 sees the request. `PostPolicy::from_json` parses `expiration` with the `time` crate's RFC 3339 parser, which accepts the space-separated `2026-10-31 12:00:00+00:00` the test sends (AWS: 400 for an expiration that is not ISO 8601). `PutObject::deserialize_http_multipart` reads the tagging form field as `x-amz-tagging` only, while the POST Object reference documents `tagging` (XML). `validate_conditions_only` refuses any form field without a policy condition (`x-amz-checksum-sha256` is not exempt), so a documented checksum field gives 403. The fourth POST test of the old group, the redirect `Location` with the quoted ETag (`etag=%22…%22`), is fixed in fakes3. Cost: **large**.

Tests: `test_post_object_invalid_date_format`, `test_post_object_tags_anonymous_request`, `test_post_object_upload_checksum`

#### SigV4: RFC 1123 `X-Amz-Date` together with `Date` is reported as `missing header: x-amz-date` (2)

`extract_amz_date` in s3s `ops/signature.rs:750` raises `InvalidRequest: missing header: x-amz-date` when the header is not in the compact ISO-8601 form; LocalStack accepts the request. Cost: **large**. Also passes on: localstack.

Tests: `test_object_create_amz_date_and_no_date`, `test_object_create_date_and_amz_date`

#### PutObject with `Transfer-Encoding: chunked` (no Content-Length) is rejected (1)

`MissingContentLength: missing header: content-length`, raised in s3s `ops/signature.rs` (lines 709/875) before fakes3 sees the request. LocalStack accepts it. Cost: **large**. Also passes on: localstack.

Tests: `test_object_write_with_chunked_transfer_encoding`

### Test is not AWS-compatible / RGW-specific / ineffective with boto3 (not marked by s3-tests): 74 tests

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

#### S3 Select with JSON output: the tests int() the whole result, AWS answers `{"_1":N}` (2)

`ValueError: invalid literal for int() with base 10: '{"_1":1}'`. `run_s3select_json` asks for JSON output and the tests parse the result as a bare number, which is what Ceph's engine writes for an aggregate; AWS writes one JSON object per record keyed `_1`, `_2`, …, which is what fakes3 returns. The queries themselves run (`count(0) from s3object[*].root`, `min(_1.c1)`).

Tests: `test_count_json_operation`, `test_json_column_sum_min_max`

#### ListObjects (v1) `Prefix` echo: botocore does not URL-decode it, the test expects it decoded (1)

botocore adds `encoding-type=url` to every ListObjects call and decodes the response in `decode_list_object`, whose top-level keys are `Delimiter`, `Marker`, `NextMarker` but not `Prefix` (the v2 and versions decoders do include it; botocore 1.43.123 `handlers.py`). The ListObjects reference lists `Prefix` among the elements returned encoded, and fakes3 encodes it once (`<Prefix>%0A</Prefix>`), so `response['Prefix']` is `%0A` for any server that follows the documentation. Fails on MinIO and LocalStack too.

Tests: `test_bucket_list_prefix_unreadable`

#### Bucket policy used by the test has no `Resource` (invalid on AWS too) (1)

`MalformedPolicy: Syntax errors in policy` for a statement with only Principal/Action; AWS rejects it as well. The test relies on RGW.

Tests: `test_bucket_logging_owner`

#### Test expects a bucket policy with only an Allow to deny the owner (1)

Referer-conditioned `Allow` policy; the test then expects the bucket owner's own GET to be 403. An Allow can never remove the owner's access, so only RGW's policy engine behaves like that (`fails_on_rgw` TODO says it is being fixed).

Tests: `test_bucket_policy_set_condition_operator_end_with_IfExists`

#### CORS: the test expects a non-preflight request to be matched on `Access-Control-Request-Method` (RGW) (1)

Wildcard origins (`*suffix`, `start*end`, `prefix*`) match; the failing step is a `PUT` that carries `Origin` and `Access-Control-Request-Method: GET` and expects the rule that allows `GET` to answer it. AWS evaluates an actual request with its own method and uses the `Access-Control-Request-Method` header only for a preflight `OPTIONS` ('How does Amazon S3 evaluate the CORS configuration'), so the PUT is not matched by the GET-only rule.

Tests: `test_cors_origin_response`

#### Needs RGW's accelerated lifecycle clock (`rgw_lc_debug_interval`) (1)

The test waits `lc_debug_interval` seconds for 'one day' to pass; AWS and fakes3 count real days (fakes3 has a virtual clock, but s3-tests cannot drive it).

Tests: `test_delete_marker_expiration`

#### HEAD of a deleted key in an unversioned bucket expected to carry `x-amz-delete-marker: false` (1)

The HeadObject / GetObject reference says of `x-amz-delete-marker`: 'Specifies whether the object retrieved was (true) or was not (false) a Delete Marker. If false, this response header does not appear in the response.' fakes3 sends it only for delete markers, as documented; MinIO and LocalStack fail the same way.

Tests: `test_delete_marker_nonversioned`

#### ListBuckets pagination: the test assumes an account without other buckets (1)

`list_buckets(MaxBuckets=1)` is paged as AWS documents (`max-buckets`, `continuation-token`, `prefix`, `bucket-region`), but the test asserts that the first page is empty, i.e. that the account owns no bucket. The suite's naming tests leave buckets such as `foo-.bar` behind (their names do not carry the test prefix, so the cleanup skips them), so it cannot pass on a shared server run.

Tests: `test_list_buckets_paginated`

#### Non-ASCII user metadata expected to round-trip verbatim; AWS returns RFC 2047 `=?UTF-8?B?…?=` (1)

`'=?UTF-8?B?SGVsbG8gV29ybGTDqQ==?=' == 'Hello World\xe9'`. fakes3 deliberately encodes like AWS (via `s3s-rfc2047`); boto3 does not decode.

Tests: `test_object_set_get_unicode_metadata`

#### SSE-KMS without a key id expected to fail; AWS falls back to the managed `aws/s3` key (1)

`ClientError not raised` for `x-amz-server-side-encryption: aws:kms` alone. MinIO and LocalStack reject it, AWS accepts it. Also passes on: localstack, minio.

Tests: `test_sse_kms_no_key`

#### S3 Select: the CSV test expects an empty field to be written as `null` (1)

`assert '' != ''` for `select _1 from s3object` on a row that starts with an empty field. Ceph's engine writes NULL as `null`; AWS reads an empty CSV field as an empty string and writes it back as an empty field (a NULL in CSV output is empty too), which is what fakes3 does. The test passed before only because SelectObjectContent answered NotImplemented, a non-empty string. Also passes on: localstack, minio.

Tests: `test_csv_parser`

#### S3 Select: the test looks for Ceph's own error text (`ProcessingTimeError`, `multiple definition of column`) (1)

`assert -1 >= 0` on `find('alias {c11} or column not exist in schema')`: the test searches the error for Ceph's wording and code. fakes3 answers a column that the header lacks with 400 `MissingHeaders`, the S3 code for it.

Tests: `test_schema_definition`

### Marked `fails_on_aws` by s3-tests itself (excluded from the baseline): 88 tests

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
- 1 × `AssertionError: assert False` — `test_copy_part_enc[unencrypted-sse-kms-STANDARD-STANDARD-8388608]`
- 1 × `KeyError: 'X-RGW-Object-Count'` — `test_head_bucket_usage`
- 1 × `ClientError AccessDenied on ListBuckets` — `test_list_buckets_anonymous`
- 1 × `ClientError MalformedPolicy on PutBucketPolicy` — `test_put_bucket_logging_tenant_s`

## fakes3: skipped tests

| skipped | reason |
|--:|---|
| 74 | ceph extension to bucket logging not supported at client |
| 11 | [s3 cloud] section missing cloud_storage_class |
| 4 | requires 3 or more storage classes |
| 1 | requires 2 or more storage classes |
| 1 | no cloud_storage_class configured |
| 1 | got empty parameter set for (source_mode_key, source_storage_class, dest_storage_class) |
| 1 | no api_name configured |

## fakes3-aws (`--bucket-defaults aws`) against fakes3

116 tests pass with the legacy defaults and fail with the AWS defaults; 0 go the other way. s3-tests predates the 2023 changes (ACLs, public policies and SSE-C all assumed available), so these are the tests that no longer apply to a bucket created the way AWS creates it today.

- 29 × `ClientError InvalidBucketAclWithObjectOwnership on CreateBucket` — `test_post_object_success_redirect_action`, `test_object_raw_get`, `test_object_raw_get_bucket_gone`, … (29)
- 23 × `ClientError AccessDenied on PutObject` — `test_copy_enc[sse-c-unencrypted-STANDARD-STANDARD-1]`, `test_copy_enc[sse-c-unencrypted-STANDARD-STANDARD-1024]`, `test_copy_enc[sse-c-unencrypted-STANDARD-STANDARD-1048576]`, … (23)
- 12 × `ClientError AccessDenied on CopyObject` — `test_copy_enc[sse-kms-sse-c-STANDARD-STANDARD-8388608]`, `test_copy_enc[unencrypted-sse-c-STANDARD-STANDARD-1]`, `test_copy_enc[unencrypted-sse-c-STANDARD-STANDARD-1024]`, … (12)
- 12 × `ClientError AccessDenied on PutBucketPolicy` — `test_bucket_policy_put_obj_acl`, `test_get_publicpolicy_acl_bucket_policy_status`, `test_multipart_upload_on_a_bucket_with_policy`, … (12)
- 10 × `ClientError AccessControlListNotSupported on PutBucketAcl` — `test_bucket_list_objects_anonymous`, `test_bucket_listv2_objects_anonymous`, `test_expected_bucket_owner`, … (10)
- 9 × `ClientError AccessControlListNotSupported on PutObjectAcl` — `test_object_put_acl_mtime`, `test_object_copy_not_owned_object_bucket`, `test_object_acl`, … (9)
- 6 × `assert N == N` — `test_block_public_put_bucket_acls`, `test_object_presigned_put_object_with_acl`, `test_object_presigned_put_object_with_acl_tenant`, … (6)
- 4 × `AssertionError: assert '' == 'ServerSideEn...NotFoundError'` — `test_get_bucket_encryption_s3`, `test_get_bucket_encryption_kms`, `test_delete_bucket_encryption_s3`, … (4)
- 3 × `ClientError AccessControlListNotSupported on PutObject` — `test_object_raw_get_bucket_acl`, `test_object_raw_response_headers`, `test_object_raw_authenticated_bucket_acl`
- 2 × `ClientError AccessDenied on CreateMultipartUpload` — `test_encryption_sse_c_multipart_upload`, `test_encryption_sse_c_unaligned_multipart_upload`
- 1 × `ClientError AccessControlListNotSupported on CopyObject` — `test_object_copy_canned_acl`
- 1 × `assert False == True` — `test_bucket_concurrent_set_canned_acl`
- 1 × `AssertionError: assert 'AccessContro...tNotSupported' == 'InvalidArgument'` — `test_bucket_acl_grant_nonexist_user`
- 1 × `AssertionError: assert 'AccessContro...tNotSupported' == 'Unresolvable...yEmailAddress'` — `test_bucket_acl_grant_email_not_exist`
- 1 × `AssertionError: assert N == N` — `test_block_public_object_canned_acls`
- … 1 more error shapes

## Prioritised backlog

Numbers are failing AWS-applicable tests on fakes3 with the default (`legacy`) bucket defaults; today 534 of the 684 in scope pass (78.1%); `main` before this branch was 521 (76.2% of the same 684, 63.8% of the old 816).

1. **Cheap fakes3 fixes: done on `fix/s3-tests-cheap`, +38 tests (475 → 513/816, 58.2% → 62.9%; 0 newly failing; fakes3-aws 382 → 417).** Request ids on every response, error body and request-log record; `CreateBucket` on an owned bucket → 200; `GetObject ?partNumber` part checksum + `x-amz-checksum-type`; SSE-C `UploadPart` with another key → 400 `InvalidRequest`; ListObjects `<Marker/>`, `Owner` (v1, v2 with `fetch-owner`), no empty `Delimiter`, `%20` for spaces under `encoding-type=url`; Object Lock mode change needs the bypass, `InvalidRetentionPeriod`; `DeleteObjects` > 1000 keys → `MalformedXML`; `Content-MD5` empty / not 16 bytes → `InvalidDigest`; generated lifecycle rule ID; SSE headers on GET/HEAD → 400; `x-amz-version-id` only for Enabled versioning; `MFADelete: Disabled` without token; default `Content-Type`; 11 tags → `InvalidTag`; header tags sorted; NotPrincipal+Allow → `MalformedPolicy`; request-level checksum on Complete → `BadDigest`; `x-amz-expected-bucket-owner` → 403.
   Not done from the original list (46 tests): the three `test_copy_part_enc[…-sse-c-…]` cases (they stop at `CompleteMultipartUpload` without the SSE-C key, not at UploadPart: unconfirmed AWS behaviour, see the group above), `test_object_read_unreadable` (the non-UTF-8 key case already gave 400; the test sends valid UTF-8 C1 controls), and `test_json_column_sum_min_max` (its bucket is now created twice fine; it now fails on the JSON output shape, see the S3 Select groups).
2. **Second identity support in fakes3 (unlocks the 36 "multi-user" tests plus whatever they hide).** Add repeatable credentials (e.g. `--user name:access:secret[:canonical-id]`, tenants as a naming rule) and point `[s3 alt]` / `[s3 tenant]` at them. Those 36 tests are the bulk of S3 access-control semantics (bucket / object ACL grants between users, policy principals, `test_access_*`, ownership, cross-user copy and multipart owner fields); today they cannot tell us anything. Re-triage afterwards: a few will turn into real bugs.
3. **Medium fixes: done on `fix/s3-tests-remaining`, +13 tests (521 → 534/684; 0 newly failing; fakes3-aws 425 → 434).** `GetBucketPolicyStatus` computed from the ACL and the policy (5); delimiter paging where the marker is the common prefix just returned, in the shared `walk` (4); POST `success_action_redirect` with the quoted ETag (1); `Content-Encoding` kept as sent (1); `BadDigest` closes the connection, which turns the SigV2 short-`Content-Length` 403 into a 400 (1); `GetObjectTorrent` 404 (1); plus `ListBuckets` `max-buckets` / `continuation-token` / `prefix` / `bucket-region` (covered by Rust tests; the one s3-tests case cannot pass on a shared server, see the test-defect groups). Three of the "bugs" were not: the CORS, `x-amz-delete-marker` and `Prefix`-echo groups turned out to be test defects (the AWS references say fakes3 is right), and three of the four POST-policy cases expiration / `tagging` / checksum cases live in s3s. Still open: the three `test_copy_part_enc[…-sse-c-…]` cases (CompleteMultipartUpload without the SSE-C key: the API reference only says the customer-key headers are 'required only when the object was created using a checksum algorithm' and documents no 400, so not enforced) and `test_object_read_unreadable` (the rule that makes C1 controls an invalid URI is not documented).
4. **s3s-bound, 28 tests; needs upstream patches or a fakes3 pre-processing layer.** (a) SigV2 on bucket-level requests (8): botocore signs `/bucket/`, s3s signs the wire path `/bucket`; this breaks every SigV2 bucket call from boto3, so it is the one with real user impact; the fix belongs in `s3s` (use `/bucket/` as the canonical resource of a bucket-level path-style request). (b) SigV2 date errors map to 400 instead of 403 (10). (c) POST policy: violations give 400 `InvalidPolicyDocument`, AWS 403 (4); an expiration written with a space instead of `T` is accepted, the `tagging` form field is ignored and `x-amz-checksum-*` fields without a policy condition are refused (3). (d) `Transfer-Encoding: chunked` PutObject (1) and RFC 1123 `x-amz-date` (2).
5. **Unimplemented features:** bucket logging (5 tests + 31 `fails_on_aws` ones that test RGW's extension; large, probably not worth it for a test double), `GetObjectTorrent` answers 404 `NoSuchKey` (BitTorrent is retired; whether S3 does the same for an existing object is unconfirmed). IAM / STS / SNS / S3 Control are out of scope and no longer run.
6. **Keep out of the target:** the test-defect tests (ineffective header manipulation, py3 sort bug, RGW-only expectations) and the 89 marked ones. Do not "fix" fakes3 for them; most contradict AWS.

### Not run / could not get working

- Only path-style addressing over plain HTTP was exercised (`is_secure = False`, host `127.0.0.1`); TLS and virtual-host addressing are not covered by this runner.
- LocalStack and MinIO use the same single credential, so their "multi-user" failures are caused by the harness, not the product; their reference value is for the single-identity groups only.
- IAM, STS, SNS and S3 Control (135 tests) are out of scope and not run (see Scope).
- Two groups stay "unconfirmed" (`sse-c-part-key`, `invalid-utf8-key`); `test-defect` classification of tests that fail on all three servers rests on the S3 API reference and on behaviour I know of AWS, not on a live AWS run (the repo has no AWS account).
