#!/usr/bin/env python3
"""Swap stored GIS coordinates to the [game_z, game_x] file convention."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import shapefile


def swap_point_xy(x: float, y: float) -> tuple[float, float]:
    return y, x


def swap_shape_points(shape: shapefile._Shape) -> list[list[float]]:
    swapped: list[list[float]] = []
    for pt in shape.points:
        x, y = swap_point_xy(pt[0], pt[1])
        if len(pt) > 2:
            swapped.append([x, y, *pt[2:]])
        else:
            swapped.append([x, y])
    return swapped


def swap_shapefile(path: Path) -> None:
    reader = shapefile.Reader(str(path))
    records = [reader.record(i) for i in range(len(reader))]
    shapes = [reader.shape(i) for i in range(len(reader))]

    writer = shapefile.Writer(str(path))
    writer.fields = [f for f in reader.fields if f[0] != "DeletionFlag"]
    for shape, record in zip(shapes, records, strict=True):
        swapped = swap_shape_points(shape)
        if shape.shapeType == shapefile.POINT:
            writer.point(swapped[0][0], swapped[0][1])
        elif shape.shapeType == shapefile.POLYLINE:
            writer.line([swapped])
        else:
            raise ValueError(f"Unsupported shape type {shape.shapeType} in {path}")
        writer.record(*record)
    writer.close()


def swap_geojson(path: Path) -> None:
    data = json.loads(path.read_text(encoding="utf-8"))
    features = data.get("features")
    if not isinstance(features, list):
        raise ValueError(f"Expected FeatureCollection in {path}")

    for feature in features:
        geometry = feature.get("geometry")
        if not isinstance(geometry, dict):
            continue
        geom_type = geometry.get("type")
        coords = geometry.get("coordinates")
        if geom_type == "Point" and isinstance(coords, list) and len(coords) >= 2:
            coords[0], coords[1] = swap_point_xy(coords[0], coords[1])
        elif geom_type == "LineString" and isinstance(coords, list):
            for pt in coords:
                if isinstance(pt, list) and len(pt) >= 2:
                    pt[0], pt[1] = swap_point_xy(pt[0], pt[1])

    path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="+", type=Path, help="Shapefiles or GeoJSON files")
    args = parser.parse_args()

    for path in args.paths:
        if not path.is_file():
            raise FileNotFoundError(path)
        if path.suffix.lower() == ".geojson":
            swap_geojson(path)
        elif path.suffix.lower() == ".shp":
            swap_shapefile(path)
        else:
            raise ValueError(f"Unsupported file type: {path}")
        print(f"swapped {path}")


if __name__ == "__main__":
    main()
