#!/usr/bin/env bash
# Run every JS SDK in this directory against $S3_ENDPOINT; one result file per SDK in $MATRIX_OUT.
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

# Install deps only when the lockfile changed (stamp holds its hash).
want=$(sha256sum package-lock.json | cut -d' ' -f1)
if [[ ! -d node_modules || "$(cat node_modules/.deps-stamp 2>/dev/null)" != "$want" ]]; then
  npm ci --no-audit --no-fund --loglevel=error
  echo "$want" > node_modules/.deps-stamp
fi

# SDKs are independent (own buckets), so run them concurrently.
pids=()
node sdk-v3.mjs & pids+=($!)
node sdk-v2.mjs & pids+=($!)
node sdk-minio.mjs & pids+=($!)
if command -v bun >/dev/null; then bun sdk-bun.mjs & pids+=($!); else echo "bun not found; skipping bun-s3" >&2; fi
rc=0
for p in "${pids[@]}"; do wait "$p" || rc=1; done
exit $rc
