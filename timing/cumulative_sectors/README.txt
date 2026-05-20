Cumulative subsection timing (GeoJSON gate lines).

For reference tracks listed in acr_timing.toml [cumulative_timing.ref_track_sectors],
these files REPLACE sectors_filtered.shp subsection timing (same track cannot use both).

Format: same as timing/timing_sectors/*.geojson
  - properties.reference_track = reference stem (e.g. hafren_north)
  - Point markers with marker_role = sector_boundary (and optional timing_start / finish)
  - marker_order = strict drive order (cumulative + modular only accept forward crossings)
  - seg_id (optional) = stable key for timing_pb.toml / timing.db (may differ from marker_order)

All crossings: cumulative pace beep ([cumulative_beep]), HTML log, no OSD split line.

hafren_north_linestrings.geojson (preferred at runtime)
  Calibrated gate LineStrings for Hafren North cumulative + modular timing.
  Loader picks {slug}_linestrings.geojson when present (see resolve_cumulative_sectors_path).

hafren_north.geojson
  Point gates (fallback). Same marker_order / labels as linestrings variant.
