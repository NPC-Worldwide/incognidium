"""Reusable Google Chrome headless screenshot helper for Incognidium jinxes.

This helper starts no HTTP server of its own; callers must supply a reachable URL
(local file:// URLs should be served separately because Chrome headless blocks
direct file access for many subresource types).
"""

import os
import shutil
import subprocess


def chrome_screenshot(
    url: str,
    out_png: str,
    profile_dir: str | None = None,
    viewport_width: int = 1024,
    viewport_height: int = 2000,
) -> dict:
    """Render `url` with Google Chrome headless and save a full-page PNG.

    Returns a dict with keys:
      - ok: bool
      - png: str
      - returncode: int
      - stderr: str
      - stdout: str
    """
    if profile_dir is None:
        profile_dir = f"/tmp/chrome_profile_{os.getpid()}"
    shutil.rmtree(profile_dir, ignore_errors=True)
    os.makedirs(profile_dir, exist_ok=True)

    chrome_cmd = [
        "google-chrome",
        "--headless",
        "--disable-gpu",
        "--no-sandbox",
        "--disable-setuid-sandbox",
        "--disable-dev-shm-usage",
        f"--user-data-dir={profile_dir}",
        f"--screenshot={out_png}",
        f"--window-size={viewport_width},{viewport_height}",
        "--hide-scrollbars",
        "--user-agent=Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36",
        url,
    ]
    result = subprocess.run(
        chrome_cmd,
        capture_output=True,
        text=True,
        timeout=120,
        start_new_session=True,
    )
    return {
        "ok": result.returncode == 0 and os.path.exists(out_png),
        "png": out_png,
        "returncode": result.returncode,
        "stderr": result.stderr or "",
        "stdout": result.stdout or "",
    }
