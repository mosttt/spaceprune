#!/usr/bin/env bash

set -euo pipefail

readonly EXPECTED_RUST_RELEASE="1.97.0"
readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly PROJECT_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
readonly OUTPUT_NAME="THIRD-PARTY-LICENSES-RUST-STDLIB.html"
readonly LICENSE_DIR="${PROJECT_DIR}/licenses/rust"
readonly OUTPUT_PATH="${LICENSE_DIR}/${OUTPUT_NAME}"

mode="${1:-check}"
if [[ "${mode}" != "generate" && "${mode}" != "check" ]]; then
    echo "usage: $0 [generate|check]" >&2
    exit 2
fi

actual_release="$(rustc --version --verbose | sed -n 's/^release: //p')"
if [[ "${actual_release}" != "${EXPECTED_RUST_RELEASE}" ]]; then
    echo "Rust ${EXPECTED_RUST_RELEASE} is required; found ${actual_release:-unknown}" >&2
    exit 1
fi

sysroot="$(rustc --print sysroot)"
source_path="${sysroot}/share/doc/rust/COPYRIGHT-library.html"
if [[ ! -f "${source_path}" ]]; then
    echo "Rust standard-library copyright notice is missing: ${source_path}" >&2
    exit 1
fi

if [[ "${mode}" == "generate" ]]; then
    install -m 0644 "${source_path}" "${OUTPUT_PATH}"
    echo "generated ${OUTPUT_PATH}"
    exit 0
fi

if [[ ! -f "${OUTPUT_PATH}" ]]; then
    echo "Rust standard-library license artifacts are missing; run $0 generate" >&2
    exit 1
fi
if ! cmp -s "${source_path}" "${OUTPUT_PATH}"; then
    echo "Rust standard-library license artifacts are stale; run $0 generate" >&2
    exit 1
fi

echo "${OUTPUT_NAME}: matches the pinned Rust standard-library notice"
