Cumulative subsection timing (GeoJSON gate lines).

For reference tracks listed in acr_timing.toml [cumulative_timing.ref_track_sectors],
these files REPLACE sectors_filtered.shp subsection timing (same track cannot use both).

Format: same as timing/timing_sectors/*.geojson
  - properties.reference_track = reference stem (e.g. hafren_north)
  - Point markers with marker_role = sector_boundary (and optional timing_start / finish)
  - marker_order = strict drive order (cumulative + modular only accept forward crossings)
  - seg_id (optional) = stable key for timing_pb.toml / timing.db (may differ from marker_order)

All crossings: cumulative pace beep ([cumulative_beep]), HTML log, no OSD split line.

hafren_north.geojson
  Copied from timing/timing_sectors/cwmbiga_afon_biga.geojson (Start, S1–S3, Finish).
  seg_id = marker_order (0…4). Insert new points with marker_role=sector_boundary,
  marker_order and seg_id strictly increasing along the route (leave gaps in seg_id for later).
