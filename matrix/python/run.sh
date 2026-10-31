#!/usr/bin/env bash
# Run every Python SDK against $S3_ENDPOINT; one result file per SDK in $MATRIX_OUT.
# Two uv environments: ./ (boto3, minio, obstore) and ./s3fs_env (s3fs pins an older botocore).
set -euo pipefail
# Resolve a relative MATRIX_OUT against the caller's cwd before changing directory.
if [[ -n "${MATRIX_OUT:-}" ]]; then mkdir -p "$MATRIX_OUT"; MATRIX_OUT=$(cd "$MATRIX_OUT" && pwd); export MATRIX_OUT; fi
cd "$(dirname "$0")"

export S3_ENDPOINT=${S3_ENDPOINT:-http://127.0.0.1:9100}
export S3_ACCESS_KEY=${S3_ACCESS_KEY:-fakes3}
export S3_SECRET_KEY=${S3_SECRET_KEY:-fakes3secret}
export S3_REGION=${S3_REGION:-us-east-1}
export MATRIX_SERVER=${MATRIX_SERVER:-fakes3}
export MATRIX_OUT=${MATRIX_OUT:-$(cd .. && pwd)/results/$MATRIX_SERVER}
mkdir -p "$MATRIX_OUT"
# Keep stray AWS config/env from leaking into the SDKs.
unset AWS_PROFILE AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN AWS_ENDPOINT_URL AWS_DEFAULT_REGION
export AWS_CONFIG_FILE=/dev/null AWS_SHARED_CREDENTIALS_FILE=/dev/null AWS_EC2_METADATA_DISABLED=true

# --frozen: install exactly the lockfile; a no-op (no network) once .venv matches it.
uv sync --frozen --quiet --python 3.14
(cd s3fs_env && uv sync --frozen --quiet --python 3.14)

PY=.venv/bin/python
PY_S3FS=s3fs_env/.venv/bin/python
pids=()
$PY sdk_boto3.py & pids+=($!)
$PY sdk_minio.py & pids+=($!)
$PY sdk_obstore.py & pids+=($!)
$PY_S3FS sdk_s3fs.py & pids+=($!)
rc=0
for p in "${pids[@]}"; do wait "$p" || rc=1; done
exit $rc
