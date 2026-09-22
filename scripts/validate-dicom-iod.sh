#!/usr/bin/env bash
# SPDX-License-Identifier: MIT

set -euo pipefail

if (( $# != 1 )); then
    printf 'usage: %s NF-BNCT-001-DIRECTORY\n' "$0" >&2
    exit 2
fi

case_root_input=$1
if [[ ! -d "$case_root_input" ]]; then
    printf 'case directory does not exist: %s\n' "$case_root_input" >&2
    exit 2
fi
case_root=$(realpath -- "$case_root_input")

dciodvfy_bin=${DCIODVFY:-dciodvfy}
dcentvfy_bin=${DCENTVFY:-dcentvfy}
for validator in "$dciodvfy_bin" "$dcentvfy_bin"; do
    if ! command -v "$validator" >/dev/null 2>&1; then
        printf 'required independent DICOM validator not found: %s\n' "$validator" >&2
        exit 2
    fi
done

shopt -s nullglob
ct_files=("$case_root"/ct/*.dcm)
rtstruct_file="$case_root/rtstruct.dcm"
if (( ${#ct_files[@]} != 40 )); then
    printf 'expected 40 CT instances, found %d\n' "${#ct_files[@]}" >&2
    exit 1
fi
if [[ ! -f "$rtstruct_file" ]]; then
    printf 'RT Structure Set not found: %s\n' "$rtstruct_file" >&2
    exit 1
fi
dicom_files=("${ct_files[@]}" "$rtstruct_file")

validation_tmp=$(mktemp -d)
cleanup() {
    rm -rf -- "$validation_tmp"
}
trap cleanup EXIT

"$dciodvfy_bin" -version

failed=0
index=0
for dicom_file in "${dicom_files[@]}"; do
    report="$validation_tmp/dciodvfy-$index.txt"
    file_failed=0
    if ! "$dciodvfy_bin" -new -filename "$dicom_file" >"$report" 2>&1; then
        file_failed=1
    fi
    if grep -Eq '^(Error|Warning|Abort) -' "$report"; then
        file_failed=1
    fi
    if (( file_failed != 0 )); then
        printf 'IOD validation failed: %s\n' "$dicom_file" >&2
        sed 's/^/  /' "$report" >&2
        failed=1
    fi
    ((index += 1))
done

entity_report="$validation_tmp/dcentvfy.txt"
entity_failed=0
if ! "$dcentvfy_bin" "${dicom_files[@]}" >"$entity_report" 2>&1; then
    entity_failed=1
fi
if grep -Eq '^(Error|Warning|Abort) -' "$entity_report"; then
    entity_failed=1
fi
if (( entity_failed != 0 )); then
    printf 'cross-instance DICOM consistency validation failed\n' >&2
    sed 's/^/  /' "$entity_report" >&2
    failed=1
fi

if (( failed != 0 )); then
    exit 1
fi

printf 'validated %d DICOM instances with dciodvfy and dcentvfy\n' "${#dicom_files[@]}"
