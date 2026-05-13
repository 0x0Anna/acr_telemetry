#!/usr/bin/env python3
"""Map PacenotePal YAML (distance along stage) to GIS points on a reference trajectory."""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
import unicodedata
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import shapefile
import yaml

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from pacenote_atoms import atomize_tokens, gis_field_schema, gis_properties

# PacenotePal stage stem -> reference_tracks/*.shp stem (without extension).
STAGE_TO_REFERENCE: dict[str, str] = {
    "afon bidno severn": "hafren_south",
    "severn afon bidno": "hafren_north",
    "afon biga banc gwyn": "hafren_south",
    "banc gwyn afon biga": "hafren_north",
    "afon biga cwmbiga": "hafren_south",
    "cwmbiga afon biga": "hafren_north",
    "cwmbiga fedw fain": "hafren_south",
    "fedw fain cwmbiga": "hafren_north",
    "col du petit ballon": "saverne",
    "foret de munster": "valee_de_munster",
    "foret de saverne": "saverne",
    "la bollene vesubie peira cava": "col_de_turini",
    "la bollene vesubie turini": "col_de_turini",
    "la traversee de la mossig": "saverne",
    "luttenbach pres munster": "valee_de_munster",
    "mezien sisteron": "sisteron",
    "mezien st. geniez": "sisteron",
    "obersteigen": "valee_de_munster",
    "peira cava la bollene vesubie": "col_de_turini",
    "peira cava turini": "col_de_turini",
    "pra d alart": "livigno",
    "sisteron mezien": "sisteron",
    "sisteron st. geniez": "sisteron",
    "sommet de munster": "valee_de_munster",
    "sommet de turini": "col_de_turini",
    "st. geniez mezien": "sisteron",
    "st. geniez sisteron": "sisteron",
    "steigenbach": "valee_de_munster",
    "turini la bollene vesubie": "col_de_turini",
    "turini peira cava": "col_de_turini",
    "vallee de munster descente": "valee_de_munster",
    "vallee de munster montee": "valee_de_munster",
}


def file_to_game_xz(file_x: float, file_y: float) -> tuple[float, float]:
    return file_y, file_x


def game_xz_to_file(game_x: float, game_z: float) -> tuple[float, float]:
    return game_z, game_x


def normalize_stage_key(name: str) -> str:
    text = unicodedata.normalize("NFKD", name)
    text = "".join(ch for ch in text if not unicodedata.combining(ch))
    text = text.casefold().replace("_", " ")
    text = re.sub(r"[^a-z0-9.]+", " ", text)
    return re.sub(r"\s+", " ", text).strip()


@dataclass(frozen=True)
class ReferenceTrajectory:
    points: list[tuple[float, float]]
    dist_m: list[float] | None


def load_reference_trajectory(shp_path: Path) -> ReferenceTrajectory:
    reader = shapefile.Reader(str(shp_path))
    field_names = [field[0] for field in reader.fields[1:]]
    dist_idx = field_names.index("dist_m") if "dist_m" in field_names else None
    points: list[tuple[float, float]] = []
    dist_m: list[float] = []
    for shape, record in zip(reader.iterShapes(), reader.iterRecords()):
        if shape.shapeType != shapefile.POINT:
            continue
        if not shape.points:
            continue
        file_x, file_y = shape.points[0]
        points.append(file_to_game_xz(float(file_x), float(file_y)))
        if dist_idx is not None:
            value = record[dist_idx]
            if value is None:
                raise ValueError(f"Missing dist_m in {shp_path}")
            dist_m.append(float(value))
    if len(points) < 2:
        raise ValueError(f"Reference shapefile needs at least two points: {shp_path}")
    if dist_idx is not None and len(dist_m) != len(points):
        raise ValueError(f"dist_m count mismatch in {shp_path}")
    return ReferenceTrajectory(points=points, dist_m=dist_m or None)


def reference_start_dist_m(trajectory: ReferenceTrajectory, override: float | None) -> float:
    if override is not None:
        return override
    if trajectory.dist_m:
        return trajectory.dist_m[0]
    return 0.0


def station_axis(trajectory: ReferenceTrajectory, start_dist_m: float) -> list[float]:
    if trajectory.dist_m:
        return [dist - start_dist_m for dist in trajectory.dist_m]
    return cumulative_distances(trajectory.points)


def cumulative_distances(points: list[tuple[float, float]]) -> list[float]:
    out = [0.0]
    for i in range(1, len(points)):
        x0, z0 = points[i - 1]
        x1, z1 = points[i]
        out.append(out[-1] + math.hypot(x1 - x0, z1 - z0))
    return out


def interpolate_at_distance(
    points: list[tuple[float, float]],
    cumulative_m: list[float],
    distance_m: float,
) -> tuple[float, float, int, float]:
    if distance_m <= cumulative_m[0]:
        return points[0][0], points[0][1], 0, 0.0
    if distance_m >= cumulative_m[-1]:
        return points[-1][0], points[-1][1], len(points) - 2, 1.0

    lo = 0
    hi = len(cumulative_m) - 1
    while lo + 1 < hi:
        mid = (lo + hi) // 2
        if cumulative_m[mid] <= distance_m:
            lo = mid
        else:
            hi = mid

    s0 = cumulative_m[lo]
    s1 = cumulative_m[lo + 1]
    span = s1 - s0
    t = 0.0 if span <= 1e-9 else (distance_m - s0) / span
    x0, z0 = points[lo]
    x1, z1 = points[lo + 1]
    return x0 + t * (x1 - x0), z0 + t * (z1 - z0), lo, t


def resolve_reference_stem(stage_stem: str) -> str:
    key = normalize_stage_key(stage_stem)
    if key in STAGE_TO_REFERENCE:
        return STAGE_TO_REFERENCE[key]
    raise KeyError(
        f"No reference mapping for stage '{stage_stem}' (normalized: '{key}'). "
        "Extend STAGE_TO_REFERENCE in convert_pacenotes_to_geojson.py."
    )


def stage_output_slug(stage_stem: str) -> str:
    key = normalize_stage_key(stage_stem)
    return re.sub(r"[^a-z0-9]+", "_", key).strip("_")


def load_pacenotes(yaml_path: Path) -> list[dict[str, Any]]:
    with yaml_path.open(encoding="utf-8") as f:
        data = yaml.safe_load(f)
    if not isinstance(data, list):
        raise ValueError(f"Expected a YAML list in {yaml_path}")
    return data


def convert_stage(
    pacenote_yaml: Path,
    references_dir: Path,
    reference_stem: str | None = None,
    swap_xz: bool = True,
    start_dist_m: float | None = None,
    subtract_start_dist: bool = True,
) -> dict[str, Any]:
    stage_stem = pacenote_yaml.stem
    ref_stem = reference_stem or resolve_reference_stem(stage_stem)
    ref_path = references_dir / f"{ref_stem}.shp"
    if not ref_path.is_file():
        raise FileNotFoundError(f"Reference shapefile not found: {ref_path}")

    trajectory = load_reference_trajectory(ref_path)
    points = trajectory.points
    start_offset_m = reference_start_dist_m(trajectory, start_dist_m)
    station_m = station_axis(trajectory, start_offset_m)
    notes = load_pacenotes(pacenote_yaml)

    features: list[dict[str, Any]] = []
    beyond = 0
    for idx, note in enumerate(notes):
        distance_m = float(note["distance"])
        lookup_m = distance_m - start_offset_m if subtract_start_dist else distance_m
        x, z, seg_idx, seg_t = interpolate_at_distance(points, station_m, lookup_m)
        if lookup_m > station_m[-1]:
            beyond += 1

        raw_tokens = note.get("notes", [])
        parsed = atomize_tokens(raw_tokens)
        parsed["flags"]["linked_to_next"] = bool(note.get("link_to_next", False))
        notes_text = ", ".join(raw_tokens)
        feature_props = {
            "kind": "pacenote",
            "stage": stage_stem,
            "reference_track": ref_stem,
            "note_index": idx,
            "distance_m": distance_m,
            "distance_lookup_m": round(lookup_m, 3),
            "link_to_next": parsed["flags"]["linked_to_next"],
            "notes": parsed["tokens"],
            "notes_text": notes_text,
            "atoms": parsed["atoms"],
            "flags": parsed["flags"],
            "ref_segment_index": seg_idx,
            "ref_segment_t": round(seg_t, 6),
        }
        feature_props.update(gis_properties(parsed["flags"], notes_text=notes_text))

        features.append(
            {
                "type": "Feature",
                "geometry": {
                    "type": "Point",
                    "coordinates": [
                        round(game_xz_to_file(x, z)[0], 3),
                        round(game_xz_to_file(x, z)[1], 3),
                    ]
                    if swap_xz
                    else [round(x, 3), round(z, 3)],
                },
                "properties": feature_props,
            }
        )

    return {
        "type": "FeatureCollection",
        "name": f"pacenotes:{stage_stem}",
        "properties": {
            "source_yaml": str(pacenote_yaml),
            "reference_shp": str(ref_path),
            "coordinate_space": "acc_world_zx" if swap_xz else "acc_world_xz",
            "swap_xz": swap_xz,
            "start_dist_m_offset": round(start_offset_m, 3),
            "subtract_start_dist": subtract_start_dist,
            "reference_point_count": len(points),
            "reference_station_max_m": round(station_m[-1], 3),
            "pacenote_count": len(notes),
            "max_pacenote_distance_m": max(float(n["distance"]) for n in notes),
            "notes_beyond_reference_end": beyond,
            "gis_field_schema": gis_field_schema(),
        },
        "features": features,
    }


def write_collection(path: Path, collection: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as f:
        json.dump(collection, f, ensure_ascii=False, indent=2)
        f.write("\n")


def convert_all_stages(
    pacenotes_dir: Path,
    references_dir: Path,
    output_dir: Path,
    **kwargs: Any,
) -> tuple[int, list[str]]:
    converted = 0
    failures: list[str] = []
    for yaml_path in sorted(pacenotes_dir.glob("*.yml")):
        if yaml_path.stem == "_blank":
            continue
        try:
            collection = convert_stage(
                yaml_path,
                references_dir,
                **kwargs,
            )
            slug = stage_output_slug(yaml_path.stem)
            write_collection(output_dir / f"{slug}.geojson", collection)
            converted += 1
            props = collection["properties"]
            print(
                f"Wrote {yaml_path.name} -> {slug}.geojson "
                f"({len(collection['features'])} notes, ref={props['reference_shp']})"
            )
        except Exception as exc:  # noqa: BLE001 - batch summary
            failures.append(f"{yaml_path.name}: {exc}")
    return converted, failures


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Convert PacenotePal YAML stages to GeoJSON using reference_tracks SHP geometry."
    )
    parser.add_argument(
        "--pacenote-yaml",
        type=Path,
        help="PacenotePal pacenotes/<stage>.yml",
    )
    parser.add_argument(
        "--pacenotes-dir",
        type=Path,
        help="Convert every stage YAML in this directory",
    )
    parser.add_argument(
        "--references-dir",
        type=Path,
        default=Path("reference_tracks"),
        help="Directory with reference *.shp files (default: reference_tracks)",
    )
    parser.add_argument(
        "--reference-stem",
        help="Override reference shapefile stem (without .shp)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="Output .geojson path for a single stage",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("timing/pacenotes"),
        help="Output directory for --pacenotes-dir batch mode",
    )
    parser.add_argument(
        "--no-swap-xz",
        action="store_true",
        help="Write raw game [x, z] instead of the GIS [z, x] file convention",
    )
    parser.add_argument(
        "--start-dist-m",
        type=float,
        help="distance_traveled at stage start on the reference recording (default: first dist_m)",
    )
    parser.add_argument(
        "--no-subtract-start-dist",
        action="store_true",
        help="Do not subtract the start distance_traveled from pacenote distances",
    )
    args = parser.parse_args()
    convert_kwargs = {
        "reference_stem": args.reference_stem,
        "swap_xz": not args.no_swap_xz,
        "start_dist_m": args.start_dist_m,
        "subtract_start_dist": not args.no_subtract_start_dist,
    }

    if args.pacenotes_dir:
        converted, failures = convert_all_stages(
            args.pacenotes_dir,
            args.references_dir,
            args.output_dir,
            **convert_kwargs,
        )
        print(f"Converted {converted} stage(s) into {args.output_dir}")
        if failures:
            print("Failures:")
            for line in failures:
                print(f"  - {line}")
            raise SystemExit(1)
        return

    if not args.pacenote_yaml or not args.output:
        parser.error("Use --pacenote-yaml with --output, or --pacenotes-dir for batch conversion")

    collection = convert_stage(
        args.pacenote_yaml,
        args.references_dir,
        **convert_kwargs,
    )
    write_collection(args.output, collection)

    props = collection["properties"]
    print(
        f"Wrote {len(collection['features'])} pacenotes to {args.output} "
        f"(ref={props['reference_shp']}, station_max={props['reference_station_max_m']} m, "
        f"start_offset={props['start_dist_m_offset']} m, "
        f"max_note={props['max_pacenote_distance_m']} m, beyond_end={props['notes_beyond_reference_end']})"
    )


if __name__ == "__main__":
    main()
