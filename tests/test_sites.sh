#!/bin/bash
# Compare Incognidium renders against Firefox headless screenshots
# Usage: ./test_sites.sh [site_name]

set -e

OUTDIR="/tmp/incognidium_tests"
mkdir -p "$OUTDIR"

# Test sites — mix of simple and complex
declare -A SITES
SITES=(
    ["google"]="https://www.google.com"
    ["wikipedia"]="https://en.wikipedia.org/wiki/Main_Page"
    ["hn"]="https://news.ycombinator.com"
    ["cnn_lite"]="https://lite.cnn.com"
    ["craigslist"]="https://www.craigslist.org"
    ["reddit_old"]="https://old.reddit.com"
    ["npr"]="https://text.npr.org"
    ["bbc"]="https://www.bbc.com"
    ["reuters"]="https://www.reuters.com"
    ["github"]="https://github.com"
)

render_site() {
    local name="$1"
    local url="$2"
    echo "=== Testing: $name ($url) ==="

    # Incognidium render
    echo "  Rendering with Incognidium..."
    timeout 30 cargo run --release --bin render_to_png "$url" "$OUTDIR/${name}_incognidium.png" 2>"$OUTDIR/${name}_incognidium.log" || true

    # Firefox headless screenshot
    echo "  Rendering with Firefox..."
    timeout 30 firefox --headless --screenshot "$OUTDIR/${name}_firefox.png" --window-size=1024,3000 "$url" 2>/dev/null || true

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
    # Render all sites
    for name in "${!SITES[@]}"; do
        render_site "$name" "${SITES[$name]}"
    done
fi

echo "All renders saved to $OUTDIR/"
ls -la "$OUTDIR/"*.png 2>/dev/null
