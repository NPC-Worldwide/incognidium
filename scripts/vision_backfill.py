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

def analyze_image_with_llm(image_bytes, site_name, url, browser_name):
    """Send image to LLM and get structured vision analysis."""
    from npcpy.llm_funcs import get_llm_response
    if not image_bytes or len(image_bytes) < 500:
        return {"error": f"No valid screenshot for {browser_name}"}
    try:
        img_b64 = base64.b64encode(image_bytes).decode('utf-8')
        prompt = f"""
        Analyze this screenshot of {site_name} ({url}) rendered by {browser_name}.

        Describe what you see and any problems.
        """ + """
        Return JSON:
        {
            "description": "",
            "issues": []
        }
        """
        result = get_llm_response(prompt, images=[img_b64], format='json')
        return result.get('response', result)
    except Exception as e:
        return {"error": f"Vision analysis failed: {str(e)[:200]}"}

def _image_prefix_from_site_path(gcs_path):
    """Derive GCS images/ prefix from a per-site parquet path.

    gs://bucket/renders/YYYY/MM/DD/HH/site_name.parquet
    -> gs://bucket/renders/YYYY/MM/DD/HH/images
    """
    if not gcs_path.startswith("gs://"):
        return None
    parts = gcs_path[5:].split("/")
    if len(parts) < 6:
        return None
    # parts = [bucket, renders, YYYY, MM, DD, HH, site_name.parquet]
    bucket = parts[0]
    prefix = "/".join(parts[1:-1])
    return f"gs://{bucket}/{prefix}/images"


def _read_image_from_gcs(image_prefix, site_name, browser):
    """Read a PNG screenshot from the standalone GCS images directory."""
    if not image_prefix:
        return None
    gcs_img = f"{image_prefix}/{site_name}_{browser}.png"
    with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
        local_path = tmp.name
    try:
        r = subprocess.run(
            ["gcloud", "storage", "cp", gcs_img, local_path],
            capture_output=True, text=True, timeout=30
        )
        if r.returncode != 0 or not os.path.exists(local_path) or os.path.getsize(local_path) < 500:
            return None
        with open(local_path, "rb") as f:
            return f.read()
    finally:
        try:
            os.remove(local_path)
        except Exception:
            pass


def process_parquet_file(gcs_path):
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

        import json as _json

        # Derive standalone image prefix from the parquet path.
        site_name_from_path = os.path.basename(gcs_path).replace(".parquet", "")
        image_prefix = _image_prefix_from_site_path(gcs_path)

        # Process each row
        vision_inc = []
        vision_ff = []
        vision_cr = []

        for idx, row in df.iterrows():
            site_name = row.get('site_name', site_name_from_path)
            url = row.get('url', '')

            # Prefer standalone GCS screenshots; parquet binary columns are
            # size-capped and usually null for full-page Incognidium renders.
            inc_png = row.get('incognidium_png') or row.get('inc_png')
            if (not inc_png or len(inc_png) < 500) and image_prefix:
                inc_png = _read_image_from_gcs(image_prefix, site_name, "incognidium")
            if inc_png and len(inc_png) > 500:
                v_inc = analyze_image_with_llm(inc_png, site_name, url, "Incognidium")
            else:
                size = len(inc_png) if inc_png else 0
                v_inc = {"error": "No image", "size": size}
            vision_inc.append(_json.dumps(v_inc) if isinstance(v_inc, dict) else str(v_inc))

            ff_png = row.get('firefox_png') or row.get('ff_png')
            if (not ff_png or len(ff_png) < 500) and image_prefix:
                ff_png = _read_image_from_gcs(image_prefix, site_name, "firefox")
            if ff_png and len(ff_png) > 500:
                v_ff = analyze_image_with_llm(ff_png, site_name, url, "Firefox")
            else:
                size = len(ff_png) if ff_png else 0
                v_ff = {"error": "No image", "size": size}
            vision_ff.append(_json.dumps(v_ff) if isinstance(v_ff, dict) else str(v_ff))

            cr_png = row.get('chromium_png') or row.get('cr_png')
            if (not cr_png or len(cr_png) > 500) and image_prefix:
                cr_png = _read_image_from_gcs(image_prefix, site_name, "chromium")
            if cr_png and len(cr_png) > 500:
                v_cr = analyze_image_with_llm(cr_png, site_name, url, "Chromium")
            else:
                size = len(cr_png) if cr_png else 0
                v_cr = {"error": "No image", "size": size}
            vision_cr.append(_json.dumps(v_cr) if isinstance(v_cr, dict) else str(v_cr))

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

    # List files
    files = list_existing_parquet()
    print(f"Found {len(files)} parquet files to process")

    # Process each
    success = 0
    failed = 0
    skipped = 0

    for gcs_path in files:
        result = process_parquet_file(gcs_path)
        if result:
            success += 1
        else:
            failed += 1

    print(f"\nDone: {success} success, {failed} failed, {skipped} skipped")
    return 0

if __name__ == "__main__":
    exit(main())
