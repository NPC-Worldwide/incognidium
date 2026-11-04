"""Reusable Mozilla Firefox headless screenshot helper for Incognidium jinxes.

This helper starts no HTTP server of its own; callers must supply a reachable URL
(local file:// URLs should be served separately because Firefox blocks direct
file access for some subresource types). A fresh temporary profile is used for
each run so a stale Marionette process from the default profile cannot block the
headless session.
"""

import os
import shutil
import subprocess


def firefox_screenshot(
    url: str,
    out_png: str,
    profile_dir: str | None = None,
    viewport_width: int = 1024,
    viewport_height: int = 2000,
) -> dict:
    """Render `url` with Firefox headless and save a full-page PNG.

    Returns a dict with keys:
      - ok: bool
      - png: str
      - returncode: int
      - stderr: str
      - stdout: str
    """
    if profile_dir is None:
        profile_dir = f"/tmp/firefox_profile_{os.getpid()}"
    shutil.rmtree(profile_dir, ignore_errors=True)
    os.makedirs(profile_dir, exist_ok=True)

    firefox_cmd = [
        "firefox",
        "--headless",
        "--no-remote",
        f"--profile={profile_dir}",
        f"--window-size={viewport_width},{viewport_height}",
        "--screenshot",
        out_png,
        url,
    ]
    result = subprocess.run(
        firefox_cmd,
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
