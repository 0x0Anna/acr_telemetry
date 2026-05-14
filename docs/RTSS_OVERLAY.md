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
  - Note: in the RTSS v2 header layout, `dwOSDArrSize` is at byte offset **28** and `dwOSDFrame` at **32**. `acr_recorder` increments `dwOSDFrame` at offset 32.

## Requirements

- **RivaTuner Statistics Server (RTSS)** is running (tray/service).
- RTSS OSD is visible in-game (same idea as FPS/Afterburner OSD).

## Binaries

After building, tools are under `target/release/`:

- `acr_rtss_osd.exe` — generic RTSS text pusher
- `acr_track_match.exe` — track matching; optional `--rtss`

Build:

```powershell
cargo build --release --bin acr_rtss_osd --bin acr_track_match
```

## `acr_rtss_osd` (manual / scripting)

### One-shot text

```powershell
.\target\release\acr_rtss_osd.exe --owner acr_demo --text "hello from acr"
```

### Text from file

```powershell
.\target\release\acr_rtss_osd.exe --owner acr_demo --file .\note.txt
```

### Follow file (poll)

```powershell
.\target\release\acr_rtss_osd.exe --owner acr_demo --file "$env:APPDATA\acr_telemetry\acr_detected_track.txt" --follow --poll-ms 200
```

### Force slot (optional)

To avoid clashes with other tools:

```powershell
.\target\release\acr_rtss_osd.exe --owner acr_demo --text "..." --slot 3
```

`--slot 0` means: find a free slot automatically / re-find owner (default).

### Clean up / release

```powershell
.\target\release\acr_rtss_osd.exe --owner acr_demo --release
```

## `acr_track_match` + RTSS (recommended for “detected track …”)

`acr_track_match` can update RTSS alongside the text file:

```powershell
.\target\release\acr_track_match.exe --refs .\reference_tracks --live --rtss --rtss-owner acr_track_match
```

Optional forced slot:

```powershell
.\target\release\acr_track_match.exe --refs .\reference_tracks --live --rtss --rtss-owner acr_track_match --rtss-slot 3
```

On exit, the tool tries to clear the owner slot via `release`.

## Text file (fallback / other overlays)

Default path (unless overridden):

- `%APPDATA%\acr_telemetry\acr_detected_track.txt`

`acr_track_match` writes there **atomically** (temp + rename) so readers do not see half-written content.

`acr_telemetry_bridge` can also expose the text as a JSON field:

- `detected_track_message`

## Limits / reality check

- RTSS OSD is **text/markup** (depends on RTSS version). Not “arbitrary binary” in the sense of embedded objects through the simple text path.
- Our strings go through **ANSI `CString`** (like many RTSS samples): **no NUL bytes** in the text; non-ASCII characters may look different depending on code page/RTSS/OSD.
- RTSS does **not** necessarily redraw every frame; `dwOSDFrame++` is the usual “please redraw” trigger.

## Troubleshooting

- **`OpenFileMappingW` fails**: RTSS is not running / no shared memory session.
- **Signature != `RTSS` / “header didn't validate”**:
  - In `acr_recorder`, opening tries in order:
    - `RTSSSharedMemoryV2`
    - `Global\\RTSSSharedMemoryV2`
    - `Local\\RTSSSharedMemoryV2`
  - If no valid signature is read, there is typically **no RTSS shared memory object** visible to your session (or it is blocked by policy/AV), or a **session/isolation** issue (rare).
  - Practical check: restart RTSS once and confirm OSD is actually active in-game (otherwise RTSS would not show anything either).
- **No free slot**: other tools occupy OSD slots; set `--slot` or change owner.
- **`dwOSDEntrySize` looks “too small” (e.g. 256)**:
  - On some RTSS versions this field is **not reliable** as the true memory width of a slot.
  - `acr_recorder` therefore uses a **conservative minimum stride** (text fields plus large buffer where needed from v2.12) and takes `max(dwOSDEntrySize, minimum_required)`.
- **Text shows but looks “wrong”**: check RTSS markup/tags (RTSS docs/forums) and length (4095+).
