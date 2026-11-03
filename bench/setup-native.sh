#!/usr/bin/env bash
# Prepare what bench/native.py (and bench/ab.py) need, on Linux, macOS or Windows (Git Bash):
#
#   bench/setup-native.sh             # everything
#   bench/setup-native.sh warp        # just some of: fakes3 suite warp minio moto
#
# fakes3 is the release build, warp and MinIO are built from source (neither publishes binaries
# any more), moto goes into a virtualenv. Needs cargo, go and uv.
set -euo pipefail
cd "$(dirname "$0")/.."
# Go wants a native path in GOBIN; `pwd -W` gives one on Git Bash.
BIN="$(pwd -W 2>/dev/null || pwd)/bench/bin"
WARP_VERSION=v1.8.2

parts=("$@")
[ $# -gt 0 ] || parts=(fakes3 suite warp minio moto)
mkdir -p bench/bin
for p in "${parts[@]}"; do
  case $p in
    fakes3) cargo build --release --locked ;;
    suite) (cd bench/suite && go build -o ../bin/ .) ;;
    warp) GOBIN="$BIN" go install "github.com/minio/warp@$WARP_VERSION" ;;
    minio) GOBIN="$BIN" go install github.com/minio/minio@latest ;;
    moto) uv venv -q --allow-existing bench/bin/moto && uv pip install -q --python bench/bin/moto "moto[server]" ;;
    *) echo "unknown part $p" >&2; exit 2 ;;
  esac
done
