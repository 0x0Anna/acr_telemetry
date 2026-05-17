ACR Recorder – Windows package
================================

Executables (run from this install folder):

  acr_recorder.exe          Record telemetry while ACC / AC Rally is running
  acr_motec.exe             Live MoTeC .ld recording (no .rkyv)
  acr_export.exe            Export .rkyv to CSV, SQLite, and MoTeC .ld
  acr_telemetry_bridge.exe  Live dashboard on phone / second device (HTTP)
  acr_analysis_export.exe   Export Grafana analysis segments
  acr_track_match.exe       Track matching / overlay (optional)
  acr_timing.exe            Sector timing CLI (optional)
  acr_rtss_osd.exe          RTSS on-screen display (optional)

Configuration (edit before first run if needed):

  acr_recorder.toml           Recorder + export paths
  acr_timing.toml             Sector timing / stage sectors
  acr_track_match.toml        Track match + RTSS
  acr_telemetry_bridge.toml   Bridge rate, HTTP address, dashboard slots
  telemetry_color.toml        Dashboard threshold colors (optional)

  batch\                      Helper scripts (stop, markers)

Folders (created by installer; paths in TOML are relative to this directory):

  telemetry_raw\              Raw .rkyv recordings
  telemetry.db                SQLite database (after export)
  timing\                     Sector shapefiles, start grid, timing.db
  reference_tracks\           Bundled .shp reference tracks (track match / timing)

Notes and stop signal (default, not in install dir):

  %APPDATA%\acr_telemetry\    acr_notes, acr_stop, acr_elapsed_secs

Quick start

  1. Start the game, then run acr_recorder.exe
  2. Drive; stop with Ctrl+C or batch\acr_stop.bat
  3. acr_export.exe telemetry_raw --csv
  4. MoTeC live: acr_motec.exe  or  acr_recorder.exe --motec
  5. Optional: acr_telemetry_bridge.exe while driving (see docs\BRIDGE.md)

  config-examples\          Commented TOML templates from the repo

More documentation: docs\ folder (all guides) and https://github.com/decnet100/acr_telemetry

License: PolyForm Noncommercial 1.0.0 (see LICENSE)
