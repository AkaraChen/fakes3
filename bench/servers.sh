#!/usr/bin/env bash
# Start/stop every S3 implementation under test in identical conditions:
# Docker, host networking, data on a container-private tmpfs, same credentials.
#
#   bench/servers.sh start <name>   # blocks until the server answers S3 requests
#   bench/servers.sh launch <name>  # starts the container without waiting
#   bench/servers.sh stop  <name>
#   bench/servers.sh port  <name>
#   bench/servers.sh list
#   bench/servers.sh images         # every Docker image the servers need
#
# S3BENCH_PREFIX / S3BENCH_PORT_OFFSET namespace container names and ports, so benchmarks can
# restart servers without disturbing a long-lived set used by the SDK matrix.
# FAKES3_BIND is the address fakes3 listens on (default 127.0.0.1).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
AK=${S3_ACCESS_KEY:-fakes3}
SK=${S3_SECRET_KEY:-fakes3secret}
SERVERS=(fakes3 minio localstack moto s3mock juicefs)
PREFIX=${S3BENCH_PREFIX:-s3bench}
OFFSET=${S3BENCH_PORT_OFFSET:-0}

port() {
  local p
  case $1 in
    fakes3) p=9100 ;; minio) p=9101 ;; localstack) p=9102 ;;
    moto) p=9103 ;; s3mock) p=9090 ;; juicefs) p=9105 ;;
    *) echo "unknown server $1" >&2; exit 2 ;;
  esac
  echo $((p + OFFSET))
}

run() { docker run -d --rm --name "$PREFIX-$1" --network host "${@:2}" >/dev/null; }

start() {
  launch "$1"
  wait_ready "$1" "$(port "$1")"
}

launch() {
  local name=$1 p
  p=$(port "$name")
  stop "$name" 2>/dev/null || true
  case $name in
    fakes3)
      # The image from ./Dockerfile (bench/setup.sh builds it as fakes3:local).
      # --auth: fakes3 does not verify signatures by default, but the other servers do, and the SDK
      # matrix (bad_credentials), the s3-tests runner and the benchmarks all measure fakes3 as a
      # server that checks them. Without it those results would silently change meaning.
      # shellcheck disable=SC2086 # FAKES3_RUN_ARGS is a list of extra `docker run` options, e.g. "-e FAKES3_BUCKET_DEFAULTS=aws"
      run fakes3 ${FAKES3_RUN_ARGS:-} "${FAKES3_IMAGE:-fakes3:local}" --host "${FAKES3_BIND:-127.0.0.1}" --port "$p" --access-key "$AK" --secret-key "$SK" --auth ;;
    minio)
      run minio --tmpfs /data -e MINIO_ROOT_USER="$AK" -e MINIO_ROOT_PASSWORD="$SK" \
        -e MINIO_BROWSER=off cgr.dev/chainguard/minio:latest server /data --address "127.0.0.1:$p" ;;
    localstack)
      run localstack -e GATEWAY_LISTEN="127.0.0.1:$p" -e SERVICES=s3 -e EAGER_SERVICE_LOADING=1 \
        -e LOCALSTACK_AUTH_TOKEN="${LOCALSTACK_AUTH_TOKEN:-}" localstack/localstack:4.14 ;;
    moto)
      run moto motoserver/moto:latest -H 127.0.0.1 -p "$p" ;;
    s3mock)
      run s3mock -e COM_ADOBE_TESTING_S3MOCK_STORE_RETAIN_FILES_ON_EXIT=false --tmpfs /tmp \
        -e COM_ADOBE_TESTING_S3MOCK_HTTPPORT="$p" -e SERVER_PORT=$((p + 101)) adobe/s3mock:latest ;;
    juicefs)
      run juicefs --tmpfs /data -e MINIO_ROOT_USER="$AK" -e MINIO_ROOT_PASSWORD="$SK" \
        --entrypoint sh juicedata/mount:ce-nightly -c "
          juicefs format --storage file --bucket /data/blobs/ sqlite3:///data/meta.db jfs >/dev/null 2>&1 &&
          exec juicefs gateway --multi-buckets --keep-etag --object-tag --object-meta --cache-dir /data/cache --no-usage-report sqlite3:///data/meta.db 127.0.0.1:$p" ;;
    *) echo "unknown server $name" >&2; exit 2 ;;
  esac
}

# Ready = an S3 request gets an HTTP response (any status) from the server.
wait_ready() {
  local deadline=$((SECONDS + 120))
  until curl -s -o /dev/null -m 1 "http://127.0.0.1:$2/" ; do
    if ! docker inspect "$PREFIX-$1" >/dev/null 2>&1; then
      echo "$1 exited during startup" >&2; return 1
    fi
    (( SECONDS < deadline )) || { echo "$1 not ready after 120s" >&2; docker logs "$PREFIX-$1" | tail -20 >&2; return 1; }
    sleep 0.005
  done
}

stop() { docker rm -f "$PREFIX-$1" >/dev/null 2>&1; }

case ${1:-} in
  start) start "$2" ;;
  launch) launch "$2" ;;
  stop) stop "$2" ;;
  port) port "$2" ;;
  list) printf '%s\n' "${SERVERS[@]}" ;;
  images) grep -oE '(alpine|cgr\.dev/chainguard/minio|localstack/localstack|motoserver/moto|adobe/s3mock|juicedata/mount):[A-Za-z0-9._-]+' "$0" | sort -u ;;
  *) echo "usage: $0 start|launch|stop|port <server> | list | images" >&2; exit 2 ;;
esac
