#!/usr/bin/env bash
# Fails when a README pins a fakes3 version other than the one in Cargo.toml: the setup-fakes3
# `version:` example, the service-container image tag and the list of image tags. Run by ci.yml on
# every push (so the version-bump PR has to update them) and by release.yml before publishing.
set -euo pipefail
cd "$(dirname "$0")/.."
want=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
status=0
for f in README.md README.zh-CN.md; do
  # `version: X.Y.Z`, `fakes3:X.Y.Z` and the full-version entry of the image tag list.
  while IFS= read -r hit; do
    line=${hit%%:*}
    for v in $(grep -oE '[0-9]+\.[0-9]+\.[0-9]+' <<<"${hit#*:}"); do
      if [ "$v" != "$want" ]; then
        echo "::error file=$f,line=$line::$f:$line pins fakes3 $v, Cargo.toml is $want"
        status=1
      fi
    done
  done < <(grep -nE 'version: [0-9]+\.[0-9]+\.[0-9]+|fakes3:[0-9]+\.[0-9]+\.[0-9]+|`latest`[,、] `[0-9]+\.[0-9]+\.[0-9]+`' "$f")
done
[ "$status" = 0 ] && echo "READMEs pin fakes3 $want"
exit "$status"
