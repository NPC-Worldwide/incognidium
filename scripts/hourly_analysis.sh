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
RESULTS+="\"commit\":\"unknown\","
RESULTS+="\"sites\":{"

GREAT=0; OK=0; POOR=0; FAIL=0; CRASH=0; EMPTY=0; TOTAL=0

for entry in "${SITES[@]}"; do
    IFS='|' read -r name url <<< "$entry"
    inc_path="$OUTDIR/${name}_inc.png"
    ff_path="$OUTDIR/${name}_ff.png"
    txt_path="$OUTDIR/${name}.txt"

    # Render with incognidium
    INC_ERR=$(timeout 60 cargo run --release --bin render_to_png "$url" "$inc_path" \
        --text "$txt_path" 2>&1)
    if [ -f "$inc_path" ]; then
        INC_OK=1
    else
        INC_OK=0
    fi

    # Render with Firefox using a throwaway profile so a stale Marionette
    # process cannot block the default profile.
    FF_PROFILE="$(mktemp -d)"
    timeout 45 firefox --headless --profile "$FF_PROFILE" --screenshot "$ff_path" \
        --window-size=1024,2000 "$url" 2>/dev/null || true
    rm -rf "$FF_PROFILE"

    # Text boxes from render stderr (structural output, not pixel metrics)
    TEXT_BOXES=$(echo "$INC_ERR" | grep -oE '[0-9]+ text boxes' | head -n1 | awk '{print $1}')
    if [ -z "$TEXT_BOXES" ]; then
        TEXT_BOXES=$(echo "$INC_ERR" | grep -oE '[0-9]+ flat boxes' | head -n1 | awk '{print $1}')
    fi
    TEXT_BOXES=${TEXT_BOXES:-0}

    # Text line count
    LINES=$(wc -l < "$txt_path" 2>/dev/null || echo 0)

    # Grade by structural text presence, not pixel diff
    if [ "$INC_OK" -ne 1 ]; then
        GRADE="CRASH"; ((CRASH++))
    elif [ "$TEXT_BOXES" -lt 3 ]; then
        GRADE="EMPTY"; ((EMPTY++))
    elif [ "$TEXT_BOXES" -gt 50 ]; then
        GRADE="GREAT"; ((GREAT++))
    elif [ "$TEXT_BOXES" -gt 10 ]; then
        GRADE="OK"; ((OK++))
    else
        GRADE="POOR"; ((POOR++))
    fi
    ((TOTAL++))

    echo "  $GRADE  boxes=$TEXT_BOXES  lines=$LINES  $name"

    [ "$TOTAL" -gt 1 ] && RESULTS+=","
    RESULTS+="\"$name\":{\"boxes\":$TEXT_BOXES,\"lines\":$LINES,\"grade\":\"$GRADE\"}"
done

RESULTS+="},\"summary\":{\"great\":$GREAT,\"ok\":$OK,\"poor\":$POOR,\"fail\":$FAIL,\"crash\":$CRASH,\"empty\":$EMPTY,\"total\":$TOTAL}}"

echo "$RESULTS" >> "$REPORT"

echo ""
echo "GREAT: $GREAT  OK: $OK  POOR: $POOR  FAIL: $FAIL  CRASH: $CRASH  EMPTY: $EMPTY  (of $TOTAL)"
echo "Report: $REPORT"
echo "Images: $OUTDIR/"
echo "=== Done ==="
