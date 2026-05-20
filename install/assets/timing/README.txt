Timing data for sector splits (install bundle).

Ring sectors (SHP subsector splits A-B):
  sectors_filtered.shp — polyline crossings → timing/timing.db

Stage sectors (Afon S1/S2/S3/Finish leg times):
  timing_sectors/<slug>.geojson
  [stage_timing.ref_stage_sectors] in acr_timing.toml

Cumulative GeoJSON subsectors (gate lines, RTSS modular OSD):
  cumulative_sectors/{track}_linestrings.geojson (preferred)
  cumulative_sectors/{track}.geojson (point fallback)
  [cumulative_timing.ref_track_sectors] in acr_timing.toml

Bundled: hafren_north → linestrings + cwmbiga_afon_biga stage gates.
Split WAVs: assets/split_sounds/ (see [cumulative_beep] in acr_timing.toml).

timing.db is created on first run. timing_pb.toml: copy from timing/timing_pb.toml.example for personal bests.
