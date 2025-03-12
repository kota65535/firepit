#!/usr/bin/env bash
set -euo pipefail
set -x

# Ensure I'm on the latest main branch
git switch main
git pull --rebase

git cliff --bump --output CHANGELOG.md
new_version="$(git cliff --bumped-version)"

cargo set-version "${new_version}"

git add CHANGELOG.md Cargo.toml Cargo.lock
git commit -m "${new_version}"
git tag "${new_version}"
git push origin HEAD
git push origin "${new_version}"
