# RTSS OSD: detected track / plain text

This repository can write text **directly to RTSS** without the RTSS *Overlay Editor* needing a file-based data source.

RTSS exposes a **shared memory** interface (typical name: `RTSSSharedMemoryV2`). Our implementation follows the usual pattern from RTSS samples/SDK:

- Open mapping: `OpenFileMappingW("RTSSSharedMemoryV2")`
- Check signature: `RTSS`
- OSD entries via offsets: `dwOSDArrOffset`, `dwOSDEntrySize`, `dwOSDArrSize`
- Text:
  - RTSS **< 2.7**: `szOSD` (short)
  - RTSS **>= 2.7**: `szOSDEx` (long; typically up to 4095 characters + NUL)
- Trigger refresh: `dwOSDFrame++`

## Requirements

- **RivaTuner Statistics Server (RTSS)** is running (tray/service).
- RTSS OSD is visible **in a hooked game** (desktop alone often shows nothing).

## Config file location

`acr_timing` / `acr_track_match` load **`acr_track_match.toml` from the process working directory first**, then next to the `.exe`, then `%APPDATA%\acr_recorder\`.

Typical dev run (config must live in this repo folder):

```powershell
cd C:\temp\acc-stage-timing
cargo run --release --bin acr_timing --features acr_timing_bin
```

On startup you should see:

```text
acr_track_match: loaded C:\temp\acc-stage-timing\acr_track_match.toml
rtss_osd: placement pixel <P=2880,120>
```

Editing `acr_telemetry\acr_track_match.toml` has **no effect** unless you run the binary from that directory or pass `--config`.

## Position (`acr_track_match.toml` or CLI)

| `rtss_osd_anchor` | Behaviour |
|-------------------|-----------|
| *(omit)* / `default` | RTSS global OSD corner (no position tag) |
| `middle_monitor` | `<P=x,y>` at center of middle display (sorted left→right) + offsets |
| `sticky_center` | `<P4><L0>` screen-center sticky anchor (recommended; ignores pixel offsets) |
| `pixel` | `<P=x,y>` at `rtss_osd_x`/`rtss_osd_y`, or at offsets only if x/y omitted |

**Important:** `<P=4>` is **wrong** — the `=` form is only for pixel coordinates `<P=x,y>`. Center screen without pixel math: `rtss_osd_anchor = "sticky_center"`.

Pixel coordinates are **RTSS “zoomed” virtual-desktop pixels** (see RTSS tray → On-Screen Display → zoom). A value that looks right in a calculator is often far off in-game; calibrate with small steps or use `middle_monitor` / `sticky_center`.

Example (triple 1920×1080, middle panel, slightly up):

```toml
rtss_osd_anchor = "middle_monitor"
rtss_osd_offset_y = -40
```

Example (screen center, RTSS 7.3.2+):

```toml
rtss_osd_anchor = "sticky_center"
```

Example (fixed virtual-desktop pixels — e.g. center of middle monitor ≈ x=2880 on 3×1920):

```toml
rtss_osd_anchor = "pixel"
rtss_osd_x = 2880
rtss_osd_y = 120
```

CLI overrides: `--rtss-osd-anchor`, `--rtss-osd-offset-x/y`, `--rtss-osd-x/y`.

## RTSS hypertext (colors, position)

| Tag | Meaning |
|-----|---------|
| `<P=x,y>` | Absolute cursor in zoomed pixels (top-left origin) |
| `<P4><L0>` | Sticky **screen center**, layer 0 — **no `=`** |
| `<C=RRGGBB>` / `<C>` | Text color / reset |

Wrong (shows literally on screen): `<P=4>hello` — use `<P4><L0>hello` or `<P=100,100>hello`.

Timing OSD colors use `<C=ff0000>` / `<C=00ff00>` in the presenter; `sanitize_multiline_osd_text` keeps `<…>` tags (does not strip hypertext).

## Binaries

```powershell
cargo build --release --bin acr_rtss_osd --bin acr_timing --features acr_timing_bin
```

Manual position test (in game with RTSS hooked):

```powershell
.\target\release\acr_rtss_osd.exe --owner acr_demo --text "<P4><L0><C=ff0000>ROT<C> <C=00ff00>GRUEN<C> test"
.\target\release\acr_rtss_osd.exe --owner acr_demo --text "<P=2880,120>pixel test (calibrate zoom!)"
```

## Troubleshooting

- **Position does nothing**: check startup log `rtss_osd: placement …`. If it says `no position tag`, anchor `pixel` without `rtss_osd_x`/`rtss_osd_y` and zero offsets does nothing. Wrong config file directory is the most common cause.
- **`OpenFileMappingW` fails**: RTSS not running.
- **Text in wrong place with pixels**: adjust RTSS OSD zoom or switch to `sticky_center` / `middle_monitor`.
