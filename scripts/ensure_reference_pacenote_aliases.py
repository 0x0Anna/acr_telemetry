#!/usr/bin/env python3
"""
Create timing/pacenotes/<reference_track>.geojson aliases for acr_track_match.

acr_track_match resolves pacenotes by filename prefix (see pacenote_course::resolve_geojson_path).
Stage files are named after rally legs (e.g. afon_bidno_severn.geojson) while the locked
reference track is the SHP stem (e.g. hafren_south). This script writes one default GeoJSON
per reference stem by copying an existing stage file (deterministic: lexicographically
first available stage slug for that reference).

Driving direction / stage choice:
- North vs south (or other disjoint refs) are separate *.shp geometries; the matcher locks one
  stem, and aliases pick a default *stage* GeoJSON built for that stem — good for that ref.
- Pacenote triggers use distance along the authored reference (see crossed_callout: enter
  trigger radius). There is no separate "reverse gear" detection; driving clearly against the
  authored polyline order can call notes out of sequence.
- If several stages share one ref SHP, the default is only one leg; use [pacenotes].stage /
  geojson for the others.

Re-run after adding new stage GeoJSONs if you want a different default; use --force to overwrite.

YAML sources are not required — only existing timing/pacenotes/*.geojson inputs.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

import convert_pacenotes_to_geojson as cvt  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--pacenotes-dir",
        type=Path,
        default=REPO_ROOT / "timing" / "pacenotes",
        help="Directory with stage *.geojson files",
    )
    ap.add_argument(
        "--dry-run",
        action="store_true",
        help="Print actions only",
    )
    ap.add_argument(
        "--force",
        action="store_true",
        help="Overwrite existing <ref>.geojson even if present",
    )
    args = ap.parse_args()
    d = args.pacenotes_dir.resolve()
    if not d.is_dir():
        print(f"Not a directory: {d}", file=sys.stderr)
        return 1

    by_ref: dict[str, list[str]] = defaultdict(list)
    for stage_key, ref in cvt.STAGE_TO_REFERENCE.items():
        slug = cvt.stage_output_slug(stage_key)
        by_ref[ref].append(slug)

    # Reference stems that have a shapefile in the repo (optional check)
    ref_dir = REPO_ROOT / "reference_tracks"
    shp_refs = set()
    if ref_dir.is_dir():
        for p in ref_dir.glob("*.shp"):
            shp_refs.add(p.stem)

    written = 0
    skipped = 0
    missing = 0

    for ref in sorted(set(cvt.STAGE_TO_REFERENCE.values())):
        slugs = sorted(set(by_ref[ref]))
        existing = [s for s in slugs if (d / f"{s}.geojson").is_file()]
        if not existing:
            print(f"  SKIP {ref}: no stage .geojson for any mapped slug ({len(slugs)} mapped)")
            missing += 1
            continue
        chosen = existing[0]
        src = d / f"{chosen}.geojson"
        dst = d / f"{ref}.geojson"
        if dst.is_file() and not args.force:
            try:
                same = dst.resolve() == src.resolve()
            except OSError:
                same = False
            if same:
                print(f"  OK   {ref}.geojson already identical path to {chosen}.geojson")
            else:
                print(f"  SKIP {ref}.geojson exists (use --force to replace with {chosen}.geojson)")
            skipped += 1
            continue

        if args.dry_run:
            print(f"  WOULD {ref}.geojson <- copy {chosen}.geojson")
            written += 1
            continue

        data = json.loads(src.read_text(encoding="utf-8"))
        if isinstance(data, dict):
            props = data.setdefault("properties", {})
            if not isinstance(props, dict):
                props = {}
                data["properties"] = props
            props["alias_source_stem"] = chosen
            props["alias_role"] = "default_stage_for_reference_track"
            if isinstance(data.get("name"), str):
                data["name"] = f"pacenotes:{ref} (alias of {chosen})"

        dst.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"  WROTE {dst.name} <- {chosen}.geojson ({len(data.get('features', []))} features)")
        written += 1

    print(
        f"\nDone: wrote={written}, skipped_existing={skipped}, "
        f"missing_sources={missing}, refs_in_STAGE_TO_REFERENCE={len(set(cvt.STAGE_TO_REFERENCE.values()))}"
    )
    if shp_refs:
        unmapped_shp = sorted(shp_refs - set(cvt.STAGE_TO_REFERENCE.values()))
        if unmapped_shp:
            print(
                "Note: reference_tracks/*.shp without any STAGE_TO_REFERENCE entry "
                f"(no auto-alias): {', '.join(unmapped_shp)}"
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
