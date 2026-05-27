#!/usr/bin/env python3
"""Backfill LLM vision analysis for existing GCS parquet files."""

import os
import subprocess
import tempfile
import base64
from datetime import datetime
import pyarrow as pa
import pyarrow.parquet as pq

GCS_BUCKET = os.environ.get("INCOGNIDIUM_GCS_BUCKET", "npcsh-sibiji-search-data")

def list_existing_parquet():
    """List all site parquet files in GCS."""
    result = subprocess.run(
        ["gcloud", "storage", "ls", f"gs://{GCS_BUCKET}/renders/**/*.parquet"],
        capture_output=True, text=True
    )
    if result.returncode != 0:
        print(f"Failed to list: {result.stderr}")
        return []
    files = [f.strip() for f in result.stdout.strip().split("\n") if f.strip()]
    # Filter to site files (not renders.parquet aggregates)
    return [f for f in files if "renders.parquet" not in f]

def download_parquet(gcs_path, local_path):
    """Download parquet from GCS."""
    result = subprocess.run(
        ["gcloud", "storage", "cp", gcs_path, local_path],
        capture_output=True, text=True
    )
    return result.returncode == 0

def upload_parquet(local_path, gcs_path):
    """Upload parquet to GCS."""
    result = subprocess.run(
        ["gcloud", "storage", "cp", local_path, gcs_path],
        capture_output=True, text=True
    )
    return result.returncode == 0

def analyze_image_with_llm(image_bytes, site_name, url, browser_name, npc):
    """Send image to LLM and get vision analysis."""
    if not image_bytes or len(image_bytes) < 500:
        return f"No valid screenshot for {browser_name}"
    try:
        img_b64 = base64.b64encode(image_bytes).decode('utf-8')
        if npc and hasattr(npc, 'get_llm_response'):
            prompt = f"Describe what you see in this screenshot of {site_name} ({url}) rendered by {browser_name}. What content is visible? Is the layout correct? What elements do you see (text, images, buttons, forms, etc.)? Be specific about any problems or missing content."
            result = npc.get_llm_response(prompt, images=[img_b64])
            if isinstance(result, dict):
                return result.get('response', result.get('output', 'No response'))
            return str(result)
        else:
            return "LLM not available"
    except Exception as e:
        return f"Vision analysis failed: {str(e)[:200]}"

def process_parquet_file(gcs_path, npc):
    """Process a single parquet file - add vision analysis."""
    print(f"Processing: {gcs_path}")

    with tempfile.TemporaryDirectory() as tmpdir:
        local_path = os.path.join(tmpdir, "input.parquet")
        output_path = os.path.join(tmpdir, "output.parquet")

        # Download
        if not download_parquet(gcs_path, local_path):
            print(f"  FAILED: download")
            return False

        # Read
        try:
            table = pq.read_table(local_path)
        except Exception as e:
            print(f"  FAILED: read - {e}")
            return False

        # Check if already has vision columns
        schema = table.schema
        has_vision = any(field.name.startswith("vision_") for field in schema)
        if has_vision:
            print(f"  SKIP: already has vision data")
            return True

        # Get data
        df = table.to_pandas()
        if len(df) == 0:
            print(f"  SKIP: empty file")
            return True

        # Process each row
        vision_inc = []
        vision_ff = []
        vision_cr = []

        for idx, row in df.iterrows():
            site_name = row.get('site_name', 'unknown')
            url = row.get('url', '')

            # Analyze incognidium screenshot
            inc_png = row.get('incognidium_png') or row.get('inc_png')
            v_inc = analyze_image_with_llm(inc_png, site_name, url, "Incognidium", npc) if inc_png else "No image"
            vision_inc.append(v_inc)

            # Analyze firefox screenshot
            ff_png = row.get('firefox_png') or row.get('ff_png')
            v_ff = analyze_image_with_llm(ff_png, site_name, url, "Firefox", npc) if ff_png else "No image"
            vision_ff.append(v_ff)

            # Analyze chromium screenshot
            cr_png = row.get('chromium_png') or row.get('cr_png')
            v_cr = analyze_image_with_llm(cr_png, site_name, url, "Chromium", npc) if cr_png else "No image"
            vision_cr.append(v_cr)

            print(f"  [{idx+1}/{len(df)}] {site_name}: inc={len(v_inc)} ff={len(v_ff)} cr={len(v_cr)}")

        # Add new columns
        df['vision_incognidium'] = vision_inc
        df['vision_firefox'] = vision_ff
        df['vision_chromium'] = vision_cr

        # Convert back to arrow and write
        new_table = pa.Table.from_pandas(df)
        pq.write_table(new_table, output_path)

        # Upload
        if upload_parquet(output_path, gcs_path):
            print(f"  SUCCESS: uploaded")
            return True
        else:
            print(f"  FAILED: upload")
            return False

def main():
    print(f"Starting backfill at {datetime.now()}")
    print(f"GCS Bucket: {GCS_BUCKET}")

    # Get NPC for LLM calls
    npc = None
    try:
        from npcsh._state import setup_shell
        from npcpy.npc_compiler import NPC
        _, team, npc = setup_shell()
        print(f"Loaded NPC: {npc.name if npc else 'None'}")
    except Exception as e:
        print(f"Warning: Could not load NPC: {e}")
        return 1

    # List files
    files = list_existing_parquet()
    print(f"Found {len(files)} parquet files to process")

    # Process each
    success = 0
    failed = 0
    skipped = 0

    for gcs_path in files:
        result = process_parquet_file(gcs_path, npc)
        if result:
            success += 1
        else:
            failed += 1

    print(f"\nDone: {success} success, {failed} failed, {skipped} skipped")
    return 0

if __name__ == "__main__":
    exit(main())
