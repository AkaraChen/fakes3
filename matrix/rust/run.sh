#!/usr/bin/env bash
# Rust SDK compatibility harness: aws-sdk-s3, object_store, opendal, rust-s3.
# Config comes from env (see matrix/SPEC.md); writes $MATRIX_OUT/<sdk>.json per SDK.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)

export S3_ENDPOINT=${S3_ENDPOINT:-http://127.0.0.1:9100}
export S3_ACCESS_KEY=${S3_ACCESS_KEY:-fakes3}
export S3_SECRET_KEY=${S3_SECRET_KEY:-fakes3secret}
export S3_REGION=${S3_REGION:-us-east-1}
export MATRIX_SERVER=${MATRIX_SERVER:-fakes3}
MATRIX_OUT=${MATRIX_OUT:-$ROOT/matrix/results/$MATRIX_SERVER}
mkdir -p "$MATRIX_OUT"
export MATRIX_OUT=$(cd "$MATRIX_OUT" && pwd)

# Build (cached in matrix/rust/target; offline once the lockfile's deps are fetched).
export CARGO_TARGET_DIR="$HERE/target"
OFFLINE=()
cargo fetch --manifest-path "$HERE/Cargo.toml" --locked --offline >/dev/null 2>&1 && OFFLINE=(--offline)
cargo build --release --locked "${OFFLINE[@]}" --quiet --manifest-path "$HERE/Cargo.toml"

# Silence any ambient AWS config that could leak into SDK credential/region chains.
unset AWS_PROFILE AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN AWS_ENDPOINT_URL AWS_REGION
exec "$CARGO_TARGET_DIR/release/s3matrix"
