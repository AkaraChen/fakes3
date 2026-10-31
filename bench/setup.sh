#!/usr/bin/env bash
# Prepare everything bench/run.py needs: the fakes3 release binary, the suite tool, the
# competitor images, and a native MinIO binary (for the no-Docker startup comparison).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"

mkdir -p bench/bin
# Native binary for the no-Docker startup comparison; the image is what the servers run.
cargo build --release --locked
if [ -z "${FAKES3_IMAGE_PREBUILT:-}" ]; then
  docker build -t fakes3:local .
fi
(cd bench/suite && go build -o ../bin/suite .)

{ bench/servers.sh images; echo alpine:3; echo minio/warp:latest; } | xargs -P 8 -n 1 docker pull -q

# MinIO no longer publishes binaries or Docker Hub images; take the one in the Chainguard image.
c=$(docker create cgr.dev/chainguard/minio:latest)
docker cp "$c:/usr/bin/minio" bench/bin/minio
docker rm "$c" >/dev/null
