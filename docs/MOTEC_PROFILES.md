# MoTeC workspace profiles (TOML)

ACR does **not** ship or edit MoTeC i2 workspace files (`.i2w`, etc.). Instead, it writes **MoTeC LD** (`.ld`) files whose **channel names and units** must match the workspace you open in i2.

A **profile** is a small TOML file that defines:

- which sim telemetry fields to export (`source`),
- what each channel is called in the `.ld` file (`name` — must match your i2 workspace),
- optional scaling (`scale`, `offset`),
- and whether a channel needs a graphics sidecar (`graphics`).

Select a profile in `acr_recorder.toml` under `[export.motec]`. The same profile is used by **acr_export**, **acr_recorder --motec**, and **acr_motec**.

For recording and opening `.ld` files, see **[MOTEC.md](MOTEC.md)**.

---

## 1. How profiles relate to i2 workspaces

| MoTeC i2 | ACR |
|----------|-----|
| Workspace (layouts, math, display) | You configure in i2 |
| Expected channel list in imported data | **`motec_profiles/<name>.toml`** |
| `.ld` log file | Written by ACR export / live MoTeC |

**Rule:** Every channel your workspace reads from the log must appear in the profile with the **exact same `name`** string i2 expects (including spaces and casing). The `unit` string should match what the workspace assumes where it matters.

If names do not match, i2 may open the file but traces stay empty or show “channel not found”.

Built-in examples:

| Profile | File | Typical i2 use |
|---------|------|----------------|
| `rbr` | `motec_profiles/rbr.toml` | RBR / sim-style ids (`Speed`, `LF.suspensionTravel`, …) |
| `rally` | `motec_profiles/rally.toml` | MoTeC **Rally Basic** / ADL names (`Engine RPM`, `Ground Speed`, …) |
| `all_data` | `motec_profiles/all_data.toml` | Dump all currently supported MoTeC sources (debug / discovery) |

Start from the profile closest to your workspace, copy it, then adjust `name` entries — not the other way around.

---

## 2. Enable a profile

In `acr_recorder.toml` (next to the executable, or under `%APPDATA%\acr_telemetry\`):

```toml
[export.motec]
profile = "rally"
# profiles_dir = "motec_profiles"   # optional; default is motec_profiles/ next to exe
```

- **Profile id** = filename without `.toml` (e.g. `my_workspace` → `motec_profiles/my_workspace.toml`).
- **Custom directory:** set `profiles_dir` to a folder relative to the install directory (or an absolute path). Empty = `motec_profiles/` beside `acr_recorder.exe` / `acr_export.exe`.

Profile lookup order:

1. `<profiles_dir>/<profile>.toml` if configured
2. `<exe_dir>/motec_profiles/<profile>.toml`
3. Current working directory `motec_profiles/<profile>.toml`
4. Built-in **`rbr`**, **`rally`**, and **`all_data`** embedded in the binary if no file is found

After adding or editing a TOML file, **no recompile** is required — restart the tool or run export again.

---

## 3. Profile file format

```toml
description = "Short note for yourself (optional)"

[[channels]]
name = "Ground Speed"    # Channel id written into the .ld file
unit = "km/h"            # MoTeC unit string (may be empty "")
source = "speed_kmh"     # Sim field id (see table below)
# scale = 1.0            # Optional multiplier (default 1)
# offset = 0.0           # Optional offset (default 0)
# graphics = false       # If true, channel only when *.graphics.rkyv exists
```

- **`[[channels]]`** — repeat for each channel. Order is the order written to the LD file.
- **`name`** — must match the workspace channel id.
- **`source`** — internal ACR field; see §4. Unknown sources fail at load time with a clear error.
- **`scale` / `offset`** — applied per sample: `value = raw * scale + offset`. Example: `gas` is 0–1 in sim data; Rally uses `scale = 100.0` for `Throttle Pos` in percent.
- **`graphics = true`** — channel is **skipped** unless a matching `*.graphics.rkyv` sidecar was recorded (`record_graphics = true` in `[recorder]`). Required for `graphics_pos_*` sources.

Minimal custom profile:

```toml
description = "My i2 workspace channel names"

[[channels]]
name = "Time"
unit = "s"
source = "time"

[[channels]]
name = "Vehicle Speed"
unit = "km/h"
source = "speed_kmh"

[[channels]]
name = "Engine Speed"
unit = "rpm"
source = "rpm"
```

Save as `motec_profiles/my_workspace.toml` and set `profile = "my_workspace"`.

---

## 4. Channel sources (`source` values)

These strings are the only valid `source` values (from `motec_profile.rs`).

### Time and driver inputs

| `source` | Description | Typical units in profiles |
|----------|-------------|---------------------------|
| `time` | Elapsed seconds (`capture_time_sec` or reconstructed) | `s` |
| `speed_kmh` | Vehicle speed | `km/h` |
| `rpm` | Engine RPM | `rpm` |
| `gas` | Throttle 0–1 (often `scale = 100` for %) | `%` or empty |
| `brake` | Brake pedal 0–1 | `%` or empty |
| `steer_angle` | Steering angle (sim units) | `deg` or empty |
| `gear` | Gear number | empty |
| `engine_rotation` | RPM × 2π/60 (rad/s) | `rad/s` |
| `gear_ok` | `(gear - 1)` as float | empty |
| `brake_status` | 1 if brake &gt; 5%, else 0 | empty |

### G-forces

| `source` | Description |
|----------|-------------|
| `g_force_x` | Lateral G |
| `g_force_y` | Longitudinal G |
| `g_force_total` | √(x² + y²) |

### Suspension

| `source` | Description |
|----------|-------------|
| `suspension_travel_fl` … `suspension_travel_rr` | Corner travel, **metres** |
| `suspension_travel_mm_fl` … `suspension_travel_mm_rr` | Same travel × 1000 (**mm**) |

### Position (physics-derived)

| `source` | Description |
|----------|-------------|
| `car_pos_x`, `car_pos_y`, `car_pos_z` | Average of four tyre contact points |
| `tyre_contact_x_fl` … `tyre_contact_z_rr` | Per-corner contact point (m) |

### Tyres, brakes, wheels

| `source` | Description |
|----------|-------------|
| `tyre_temp_c_fl` … `tyre_temp_c_rr` | Core temp, **°C** (converted from Kelvin) |
| `wheel_pressure_bar_fl` … `wheel_pressure_bar_rr` | Pressure, **bar** (from PSI in sim) |
| `brake_temp_c_fl` … `brake_temp_c_rr` | Brake disc temp, **°C** |
| `tyre_wear_pct_fl` … `tyre_wear_pct_rr` | Wear 0–100% |
| `wheel_speed_kmh_fl` … `wheel_speed_kmh_rr` | Per-wheel speed from angular speed |
| `wheel_speed_kmh_front`, `wheel_speed_kmh_rear` | Average front / rear |
| `wheel_slip_max` | Max slip over four wheels |

### Graphics sidecar only (`graphics = true`)

| `source` | Description |
|----------|-------------|
| `graphics_pos_x`, `graphics_pos_y`, `graphics_pos_z` | World coordinates from graphics packet; resampled to physics length |

Sim-only channels (lambda, oil pressure, ECU maps, etc.) are **not** available from ACC/AC Rally physics and cannot be filled by these profiles.

---

## 5. Workflow: match a new i2 workspace

1. **Open the workspace in MoTeC i2** and note the channel names it expects (channel list / ADL / import dialog).
2. **Copy** `config/motec_profiles/rally.toml` or `rbr.toml` to `motec_profiles/<your_id>.toml` (or edit in the install folder `{app}\motec_profiles\` — installer copies defaults once; your edits are kept).
3. For each required channel, set **`name`** to the i2 string and **`source`** to the closest sim field from §4.
4. Adjust **`unit`** and **`scale`** if values look wrong (e.g. throttle 0–1 vs 0–100%).
5. Set in config:
   ```toml
   [export.motec]
   profile = "<your_id>"
   ```
6. **Record** with `record_graphics = true` if you need `graphics_pos_*` or any `graphics = true` channels.
7. **Export** one lap: `acr_export "telemetry_raw\session.rkyv" --csv` (writes `.ld` too) or use `acr_motec` for a quick live test.
8. **Open the `.ld` in i2** with your workspace; fix any remaining name mismatches in the TOML and re-export.

You do not need to change Rust code unless you need a **new** `source` type — then extend `ChannelSource` in `src/export/motec_profile.rs` and document it here.

---

## 6. Shipped profiles (reference)

### `rally`

MoTeC Rally Basic / ADL-style names (`Engine RPM`, `Throttle Pos`, `Damper Pos FL`, …). Omits ECU-only channels. Good when i2 workspace profile is **Rally**.

### `rbr`

Longer sim-style list (`LF.suspensionTravel`, `car.pos.*`, duplicate speed/throttle names for legacy layouts). Includes optional `position.*` from graphics when sidecar exists.

### `all_data`

Raw dump profile with one channel per currently supported MoTeC `source` id. Useful to quickly inspect everything the MoTeC exporter can output today.  
Important: this is **not** 1:1 with all entries in `FIELDS.md`; it covers all sources currently implemented in `src/export/motec_profile.rs`.

Compare with the files in **`config/motec_profiles/`** in the repository (same content shipped under `install/config/motec_profiles/`).

---

## 7. Tools that use profiles

| Tool | Profile from |
|------|----------------|
| `acr_export` | `acr_recorder.toml` → `[export.motec]` |
| `acr_recorder --motec` | Same |
| `acr_motec` | Same |

Errors such as `unknown MoTeC channel source` or `profile 'x' not found` come from TOML load — check filename, `profile =` spelling, and `source` strings.

---

## 8. Related docs

- **[MOTEC.md](MOTEC.md)** — live vs post-export, opening `.ld` in i2
- **[EXPORT.md](EXPORT.md)** — batch export, CSV/SQLite
- **[FIELDS.md](FIELDS.md)** — underlying telemetry field semantics
