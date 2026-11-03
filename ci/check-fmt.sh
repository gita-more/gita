#!/usr/bin/env bash
# Fail when a Rust file touched by this change is not rustfmt-clean.
#
# The tree predates a formatting gate, so a repository-wide `cargo fmt
# --check` would mix a large mechanical rewrite into every unrelated PR.
# Checking only the files in the change keeps the gate actionable today and
# ratchets the tree towards fully formatted over time.
set -euo pipefail

base="${BASE_SHA:-}"
if [[ -z "${base}" || "${base}" == "0000000000000000000000000000000000000000" ]]; then
    base="$(git rev-parse HEAD^)"
fi

if ! git cat-file -e "${base}^{commit}" 2>/dev/null; then
    git fetch --no-tags --depth=1 origin "${base}" >/dev/null 2>&1 || true
fi

mapfile -t files < <(git diff --name-only --diff-filter=ACMR "${base}" HEAD -- '*.rs')

if [[ ${#files[@]} -eq 0 ]]; then
    echo "No Rust files changed; format check skipped."
    exit 0
fi

failed=0
for file in "${files[@]}"; do
    [[ -f "${file}" ]] || continue
    if ! rustfmt --edition 2021 --check "${file}"; then
        failed=1
    fi
done

if [[ ${failed} -ne 0 ]]; then
    echo >&2
    echo "Run 'rustfmt --edition 2021 <file>' on the files above." >&2
    exit 1
fi

echo "All changed Rust files are formatted."
