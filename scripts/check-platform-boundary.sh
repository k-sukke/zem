#!/bin/sh
# OS-specific code must live in src/platform/ only (see PLAN.md §7).
set -eu
cd "$(dirname "$0")/.."

violations=$(grep -rnE 'cfg!?\(.*\b(unix|windows|target_os|target_family)\b' src --include='*.rs' | grep -v '^src/platform/' || true)
if [ -n "$violations" ]; then
    echo "error: OS-specific cfg found outside src/platform/:" >&2
    echo "$violations" >&2
    exit 1
fi
echo "platform boundary OK"
