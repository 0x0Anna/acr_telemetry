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

## Sector / finish line templates (`acr_timing.toml` → `[osd_display]`)

You do **not** need hardcoded `type1`/`type2` only — use a **format string** with variables, and embed RTSS tags directly in the template.

| Variable | Meaning |
|----------|---------|
| `{sector}` | Main sector number (1-based) |
| `{cum_delta}` / `{cum_delta:+.3}` | Sector cumulative Δ |
| `{cum_delta_colored}` | Δ with RTSS color (scope from `[delta_display]` `delta_scope`) |
| `{subs}` | Expands `sub_slot` for each sub gate (last `max_sub_slots`) |
| `{time:time}` / `{delta:+.3}` | Inside `sub_slot`: leg time / Δ |
| `{ref:time}` `{tot:time}` | Reference / sector total |
| `{cum_tot:time}` `{ref_tot:time}` `{delta_colored}` | On finish line |

| Preset | Live (driving) | Completed sector | Finish |
|--------|----------------|------------------|--------|
| `default` | Full line + subs | Full detail | Track completed + cum/ref/Δ |
| `compact` / `type2` | S# + Δ + subs | Shorter | Done + times |
| `minimal` / `type1` | **`{cum_delta_colored}` only** (150% font while driving) | S# + Δ + tot (carousel after finish) | Δ only |
| `custom` | `live_sector_line` (required for live) | `sector_line` | `finish_line` |

Override any line explicitly:

```toml
[osd_display]
preset = "minimal"
# live_sector_line = "{cum_delta_colored}"
```

Example (sticky center + custom live):

```toml
[osd_display]
preset = "custom"
live_sector_line = "<P4><L0>{cum_delta_colored}"
sector_line = "S{sector} {cum_delta_colored} tot: {tot:time}"
finish_line = "Done {cum_tot:time} {delta_colored}"
```

### Δ scope (`[delta_display]`)

| `delta_scope` | Live `{cum_delta_colored}` | Resets |
|---------------|----------------------------|--------|
| `subsector` | last gate `delta_i` | each sub gate |
| `sector` | cumulative Δ in current main sector | each main sector |
| `stage` | sum of sector Δ over the whole run | only at run start |

Minimal preset + stage-wide Δ:

```toml
[osd_display]
preset = "minimal"
live_delta_font_scale = 150   # RTSS <S=150>…<S> on live Δ only; 0 = normal

[delta_display]
delta_scope = "stage"
sector_recap_sec = 5.0        # after finish: upper line cycles S1..Sn
```

While driving: upper = `[time Δ]` tape (+ `S# ref` before first sub in sector). Lower = enlarged Δ only.

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
