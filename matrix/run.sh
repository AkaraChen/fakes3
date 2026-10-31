#!/usr/bin/env bash
# Run every language harness against every server (or a subset).
#
#   matrix/run.sh                         # all servers × all languages
#   SERVERS="fakes3 minio" LANGS="python go" matrix/run.sh
#
# Afterwards: python3 matrix/report.py  → matrix/MATRIX.md
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
SERVERS=${SERVERS:-$("$ROOT/bench/servers.sh" list | tr '\n' ' ')}
LANGS=${LANGS:-$(cd "$ROOT/matrix" && for d in */; do [[ -x "$d/run.sh" ]] && printf '%s ' "${d%/}"; done)}
PREFIX=${S3BENCH_PREFIX:-s3bench}

export S3_ACCESS_KEY=${S3_ACCESS_KEY:-fakes3} S3_SECRET_KEY=${S3_SECRET_KEY:-fakes3secret} S3_REGION=us-east-1

for server in $SERVERS; do
  if [[ $(docker inspect -f '{{.State.Running}}' "$PREFIX-$server" 2>/dev/null) != true ]]; then
    "$ROOT/bench/servers.sh" start "$server" || { echo "!! $server failed to start" >&2; continue; }
  fi
  export MATRIX_SERVER=$server
  export S3_ENDPOINT="http://127.0.0.1:$("$ROOT/bench/servers.sh" port "$server")"
  export MATRIX_OUT="$ROOT/matrix/results/$server"
  mkdir -p "$MATRIX_OUT"
  for lang in $LANGS; do
    echo "== $server / $lang" >&2
    start=$SECONDS
    "$ROOT/matrix/$lang/run.sh" > "$MATRIX_OUT/.$lang.log" 2>&1 || echo "!! $lang harness exited non-zero (see $MATRIX_OUT/.$lang.log)" >&2
    echo "   $((SECONDS - start))s" >&2
  done
done
