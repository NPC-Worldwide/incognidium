#!/bin/bash
# Hourly rendering analysis — runs comparison against Firefox,
# logs results, and tracks improvements/regressions over time.
# Site list is read from sites.txt so this script stays site-agnostic.
#
# Usage: ./scripts/hourly_analysis.sh
# Set up via: nohup bash -c 'for i in $(seq 1 8); do ./scripts/hourly_analysis.sh; sleep 3600; done' &

set -e
cd "$(dirname "$0")/.."
export DISPLAY=:0

TIMESTAMP=$(date +%Y%m%d_%H%M%S)
OUTDIR="test_archive/hourly/${TIMESTAMP}"
REPORT="test_archive/hourly/history.jsonl"
mkdir -p "$OUTDIR" test_archive/hourly

echo "=== Incognidium Hourly Analysis — $TIMESTAMP ==="

# Build
cargo build --release --bin render_to_png 2>/dev/null

# Sites to test are read from sites.txt; defaults to the first 15 entries so
# the run finishes in a reasonable amount of time.
SITES_FILE="sites.txt"
declare -a SITES
while IFS='|' read -r name url _category; do
    name="${name%%#*}"
    name="$(echo "$name" | tr -d '[:space:]')"
    url="${url%%#*}"
    url="$(echo "$url" | tr -d '[:space:]')"
    if [[ -n "$name" && -n "$url" && "$name" != @("#"*) ]]; then
        SITES+=("$name|$url")
    fi
    if [ "${#SITES[@]}" -ge 15 ]; then
        break
    fi
done < "$SITES_FILE"

RESULTS="{"
RESULTS+="\"timestamp\":\"$TIMESTAMP\","
RESULTS+="\"commit\":\"$(git rev-parse --short HEAD)\","
RESULTS+="\"sites\":{"

GREAT=0; OK=0; POOR=0; FAIL=0; TOTAL=0

for entry in "${SITES[@]}"; do
    IFS='|' read -r name url <<< "$entry"
    inc_path="$OUTDIR/${name}_inc.png"
    ff_path="$OUTDIR/${name}_ff.png"
    txt_path="$OUTDIR/${name}.txt"

    # Render with incognidium
    timeout 60 cargo run --release --bin render_to_png "$url" "$inc_path" \
        --text "$txt_path" 2>/dev/null || true

    # Render with Firefox using a throwaway profile so a stale Marionette
    # process cannot block the default profile.
    FF_PROFILE="$(mktemp -d)"
    timeout 45 firefox --headless --profile "$FF_PROFILE" --screenshot "$ff_path" \
        --window-size=1024,2000 "$url" 2>/dev/null || true
    rm -rf "$FF_PROFILE"

    # Pixel diff
    DIFF=$(python3 -c "
import numpy as np
from PIL import Image
import sys
try:
    a = np.array(Image.open('$inc_path').convert('RGB').resize((512,1000)))
    b = np.array(Image.open('$ff_path').convert('RGB').resize((512,1000)))
    print(int(np.abs(a.astype(int)-b.astype(int)).mean()))
except:
    print(-1)
" 2>/dev/null)

    # Text line count
    LINES=$(wc -l < "$txt_path" 2>/dev/null || echo 0)

    # Grade
    if [ "$DIFF" -lt 0 ] 2>/dev/null; then
        GRADE="UNKNOWN"
    elif [ "$DIFF" -lt 25 ]; then
        GRADE="GREAT"; ((GREAT++))
    elif [ "$DIFF" -lt 50 ]; then
        GRADE="OK"; ((OK++))
    elif [ "$DIFF" -lt 80 ]; then
        GRADE="POOR"; ((POOR++))
    else
        GRADE="FAIL"; ((FAIL++))
    fi
    ((TOTAL++))

    echo "  $GRADE  diff=$DIFF  lines=$LINES  $name"

    [ "$TOTAL" -gt 1 ] && RESULTS+=","
    RESULTS+="\"$name\":{\"diff\":$DIFF,\"lines\":$LINES,\"grade\":\"$GRADE\"}"
done

RESULTS+="},\"summary\":{\"great\":$GREAT,\"ok\":$OK,\"poor\":$POOR,\"fail\":$FAIL,\"total\":$TOTAL}}"

echo "$RESULTS" >> "$REPORT"

echo ""
echo "GREAT: $GREAT  OK: $OK  POOR: $POOR  FAIL: $FAIL  (of $TOTAL)"
echo "Report: $REPORT"
echo "Images: $OUTDIR/"
echo "=== Done ==="
