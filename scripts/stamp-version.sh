#!/bin/bash
#
# Assemble the full package version from:
#   - VERSION file (base semver, e.g. "0.2.0")
#   - Current UTC timestamp (YYYYMMDD.HHMMSS)
#
# Tauri requires strict semver, so the format uses pre-release syntax:
#   0.2.0-20260615.143022
#
# The timestamp guarantees chronological sorting across all package
# managers (RPM, DEB, NSIS, DMG) and GitHub Releases.
#
# Commit hashes for both koku-desktop and koku-ui are written to
# .stamped-version-meta for inclusion in release notes, but are NOT
# part of the version string itself.
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
TIMESTAMP=$(date -u +%Y%m%d.%H%M%S)
FULL_VERSION="${BASE_VERSION}-${TIMESTAMP}"

DESKTOP_HASH=$(cd "$PROJECT_DIR" && git rev-parse HEAD 2>/dev/null | head -c 10 || echo "unknown")

if [[ -f "$BUILD_INFO" ]]; then
  read -r UI_DATE UI_HASH UI_REF_REST < "$BUILD_INFO"
else
  echo "Warning: ui/.build-info not found" >&2
  UI_DATE="unknown"
  UI_HASH="unknown"
fi

echo "Version: $FULL_VERSION"
echo "  base:         $BASE_VERSION"
echo "  timestamp:    $TIMESTAMP"
echo "  desktop hash: $DESKTOP_HASH"
echo "  ui hash:      $UI_HASH"
echo "  ui date:      $UI_DATE"

# Patch tauri.conf.json using python for reliable cross-platform JSON editing.
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
echo "  wrote: .stamped-version"

# Write metadata file with commit hashes for release notes
cat > "$PROJECT_DIR/.stamped-version-meta" <<EOF
VERSION=$FULL_VERSION
DESKTOP_HASH=$DESKTOP_HASH
UI_HASH=$UI_HASH
UI_DATE=$UI_DATE
EOF
echo "  wrote: .stamped-version-meta"
