#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
UI_DEST="$PROJECT_DIR/ui"

CLEAN=false

usage() {
  cat <<EOF
Usage: $(basename "$0") [--clean]

Build koku-ui on-prem static files and copy them into ui/.

Environment:
  KOKU_UI_DIR   Path to the koku-ui repository (optional)

Options:
  --clean       Remove ui/ before copying new build output
  -h, --help    Show this help message
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --clean)
      CLEAN=true
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "Error: unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

resolve_koku_ui_dir() {
  if [[ -n "${KOKU_UI_DIR:-}" ]]; then
    echo "$KOKU_UI_DIR"
    return 0
  fi

  local candidates=(
    "$HOME/dev/koku/koku-ui"
    "$PROJECT_DIR/../koku-ui"
  )

  for dir in "${candidates[@]}"; do
    if [[ -d "$dir" && -f "$dir/package.json" ]]; then
      echo "$dir"
      return 0
    fi
  done

  return 1
}

if ! KOKU_UI_DIR="$(resolve_koku_ui_dir)"; then
  echo "Error: could not find koku-ui directory." >&2
  echo "Set KOKU_UI_DIR to the path of your koku-ui checkout, or clone it to one of:" >&2
  echo "  - $HOME/dev/koku/koku-ui" >&2
  echo "  - $PROJECT_DIR/../koku-ui" >&2
  exit 1
fi

if [[ ! -d "$KOKU_UI_DIR" ]]; then
  echo "Error: koku-ui directory does not exist: $KOKU_UI_DIR" >&2
  exit 1
fi

if [[ ! -f "$KOKU_UI_DIR/package.json" ]]; then
  echo "Error: package.json not found in koku-ui directory: $KOKU_UI_DIR" >&2
  exit 1
fi

echo "Using koku-ui directory: $KOKU_UI_DIR"
echo "Destination: $UI_DEST"

if [[ "$CLEAN" == true ]]; then
  echo "Removing existing ui/ directory..."
  rm -rf "$UI_DEST"
fi

cd "$KOKU_UI_DIR"

echo "Running npm ci..."
npm ci

echo "Running npm run build:onprem..."
npm run build:onprem

copy_dist() {
  local src="$1"
  local dest="$2"
  local label="$3"

  if [[ ! -d "$src" ]]; then
    echo "Error: build output not found: $src" >&2
    exit 1
  fi

  mkdir -p "$dest"
  cp -r "$src"/. "$dest"/
  echo "  $label: $src -> $dest"
}

echo "Copying build output..."
copy_dist \
  "$KOKU_UI_DIR/apps/koku-ui-onprem/dist" \
  "$UI_DEST" \
  "on-prem shell"
copy_dist \
  "$KOKU_UI_DIR/apps/koku-ui-hccm/dist" \
  "$UI_DEST/costManagement" \
  "HCCM (costManagement)"
copy_dist \
  "$KOKU_UI_DIR/apps/koku-ui-ros/dist" \
  "$UI_DEST/costManagementRos" \
  "ROS (costManagementRos)"
copy_dist \
  "$KOKU_UI_DIR/apps/koku-ui-sources/dist" \
  "$UI_DEST/sources" \
  "Sources"

echo
echo "Build complete. Summary:"
echo "  ui/                  <- apps/koku-ui-onprem/dist"
echo "  ui/costManagement/   <- apps/koku-ui-hccm/dist"
echo "  ui/costManagementRos/ <- apps/koku-ui-ros/dist"
echo "  ui/sources/          <- apps/koku-ui-sources/dist"
