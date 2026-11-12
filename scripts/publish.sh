#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
#
# Publish Counsel to crates.io in the order the dependency graph requires.
#
# `counsel` depends on `counsel-macros` by version+path, so the companion must
# reach the index first: cargo refuses to package a crate whose versioned
# dependency is absent ("no matching package named `counsel-macros` found").
# Packaging by hand is also easy to skip, so this script dry-runs both before
# touching the registry and retries the dependent publish while the index
# propagates.
#
# Usage:  cargo login <token>   # once
#         ./scripts/publish.sh
set -euo pipefail

cd "$(dirname "$0")/.."

# A dirty tree changes what gets packaged; cargo refuses anyway, but say why.
if [ -n "$(git status --porcelain)" ]; then
    echo "error: working tree is dirty — commit or stash before publishing" >&2
    exit 1
fi

# Fail before packaging twice if no credential is configured.
credentials="${CARGO_HOME:-$HOME/.cargo}/credentials.toml"
if [ -z "${CARGO_REGISTRY_TOKEN:-}" ] && [ ! -f "$credentials" ]; then
    echo "error: no crates.io token found." >&2
    echo "       run: cargo login <token>   (https://crates.io/settings/tokens)" >&2
    exit 1
fi

echo "==> dry-run: counsel-macros"
cargo publish --dry-run -p counsel-macros

# `counsel`'s dry run only works once counsel-macros is on the index, so it is
# skipped on the very first publish and exercised on every later one.
if curl -fsS https://crates.io/api/v1/crates/counsel-macros >/dev/null 2>&1; then
    echo "==> dry-run: counsel"
    cargo publish --dry-run -p counsel
else
    echo "==> counsel dry-run skipped (counsel-macros is not on the index yet)"
fi

echo "==> publish: counsel-macros"
cargo publish -p counsel-macros

# The index is eventually consistent; retry the dependent publish rather than
# reporting a propagation delay as a packaging failure.
echo "==> publish: counsel (retrying while the index propagates)"
for attempt in $(seq 1 20); do
    if cargo publish -p counsel; then
        echo "==> published"
        echo "    verify with: cargo add counsel"
        exit 0
    fi
    echo "    attempt $attempt/20 failed (index not ready yet); retrying in 10s"
    sleep 10
done

echo "error: counsel did not publish after 20 attempts." >&2
echo "       counsel-macros is published; re-run: cargo publish -p counsel" >&2
exit 1
