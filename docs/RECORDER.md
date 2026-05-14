# acr_recorder – configuration and behaviour

**acr_recorder** reads **Assetto Corsa Competizione** or **Assetto Corsa Rally** shared memory and writes high-rate telemetry to **`.rkyv`** files (plus optional sidecars). This page covers **where configuration lives**, **CLI overrides**, **sampling rates** (what is fixed vs what follows the game), and the main **`[recorder]`** options.

The same TOML file also holds **`[export]`** defaults for **acr_export**; see [EXPORT.md](EXPORT.md) for export-only keys.

---

## Configuration file location

Settings are read from the first file that exists, in this order:

1. `acr_recorder.toml` next to the **acr_recorder** executable  
2. `acr_recorder.toml` in the **current working directory**  
3. `config.toml` under the OS config directory: `~/.config/acr_recorder/config.toml` (Linux) or equivalent via `dirs` on Windows  

Copy and edit **`config-examples/acr_recorder.toml`** as a starting point.

Relative paths in the TOML are resolved against the **executable’s directory** (with a fallback to the current working directory if the exe path is unavailable). See `resolve_path` in the crate’s `config` module.

---

## Sampling rates (physics vs graphics)

These rates are **not** user-configurable in the TOML; they follow the recorder implementation and the game.

| Stream | Nominal rate | Notes |
|--------|----------------|--------|
| **Physics** | **~333 Hz** | Matches the ACC/AC Rally physics map update rate. Each successful shared-memory read appends one physics sample. The `.rkyv` header stores `333` as the declared sample rate. |
| **Graphics** | **~60 Hz** | When enabled, a graphics snapshot is taken at most about every **16 ms** while live data is present (time-based in the main loop), aligned with typical GraphicsMap cadence. The sidecar header stores `60` as the declared rate. |

When the game is not providing data (e.g. menus), the loop backs off (longer sleeps after repeated empty reads) to reduce CPU use and input lag.

**Progress output:** about every 5 seconds the recorder prints elapsed time, sample count, and an **effective Hz** (samples divided by elapsed wall time). That number should sit near **333** while you are on track with steady updates.

---

## Command-line flags (graphics)

```text
acr_recorder [--graphics | --no-graphics]
```

| Flag | Effect |
|------|--------|
| **`--no-graphics`** | **Disables** writing `*.graphics.rkyv`, regardless of `record_graphics` in config. Use this to save disk space or if you do not need distance/time-in-lap style channels from graphics. |
| **`--graphics`** | **Forces** graphics recording **on**, even if `record_graphics = false` in config. |

If neither flag is passed, the value comes from **`record_graphics`** in **`[recorder]`** (default **`true`**).

When graphics recording is on, stderr notes that GraphicsMap recording is enabled (~60 Hz, useful for Grafana / `distance_traveled`, etc.).

---

## `[recorder]` options (TOML)

| Key | Default (if omitted) | Meaning |
|-----|----------------------|---------|
| **`raw_output_dir`** | `telemetry_raw` (relative to exe/CWD) | Directory for `.rkyv` output. Created if missing. |
| **`notes_dir`** | OS default notes area (`%APPDATA%\acr_telemetry` on Windows, `~/.config/acr_telemetry` on Linux) | Where **`acr_elapsed_secs`**, stop helpers, and integration files used with **acr_export** / batch scripts live. |
| **`stop_file_path`** | Default stop file under the config-dir `acr_telemetry` folder | If this file **exists**, the recorder stops and removes it. Override to put `acr_stop` somewhere else. |
| **`record_graphics`** | `true` | Enables **`<stem>.graphics.rkyv`** alongside each physics file (unless overridden by CLI). |
| **`ring_mode`** | `false` | If **`true`**, writes rotating slot files **`<prefix>_slot_NN.rkyv`** instead of a single timestamped `acc_physics_<unix>.rkyv` per run. |
| **`ring_slots`** | `8` | Number of ring slots (clamped to at least **2** when ring mode is on). Oldest slot is overwritten after wrap-around. |
| **`ring_prefix`** | `acc_ring` | Prefix for slot filenames and **`<prefix>.state.json`** (current/previous slot metadata). |
| **`rotate_on_distance_reset`** | `true` | In ring mode **with** graphics: advance to the next slot when **`distance_traveled`** drops from a high value to a low value (lap-style reset), subject to the thresholds below. |
| **`distance_reset_min_prev_m`** | `200.0` | Previous `distance_traveled` must be ≥ this (metres) to count as a valid reset. |
| **`distance_reset_max_curr_m`** | `30.0` | Current `distance_traveled` must be ≤ this (metres) after the drop. |
| **`distance_reset_cooldown_secs`** | `8` | Minimum seconds between two automatic rotations (avoids double triggers). |

### One-shot vs ring output files

- **Normal mode:** `acc_physics_<unix_timestamp>.rkyv` under `raw_output_dir`, plus optional `acc_physics_<ts>.graphics.rkyv` and metadata `*.json` as implemented in the recorder.  
- **Ring mode:** files like `acc_ring_slot_00.rkyv` … `acc_ring_slot_07.rkyv` and **`acc_ring.state.json`** for tooling to see active slots.

---

## Stopping the recorder

- **Ctrl+C** in the terminal  
- Create the **stop file** (default path printed at startup); **`acr_stop.bat`** in **`batch/`** creates it in the configured notes area  

On startup, an existing stop file at the resolved path is **removed** so a stale stop does not exit immediately.

---

## Output sidecars and metadata

For each physics file, the recorder can create:

- **`<stem>.json`** – format / statics metadata (refreshed when statics such as track name become available)  
- **`<stem>.graphics.rkyv`** – when graphics recording is enabled  
- **`<stem>.notes.json`** – recording start/end times on stop (used together with **acr_export**; see [EXPORT.md](EXPORT.md) and the main [README](../README.md))  

Which graphics/statics fields are populated depends on the game; see [GRAPHICS_STATICS_FIELDS.md](GRAPHICS_STATICS_FIELDS.md).

---

## Related documentation

- **[EXPORT.md](EXPORT.md)** – **acr_export**, SQLite vs CSV/LD, batch skip rules  
- **[FIELDS.md](FIELDS.md)** – channel list for physics (and related data)  
- **[MOTEC.md](MOTEC.md)** – why keeping graphics on helps LD export  
- **[BRIDGE.md](BRIDGE.md)** – live telemetry (**acr_telemetry_bridge**), not the recorder  
