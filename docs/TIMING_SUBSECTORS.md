# Cumulative subsector timing (GeoJSON + RTSS)

Release-ready bundle on branch `feature/subsection-split-stats`: calibrated gate lines, split WAVs, modular sector OSD, and configurable Δ display.

## What ships in the repo

| Path | Role |
|------|------|
| `timing/cumulative_sectors/{track}_linestrings.geojson` | Gate **LineStrings** (preferred; auto-selected over `{track}.geojson`) |
| `timing/cumulative_sectors/hafren_north.geojson` | Point gates (fallback / legacy) |
| `timing/timing_sectors/*.geojson` | Afon **stage** sector lines (S1–S3, Finish) |
| `assets/split_sounds/*.wav` | Tiered faster/slower clips for `[cumulative_beep]` |
| `acr_timing.toml` | Timing + cumulative + `[delta_display]` + `[cumulative_beep]` |
| `config-examples/acr_timing.toml` | Copy template for installs |

## Run (Hafren North example)

From repo root (so TOML paths resolve):

```powershell
cargo build --release --bin acr_track_match
.\target\release\acr_track_match.exe --rtss
```

Startup log should include:

```text
cumulative timing: hafren_north → …/hafren_north_linestrings.geojson (30 gates, LineString gate lines, …)
```

## Config highlights (`acr_timing.toml`)

```toml
[reference_times]
# best_sector | best_stage | best_subsector
mode = "best_sector"

[delta_display]
split_feedback = "subsector"   # or "sector" (cum Δ in main sector; alias: stage)
sector_recap_sec = 5.0         # after Finish: rotate S1..Sn on upper RTSS line (0 = last only)

[cumulative_beep]
mode = "wav"                   # paths relative to acr_timing.toml directory
faster_wav_1 = "assets/split_sounds/good.wav"
slower_wav_1 = "assets/split_sounds/bad.wav"
```

## RTSS lines after a run

1. **Upper line:** last completed sector detail (sub splits + `tot`), or **carousel** through all sectors when `sector_recap_sec > 0`.
2. **Lower line:** `Track completed` (sum of sector totals + Σ Δ).

See also [RTSS_OVERLAY.md](RTSS_OVERLAY.md).

## Tools

```powershell
cargo build --release --bin acr_analyze_timing_recording
.\target\release\acr_analyze_timing_recording.exe telemetry_raw\acc_physics_*.rkyv
```

Compares packet-id timing vs log anchors (debugging Δ gaps).

## Editing gates

Export or edit in QGIS; keep `marker_order` strictly along the driven route. Main sector boundaries use labels `Sector 1`, `Sector 2`, `Sector 3`, `Finish`. Rebuild not required after GeoJSON-only changes.
