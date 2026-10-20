#!/usr/bin/env bash
# install-local.sh - Install proj CLI to a writable directory already in PATH

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'
RELEASE_REPO="${PROJ_RELEASE_REPO:-MiguelRodo/projcli}"
BINARY_NAME="${PROJ_BINARY_NAME:-proj}"
RELEASE_VERSION="${PROJ_RELEASE_VERSION:-}"
if [ -n "$RELEASE_VERSION" ]; then
  DOWNLOAD_BASE_URL="${PROJ_DOWNLOAD_BASE_URL:-https://github.com/${RELEASE_REPO}/releases/download/v${RELEASE_VERSION#v}}"
else
  DOWNLOAD_BASE_URL="${PROJ_DOWNLOAD_BASE_URL:-https://github.com/${RELEASE_REPO}/releases/latest/download}"
fi
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/proj"
STATE_FILE="${STATE_DIR}/install-dir"

map_os() {
  case "$(uname -s)" in
    Linux) echo "linux" ;;
    Darwin) echo "darwin" ;;
    *)
      echo -e "${RED}Error: Unsupported OS: $(uname -s)${NC}" >&2
      exit 1
      ;;
  esac
}

map_arch() {
  case "$(uname -m)" in
    x86_64|amd64) echo "amd64" ;;
    arm64|aarch64) echo "arm64" ;;
    *)
      echo -e "${RED}Error: Unsupported architecture: $(uname -m)${NC}" >&2
      exit 1
      ;;
  esac
}

find_writable_path_dir() {
  local dir probe_file
  IFS=':' read -r -a path_entries <<< "$PATH"
  for dir in "${path_entries[@]}"; do
    [ -n "$dir" ] || continue
    case "$dir" in
      /*) ;;
      *) continue ;;
    esac
    [ -d "$dir" ] || continue
    [ -x "$dir" ] || continue
    [ -w "$dir" ] || continue
    probe_file="$(mktemp "${dir}/.proj-write-test.XXXXXX" 2>/dev/null || true)"
    [ -n "$probe_file" ] || continue
    rm -f "$probe_file"
    echo "$dir"
    return 0
  done
  return 1
}

echo "Installing proj from ${RELEASE_REPO}"
command -v curl >/dev/null 2>&1 || { echo "Error: curl is required." >&2; exit 1; }
if command -v sha256sum >/dev/null 2>&1; then
  sha256_tool=(sha256sum)
elif command -v shasum >/dev/null 2>&1; then
  sha256_tool=(shasum -a 256)
else
  echo "Error: sha256sum or shasum is required; no unverified installation." >&2
  exit 1
fi
[[ "$BINARY_NAME" =~ ^[A-Za-z0-9_-]+$ ]] || { echo "Error: invalid binary name." >&2; exit 1; }
OS_NAME="$(map_os)"
ARCH_NAME="$(map_arch)"
if [ -n "${PROJ_INSTALL_DIR:-}" ]; then
  INSTALL_DIR="$PROJ_INSTALL_DIR"
  [[ "$INSTALL_DIR" == /* && -d "$INSTALL_DIR" && -w "$INSTALL_DIR" ]] || { echo "Error: install directory must be an existing writable absolute directory." >&2; exit 1; }
else
  INSTALL_DIR="$(find_writable_path_dir)" || { echo "Error: create a user-writable directory in PATH first." >&2; exit 1; }
fi
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/proj-install.XXXXXXXX")"
STAGED_BIN=""
cleanup() {
  rm -rf "$TMP_DIR"
  if [ -n "$STAGED_BIN" ]; then rm -f "$STAGED_BIN"; fi
}
trap cleanup EXIT
curl -fsSL "${DOWNLOAD_BASE_URL}/version.txt" -o "$TMP_DIR/version.txt"
VERSION="$(cat "$TMP_DIR/version.txt")"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?(\+[A-Za-z0-9.-]+)?$ ]] || { echo "Error: invalid release version metadata." >&2; exit 1; }
if [ -n "$RELEASE_VERSION" ] && [ "${RELEASE_VERSION#v}" != "$VERSION" ]; then
  echo "Error: downloaded version differs from the requested release." >&2
  exit 1
fi
ASSET="${BINARY_NAME}_${VERSION}_${OS_NAME}_${ARCH_NAME}"
curl -fsSL "${DOWNLOAD_BASE_URL}/${ASSET}" -o "$TMP_DIR/binary"
curl -fsSL "${DOWNLOAD_BASE_URL}/checksums.txt" -o "$TMP_DIR/checksums.txt"
expected_sum="$(awk -v asset="$ASSET" 'NF == 2 { name=$2; sub(/^\*/, "", name); if (name == asset) { print $1; count++ } } END { if (count != 1) exit 1 }' "$TMP_DIR/checksums.txt")" || { echo "Error: release requires exactly one matching checksum." >&2; exit 1; }
[[ "$expected_sum" =~ ^[0-9a-fA-F]{64}$ ]] || { echo "Error: invalid SHA256 checksum." >&2; exit 1; }
actual_sum="$("${sha256_tool[@]}" "$TMP_DIR/binary" | awk '{print $1}')"
if [ "$(printf '%s' "$expected_sum" | tr 'A-F' 'a-f')" != "$actual_sum" ]; then
  echo "Error: checksum verification failed; installed binary unchanged." >&2
  exit 1
fi
target="${INSTALL_DIR}/${BINARY_NAME}"
[ ! -d "$target" ] || { echo "Error: binary target is a directory." >&2; exit 1; }
# Stage on the destination filesystem; download/checksum failures never replace it.
STAGED_BIN="$(mktemp "${INSTALL_DIR}/.proj-install.XXXXXXXX")"
cp "$TMP_DIR/binary" "$STAGED_BIN"
chmod 0755 "$STAGED_BIN"
mv -f "$STAGED_BIN" "$target"
STAGED_BIN=""
mkdir -p "$STATE_DIR"
printf '%s\n' "$INSTALL_DIR" > "$STATE_FILE"
echo "Installed ${BINARY_NAME} ${VERSION} to ${target} (SHA256 verified)."
echo "Run: ${BINARY_NAME} --help"
