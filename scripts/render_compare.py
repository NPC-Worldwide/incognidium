#!/usr/bin/env python3
"""Standalone render comparison — Incognidium vs Firefox.

Mirrors npc_team/jinxes/render_compare.jinx but runs directly without the
NPC/LLM dispatcher, which currently hangs on render_compare invocations.
"""

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path("/home/caug/incognidium")
OUTDIR = Path("/tmp/incognidium_tests")


def run(cmd, timeout=120, env=None, **kwargs):
    return subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        timeout=timeout,
        env=env,
        **kwargs,
    )


def build():
    cargo = Path.home() / ".cargo" / "bin" / "cargo"
    result = run(
        [str(cargo), "build", "--release", "--bin", "render_to_png"],
        cwd=REPO,
        timeout=600,
    )
    if result.returncode != 0:
        print("BUILD FAILED", file=sys.stderr)
        print(result.stderr[-1000:], file=sys.stderr)
        sys.exit(1)
    print("Build OK")


def render_incognidium(url: str, name: str):
    outpath = OUTDIR / f"{name}_incognidium.png"
    OUTDIR.mkdir(parents=True, exist_ok=True)
    binary = REPO / "target" / "release" / "render_to_png"
    result = run(
        [str(binary), url, str(outpath), "--max-height", "2000"],
        cwd=REPO,
        timeout=120,
    )
    if result.returncode != 0:
        print("INCOGNIDIUM RENDER FAILED", file=sys.stderr)
        print(result.stderr[-1000:], file=sys.stderr)
        sys.exit(1)
    print(f"Incognidium render: {outpath}")
    return outpath


def render_firefox(url: str, name: str):
    outpath = OUTDIR / f"{name}_firefox.png"
    OUTDIR.mkdir(parents=True, exist_ok=True)
    profile = Path(f"/tmp/ff_profile_{name}_{os.getpid()}")
    shutil.rmtree(profile, ignore_errors=True)
    profile.mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    env["MOZ_HEADLESS"] = "1"
    env["MOZ_ENABLE_WAYLAND"] = "0"
    env.pop("WAYLAND_DISPLAY", None)
    env.pop("DISPLAY", None)

    result = run(
        [
            "timeout", "90",
            "firefox", "--headless", "--screenshot", str(outpath),
            "--window-size=1024,2000",
            "--profile", str(profile),
            "--no-remote",
            url,
        ],
        env=env,
        timeout=120,
    )
    shutil.rmtree(profile, ignore_errors=True)
    if not outpath.exists():
        print("FIREFOX RENDER FAILED", file=sys.stderr)
        print(result.stderr[-500:], file=sys.stderr)
        sys.exit(1)
    print(f"Firefox render: {outpath}")
    return outpath


def pixel_diff(inc_path: Path, ff_path: Path, name: str):
    from PIL import Image

    inc = Image.open(inc_path).convert("RGB")
    ff = Image.open(ff_path).convert("RGB")
    w = min(inc.width, ff.width)
    h = min(inc.height, ff.height)
    inc = inc.crop((0, 0, w, h))
    ff = ff.crop((0, 0, w, h))

    inc_data = list(inc.getdata())
    ff_data = list(ff.getdata())
    threshold = 10
    diff_count = 0
    for a, b in zip(inc_data, ff_data):
        if sum(abs(x - y) for x, y in zip(a, b)) > threshold:
            diff_count += 1

    total = len(inc_data)
    diff_percent = (diff_count / total * 100) if total > 0 else 0.0

    diff_png = OUTDIR / f"{name}_diff.png"
    diff_img = Image.new("RGB", (w, h))
    diff_pixels = []
    for a, b in zip(inc_data, ff_data):
        d = sum(abs(x - y) for x, y in zip(a, b))
        diff_pixels.append((255, 0, 0) if d > threshold else a)
    diff_img.putdata(diff_pixels)
    diff_img.save(diff_png)

    print(f"Pixel diff: {diff_percent:.2f}% ({diff_count}/{total} pixels)")
    print(f"Diff image: {diff_png}")
    return diff_percent, diff_count, total


def main():
    parser = argparse.ArgumentParser(description="Compare Incognidium and Firefox renders")
    parser.add_argument("--url", required=True, help="URL or file:// path to render")
    parser.add_argument("--name", required=True, help="Base name for output files")
    parser.add_argument("--skip-build", action="store_true", help="Assume render_to_png is already built")
    args = parser.parse_args()

    if not args.skip_build:
        build()

    inc_path = render_incognidium(args.url, args.name)
    ff_path = render_firefox(args.url, args.name)
    pixel_diff(inc_path, ff_path, args.name)


if __name__ == "__main__":
    main()
