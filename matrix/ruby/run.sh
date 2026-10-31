#!/usr/bin/env bash
# Runs the Ruby SDK matrix inside the official ruby image. Gems are cached in ./vendor/bundle.
set -euo pipefail
DIR=$(cd "$(dirname "$0")" && pwd)
: "${S3_ENDPOINT:?S3_ENDPOINT required}"
: "${MATRIX_SERVER:?MATRIX_SERVER required}"
OUT=${MATRIX_OUT:-$DIR/../results/$MATRIX_SERVER}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
IMAGE=${RUBY_IMAGE:-ruby:3}

docker run --rm --network host --user "$(id -u):$(id -g)" \
  -e HOME=/tmp -e BUNDLE_PATH=/work/vendor/bundle -e BUNDLE_APP_CONFIG=/work/.bundle \
  -e S3_ENDPOINT -e S3_ACCESS_KEY="${S3_ACCESS_KEY:-fakes3}" -e S3_SECRET_KEY="${S3_SECRET_KEY:-fakes3secret}" \
  -e S3_REGION="${S3_REGION:-us-east-1}" -e MATRIX_SERVER -e MATRIX_OUT=/out \
  -v "$DIR:/work" -v "$OUT:/out" -w /work "$IMAGE" \
  sh -c 'bundle check >/dev/null 2>&1 || bundle install --quiet; exec bundle exec ruby matrix.rb'
