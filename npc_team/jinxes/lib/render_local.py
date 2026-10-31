import os
import re
import subprocess
from pathlib import Path


def render_local_file(
    fixture_path,
    out_png,
    repo=".",
    max_height="",
    viewport_width="",
    dump_boxes="",
    no_js=False,
):
    repo = Path(repo).resolve()
    out = Path(out_png).resolve()
    out.parent.mkdir(parents=True, exist_ok=True)

    fixture = Path(fixture_path).resolve()
    if not fixture.exists():
        fallback = repo / Path(fixture_path).name
        if fallback.exists():
            fixture = fallback
        else:
            return {
                "ok": False,
                "stderr": f"fixture not found: {fixture_path}",
                "extracted_text": "",
                "flat_box_lines": [],
                "text_lines": [],
            }

    url = "file://" + str(fixture)

    render_bin = repo / "target" / "release" / "render_to_png"
    if not render_bin.exists():
        render_bin = repo / "target" / "debug" / "render_to_png"

    cmd = [str(render_bin), url, str(out)]
    if max_height:
        cmd.extend(["--max-height", str(max_height)])
    if viewport_width:
        cmd.extend(["--viewport-width", str(viewport_width)])
    if no_js:
        cmd.append("--no-js")

    text_path = out.parent / (out.stem + "_text.txt")
    cmd.extend(["--text", str(text_path)])

    boxes_path = None
    if dump_boxes:
        boxes_path = Path(dump_boxes).resolve()
    else:
        boxes_path = out.parent / (out.stem + "_boxes.txt")
    cmd.extend(["--dump-boxes", str(boxes_path)])

    result = subprocess.run(cmd, capture_output=True, text=True, cwd=repo, timeout=180)

    stderr = result.stderr
    if result.returncode != 0:
        return {
            "ok": False,
            "stderr": stderr,
            "extracted_text": "",
            "flat_box_lines": [],
            "text_lines": [],
        }

    extracted_text = ""
    if text_path.exists():
        extracted_text = text_path.read_text()
    else:
        extracted_text = result.stdout

    flat_box_lines = []
    text_lines = []
    if boxes_path.exists():
        raw = boxes_path.read_text()
        pattern = re.compile(
            r"node=(\d+)\s+tag=([^\s]+)\s+class=([^\s]*)\s+\[([\d\.]+),([\d\.]+)\s+([\d\.]+)x([\d\.]+)\]\s+bg=([^\s]+)\s+fg=([^\s]+)\s+.*img=([^\s]*)\s+text=(.*)"
        )
        for line in raw.splitlines():
            m = pattern.match(line)
            if m:
                node_id = int(m.group(1))
                tag = m.group(2)
                cls = m.group(3)
                x = float(m.group(4))
                y = float(m.group(5))
                w = float(m.group(6))
                h = float(m.group(7))
                img = m.group(10)
                text = m.group(11)
                flat_box_lines.append((node_id, tag, cls, x, y, w, h, img, text))
                if text:
                    text_lines.append((x, y, w, h, text))

    return {
        "ok": True,
        "stderr": stderr,
        "extracted_text": extracted_text,
        "flat_box_lines": flat_box_lines,
        "text_lines": text_lines,
    }
