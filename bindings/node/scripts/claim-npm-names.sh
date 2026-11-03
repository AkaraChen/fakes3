#!/usr/bin/env bash
# One-time setup for npm trusted publishing (see ../PUBLISHING.md).
#
# npm cannot attach a trusted publisher to a package that does not exist yet, so the very first
# version of each package has to be published by a logged-in human. This publishes an empty
# 0.0.0 placeholder under each name; every real release after that is published by CI
# (.github/workflows/node.yml) through OIDC, without a token.
#
# Usage: npm login && bindings/node/scripts/claim-npm-names.sh
set -euo pipefail

NAMES=(
  fakes3-sdk
  fakes3-sdk-linux-x64-gnu
  fakes3-sdk-linux-arm64-gnu
  fakes3-sdk-linux-x64-musl
  fakes3-sdk-linux-arm64-musl
  fakes3-sdk-darwin-arm64
  fakes3-sdk-darwin-x64
  fakes3-sdk-win32-x64-msvc
)

npm whoami >/dev/null || { echo "run 'npm login' first" >&2; exit 1; }

for name in "${NAMES[@]}"; do
  if npm view "$name" version >/dev/null 2>&1; then
    echo "$name already exists, skipping"
    continue
  fi
  dir=$(mktemp -d)
  cat > "$dir/package.json" <<EOF
{
  "name": "$name",
  "version": "0.0.0",
  "description": "Placeholder that reserves the name; real releases of fakes3-sdk are published from CI.",
  "license": "MIT",
  "repository": {
    "type": "git",
    "url": "git+https://github.com/AkaraChen/fakes3.git",
    "directory": "bindings/node"
  }
}
EOF
  printf '# %s\n\nPlaceholder. See https://github.com/AkaraChen/fakes3\n' "$name" > "$dir/README.md"
  (cd "$dir" && npm publish --access public)
  rm -rf "$dir"
done

echo
echo "Now add a trusted publisher to each package on npmjs.com (Settings -> Trusted publishing):"
echo "  repository AkaraChen/fakes3, workflow node.yml, no environment"
for name in "${NAMES[@]}"; do echo "  https://www.npmjs.com/package/$name/access"; done
