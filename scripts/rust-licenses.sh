#!/usr/bin/env bash

set -euo pipefail

readonly CARGO_ABOUT_VERSION="0.9.2"
readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PROJECT_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
readonly OUTPUT_NAME="THIRD-PARTY-LICENSES-RUST.txt"
readonly LICENSE_DIR="${PROJECT_DIR}/licenses/rust"
readonly OUTPUT_PATH="${LICENSE_DIR}/${OUTPUT_NAME}"

mode="${1:-check}"
if [[ "${mode}" != "generate" && "${mode}" != "check" ]]; then
    echo "usage: $0 [generate|check]" >&2
    exit 2
fi

actual_version="$(cargo about --version 2>/dev/null || true)"
if [[ "${actual_version}" != "cargo-about ${CARGO_ABOUT_VERSION}" ]]; then
    echo "cargo-about ${CARGO_ABOUT_VERSION} is required" >&2
    echo "install it with: cargo install cargo-about --version ${CARGO_ABOUT_VERSION} --locked --features cli" >&2
    exit 1
fi

temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/spaceprune-rust-licenses.XXXXXX")"
trap 'find "${temp_dir}" -depth -delete' EXIT
temp_output="${temp_dir}/${OUTPUT_NAME}"

cd -- "${PROJECT_DIR}"
cargo fetch --locked
python3 "${SCRIPT_DIR}/stage-license-audit.py" "${temp_dir}/source"
cargo about generate \
    --manifest-path "${temp_dir}/source/Cargo.toml" \
    --config "${PROJECT_DIR}/about.toml" \
    --workspace \
    --frozen \
    --fail \
    --output-file "${temp_output}" \
    "${LICENSE_DIR}/about.hbs"

# Keep notices reproducible when registry license files use CRLF.
python3 - "${temp_output}" <<'PYTHON'
import sys
from pathlib import Path
path = Path(sys.argv[1])
path.write_bytes(path.read_bytes().replace(b"\r\n", b"\n"))
PYTHON

if [[ "${mode}" == "generate" ]]; then
    install -m 0644 "${temp_output}" "${OUTPUT_PATH}"
    echo "generated ${OUTPUT_PATH}"
    exit 0
fi

if [[ ! -f "${OUTPUT_PATH}" ]]; then
    echo "Rust license artifacts are missing; run scripts/rust-licenses.sh generate" >&2
    exit 1
fi

if ! cmp -s "${temp_output}" "${OUTPUT_PATH}"; then
    echo "Rust license artifacts are stale; run scripts/rust-licenses.sh generate" >&2
    exit 1
fi

echo "${OUTPUT_NAME}: matches generated dependency notices"
