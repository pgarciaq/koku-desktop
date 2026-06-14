#!/bin/bash
#
# Assemble the full package version from:
#   - VERSION file (base semver, e.g. "0.2.0")
#   - ui/.build-info (koku-ui date + 10-char git hash)
#
# Tauri requires strict semver, so the format uses pre-release syntax:
#   0.2.0-20260602.6dc900b701
#
# RPM/DEB package managers sort the date component chronologically,
# making every koku-ui rebuild an upgradeable package.
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

FULL_VERSION="${BASE_VERSION}-${UI_DATE}.${UI_HASH}"

echo "Version: $FULL_VERSION"
echo "  base:      $BASE_VERSION"
echo "  ui date:   $UI_DATE"
echo "  ui commit: $UI_HASH"

# Patch tauri.conf.json using python for reliable cross-platform JSON editing.
# Pass the file path and version as arguments to avoid shell path quoting issues.
if [[ -f "$TAURI_CONF" ]]; then
  python3 - "$TAURI_CONF" "$FULL_VERSION" <<'PYEOF'
import json, sys
conf_path, version = sys.argv[1], sys.argv[2]
with open(conf_path, 'r') as f:
    conf = json.load(f)
conf['version'] = version
with open(conf_path, 'w') as f:
    json.dump(conf, f, indent=2)
    f.write('\n')
PYEOF
  echo "  patched: $TAURI_CONF"
fi

# Patch Cargo.toml version field
if [[ -f "$CARGO_TOML" ]]; then
  sed -i.bak "s/^version = \".*\"/version = \"$FULL_VERSION\"/" "$CARGO_TOML"
  rm -f "$CARGO_TOML.bak"
  echo "  patched: $CARGO_TOML"
fi

echo "$FULL_VERSION" > "$PROJECT_DIR/.stamped-version"
echo "  wrote:   .stamped-version"
