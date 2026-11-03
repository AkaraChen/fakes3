#!/usr/bin/env bash
# Run Apache OpenDAL's behavior tests (pinned) for its S3 service against fakes3.
#
#   conformance/opendal/run.sh [config...]
#
#   Each config gets a fresh fakes3 started with --auth (signatures are verified) and a bucket
#   named `test`; the same test binary then runs with OPENDAL_TEST=s3 and that config's settings:
#
#   versioned     versioning on, no capability overrides: everything OpenDAL's S3 service claims
#   unversioned   versioning off, version capabilities overridden off (OpenDAL's aws_s3 setup,
#                 without its write_can_append override)
#   list-v1       unversioned, with ListObjects v1 instead of v2
#   virtual-host  unversioned, with virtual-host-style addressing (<bucket>.localhost)
#
#   No config means all four.
#
#   OPENDAL_REPO  repository to fetch (default https://github.com/apache/opendal.git)
#   OPENDAL_REF   tag or commit to test (default: the pin below)
#   OPENDAL_WORK  checkout and build directory (default conformance/opendal/.work)
#   OPENDAL_OUT   result directory (default conformance/opendal/results)
#   FAKES3_BIN    fakes3 binary (default: `cargo build` and target/debug/fakes3)
#
# Writes <config>.log (the libtest output) and <config>.failed (failing test names) to the result
# directory. A failing test is not a failing run: the exit code is non-zero only when the run
# itself broke.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
REPO=${OPENDAL_REPO:-https://github.com/apache/opendal.git}
REF=${OPENDAL_REF:-v0.59.5} # fce9cdefdea7c8e54dfbaf3c88c3b6d5ec68146e, 2026-10-15
WORK=${OPENDAL_WORK:-$HERE/.work}
OUT=${OPENDAL_OUT:-$HERE/results}
SRC=$WORK/opendal
AK=fakes3
SK=fakes3secret
CONFIGS=("$@")
[ ${#CONFIGS[@]} -gt 0 ] || CONFIGS=(versioned unversioned list-v1 virtual-host)

VERSION_CAPS=stat_with_version=false,read_with_version=false,delete_with_version=false,list_with_versions=false,list_with_deleted=false,copy_with_source_version=false,restore=false,restore_with_version=false,restore_with_if_not_exists=false

# --- OpenDAL checkout and test binary
if [ "$(cat "$WORK/opendal.ref" 2>/dev/null || true)" != "$REF" ]; then
  rm -rf "$SRC"
  mkdir -p "$SRC"
  git -C "$SRC" init -q
  git -C "$SRC" fetch -q --depth 1 "$REPO" "$REF"
  git -C "$SRC" checkout -q FETCH_HEAD
  echo "$REF" > "$WORK/opendal.ref"
fi
BEHAVIOR=$(cd "$SRC/core" && CARGO_TARGET_DIR=$WORK/target cargo test --test behavior --features tests,services-s3 \
  --no-run --message-format=json 2>/dev/null |
  python3 -c 'import json,sys
for l in sys.stdin:
    m = json.loads(l)
    if m.get("reason") == "compiler-artifact" and m["target"]["name"] == "behavior" and m.get("executable"):
        print(m["executable"])')
[ -x "$BEHAVIOR" ] || { echo "could not build OpenDAL's behavior tests" >&2; exit 1; }

if [ -z "${FAKES3_BIN:-}" ]; then
  (cd "$ROOT" && cargo build -q)
  FAKES3_BIN=$ROOT/target/debug/fakes3
fi

mkdir -p "$OUT"
SERVER_PID=
stop_server() {
  [ -z "$SERVER_PID" ] || kill "$SERVER_PID" 2>/dev/null || true
  [ -z "$SERVER_PID" ] || wait "$SERVER_PID" 2>/dev/null || true
  SERVER_PID=
}
trap stop_server EXIT

s3() { # method path [curl args...]: a request signed as the default account
  local method=$1 path=$2
  shift 2
  curl -sf -o /dev/null -X "$method" --aws-sigv4 "aws:amz:us-east-1:s3" --user "$AK:$SK" "$@" \
    "http://127.0.0.1:$PORT$path"
}

for config in "${CONFIGS[@]}"; do
  # Dual-stack, because <bucket>.localhost may resolve to ::1.
  coproc FAKES3 { exec "$FAKES3_BIN" --host :: --port 0 --auth --access-key "$AK" --secret-key "$SK" 2>/dev/null; }
  # shellcheck disable=SC2153 # set by coproc
  SERVER_PID=$FAKES3_PID
  read -r addr <&"${FAKES3[0]}"
  PORT=${addr##*:}

  s3 PUT /test
  env=(OPENDAL_TEST=s3 OPENDAL_S3_BUCKET=test OPENDAL_S3_REGION=us-east-1
    OPENDAL_S3_ENDPOINT="http://127.0.0.1:$PORT" "OPENDAL_S3_ACCESS_KEY_ID=$AK" "OPENDAL_S3_SECRET_ACCESS_KEY=$SK"
    OPENDAL_S3_DISABLE_CONFIG_LOAD=true OPENDAL_S3_DISABLE_EC2_METADATA=true)
  case $config in
    versioned)
      s3 PUT '/test?versioning' -H 'content-type: application/xml' \
        --data '<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>'
      ;;
    unversioned) env+=("OPENDAL_TEST_CAPABILITY_OVERRIDES=$VERSION_CAPS") ;;
    list-v1) env+=("OPENDAL_TEST_CAPABILITY_OVERRIDES=$VERSION_CAPS" OPENDAL_S3_DISABLE_LIST_OBJECTS_V2=true) ;;
    virtual-host)
      env+=("OPENDAL_TEST_CAPABILITY_OVERRIDES=$VERSION_CAPS" OPENDAL_S3_ENABLE_VIRTUAL_HOST_STYLE=true
        OPENDAL_S3_ENDPOINT="http://localhost:$PORT")
      ;;
    *) echo "unknown config: $config" >&2; exit 2 ;;
  esac

  # From an empty directory: the harness reads a .env from the working directory.
  (cd "$WORK" && env "${env[@]}" "$BEHAVIOR" --color never) > "$OUT/$config.log" 2>&1 || true
  stop_server
  grep -E '^test .* \.\.\. FAILED$' "$OUT/$config.log" | sed -E 's/^test (.*[^ ]) +\.\.\. FAILED$/\1/' | LC_ALL=C sort > "$OUT/$config.failed" || true
  echo "$config: $(grep -E '^test result:' "$OUT/$config.log" || echo 'no result line, see the log')"
done
