"""Reusable site verification helper for the Incognidium 2026-09 suite.

This module is imported by the `verify_site` jinx and by the per-site
verification jinxes under `npc_team/jinxes/2026-09/`. Keeping the logic in one
place avoids duplication and works around the deterministic NPC runtime's
inability to pass inputs through nested jinx engines.
"""

import os
import subprocess
from pathlib import Path

from npcpy.llm_funcs import get_llm_response


def verify_site(
    url: str,
    expected_site: str,
    out_png: str,
    max_height: str = "1200",
    model: str = "kimi-k2.7-code:cloud",
    provider: str = "ollama",
) -> str:
    os.environ.setdefault("OLLAMA_HOST", "http://127.0.0.1:11434")

    repo = Path.cwd()
    out_path = repo / out_png
    out_path.parent.mkdir(parents=True, exist_ok=True)
    render_bin = repo / "target" / "release" / "render_to_png"

    cmd = [str(render_bin), url, str(out_path)]
    if max_height:
        cmd.extend(["--max-height", str(max_height)])

    render_result = subprocess.run(
        cmd, capture_output=True, text=True, cwd=repo, timeout=180
    )
    if render_result.returncode != 0:
        raise RuntimeError(
            f"render failed: {' '.join(cmd)}\n{render_result.stderr}"
        )

    prompt = f"""Look at the attached full-page browser screenshot of {expected_site} rendered by the Incognidium engine.

    Describe what you see precisely and answer these questions in plain language:
    1. Does the page look like a fully rendered {expected_site} homepage, or is it blank/broken?
    2. Is the top header present and visually correct (color, logo/nav, no huge empty gaps)?
    3. Is the main content area readable, with article headlines/teasers visible?
    4. Is the footer or bottom of the page present and normal-looking?
    5. List any specific visual defects (missing elements, wrong colors, overlapping text, overflow, huge white gaps, broken layout).

    Do not report pixel counts, percentages, RMSE, or any numeric image metrics.
    At the end, state either PASS or FAIL and one short sentence explaining why.
    """

    review_response = get_llm_response(
        prompt,
        model=model,
        provider=provider,
        images=[str(out_path)],
        temperature=0.3,
        timeout=300,
    )
    review = review_response.get("response", "")
    if review is None:
        review = ""
    if isinstance(review, dict):
        import json
        review = json.dumps(review)

    cargo = os.path.expanduser("~/.cargo/bin/cargo")
    cargo_result = subprocess.run(
        [cargo, "check"], capture_output=True, text=True, cwd=repo, timeout=300
    )
    cargo_text = (
        "cargo check OK"
        if cargo_result.returncode == 0
        else f"cargo check FAILED:\n{cargo_result.stderr[-800:]}"
    )

    return f"{review}\n\n[{cargo_text}]".strip()
