#!/usr/bin/env python3
"""Batch render test — reads sites.txt, renders with incognidium,
optionally compares with Firefox, produces scored report."""

import subprocess, os, sys, json, time
from datetime import datetime
from pathlib import Path

REPO = Path("/home/caug/incognidium")
SITES_FILE = REPO / "sites.txt"
ARCHIVE = Path.home() / ".incognidium" / "test_results"

def load_sites(category=None, limit=None):
    sites = []
    for line in SITES_FILE.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split("|")
        if len(parts) >= 2:
            name, url = parts[0], parts[1]
            cat = parts[2] if len(parts) > 2 else "other"
            if category and cat != category:
                continue
            sites.append((name, url, cat))
    if limit:
        sites = sites[:limit]
    return sites

def safe_run(cmd, timeout=180):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
    except (subprocess.TimeoutExpired, Exception):
        return type('R', (), {'returncode': 1, 'stderr': 'TIMEOUT', 'stdout': ''})()

def render_site(name, url, outdir, with_firefox=False):
    inc_path = f"{outdir}/{name}_inc.png"
    txt_path = f"{outdir}/{name}.txt"
    ff_path = f"{outdir}/{name}_ff.png"

    # Incognidium render (no-JS to match the Firefox reference)
    inc = safe_run([
        str(REPO / "target" / "release" / "render_to_png"),
        url, inc_path, "--no-js", "--text", txt_path
    ], timeout=180)

    result = {
        "name": name, "url": url,
        "inc_ok": inc.returncode == 0 and os.path.exists(inc_path),
        "text_boxes": 0, "text_lines": 0, "text_chars": 0,
    }

    # Parse stats ("N text boxes" / "N flat boxes" on stderr)
    for line in (getattr(inc, 'stderr', '') or '').split('\n'):
        if 'boxes' in line:
            try: result["text_boxes"] = int(line.split()[0])
            except: pass

    # Read text
    if os.path.exists(txt_path):
        txt = open(txt_path).read()
        result["text_lines"] = txt.count('\n') + 1
        result["text_chars"] = len(txt)

    # Firefox comparison
    if with_firefox and result["inc_ok"]:
        env = os.environ.copy()
        env["DISPLAY"] = ":0"
        safe_run([
            "firefox", "--headless", "--screenshot", ff_path,
            "--window-size=1024,2000", url
        ], timeout=45)


    # Grade
    tb = result["text_boxes"]
    if not result["inc_ok"]:
        result["grade"] = "CRASH"
    elif tb < 3:
        result["grade"] = "EMPTY"
    elif tb > 50:
        result["grade"] = "GREAT"
    elif tb > 10:
        result["grade"] = "OK"
    else:
        result["grade"] = "POOR"
    return result

def main():
    import argparse
    parser = argparse.ArgumentParser(description="Batch render test")
    parser.add_argument("--category", "-c", help="Filter by category")
    parser.add_argument("--limit", "-n", type=int, help="Max sites to test")
    parser.add_argument("--firefox", "-f", action="store_true", help="Compare with Firefox")
    args = parser.parse_args()

    sites = load_sites(category=args.category, limit=args.limit)


    # Build
    print("Building...")
    safe_run(["cargo", "build", "--release", "--bin", "render_to_png"], timeout=120)

    ts = datetime.now().strftime("%Y%m%d_%H%M%S")
    outdir = str(ARCHIVE / ts)
    os.makedirs(outdir, exist_ok=True)

    print(f"\nTesting {len(sites)} sites {'(with Firefox)' if args.firefox else '(raw)'}")
    print(f"Output: {outdir}\n")

    results = []
    counts = {}
    for i, (name, url, cat) in enumerate(sites):
        r = render_site(name, url, outdir, with_firefox=args.firefox)
        r["category"] = cat
        results.append(r)

        g = r["grade"]
        counts[g] = counts.get(g, 0) + 1
        icon = {"GREAT": "✅", "OK": "🟡", "POOR": "🟠", "FAIL": "❌", "CRASH": "💀", "EMPTY": "⬜"}.get(g, "?")
        print(f"  {icon} {g:5s}  {name:25s}  text={r['text_boxes']:5d}  [{i+1}/{len(sites)}]")

    # Summary
    total = len(results)
    print(f"\n{'='*60}")
    print(f"SCORECARD: {total} sites  ({datetime.now().strftime('%Y-%m-%d %H:%M')})")
    for g in ["GREAT", "OK", "POOR", "FAIL", "CRASH", "EMPTY"]:
        if counts.get(g, 0) > 0:
            pct = counts[g] / total * 100
            bar = "█" * int(pct / 2)
            print(f"  {g:5s}: {counts[g]:3d}/{total}  ({pct:4.1f}%)  {bar}")
    print(f"{'='*60}")

    # Save report
    report = {
        "timestamp": ts,
        "commit": "unknown",
        "total": total,
        "summary": counts,
        "sites": results,
    }
    report_path = f"{outdir}/report.json"
    with open(report_path, "w") as f:
        json.dump(report, f, indent=2)

    # Append to timeline
    timeline = str(ARCHIVE / "timeline.jsonl")
    with open(timeline, "a") as f:
        f.write(json.dumps({"timestamp": ts, "commit": report["commit"], "summary": counts, "total": total}) + "\n")

    print(f"\nReport: {report_path}")
    print(f"Timeline: {timeline}")

if __name__ == "__main__":
    main()
