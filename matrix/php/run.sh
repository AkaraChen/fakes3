#!/usr/bin/env bash
# Runs the PHP SDK matrix inside the official composer image (PHP CLI + composer).
# Dependencies live in ./vendor and the composer cache in ./.composer.
set -euo pipefail
DIR=$(cd "$(dirname "$0")" && pwd)
: "${S3_ENDPOINT:?S3_ENDPOINT required}"
: "${MATRIX_SERVER:?MATRIX_SERVER required}"
OUT=${MATRIX_OUT:-$DIR/../results/$MATRIX_SERVER}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
IMAGE=${PHP_IMAGE:-composer:latest}

docker run --rm --network host --user "$(id -u):$(id -g)" \
  -e HOME=/tmp -e COMPOSER_HOME=/work/.composer \
  -e S3_ENDPOINT -e S3_ACCESS_KEY="${S3_ACCESS_KEY:-fakes3}" -e S3_SECRET_KEY="${S3_SECRET_KEY:-fakes3secret}" \
  -e S3_REGION="${S3_REGION:-us-east-1}" -e MATRIX_SERVER -e MATRIX_OUT=/out \
  -v "$DIR:/work" -v "$OUT:/out" -w /work --entrypoint sh "$IMAGE" \
  -c '[ -f vendor/autoload.php ] || composer install --no-interaction --no-progress --quiet; exec php -d memory_limit=1G matrix.php'
