#!/bin/bash
#
# Assemble the full package version from:
#   - VERSION file (base semver, e.g. "0.2.0")
#   - ui/.build-info (koku-ui date + 10-char git hash)
#
# Result: 0.2.0.20260615.962d73fed3
#
# This stamps tauri.conf.json and Cargo.toml so that deb/rpm packages
# carry the full version, making every koku-ui rebuild upgradeable.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

VERSION_FILE="$PROJECT_DIR/VERSION"
BUILD_INFO="$PROJECT_DIR/ui/.build-info"
TAURI_CONF="$PROJECT_DIR/src-tauri/tauri.conf.json"
CARGO_TOML="$PROJECT_DIR/src-tauri/Cargo.toml"

if [[ ! -f "$VERSION_FILE" ]]; then
  echo "Error: VERSION file not found at $VERSION_FILE" >&2
  exit 1
fi

BASE_VERSION=$(tr -d '[:space:]' < "$VERSION_FILE")

if [[ -f "$BUILD_INFO" ]]; then
  read -r UI_DATE UI_HASH _REST < "$BUILD_INFO"
else
  echo "Warning: ui/.build-info not found, using koku-desktop git info" >&2
  UI_DATE=$(date +%Y%m%d)
  UI_HASH=$(cd "$PROJECT_DIR" && git rev-parse HEAD 2>/dev/null | head -c 10 || echo "0000000000")
fi

FULL_VERSION="${BASE_VERSION}.${UI_DATE}.${UI_HASH}"

echo "Version: $FULL_VERSION"
echo "  base:      $BASE_VERSION"
echo "  ui date:   $UI_DATE"
echo "  ui commit: $UI_HASH"

# Patch tauri.conf.json — replace the "version" field value
if [[ -f "$TAURI_CONF" ]]; then
  # Use python for reliable JSON editing (available on all CI runners)
  python3 -c "
import json, sys
with open('$TAURI_CONF', 'r') as f:
    conf = json.load(f)
conf['version'] = '$FULL_VERSION'
with open('$TAURI_CONF', 'w') as f:
    json.dump(conf, f, indent=2)
    f.write('\n')
"
  echo "  patched: $TAURI_CONF"
fi

# Patch Cargo.toml — version field (Cargo accepts any string in quotes)
if [[ -f "$CARGO_TOML" ]]; then
  sed -i.bak "s/^version = \".*\"/version = \"$FULL_VERSION\"/" "$CARGO_TOML"
  rm -f "$CARGO_TOML.bak"
  echo "  patched: $CARGO_TOML"
fi

echo "$FULL_VERSION" > "$PROJECT_DIR/.stamped-version"
echo "  wrote:   .stamped-version"
