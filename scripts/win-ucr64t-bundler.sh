#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "Usage: $0 <path/to/app.exe> <path/to/new-dist-dir>" >&2
    exit 1
}

[[ $# -eq 2 ]] || usage

EXE="$1"
DIST="$2"

[[ -f "$EXE" ]] || {
    echo "Error: executable not found:"
    echo "  $EXE"
    exit 1
}

[[ "$EXE" == *.exe ]] || {
    echo "Error: first argument must be a .exe"
    exit 1
}

[[ ! -e "$DIST" ]] || {
    echo "Error: destination already exists:"
    echo "  $DIST"
    exit 1
}

command -v ntldd >/dev/null || {
    echo "Error: ntldd not found in PATH"
    exit 1
}

EXE="$(realpath "$EXE")"

mkdir "$DIST"

echo "Copying executable..."
cp "$EXE" "$DIST/"

echo "Copying DLLs..."

ntldd -R "$EXE" \
| sed -nE 's/.*=>[[:space:]]+([^[:space:]]+\.dll).*/\1/p' \
| tr '\\' '/' \
| grep '/ucrt64/bin/' \
| sort -u \
| while read -r dll; do
    echo "  $(basename "$dll")"
    cp "$dll" "$DIST/"
done

echo
echo "Bundle complete."