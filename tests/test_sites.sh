#!/bin/bash
# Compare Incognidium renders against Firefox headless screenshots.
# Site list is read from sites.txt so this script stays site-agnostic.
# Usage: ./test_sites.sh [site_name]

set -e

OUTDIR="/tmp/incognidium_tests"
mkdir -p "$OUTDIR"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SITES_FILE="$SCRIPT_DIR/../sites.txt"

# Build an associative array of name -> URL from sites.txt, ignoring comments
# and blank lines.
declare -A SITES
while IFS='|' read -r name url _category; do
    name="${name%%#*}"
    name="$(echo "$name" | tr -d '[:space:]')"
    url="${url%%#*}"
    url="$(echo "$url" | tr -d '[:space:]')"
    if [[ -n "$name" && -n "$url" && "$name" != @("#"*) ]]; then
        SITES["$name"]="$url"
    fi
done < "$SITES_FILE"

render_site() {
    local name="$1"
    local url="$2"
    echo "=== Testing: $name ($url) ==="

    # Incognidium render
    echo "  Rendering with Incognidium..."
    timeout 30 cargo run --release --bin render_to_png "$url" "$OUTDIR/${name}_incognidium.png" 2>"$OUTDIR/${name}_incognidium.log" || true

    # Firefox headless screenshot (use a throwaway profile so a stale Marionette
    # process cannot block the default profile).
    echo "  Rendering with Firefox..."
    FF_PROFILE="$(mktemp -d)"
    timeout 30 firefox --headless --profile "$FF_PROFILE" --screenshot "$OUTDIR/${name}_firefox.png" --window-size=1024,3000 "$url" 2>/dev/null || true
    rm -rf "$FF_PROFILE"

    echo "  Done: $OUTDIR/${name}_incognidium.png vs $OUTDIR/${name}_firefox.png"
    echo ""
}

if [ -n "$1" ]; then
    # Render single site
    if [ -n "${SITES[$1]}" ]; then
        render_site "$1" "${SITES[$1]}"
    else
        echo "Unknown site: $1. Available: ${!SITES[@]}"
    fi
else
    # Render the first ten sites so the default run stays light.
    count=0
    for name in "${!SITES[@]}"; do
        if [ "$count" -ge 10 ]; then
            break
        fi
        render_site "$name" "${SITES[$name]}"
        count=$((count + 1))
    done
fi

echo "All renders saved to $OUTDIR/"
ls -la "$OUTDIR/"*.png 2>/dev/null
