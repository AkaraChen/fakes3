#!/usr/bin/env bash
# Run ceph/s3-tests (pinned) against one server started by bench/servers.sh.
#
#   conformance/s3-tests/run.sh <server> [pytest args...]
#
#   <server> is a bench/servers.sh name, or `fakes3-aws`: fakes3 started with --bucket-defaults aws
#   (AWS's 2023+ defaults for new buckets: ACLs disabled, Block Public Access, default SSE-S3).
#   bench/servers.sh starts fakes3 with --auth (strict signature verification), the mode this suite
#   has always been measured in; fakes3's default, which accepts any credentials, is not tested here.
#   On fakes3 the `alt` and `tenant` users are separate accounts with their own credentials
#   (FAKES3_USERS, docs/multi-user.md); the other servers only know one credential, so there
#   every user below is that credential and the tests that need a second one cannot pass.
#
#   S3TESTS_REF   ceph/s3-tests commit to test (default: the pin below)
#   S3T_WORKERS   pytest-xdist workers, 1 = serial (default 4)
#   S3T_MARKER    pytest -m expression, e.g. 'not fails_on_aws'          (default: everything)
#   S3T_KEYWORD   pytest -k expression                                  (default: everything)
#   S3T_PATHS     test files/dirs inside the checkout (default s3tests/functional)
#   S3T_TIMEOUT   per-test timeout in seconds (default 180)
#   S3T_OUT       result directory (default conformance/s3-tests/results/<server>)
#   S3T_WORK      checkout and venv directory (default conformance/s3-tests/.work)
#   S3T_LENIENT_CLEANUP  1 (default) = bucket cleanup errors are logged, not raised (see s3t_plugin.py)
#   S3T_NO_SERVER set to test a server that is already running (needs S3T_PORT)
#   S3T_ALL       set to also run the tests triage.json lists as out of scope (other services:
#                 IAM, STS, SNS, S3 Control); by default they are neither run nor counted
#
# Writes junit.xml, phases.*.jsonl (one line per test phase, with markers; see s3t_plugin.py) and
# meta.json to the result directory. A failing test is not a failing run: the exit code is non-zero
# only when the run itself broke (setup error, crash, nothing collected).
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
SERVER=${1:?usage: run.sh <server> [pytest args...]}
shift
# `fakes3-aws` is a variant of the fakes3 container, not another server.
SERVER_NAME=$SERVER
if [ "$SERVER" = fakes3-aws ]; then
  SERVER_NAME=fakes3
  export FAKES3_RUN_ARGS="-e FAKES3_BUCKET_DEFAULTS=aws"
fi

S3TESTS_REPO=${S3TESTS_REPO:-https://github.com/ceph/s3-tests.git}
S3TESTS_REF=${S3TESTS_REF:-5b3987116eca0b4b8fa57d6b10f3db57bab196b6} # master, 2026-10-12
WORKERS=${S3T_WORKERS:-4}
WORK=${S3T_WORK:-$HERE/.work}
OUT=${S3T_OUT:-$HERE/results/$SERVER}
SRC=$WORK/s3-tests
VENV=$WORK/venv
AK=${S3_ACCESS_KEY:-fakes3}
SK=${S3_SECRET_KEY:-fakes3secret}
# alt and tenant: the identities of the config below. Servers that know only one credential give
# every user that one; fakes3 is started with the extra accounts.
ALT_AK=$AK ALT_SK=$SK TENANT_AK=$AK TENANT_SK=$SK
ALT_ID=56789abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234
TENANT_ID='testx$9876543210abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
if [ "$SERVER_NAME" = fakes3 ]; then
  ALT_AK=ALTUSERACCESSKEY00 ALT_SK=altusersecretkey0000000000000000000000000
  TENANT_AK=TENANTACCESSKEY0000 TENANT_SK=tenantsecretkey000000000000000000000000
  # name:access-key:secret:account-id:canonical-id (no spaces: the value is word-split by servers.sh)
  export FAKES3_RUN_ARGS="${FAKES3_RUN_ARGS:-} -e FAKES3_USERS=alt-user:$ALT_AK:$ALT_SK:222222222222:$ALT_ID,tenant-user:$TENANT_AK:$TENANT_SK:333333333333:$TENANT_ID"
fi

# --- checkout at the pinned commit
if [ "$(git -C "$SRC" rev-parse HEAD 2>/dev/null || true)" != "$S3TESTS_REF" ]; then
  rm -rf "$SRC"
  mkdir -p "$SRC"
  git -C "$SRC" init -q
  git -C "$SRC" fetch -q --depth 1 "$S3TESTS_REPO" "$S3TESTS_REF"
  git -C "$SRC" checkout -q FETCH_HEAD
fi

# --- python environment (tox is only a convenience wrapper upstream; pytest is what runs)
if [ ! -x "$VENV/bin/python" ]; then
  uv venv -q --python 3.12 "$VENV"
fi
grep -v '^tox' "$SRC/requirements.txt" > "$WORK/requirements.txt"
uv pip install -q --python "$VENV/bin/python" -r "$WORK/requirements.txt" -c "$HERE/constraints.txt" \
  pytest-xdist pytest-timeout

# --- server
PORT=${S3T_PORT:-$("$ROOT/bench/servers.sh" port "$SERVER_NAME")}
if [ -z "${S3T_NO_SERVER:-}" ]; then
  "$ROOT/bench/servers.sh" stop "$SERVER_NAME" 2>/dev/null || true
  "$ROOT/bench/servers.sh" start "$SERVER_NAME"
  cleanup() {
    mkdir -p "$OUT"
    docker logs "${S3BENCH_PREFIX:-s3bench}-$SERVER_NAME" > "$OUT/server.log" 2>&1 || true
    "$ROOT/bench/servers.sh" stop "$SERVER_NAME" 2>/dev/null || true
  }
  trap cleanup EXIT
fi

# --- s3tests.conf
# main's ids are what fakes3 reports as owner. Where the server knows only one credential (everything
# but fakes3) tests that need a second identity cannot pass and report.py classifies them as such.
rm -rf "$OUT"
mkdir -p "$OUT"
CONF=$OUT/s3tests.conf
cat > "$CONF" <<CONF_EOF
[DEFAULT]
host = 127.0.0.1
port = $PORT
is_secure = False
ssl_verify = False

[fixtures]
bucket prefix = s3t-{random}-
iam name prefix = s3t-
iam path prefix = /s3t/

[s3 main]
display_name = fakes3
user_id = fakes3
email = main@example.com
access_key = $AK
secret_key = $SK
lc_debug_interval = 1

[s3 alt]
display_name = alt-user
user_id = $ALT_ID
email = alt@example.com
access_key = $ALT_AK
secret_key = $ALT_SK

[s3 tenant]
display_name = tenant-user
user_id = testx\$9876543210abcdef0123456789abcdef0123456789abcdef0123456789abcdef
email = tenant@example.com
tenant = testx
access_key = $TENANT_AK
secret_key = $TENANT_SK

[iam]
display_name = iam-user
user_id = 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
email = iam@example.com
access_key = $AK
secret_key = $SK

[iam root]
account_id = RGW11111111111111111
user_id = RGW11111111111111111
email = account1@example.com
access_key = $AK
secret_key = $SK

[iam alt root]
account_id = RGW22222222222222222
user_id = RGW22222222222222222
email = account2@example.com
access_key = $AK
secret_key = $SK
CONF_EOF

# --- run
ARGS=(-p s3t_plugin --junitxml="$OUT/junit.xml" -o junit_family=xunit1 -o cache_dir="$WORK/.pytest_cache"
  --timeout "${S3T_TIMEOUT:-180}" -q -rN --no-header)
[ "$WORKERS" -gt 1 ] && ARGS+=(-n "$WORKERS")
# Out of scope (triage.json "out_of_scope"): tests of other AWS services. They are left out of the run
# (and so of every count) with --ignore for whole files, a marker expression and --deselect for single tests.
MARKER=${S3T_MARKER:-}
if [ -z "${S3T_ALL:-}" ]; then
  SCOPE=$(python3 -I - "$HERE/triage.json" <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))["out_of_scope"]
print(" or ".join(s["markers"]))
for f in s["files"]:
    print("--ignore=s3tests/functional/" + f)
for t in s["tests"]:
    print("--deselect=s3tests/functional/test_s3.py::" + t)
PY
  )
  { read -r OOS_MARKERS; mapfile -t OOS_ARGS; } <<< "$SCOPE"
  ARGS+=("${OOS_ARGS[@]}")
  MARKER=${MARKER:+($MARKER) and }"not ($OOS_MARKERS)"
fi
[ -n "$MARKER" ] && ARGS+=(-m "$MARKER")
[ -n "${S3T_KEYWORD:-}" ] && ARGS+=(-k "$S3T_KEYWORD")
read -r -a PATHS <<< "${S3T_PATHS:-s3tests/functional}"

START=$SECONDS
set +e
(
  cd "$SRC"
  S3TEST_CONF=$CONF S3T_RESULTS_DIR=$OUT PYTHONPATH=$HERE \
    AWS_DEFAULT_REGION=us-east-1 AWS_EC2_METADATA_DISABLED=true \
    "$VENV/bin/python" -m pytest "${ARGS[@]}" "$@" "${PATHS[@]}"
) 2>&1 | tee "$OUT/pytest.log" | tail -n 40
RC=${PIPESTATUS[0]}
set -e

cat > "$OUT/meta.json" <<META_EOF
{"server": "$SERVER", "s3tests_ref": "$S3TESTS_REF", "workers": $WORKERS, "marker": "${S3T_MARKER:-}",
 "keyword": "${S3T_KEYWORD:-}", "all": $([ -n "${S3T_ALL:-}" ] && echo true || echo false), "seconds": $((SECONDS - START)), "pytest_exit": $RC}
META_EOF
echo "$SERVER: pytest exit $RC after $((SECONDS - START))s; results in $OUT"
# 0 = all passed, 1 = some failed; anything else means the run itself was broken.
[ "$RC" -le 1 ]
