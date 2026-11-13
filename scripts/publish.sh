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

# A dry run fails when that exact version is already on the index; that is a
# "nothing to do", not an error — the companion is often already published by
# the time only the dependent crate has been bumped.
dry_run() {
    local pkg="$1"
    local log="/tmp/counsel-dryrun-${pkg}.log"
    if cargo publish --dry-run -p "$pkg" 2>&1 | tee "$log"; then
        return 0
    fi
    if grep -qiE "already (uploaded|exists)" "$log"; then
        echo "==> ${pkg} already published at this version; skipping dry run"
        return 0
    fi
    return 1
}

# Prove the working tree verifies what it claims BEFORE packaging it. 0.1.6 and
# 0.2.0 each shipped a construct that was already broken at publish time; this is
# the gate that would have stopped both.
echo "==> acceptance (working tree)"
./scripts/acceptance.sh

echo "==> dry-run: counsel-macros"
dry_run counsel-macros

# `counsel`'s dry run only works once counsel-macros is on the index, so it is
# skipped on the very first publish and exercised on every later one.
if curl -fsS https://crates.io/api/v1/crates/counsel-macros >/dev/null 2>&1; then
    echo "==> dry-run: counsel"
    dry_run counsel
else
    echo "==> counsel dry-run skipped (counsel-macros is not on the index yet)"
fi

# Publish one package. Exits 0 on success or when that exact version is already
# on the index (so bumping only the dependent crate is a safe re-run), 2 when
# the index has not caught up yet, and 1 on a real failure.
publish_crate() {
    local pkg="$1"
    local log="/tmp/counsel-publish-${pkg}.log"
    if cargo publish -p "$pkg" 2>&1 | tee "$log"; then
        echo "==> published ${pkg}"
        return 0
    fi
    if grep -qiE "already (uploaded|exists)" "$log"; then
        echo "==> ${pkg} already published at this version; continuing"
        return 0
    fi
    if grep -qiE "no matching package|failed to select a version|not found in registry" "$log"; then
        return 2
    fi
    return 1
}

echo "==> publish: counsel-macros"
if ! publish_crate counsel-macros; then
    rc=$?
    [ "$rc" -eq 2 ] && { echo "error: the index does not know counsel-macros yet." >&2; }
    exit 1
fi

# The index is eventually consistent; retry only while it is behind, and fail
# fast on a real packaging/verification error.
echo "==> publish: counsel (retrying while the index propagates)"
for attempt in $(seq 1 20); do
    publish_crate counsel && {
        echo "==> done"
        echo "    verify with: cargo add counsel"
        exit 0
    }
    rc=$?
    if [ "$rc" -ne 2 ]; then
        echo "error: counsel failed to publish (see /tmp/counsel-publish-counsel.log)" >&2
        exit 1
    fi
    echo "    attempt $attempt/20: index not ready yet; retrying in 10s"
    sleep 10
done

echo "error: counsel did not publish after 20 attempts." >&2
echo "       counsel-macros is published; re-run: cargo publish -p counsel" >&2
exit 1
