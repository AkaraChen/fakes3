#!/usr/bin/env bash
# Go SDK compatibility matrix: runs every Go SDK against $S3_ENDPOINT and writes
# $MATRIX_OUT/<sdk>.json. See ../SPEC.md.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
cd "$HERE"

export S3_ENDPOINT=${S3_ENDPOINT:-http://127.0.0.1:9100}
export S3_ACCESS_KEY=${S3_ACCESS_KEY:-fakes3}
export S3_SECRET_KEY=${S3_SECRET_KEY:-fakes3secret}
export S3_REGION=${S3_REGION:-us-east-1}
export MATRIX_SERVER=${MATRIX_SERVER:-fakes3}
export MATRIX_OUT=${MATRIX_OUT:-$HERE/../results/$MATRIX_SERVER}
# go.mod requires a newer Go than may be installed; let the toolchain auto-download (cached).
export GOTOOLCHAIN=${GOTOOLCHAIN:-auto}
export GOFLAGS=${GOFLAGS:--mod=readonly}

# Rebuild only when sources changed (go build is incremental, but skipping it avoids toolchain startup).
BIN=bin/matrix
if [[ ! -x $BIN ]] || [[ -n $(find . -name '*.go' -newer "$BIN" -o -name 'go.*' -newer "$BIN" | head -1) ]]; then
  go build -o "$BIN" .
fi

mkdir -p "$MATRIX_OUT"
SDKS=${MATRIX_SDKS:-aws-sdk-go-v2,aws-sdk-go,minio-go,gocloud-blob}
for sdk in ${SDKS//,/ }; do
  "$BIN" -sdk "$sdk" || echo "go/$sdk: harness exited with $?" >&2
done
