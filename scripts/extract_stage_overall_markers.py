#!/usr/bin/env python3
"""Extract stage start / finish / sector points from pacenote GeoJSON for overall timing.

Reads pacenote features (kind=pacenote) and emits timing/overall_markers/<slug>.geojson
with Point features for Gesamtzeit (start → finish) and optional sector checkpoints.

Sector pacenotes are detected via flg_finish, Finish control atoms, or notes matching
Sector/Split/Checkpoint patterns. Post-finish marshalling notes are excluded.

Usage:
  python scripts/extract_stage_overall_markers.py timing/pacenotes/cwmbiga_afon_biga.geojson
  python scripts/extract_stage_overall_markers.py --all
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUT_DIR = REPO_ROOT / "timing" / "overall_markers"

SECTOR_NOTE_RE = re.compile(
    r"\b(sector|split|checkpoint|intermediate)\s*(\d+)?\b", re.IGNORECASE
)
GO_CONTROL_NAMES = frozenset(
    {"go_straight", "go_right", "go_left", "finish", "stop_at_marshals"}
)


def pacenote_features(data: dict[str, Any]) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for f in data.get("features") or []:
        props = f.get("properties") or {}
        if props.get("kind") != "pacenote":
            continue
        try:
            props["_note_index"] = int(props["note_index"])
        except (KeyError, TypeError, ValueError):
            continue
        out.append(f)
    out.sort(key=lambda f: f["properties"]["_note_index"])
    return out


def is_finish_feature(props: dict[str, Any]) -> bool:
    if props.get("flg_finish"):
        return True
    notes_text = (props.get("notes_text") or "").strip()
    if notes_text == "Finish" or notes_text.startswith("Finish,"):
        return True
    for atom in props.get("atoms") or []:
        if atom.get("kind") == "control" and atom.get("name") == "finish":
            return True
        if atom.get("source") == "Finish":
            return True
    return False


def sector_label_from_props(props: dict[str, Any]) -> str | None:
    notes_text = props.get("notes_text") or ""
    m = SECTOR_NOTE_RE.search(notes_text)
    if m:
        num = m.group(2)
        return f"sector_{num}" if num else "sector"
    for atom in props.get("atoms") or []:
        if atom.get("kind") != "control":
            continue
        name = (atom.get("name") or "").strip().lower()
        if name in GO_CONTROL_NAMES:
            continue
        if "sector" in name or "split" in name or "checkpoint" in name:
            return name
    return None


def marker_feature(
    role: str,
    source: dict[str, Any],
    *,
    sector_label: str | None = None,
    order: int,
) -> dict[str, Any]:
    props = source["properties"]
    geom = source["geometry"]
    out_props: dict[str, Any] = {
        "kind": f"overall_{role}",
        "marker_role": role,
        "marker_order": order,
        "stage": props.get("stage"),
        "stage_slug": props.get("stage_slug") or props.get("_stage_slug"),
        "reference_track": props.get("reference_track"),
        "source_pacenote_index": props.get("note_index"),
        "distance_m": props.get("distance_m"),
        "distance_lookup_m": props.get("distance_lookup_m"),
        "notes_text": props.get("notes_text"),
    }
    if sector_label:
        out_props["sector_label"] = sector_label
    return {
        "type": "Feature",
        "geometry": geom,
        "properties": out_props,
    }


def extract_from_pacenotes(path: Path) -> dict[str, Any] | None:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        print(f"skip {path}: {e}", file=sys.stderr)
        return None

    feats = pacenote_features(data)
    if not feats:
        print(f"skip {path.name}: no pacenote features", file=sys.stderr)
        return None

    root = data.get("properties") or {}
    slug = path.stem
    stage = (feats[0]["properties"].get("stage") or slug).strip()
    ref_track = (feats[0]["properties"].get("reference_track") or "").strip()

    for f in feats:
        f["properties"]["_stage_slug"] = slug

    start_f = feats[0]
    finish_candidates = [f for f in feats if is_finish_feature(f["properties"])]
    if not finish_candidates:
        print(f"skip {path.name}: no Finish pacenote", file=sys.stderr)
        return None
    finish_f = max(
        finish_candidates,
        key=lambda f: float(f["properties"].get("distance_lookup_m") or 0.0),
    )
    finish_lookup = float(finish_f["properties"].get("distance_lookup_m") or 0.0)

    markers: list[dict[str, Any]] = []
    markers.append(marker_feature("start", start_f, order=0))
    order = 1
    for f in feats:
        props = f["properties"]
        if f is start_f or f is finish_f:
            continue
        lookup = float(props.get("distance_lookup_m") or 0.0)
        if lookup > finish_lookup + 0.5:
            continue
        label = sector_label_from_props(props)
        if label:
            markers.append(marker_feature("sector", f, sector_label=label, order=order))
            order += 1

    markers.append(marker_feature("finish", finish_f, order=order))

    start_lookup = float(start_f["properties"].get("distance_lookup_m") or 0.0)
    finish_lookup = float(finish_f["properties"].get("distance_lookup_m") or 0.0)

    return {
        "type": "FeatureCollection",
        "name": f"overall_markers:{slug}",
        "properties": {
            "source_pacenotes": str(path.relative_to(REPO_ROOT)).replace("\\", "/"),
            "stage": stage,
            "stage_slug": slug,
            "reference_track": ref_track,
            "coordinate_space": root.get("coordinate_space", "acc_world_zx"),
            "start_distance_lookup_m": round(start_lookup, 3),
            "finish_distance_lookup_m": round(finish_lookup, 3),
            "overall_route_lookup_m": round(max(0.0, finish_lookup - start_lookup), 3),
            "sector_marker_count": sum(
                1 for m in markers if m["properties"]["marker_role"] == "sector"
            ),
            "pacenote_count": len(feats),
        },
        "features": markers,
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "geojson",
        nargs="*",
        type=Path,
        help="Pacenote GeoJSON file(s); default: --all",
    )
    ap.add_argument(
        "--all",
        action="store_true",
        help="Process every timing/pacenotes/*.geojson",
    )
    ap.add_argument(
        "--out-dir",
        type=Path,
        default=DEFAULT_OUT_DIR,
        help=f"Output directory (default: {DEFAULT_OUT_DIR})",
    )
    args = ap.parse_args()

    paths: list[Path]
    if args.all or not args.geojson:
        pac_dir = REPO_ROOT / "timing" / "pacenotes"
        paths = sorted(pac_dir.glob("*.geojson"))
    else:
        paths = [p if p.is_absolute() else REPO_ROOT / p for p in args.geojson]

    args.out_dir.mkdir(parents=True, exist_ok=True)
    ok = 0
    for path in paths:
        coll = extract_from_pacenotes(path)
        if not coll:
            continue
        out_path = args.out_dir / f"{path.stem}.geojson"
        with out_path.open("w", encoding="utf-8") as f:
            json.dump(coll, f, indent=2, ensure_ascii=False)
            f.write("\n")
        n_sec = coll["properties"]["sector_marker_count"]
        print(
            f"{out_path.name}: start + finish"
            + (f" + {n_sec} sector marker(s)" if n_sec else "")
            + f" ({coll['properties']['overall_route_lookup_m']:.1f} m route)"
        )
        ok += 1
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
