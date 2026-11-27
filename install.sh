#!/bin/sh
# Install precompiled chronicle-cli release binaries with SHA-256 verification.
set -eu

REPO="david-engelmann/chronicle-lob"
DEFAULT_BASE_URL="https://github.com/${REPO}/releases"
API_LATEST_URL="https://api.github.com/repos/${REPO}/releases/latest"

VERSION="${VERSION:-}"
INSTALL_DIR="${INSTALL_DIR:-${HOME}/.local/bin}"
BASE_URL="${BASE_URL:-${DEFAULT_BASE_URL}}"
STAGE_DIR=""

usage() {
  cat <<'EOF'
Usage: install.sh [OPTIONS]

Download, verify (SHA256SUMS), and install chronicle-cli.

Options:
  --version <vX.Y.Z>    Release tag to install (defaults to latest GitHub release)
  --install-dir <DIR>   Destination directory (default: $INSTALL_DIR or ~/.local/bin)
  --base-url <URL>      Release base URL (default: https://github.com/david-engelmann/chronicle-lob/releases)
  -h, --help            Print this help message
EOF
}

err() {
  printf 'install.sh: error: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  if [ -n "${STAGE_DIR}" ] && [ -d "${STAGE_DIR}" ]; then
    rm -rf "${STAGE_DIR}"
  fi
}

fetch() {
  src_url="$1"
  dest_path="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "${dest_path}" "${src_url}"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "${dest_path}" "${src_url}"
  else
    err "curl (or wget) is required to download release assets"
  fi
}

fetch_text() {
  src_url="$1"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "${src_url}"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO- "${src_url}"
  else
    err "curl (or wget) is required to query the latest release tag"
  fi
}

detect_target() {
  uname_s="$(uname -s)"
  uname_m="$(uname -m)"

  case "${uname_s}" in
    Linux)
      target_os="unknown-linux-gnu"
      ;;
    Darwin)
      target_os="apple-darwin"
      ;;
    *)
      err "unsupported operating system '${uname_s}' (supported: Linux, Darwin)"
      ;;
  esac

  case "${uname_m}" in
    x86_64|amd64)
      target_arch="x86_64"
      ;;
    aarch64|arm64)
      target_arch="aarch64"
      ;;
    *)
      err "unsupported architecture '${uname_m}' (supported: x86_64, aarch64)"
      ;;
  esac

  printf '%s-%s\n' "${target_arch}" "${target_os}"
}

compute_sha256() {
  file_path="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${file_path}" | awk '{print tolower($1)}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "${file_path}" | awk '{print tolower($1)}'
  else
    err "neither sha256sum nor shasum is available to verify SHA256SUMS"
  fi
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      [ "$#" -ge 2 ] || err "--version requires a value"
      VERSION="$2"
      shift 2
      ;;
    --version=*)
      VERSION="${1#--version=}"
      shift
      ;;
    --install-dir)
      [ "$#" -ge 2 ] || err "--install-dir requires a value"
      INSTALL_DIR="$2"
      shift 2
      ;;
    --install-dir=*)
      INSTALL_DIR="${1#--install-dir=}"
      shift
      ;;
    --base-url)
      [ "$#" -ge 2 ] || err "--base-url requires a value"
      BASE_URL="$2"
      shift 2
      ;;
    --base-url=*)
      BASE_URL="${1#--base-url=}"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      err "unknown argument '$1' (pass --help for usage)"
      ;;
  esac
done

if [ -z "${VERSION}" ]; then
  latest_json="$(fetch_text "${API_LATEST_URL}")" || err "failed to query ${API_LATEST_URL}"
  VERSION="$(printf '%s\n' "${latest_json}" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
  [ -n "${VERSION}" ] || err "could not parse tag_name from ${API_LATEST_URL}"
fi

case "${VERSION}" in
  v*) TAG="${VERSION}" ;;
  *)  TAG="v${VERSION}" ;;
esac

TARGET="$(detect_target)"
STEM="chronicle-cli-${TAG}-${TARGET}"
ARCHIVE="${STEM}.tar.gz"

BASE_TRIMMED="${BASE_URL%/}"
DOWNLOAD_PREFIX="${BASE_TRIMMED}/download/${TAG}"
case "${BASE_TRIMMED}" in
  file://*)
    local_root="${BASE_TRIMMED#file://}"
    if [ ! -f "${local_root}/download/${TAG}/SHA256SUMS" ] && [ -f "${local_root}/SHA256SUMS" ]; then
      DOWNLOAD_PREFIX="${BASE_TRIMMED}"
    fi
    ;;
esac

STAGE_DIR="$(mktemp -d)"
trap cleanup EXIT INT TERM

ARCHIVE_PATH="${STAGE_DIR}/${ARCHIVE}"
SUMS_PATH="${STAGE_DIR}/SHA256SUMS"

fetch "${DOWNLOAD_PREFIX}/${ARCHIVE}" "${ARCHIVE_PATH}" || err "failed to download ${DOWNLOAD_PREFIX}/${ARCHIVE}"
fetch "${DOWNLOAD_PREFIX}/SHA256SUMS" "${SUMS_PATH}" || err "failed to download ${DOWNLOAD_PREFIX}/SHA256SUMS"

EXPECTED_SHA256="$(awk -v target_file="${ARCHIVE}" '
  NF >= 2 {
    candidate = $2
    sub(/^\*/, "", candidate)
    if (candidate == target_file) {
      print tolower($1)
      exit
    }
  }
' "${SUMS_PATH}")"

if [ -z "${EXPECTED_SHA256}" ]; then
  err "archive '${ARCHIVE}' not found in SHA256SUMS; refusing to extract"
fi

case "${EXPECTED_SHA256}" in
  *[!0-9a-f]*|"")
    err "malformed SHA-256 digest in SHA256SUMS for '${ARCHIVE}'"
    ;;
esac

if [ "${#EXPECTED_SHA256}" -ne 64 ]; then
  err "invalid SHA-256 digest length in SHA256SUMS for '${ARCHIVE}'"
fi

ACTUAL_SHA256="$(compute_sha256 "${ARCHIVE_PATH}")"
if [ "${ACTUAL_SHA256}" != "${EXPECTED_SHA256}" ]; then
  err "SHA-256 mismatch for ${ARCHIVE} (expected ${EXPECTED_SHA256}, got ${ACTUAL_SHA256}); refusing to extract"
fi

tar -xzf "${ARCHIVE_PATH}" -C "${STAGE_DIR}"
EXTRACTED_BIN="${STAGE_DIR}/${STEM}/chronicle-cli"
[ -f "${EXTRACTED_BIN}" ] || err "expected binary '${STEM}/chronicle-cli' missing from archive"

mkdir -p "${INSTALL_DIR}"
TMP_BIN="${INSTALL_DIR}/.chronicle-cli.tmp.$$"
cp "${EXTRACTED_BIN}" "${TMP_BIN}"
chmod 0755 "${TMP_BIN}"
mv -f "${TMP_BIN}" "${INSTALL_DIR}/chronicle-cli"

printf 'Installed chronicle-cli (%s, %s) to %s/chronicle-cli\n' "${TAG}" "${TARGET}" "${INSTALL_DIR}"

case ":${PATH:-}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    printf 'Add %s to your PATH if needed:\n  export PATH="%s:%s"\n' "${INSTALL_DIR}" "${INSTALL_DIR}" "\$PATH"
    ;;
esac
