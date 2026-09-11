#!/usr/bin/env bash
# Route changed files to their owning test layer. Missing history is conservative.
set -euo pipefail
native=false
browser=false
base=${1:-}
head=${2:-HEAD}
if [[ -z "$base" || "$base" =~ ^0+$ ]] || ! git cat-file -e "$base^{commit}" 2>/dev/null || ! git cat-file -e "$head^{commit}" 2>/dev/null; then
  native=true
  browser=true
else
  paths=$(mktemp)
  trap 'rm -f "$paths"' EXIT
  git diff --name-only -z "$base" "$head" > "$paths"
  while IFS= read -r -d '' path; do
    case "$path" in
      docs/*|*.md|LICENSE*|.gitignore) ;;
      tests/*.rs|*/tests/*.rs) native=true ;;
      *.rs|Cargo.toml|*/Cargo.toml|Cargo.lock|dependencies.json|.cargo/*|rust-toolchain*) native=true; browser=true ;;
      browser/*|qualification/*|package.json|bun.lock) browser=true ;;
      *) native=true; browser=true ;;
    esac
  done < "$paths"
fi
printf 'native=%s\nbrowser=%s\n' "$native" "$browser"
