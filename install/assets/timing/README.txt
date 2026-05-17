Timing data for sector splits.

Ring sectors (subsector splits A-B):
  sectors_filtered.shp — polyline crossings, stored in timing/timing.db

Stage sectors (S1, S2, … leg times on a calibrated stage):
  timing_sectors/<slug>.geojson — gate lines per stage
  acr_timing.toml [stage_timing.ref_stage_sectors] — maps reference track name to <slug>

Bundled calibration: hafren_north → cwmbiga_afon_biga.geojson

timing.db is created on first run. Add more stages: new GeoJSON under timing_sectors/
and a matching entry in [stage_timing.ref_stage_sectors].
